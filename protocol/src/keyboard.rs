//! Keyboard and consumer control report types and report IDs.
//!
//! The keyboard report follows the standard USB HID boot keyboard layout
//! (8 bytes). Used by both firmware (BLE HID output) and host (key event
//! translation over UART).

/// Report ID for the keyboard input/output reports.
pub const REPORTID_KEYBOARD: u8 = 0x01;
/// Report ID for the consumer control input report.
pub const REPORTID_CONSUMER: u8 = 0x02;

/// Standard 8-byte boot keyboard report (excluding the report ID byte,
/// which the BLE layer handles).
///
/// `1 B modifiers + 1 B reserved + 6 B keycodes = 8 B`
#[repr(C, packed)]
#[derive(Clone, Copy, Default, zerocopy::IntoBytes, zerocopy::Immutable)]
pub struct KeyboardReport {
    /// Modifier key bit flags (bit 0 = LCtrl, 1 = LShift, 2 = LAlt,
    /// 3 = LGui, 4–7 = right equivalents).
    pub modifiers: u8,
    /// Reserved byte (always 0).
    pub reserved: u8,
    /// Up to 6 simultaneous key codes (USB HID usage table).
    pub keycodes: [u8; 6],
}

const _: () = assert!(size_of::<KeyboardReport>() == 8);
