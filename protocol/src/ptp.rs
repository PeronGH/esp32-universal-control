//! PTP report types, report IDs, and shared constants.
//!
//! Structs match the PTP HID report layout from
//! `imbushuo/mac-precision-touchpad` `Hid.h`. Used by both firmware
//! (BLE HID output) and host (report construction over UART).

use crate::input::{MAX_TOUCH_CONTACTS, TouchContact, TouchFrame};

// ---------------------------------------------------------------------------
// Report IDs
// ---------------------------------------------------------------------------

/// Input report: 5-finger multitouch + scan time + button.
pub const REPORTID_MULTITOUCH: u8 = 0x05;
/// Feature report: input mode (host writes 3 for PTP).
pub const REPORTID_REPORTMODE: u8 = 0x04;
/// Feature report: function switch (surface/button selective reporting).
pub const REPORTID_FUNCSWITCH: u8 = 0x06;
/// Feature report: device capabilities (max contacts, button type).
pub const REPORTID_DEVICE_CAPS: u8 = 0x07;
/// Feature report: PTPHQA certification blob (256 bytes).
pub const REPORTID_PTPHQA: u8 = 0x08;

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

/// Maximum simultaneous contacts reported to Windows.
pub const MAX_CONTACTS: u8 = 5;
/// Touchpad X coordinate maximum in logical units.
pub const LOGICAL_X_MAX: u16 = 12_480;
/// Touchpad Y coordinate maximum in logical units.
pub const LOGICAL_Y_MAX: u16 = 7_680;
/// Scale factor between logical units and advertised physical units.
pub const PHYSICAL_SCALE_DIVISOR: u16 = 10;
/// Touchpad X size in physical units advertised by the HID descriptor.
pub const PHYSICAL_X_MAX: u16 = LOGICAL_X_MAX / PHYSICAL_SCALE_DIVISOR;
/// Touchpad Y size in physical units advertised by the HID descriptor.
pub const PHYSICAL_Y_MAX: u16 = LOGICAL_Y_MAX / PHYSICAL_SCALE_DIVISOR;

// ---------------------------------------------------------------------------
// Input report struct (report ID 0x05, 49 bytes excluding report ID)
// ---------------------------------------------------------------------------

/// A single finger slot inside a PTP input report.
///
/// Layout per the HID descriptor: 1 byte flags + 4 byte contact ID +
/// 2 byte X + 2 byte Y = 9 bytes.
#[repr(C, packed)]
#[derive(
    Clone,
    Copy,
    Default,
    Debug,
    zerocopy::IntoBytes,
    zerocopy::Immutable,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PtpContact {
    /// Bit 0: confidence, bit 1: tip switch.
    pub flags: u8,
    /// Contact identifier (unique per tracked finger).
    pub contact_id: u32,
    /// X coordinate in logical units (0–12480).
    pub x: u16,
    /// Y coordinate in logical units (0–7680).
    pub y: u16,
}

impl PtpContact {
    /// Bit 0: contact is intentional / confident.
    pub const CONFIDENCE: u8 = 0x01;
    /// Bit 1: contact is touching the surface.
    pub const TIP_SWITCH: u8 = 0x02;
    /// Flags byte for a confident finger that is touching the surface.
    pub const FINGER_DOWN: u8 = Self::CONFIDENCE | Self::TIP_SWITCH;
    /// Flags byte for a confident finger lift at the last reported position.
    pub const FINGER_UP: u8 = Self::CONFIDENCE;
}

/// Complete PTP input report (excluding the report ID byte, which the BLE
/// layer handles).
///
/// `5 contacts * 9 B + 2 B scan_time + 1 B contact_count + 1 B button = 49 B`
#[repr(C, packed)]
#[derive(
    Clone,
    Copy,
    Debug,
    zerocopy::IntoBytes,
    zerocopy::Immutable,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PtpReport {
    /// Up to 5 finger slots; unused slots are zeroed.
    pub contacts: [PtpContact; MAX_CONTACTS as usize],
    /// Scan time in 100 µs increments (wraps at u16::MAX).
    pub scan_time: u16,
    /// Number of contacts encoded in this report, including one-frame
    /// liftoff reports that keep confidence set while clearing tip switch.
    pub contact_count: u8,
    /// 1 if the clickpad button is pressed, 0 otherwise.
    pub button: u8,
}

