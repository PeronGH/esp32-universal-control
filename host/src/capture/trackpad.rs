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
use std::sync::{Mutex, PoisonError};

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
static FRAME_BUILDER: Mutex<PtpFrameBuilder> = Mutex::new(PtpFrameBuilder::new());

const TOUCHING_STATE: i32 = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TrackedContact {
    contact_id: u32,
    x: u16,
    y: u16,
}

impl TrackedContact {
    fn from_touch(touch: &MTTouch) -> Self {
        Self {
            contact_id: touch.identifier as u32,
            x: (touch.normalized.position.x * PTP_X_MAX) as u16,
            y: ((1.0 - touch.normalized.position.y) * PTP_Y_MAX) as u16,
        }
    }

    fn into_report_contact(self, flags: u8) -> PtpContact {
        PtpContact {
            flags,
            contact_id: self.contact_id,
            x: self.x,
            y: self.y,
        }
    }
}

#[derive(Debug)]
struct PtpFrameBuilder {
    tracked: [Option<TrackedContact>; ptp::MAX_CONTACTS as usize],
}

impl PtpFrameBuilder {
    const fn new() -> Self {
        Self {
            tracked: [None; ptp::MAX_CONTACTS as usize],
        }
    }

    fn reset(&mut self) {
        self.tracked = [None; ptp::MAX_CONTACTS as usize];
    }

    fn build_report(&mut self, touches: &[MTTouch], clicked: bool) -> Option<PtpReport> {
        // Windows expects a lifted contact to be reported once more at the
        // last known position with tip switch cleared before it disappears.
        let mut report = PtpReport {
            button: clicked as u8,
            ..PtpReport::default()
        };
        let mut next_tracked = [None; ptp::MAX_CONTACTS as usize];
        let mut report_count = 0usize;
        let mut next_count = 0usize;

        for previous in self.tracked.iter().flatten() {
            if let Some(current) = find_touch_by_id(touches, previous.contact_id) {
                report.contacts[report_count] =
                    current.into_report_contact(PtpContact::FINGER_DOWN);
                report_count += 1;
                next_tracked[next_count] = Some(current);
                next_count += 1;
            }
        }

        for previous in self.tracked.iter().flatten() {
            if report_count >= ptp::MAX_CONTACTS as usize {
                break;
            }
            if find_touch_by_id(touches, previous.contact_id).is_none() {
                report.contacts[report_count] = previous.into_report_contact(PtpContact::FINGER_UP);
                report_count += 1;
            }
        }

        for touch in touches.iter().filter(|touch| touch.state == TOUCHING_STATE) {
            if report_count >= ptp::MAX_CONTACTS as usize
                || next_count >= ptp::MAX_CONTACTS as usize
            {
                break;
            }

            let current = TrackedContact::from_touch(touch);
            if self
                .tracked
                .iter()
                .flatten()
                .any(|tracked| tracked.contact_id == current.contact_id)
            {
                continue;
            }

            report.contacts[report_count] = current.into_report_contact(PtpContact::FINGER_DOWN);
            report_count += 1;
            next_tracked[next_count] = Some(current);
            next_count += 1;
        }

        self.tracked = next_tracked;
        report.contact_count = report_count as u8;

        (report_count > 0).then_some(report)
    }
}

fn find_touch_by_id(touches: &[MTTouch], contact_id: u32) -> Option<TrackedContact> {
    touches
        .iter()
        .find(|touch| touch.state == TOUCHING_STATE && touch.identifier as u32 == contact_id)
        .map(TrackedContact::from_touch)
}

unsafe extern "C" fn mt_callback(
    _device: MTDeviceRef,
    touches: *const MTTouch,
    touch_count: i32,
    _timestamp: f64,
    _frame: i32,
) -> i32 {
    let forwarding = FORWARDING.get().is_some_and(|f| f.load(Ordering::Acquire));
    if !forwarding {
        let mut builder = FRAME_BUILDER.lock().unwrap_or_else(PoisonError::into_inner);
        builder.reset();
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
    let mut builder = FRAME_BUILDER.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(report) = builder.build_report(touch_slice, clicked) {
        let _ = tx.send(HostMsg::Touch(report));
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct ContactSnapshot {
        flags: u8,
        contact_id: u32,
        x: u16,
        y: u16,
    }

    fn touch(identifier: i32, x: f32, y: f32) -> MTTouch {
        MTTouch {
            frame: 0,
            timestamp: 0.0,
            identifier,
            state: TOUCHING_STATE,
            finger_id: identifier,
            hand_id: 0,
            normalized: MTVector {
                position: MTPoint { x, y },
                velocity: MTPoint { x: 0.0, y: 0.0 },
            },
            total: 0.0,
            pressure: 0.0,
            angle: 0.0,
            major_axis: 0.0,
            minor_axis: 0.0,
            absolute: MTVector {
                position: MTPoint { x: 0.0, y: 0.0 },
                velocity: MTPoint { x: 0.0, y: 0.0 },
            },
            _field14: 0,
            _field15: 0,
            density: 0.0,
        }
    }

    impl From<PtpContact> for ContactSnapshot {
        fn from(contact: PtpContact) -> Self {
            Self {
                flags: contact.flags,
                contact_id: contact.contact_id,
                x: contact.x,
                y: contact.y,
            }
        }
    }

    fn reported_contacts(report: PtpReport) -> Vec<ContactSnapshot> {
        let contacts = report.contacts;
        contacts[..report.contact_count as usize]
            .iter()
            .copied()
            .map(ContactSnapshot::from)
            .collect()
    }

    #[test]
    fn emits_explicit_lift_before_contact_disappears() {
        let mut builder = PtpFrameBuilder::new();

        let down = builder
            .build_report(&[touch(7, 0.25, 0.75)], false)
            .expect("first contact should be reported");
        let down_contacts = reported_contacts(down);
        assert_eq!(down.contact_count, 1);
        assert_eq!(down_contacts[0].flags, PtpContact::FINGER_DOWN);
        assert_eq!(down_contacts[0].contact_id, 7);

        let lift = builder
            .build_report(&[], false)
            .expect("lift frame should be reported");
        let lift_contacts = reported_contacts(lift);
        assert_eq!(lift.contact_count, 1);
        assert_eq!(lift_contacts[0].flags, PtpContact::FINGER_UP);
        assert_eq!(lift_contacts[0].contact_id, down_contacts[0].contact_id);
        assert_eq!(lift_contacts[0].x, down_contacts[0].x);
        assert_eq!(lift_contacts[0].y, down_contacts[0].y);

        assert!(builder.build_report(&[], false).is_none());
    }

    #[test]
    fn keeps_remaining_contact_while_reporting_lifted_one() {
        let mut builder = PtpFrameBuilder::new();

        let first = builder
            .build_report(&[touch(1, 0.20, 0.30), touch(2, 0.60, 0.40)], false)
            .expect("initial frame should be reported");
        assert_eq!(first.contact_count, 2);

        let second = builder
            .build_report(&[touch(2, 0.65, 0.45)], false)
            .expect("separated lift should be reported");
        let contacts = reported_contacts(second);
        assert_eq!(second.contact_count, 2);
        assert!(
            contacts
                .iter()
                .any(|contact| contact.contact_id == 2 && contact.flags == PtpContact::FINGER_DOWN)
        );
        assert!(
            contacts
                .iter()
                .any(|contact| contact.contact_id == 1 && contact.flags == PtpContact::FINGER_UP)
        );
    }
}
