//! Trackpad capture via MultitouchSupport.framework (private API).
//!
//! Dynamically loads the framework at runtime using `libloading` and
//! registers a contact frame callback that receives raw finger data.
//! Translates normalized coordinates to semantic touch frames and sends
//! `HostMsg::TouchFrame` messages over the serial channel.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use core_foundation::array::CFArrayGetCount;
use core_foundation::array::CFArrayGetValueAtIndex;
use log::info;

use esp32_uc_protocol::input::{TouchContact, TouchFrame};
use esp32_uc_protocol::ptp;
use esp32_uc_protocol::wire::HostMsg;

use super::outbox::Outbox;

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

/// Callback return: 0 = pass through to system, non-zero = consume.
type MTContactCallbackFn = unsafe extern "C" fn(MTDeviceRef, *const MTTouch, i32, f64, i32) -> i32;

const TOUCHING_STATE: i32 = 4;

static TX: std::sync::OnceLock<Arc<Outbox>> = std::sync::OnceLock::new();
static CLICK_STATE: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
static FORWARDING: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
static HAD_CONTACTS: AtomicBool = AtomicBool::new(false);

fn build_touch_frame(touches: &[MTTouch], clicked: bool) -> TouchFrame {
    let mut frame = TouchFrame {
        button: clicked,
        ..TouchFrame::default()
    };

    let mut active = 0usize;
    for touch in touches {
        if active >= frame.contacts.len() {
            break;
        }
        if touch.state == TOUCHING_STATE {
            frame.contacts[active] = TouchContact {
                contact_id: touch.identifier as u32,
                x: (touch.normalized.position.x * f32::from(ptp::LOGICAL_X_MAX)) as u16,
                y: ((1.0 - touch.normalized.position.y) * f32::from(ptp::LOGICAL_Y_MAX)) as u16,
                touching: true,
                confident: true,
            };
            active += 1;
        }
    }

    frame.contact_count = active as u8;
    frame
}

/// MultitouchSupport callback entrypoint.
///
/// # Safety
/// `touches` must either be null or point to `touch_count` valid `MTTouch`
/// records for the duration of the callback.
unsafe extern "C" fn mt_callback(
    _device: MTDeviceRef,
    touches: *const MTTouch,
    touch_count: i32,
    _timestamp: f64,
    _frame: i32,
) -> i32 {
    let forwarding = FORWARDING.get().is_some_and(|f| f.load(Ordering::Acquire));
    if !forwarding {
        HAD_CONTACTS.store(false, Ordering::Relaxed);
        return 0;
    }

    let Some(tx) = TX.get() else { return 0 };
    let touch_slice = if touch_count > 0 && !touches.is_null() {
        // SAFETY: `touches` is provided by MultitouchSupport for this callback.
        unsafe { std::slice::from_raw_parts(touches, touch_count as usize) }
    } else {
        &[]
    };

    let clicked = CLICK_STATE.get().is_some_and(|b| b.load(Ordering::Acquire));
    let frame = build_touch_frame(touch_slice, clicked);

    if frame.contact_count > 0 {
        HAD_CONTACTS.store(true, Ordering::Relaxed);
        tx.push(HostMsg::TouchFrame(frame));
    } else if HAD_CONTACTS.swap(false, Ordering::Relaxed) {
        tx.push(HostMsg::TouchFrame(frame));
    }

    1
}

/// Start trackpad capture. Blocks the calling thread.
pub fn run(
    tx: Arc<Outbox>,
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

    // SAFETY: loading a known system framework path.
    let lib = unsafe {
        libloading::Library::new(
            "/System/Library/PrivateFrameworks/MultitouchSupport.framework/MultitouchSupport",
        )
    }
    .map_err(|e| anyhow::anyhow!("Failed to load MultitouchSupport.framework: {e}"))?;

    let mt_device_create_list: libloading::Symbol<unsafe extern "C" fn() -> CFArrayRef> =
        // SAFETY: symbol name is provided by the framework ABI.
        unsafe { lib.get(b"MTDeviceCreateList\0") }
            .map_err(|e| anyhow::anyhow!("MTDeviceCreateList: {e}"))?;

    let mt_register_contact_frame_callback: libloading::Symbol<
        unsafe extern "C" fn(MTDeviceRef, MTContactCallbackFn),
    > =
        // SAFETY: symbol name is provided by the framework ABI.
        unsafe { lib.get(b"MTRegisterContactFrameCallback\0") }
            .map_err(|e| anyhow::anyhow!("MTRegisterContactFrameCallback: {e}"))?;

    let mt_device_start: libloading::Symbol<unsafe extern "C" fn(MTDeviceRef, i32)> =
        // SAFETY: symbol name is provided by the framework ABI.
        unsafe { lib.get(b"MTDeviceStart\0") }
            .map_err(|e| anyhow::anyhow!("MTDeviceStart: {e}"))?;

    // SAFETY: framework returns an array of device refs owned by CoreFoundation.
    let device_list = unsafe { mt_device_create_list() };
    if device_list.is_null() {
        anyhow::bail!("MTDeviceCreateList returned null");
    }

    // SAFETY: `device_list` is a valid CFArrayRef returned by the framework.
    let device_count = unsafe { CFArrayGetCount(device_list as *const _) };
    info!("Found {device_count} multitouch device(s)");

    for i in 0..device_count {
        // SAFETY: index is in-bounds for `device_list`.
        let device = unsafe { CFArrayGetValueAtIndex(device_list as *const _, i) } as MTDeviceRef;
        // SAFETY: `device` comes from `device_list`, and callback has the required ABI.
        unsafe {
            mt_register_contact_frame_callback(device, mt_callback);
            mt_device_start(device, 0);
        }
        info!("Started multitouch device {i}");
    }

    core_foundation::runloop::CFRunLoop::run_current();
    Ok(())
}