impl Default for PtpReport {
    fn default() -> Self {
        Self {
            contacts: [PtpContact::default(); MAX_CONTACTS as usize],
            scan_time: 0,
            contact_count: 0,
            button: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TrackedContact {
    contact_id: u32,
    x: u16,
    y: u16,
    confident: bool,
    touching: bool,
}

impl TrackedContact {
    fn from_contact(contact: &TouchContact) -> Self {
        Self {
            contact_id: contact.contact_id,
            x: contact.x,
            y: contact.y,
            confident: contact.confident,
            touching: contact.touching,
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

    fn flags(self) -> u8 {
        match (self.confident, self.touching) {
            (true, true) => PtpContact::FINGER_DOWN,
            (true, false) => PtpContact::FINGER_UP,
            (false, true) => PtpContact::TIP_SWITCH,
            (false, false) => 0,
        }
    }
}

/// Stateful encoder that turns semantic touch frames into Windows PTP reports.
#[derive(Debug, Default)]
pub struct TouchReportEncoder {
    tracked: [Option<TrackedContact>; MAX_TOUCH_CONTACTS],
}

impl TouchReportEncoder {
    /// Create a fresh encoder with no tracked contacts.
    pub const fn new() -> Self {
        Self {
            tracked: [None; MAX_TOUCH_CONTACTS],
        }
    }

    /// Clear all tracked touch state.
    pub fn reset(&mut self) {
        self.tracked = [None; MAX_TOUCH_CONTACTS];
    }

    /// Encode the next semantic touch frame.
    pub fn encode(&mut self, frame: &TouchFrame, scan_time: u16) -> Option<PtpReport> {
        let mut report = PtpReport {
            scan_time,
            button: u8::from(frame.button),
            ..PtpReport::default()
        };
        let mut next_tracked = [None; MAX_TOUCH_CONTACTS];
        let mut report_count = 0usize;
        let mut next_count = 0usize;

        for previous in self.tracked.iter().flatten() {
            if let Some(current) = find_contact_by_id(frame, previous.contact_id) {
                report.contacts[report_count] = current.into_report_contact(current.flags());
                report_count += 1;
                if current.touching {
                    next_tracked[next_count] = Some(current);
                    next_count += 1;
                }
            }
        }

        for previous in self.tracked.iter().flatten() {
            if report_count >= MAX_TOUCH_CONTACTS {
                break;
            }
            if find_contact_by_id(frame, previous.contact_id).is_none() {
                report.contacts[report_count] = previous.into_report_contact(PtpContact::FINGER_UP);
                report_count += 1;
            }
        }

        for contact in frame.contacts() {
            if report_count >= MAX_TOUCH_CONTACTS || next_count >= MAX_TOUCH_CONTACTS {
                break;
            }
            let current = TrackedContact::from_contact(contact);
            if self
                .tracked
                .iter()
                .flatten()
                .any(|tracked| tracked.contact_id == current.contact_id)
            {
                continue;
            }

            report.contacts[report_count] = current.into_report_contact(current.flags());
            report_count += 1;
            if current.touching {
                next_tracked[next_count] = Some(current);
                next_count += 1;
            }
        }

        self.tracked = next_tracked;
        report.contact_count = report_count as u8;

        (report_count > 0).then_some(report)
    }
}

fn find_contact_by_id(frame: &TouchFrame, contact_id: u32) -> Option<TrackedContact> {
    frame
        .contacts()
        .iter()
        .find(|contact| contact.contact_id == contact_id)
        .map(TrackedContact::from_contact)
}

const _: () = assert!(size_of::<PtpReport>() == 49);

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct ContactSnapshot {
        flags: u8,
        contact_id: u32,
        x: u16,
        y: u16,
    }

    fn touch(contact_id: u32, x: u16, y: u16) -> TouchContact {
        TouchContact {
            contact_id,
            x,
            y,
            touching: true,
            confident: true,
        }
    }

    fn contacts(report: PtpReport) -> [ContactSnapshot; MAX_TOUCH_CONTACTS] {
        report.contacts.map(|contact| ContactSnapshot {
            flags: contact.flags,
            contact_id: contact.contact_id,
            x: contact.x,
            y: contact.y,
        })
    }

    #[test]
    fn emits_lift_frame_when_contact_disappears() {
        let mut encoder = TouchReportEncoder::new();

        let down = encoder
            .encode(
                &TouchFrame {
                    contacts: [
                        touch(7, 4000, 5000),
                        TouchContact::default(),
                        TouchContact::default(),
                        TouchContact::default(),
                        TouchContact::default(),
                    ],
                    contact_count: 1,
                    button: false,
                },
                10,
            )
            .expect("down frame");
        let down_contacts = contacts(down);
        assert_eq!(down.contact_count, 1);
        assert_eq!(down_contacts[0].flags, PtpContact::FINGER_DOWN);

        let lift = encoder
            .encode(&TouchFrame::default(), 20)
            .expect("lift frame");
        let lift_contacts = contacts(lift);
        assert_eq!(lift.contact_count, 1);
        assert_eq!(lift_contacts[0].flags, PtpContact::FINGER_UP);
        assert_eq!(lift_contacts[0].contact_id, 7);

        assert!(encoder.encode(&TouchFrame::default(), 30).is_none());
    }

    #[test]
    fn keeps_other_contact_active_while_lifting_missing_one() {
        let mut encoder = TouchReportEncoder::new();
        let initial = TouchFrame {
            contacts: [
                touch(1, 1000, 2000),
                touch(2, 3000, 4000),
                TouchContact::default(),
                TouchContact::default(),
                TouchContact::default(),
            ],
            contact_count: 2,
            button: true,
        };
        encoder.encode(&initial, 10).expect("initial frame");

        let report = encoder
            .encode(
                &TouchFrame {
                    contacts: [
                        touch(2, 3200, 4200),
                        TouchContact::default(),
                        TouchContact::default(),
                        TouchContact::default(),
                        TouchContact::default(),
                    ],
                    contact_count: 1,
                    button: false,
                },
                20,
            )
            .expect("mixed frame");
        let report_contacts = contacts(report);

        assert_eq!(report.button, 0);
        assert_eq!(report.contact_count, 2);
        assert!(
            report_contacts
                .iter()
                .any(|contact| contact.contact_id == 1 && contact.flags == PtpContact::FINGER_UP)
        );
        assert!(
            report_contacts
                .iter()
                .any(|contact| contact.contact_id == 2 && contact.flags == PtpContact::FINGER_DOWN)
        );
    }
}
