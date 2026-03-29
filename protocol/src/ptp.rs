//! PTP report types, report IDs, and shared constants.
//!
//! Structs match the PTP HID report layout from
//! `imbushuo/mac-precision-touchpad` `Hid.h`. Used by both firmware
//! (BLE HID output) and host (report construction over UART).

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
    /// X coordinate in logical units (0–20000).
    pub x: u16,
    /// Y coordinate in logical units (0–12000).
    pub y: u16,
}

impl PtpContact {
    /// Flags byte for a confident, touching finger.
    pub const FINGER_DOWN: u8 = 0x03; // confidence | tip_switch
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
    /// Number of active contacts in this report.
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

const _: () = assert!(size_of::<PtpReport>() == 49);
