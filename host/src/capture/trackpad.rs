//! Trackpad capture via MultitouchSupport.framework (private API).
//!
//! Dynamically loads the framework at runtime using `libloading` and
//! registers a contact frame callback that receives raw finger data.
//! Translates normalized coordinates to PTP logical space and sends
//! `HostMsg::Touch` reports over the serial channel.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use core_foundation::array::CFArrayGetCount;
use core_foundation::array::CFArrayGetValueAtIndex;
use log::info;

use esp32_uc_protocol::ptp::{self, PtpContact, PtpReport};
use esp32_uc_protocol::wire::HostMsg;

// ---------------------------------------------------------------------------
// FFI types from MultitouchSupport.framework
// (from OpenMultitouchSupport + macos-multitouch)
// ---------------------------------------------------------------------------

type MTDeviceRef = *mut c_void;
type CFArrayRef = *const c_void;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct MTPoint {
    x: f32,
    y: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct MTVector {
    position: MTPoint,
    velocity: MTPoint,
}

/// Raw finger data from MultitouchSupport.framework.
///
/// Layout from `OpenMultitouchSupport` (`OpenMTInternal.h`).
/// Must be exactly 96 bytes (8-byte aligned due to `timestamp: f64`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct MTTouch {
    frame: i32,
    timestamp: f64,
    identifier: i32,
    state: i32,
    finger_id: i32,
    hand_id: i32,
    normalized: MTVector,
    total: f32,
    pressure: f32,
    angle: f32,
    major_axis: f32,
    minor_axis: f32,
    absolute: MTVector,
    _field14: i32,
    _field15: i32,
    density: f32,
}

const _: () = assert!(size_of::<MTTouch>() == 96);

/// Callback return: 0 = pass through to system, non-zero = consume (blocks system gestures).
type MTContactCallbackFn = unsafe extern "C" fn(MTDeviceRef, *const MTTouch, i32, f64, i32) -> i32;

// ---------------------------------------------------------------------------
// PTP coordinate translation constants
// (from hid_descriptor: logical max X=20000, Y=12000)
// ---------------------------------------------------------------------------

const PTP_X_MAX: f32 = 20_000.0;
const PTP_Y_MAX: f32 = 12_000.0;

// ---------------------------------------------------------------------------
// Global state for the C callback (which can't capture).
// ---------------------------------------------------------------------------

static TX: std::sync::OnceLock<mpsc::Sender<HostMsg>> = std::sync::OnceLock::new();
static CLICK_STATE: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
static FORWARDING: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
/// Track whether the previous frame had active contacts, so we send
/// exactly one lift report when all fingers leave.
static HAD_CONTACTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "C" fn mt_callback(
    _device: MTDeviceRef,
    touches: *const MTTouch,
    touch_count: i32,
    _timestamp: f64,
    _frame: i32,
) -> i32 {
    let forwarding = FORWARDING.get().is_some_and(|f| f.load(Ordering::Acquire));
    if !forwarding {
        return 0;
    }
    let Some(tx) = TX.get() else { return 0 };
    let touch_slice = if touch_count > 0 && !touches.is_null() {
        unsafe { std::slice::from_raw_parts(touches, touch_count as usize) }
    } else {
        &[]
    };

    let clicked = CLICK_STATE.get().is_some_and(|b| b.load(Ordering::Acquire));

    // scan_time is set to 0 here; firmware overwrites it at BLE delivery time.
    let mut report = PtpReport {
        button: clicked as u8,
        ..PtpReport::default()
    };

    // Include fingers that are touching (state 4) or in transition
    // (state 3 = hover start). State 4 gets tip_switch; state 3 gets
    // confidence only (allows Windows to track approaching fingers
    // for gesture recognition).
    let mut active = 0usize;
    for t in touch_slice {
        if active >= ptp::MAX_CONTACTS as usize {
            break;
        }
        if t.state == 4 {
            report.contacts[active] = PtpContact {
                flags: PtpContact::FINGER_DOWN,
                contact_id: t.identifier as u32,
                x: (t.normalized.position.x * PTP_X_MAX) as u16,
                y: ((1.0 - t.normalized.position.y) * PTP_Y_MAX) as u16,
            };
            active += 1;
        }
    }
    report.contact_count = active as u8;

    let had = HAD_CONTACTS.load(Ordering::Relaxed);
    if active > 0 {
        HAD_CONTACTS.store(true, Ordering::Relaxed);
        let _ = tx.send(HostMsg::Touch(report));
    } else if had {
        // Send exactly one lift report, then stop until next touch.
        HAD_CONTACTS.store(false, Ordering::Relaxed);
        let _ = tx.send(HostMsg::Touch(report));
    }
    // When active==0 and had==false, don't send (no redundant empty frames).

    // Return non-zero to consume the touch data and prevent system gestures.
    1
}

/// Start trackpad capture. Blocks the calling thread.
/// Touch events are translated to PTP reports and sent to `tx`.
pub fn run(
    tx: mpsc::Sender<HostMsg>,
    click: Arc<AtomicBool>,
    forwarding: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    info!("Starting trackpad capture (MultitouchSupport.framework)");

    TX.set(tx)
        .map_err(|_| anyhow::anyhow!("trackpad TX already initialized"))?;
    CLICK_STATE
        .set(click)
        .map_err(|_| anyhow::anyhow!("trackpad click state already initialized"))?;
    FORWARDING
        .set(forwarding)
        .map_err(|_| anyhow::anyhow!("trackpad forwarding already initialized"))?;

    let lib = unsafe {
        libloading::Library::new(
            "/System/Library/PrivateFrameworks/MultitouchSupport.framework/MultitouchSupport",
        )
    }
    .map_err(|e| anyhow::anyhow!("Failed to load MultitouchSupport.framework: {e}"))?;

    // Load symbols.
    let mt_device_create_list: libloading::Symbol<unsafe extern "C" fn() -> CFArrayRef> =
        unsafe { lib.get(b"MTDeviceCreateList\0") }
            .map_err(|e| anyhow::anyhow!("MTDeviceCreateList: {e}"))?;

    let mt_register_contact_frame_callback: libloading::Symbol<
        unsafe extern "C" fn(MTDeviceRef, MTContactCallbackFn),
    > = unsafe { lib.get(b"MTRegisterContactFrameCallback\0") }
        .map_err(|e| anyhow::anyhow!("MTRegisterContactFrameCallback: {e}"))?;

    let mt_device_start: libloading::Symbol<unsafe extern "C" fn(MTDeviceRef, i32)> =
        unsafe { lib.get(b"MTDeviceStart\0") }
            .map_err(|e| anyhow::anyhow!("MTDeviceStart: {e}"))?;

    // Enumerate devices and register callback on each.
    let device_list = unsafe { mt_device_create_list() };
    if device_list.is_null() {
        anyhow::bail!("MTDeviceCreateList returned null");
    }

    let device_count = unsafe { CFArrayGetCount(device_list as *const _) };
    info!("Found {device_count} multitouch device(s)");

    for i in 0..device_count {
        let device = unsafe { CFArrayGetValueAtIndex(device_list as *const _, i) } as MTDeviceRef;
        unsafe {
            mt_register_contact_frame_callback(device, mt_callback);
            mt_device_start(device, 0);
        }
        info!("Started multitouch device {i}");
    }

    // Block forever. Callbacks fire on this thread's run loop.
    // MultitouchSupport uses the current thread's CFRunLoop internally.
    core_foundation::runloop::CFRunLoop::run_current();

    Ok(())
}
