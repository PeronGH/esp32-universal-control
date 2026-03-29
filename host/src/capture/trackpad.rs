//! Trackpad capture via MultitouchSupport.framework (private API).
//!
//! Dynamically loads the framework at runtime using `libloading` and
//! registers a contact frame callback that receives raw finger data.
//! Translates normalized coordinates to PTP logical space and sends
//! `HostMsg::Touch` reports over the serial channel.

use std::ffi::c_void;
use std::sync::mpsc;
use std::time::Instant;

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

/// Raw finger data from MultitouchSupport.
///
/// Field layout reverse-engineered from OpenMultitouchSupport.
/// The struct may vary between macOS versions — fields after `size`
/// are not used and exist only for padding.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct MTFinger {
    frame: i32,
    timestamp: f64,
    identifier: i32,
    state: i32,
    finger_number: i32,
    _unknown1: i32,
    normalized_pos: MTPoint,
    normalized_vel: MTPoint,
    _unknown2: f32,
    angle: f32,
    major_axis: f32,
    minor_axis: f32,
    _unknown3: MTPoint,
    _unknown4: f32,
    _unknown5: f32,
    size: f32,
    _pad: [u8; 32],
}

type MTContactCallbackFn = unsafe extern "C" fn(MTDeviceRef, *const MTFinger, i32, f64, i32);

// ---------------------------------------------------------------------------
// PTP coordinate translation constants
// (from hid_descriptor: logical max X=20000, Y=12000)
// ---------------------------------------------------------------------------

const PTP_X_MAX: f32 = 20_000.0;
const PTP_Y_MAX: f32 = 12_000.0;

// ---------------------------------------------------------------------------
// Global channel — the callback is a C function pointer, can't capture.
// ---------------------------------------------------------------------------

static TX: std::sync::OnceLock<mpsc::Sender<HostMsg>> = std::sync::OnceLock::new();
static START_TIME: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

unsafe extern "C" fn mt_callback(
    _device: MTDeviceRef,
    fingers: *const MTFinger,
    finger_count: i32,
    _timestamp: f64,
    _frame: i32,
) {
    let Some(tx) = TX.get() else { return };
    let finger_slice = if finger_count > 0 && !fingers.is_null() {
        unsafe { std::slice::from_raw_parts(fingers, finger_count as usize) }
    } else {
        &[]
    };

    // Scan time in 100µs units since start, wrapping at u16::MAX.
    let start = START_TIME.get_or_init(Instant::now);
    let scan_time = (start.elapsed().as_micros() / 100) as u16;
    let mut report = PtpReport {
        scan_time,
        ..PtpReport::default()
    };

    // Only include fingers with state == 4 (touching).
    let mut active = 0usize;
    for f in finger_slice {
        if f.state != 4 || active >= ptp::MAX_CONTACTS as usize {
            continue;
        }
        report.contacts[active] = PtpContact {
            flags: PtpContact::FINGER_DOWN,
            contact_id: f.identifier as u32,
            x: (f.normalized_pos.x * PTP_X_MAX) as u16,
            y: ((1.0 - f.normalized_pos.y) * PTP_Y_MAX) as u16,
        };
        active += 1;
    }
    report.contact_count = active as u8;

    let _ = tx.send(HostMsg::Touch(report));
}

/// Start trackpad capture. Blocks the calling thread.
/// Touch events are translated to PTP reports and sent to `tx`.
pub fn run(tx: mpsc::Sender<HostMsg>) -> anyhow::Result<()> {
    info!("Starting trackpad capture (MultitouchSupport.framework)");

    TX.set(tx)
        .map_err(|_| anyhow::anyhow!("trackpad TX already initialized"))?;

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

    // Block forever — callbacks fire on this thread's run loop.
    // MultitouchSupport uses the current thread's CFRunLoop internally.
    core_foundation::runloop::CFRunLoop::run_current();

    Ok(())
}
