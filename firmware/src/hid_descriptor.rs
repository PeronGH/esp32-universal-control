//! Composite HID report descriptor: keyboard + consumer + PTP.
//!
//! Four top-level collections in a single report map:
//! 1. **Generic Desktop / Keyboard** (report ID 0x01): standard boot keyboard
//! 2. **Consumer / Consumer Control** (report ID 0x02): 16 media keys
//! 3. **Digitizer / Touch Pad** (report ID 0x05): 5-finger PTP multitouch
//! 4. **Digitizer / Configuration** (report ID 0x04/0x06): PTP input mode + function switch

use esp32_nimble::hid::*;

use esp32_uc_protocol::keyboard::{REPORTID_CONSUMER, REPORTID_KEYBOARD};
use esp32_uc_protocol::ptp;
pub use esp32_uc_protocol::ptp::{
    REPORTID_DEVICE_CAPS, REPORTID_FUNCSWITCH, REPORTID_MULTITOUCH, REPORTID_PTPHQA,
    REPORTID_REPORTMODE,
};

// ---------------------------------------------------------------------------
// TLC 1 & 2: Keyboard + Consumer (via hid! macro)
// ---------------------------------------------------------------------------

/// Keyboard + consumer control HID descriptor fragment.
///
/// From esp32-nimble `ble_keyboard.rs` example. Standard boot keyboard
/// layout (8 bytes) plus 16-bit consumer control.
const KEYBOARD_CONSUMER_DESCRIPTOR: &[u8] = hid!(
    // =========================================================================
    // TLC 1: Generic Desktop / Keyboard
    // =========================================================================
    (USAGE_PAGE, 0x01), // Generic Desktop
    (USAGE, 0x06),      // Keyboard
    (COLLECTION, 0x01), // Application
    (REPORT_ID, REPORTID_KEYBOARD),
    // --- Modifier keys (8 bits) ---
    (USAGE_PAGE, 0x07),    // Keyboard/Keypad
    (USAGE_MINIMUM, 0xE0), // Left Control
    (USAGE_MAXIMUM, 0xE7), // Right GUI
    (LOGICAL_MINIMUM, 0x00),
    (LOGICAL_MAXIMUM, 0x01),
    (REPORT_SIZE, 0x01),
    (REPORT_COUNT, 0x08),
    (HIDINPUT, 0x02), // Data, Var, Abs
    // --- Reserved byte ---
    (REPORT_COUNT, 0x01),
    (REPORT_SIZE, 0x08),
    (HIDINPUT, 0x01), // Const
    // --- LED output (5 bits + 3 padding) ---
    (REPORT_COUNT, 0x05),
    (REPORT_SIZE, 0x01),
    (USAGE_PAGE, 0x08),    // LEDs
    (USAGE_MINIMUM, 0x01), // Num Lock
    (USAGE_MAXIMUM, 0x05), // Kana
    (HIDOUTPUT, 0x02),     // Data, Var, Abs
    (REPORT_COUNT, 0x01),
    (REPORT_SIZE, 0x03),
    (HIDOUTPUT, 0x01), // Const (padding)
    // --- Key codes (6 bytes) ---
    (REPORT_COUNT, 0x06),
    (REPORT_SIZE, 0x08),
    (LOGICAL_MINIMUM, 0x00),
    (LOGICAL_MAXIMUM, 0x65), // 101 keys
    (USAGE_PAGE, 0x07),      // Keyboard/Keypad
    (USAGE_MINIMUM, 0x00),
    (USAGE_MAXIMUM, 0x65),
    (HIDINPUT, 0x00), // Data, Array, Abs
    (END_COLLECTION),
    // =========================================================================
    // TLC 2: Consumer / Consumer Control
    // =========================================================================
    (USAGE_PAGE, 0x0C), // Consumer
    (USAGE, 0x01),      // Consumer Control
    (COLLECTION, 0x01), // Application
    (REPORT_ID, REPORTID_CONSUMER),
    (USAGE_PAGE, 0x0C), // Consumer
    (LOGICAL_MINIMUM, 0x00),
    (LOGICAL_MAXIMUM, 0x01),
    (REPORT_SIZE, 0x01),
    (REPORT_COUNT, 0x10), // 16 bits
    (USAGE, 0xB5),        // Scan Next Track
    (USAGE, 0xB6),        // Scan Previous Track
    (USAGE, 0xB7),        // Stop
    (USAGE, 0xCD),        // Play/Pause
    (USAGE, 0xE2),        // Mute
    (USAGE, 0xE9),        // Volume Up
    (USAGE, 0xEA),        // Volume Down
    (USAGE, 0x23, 0x02),  // WWW Home
    (USAGE, 0x94, 0x01),  // My Computer
    (USAGE, 0x92, 0x01),  // Calculator
    (USAGE, 0x2A, 0x02),  // WWW Favorites
    (USAGE, 0x21, 0x02),  // WWW Search
    (USAGE, 0x26, 0x02),  // WWW Stop
    (USAGE, 0x24, 0x02),  // WWW Back
    (USAGE, 0x83, 0x01),  // Media Select
    (USAGE, 0x8A, 0x01),  // Mail
    (HIDINPUT, 0x02),     // Data, Var, Abs
    (END_COLLECTION),
);

// ---------------------------------------------------------------------------
// TLC 3 & 4: PTP touchpad + configuration (raw bytes, needs 4-byte items)
// ---------------------------------------------------------------------------

// Sized tag variants for the PTP descriptor (the hid! macro can't encode
// 4-byte items). Base tags come from esp32_nimble::hid.
const USAGE_PAGE_8: u8 = USAGE_PAGE | 1;
const USAGE_8: u8 = USAGE | 1;
const LOGICAL_MINIMUM_8: u8 = LOGICAL_MINIMUM | 1;
const LOGICAL_MAXIMUM_8: u8 = LOGICAL_MAXIMUM | 1;
const PHYSICAL_MAXIMUM_8: u8 = PHYSICAL_MAXIMUM | 1;
const UNIT_EXPONENT_8: u8 = UNIT_EXPONENT | 1;
const UNIT_8: u8 = UNIT | 1;
const REPORT_SIZE_8: u8 = REPORT_SIZE | 1;
const REPORT_ID_8: u8 = REPORT_ID | 1;
const REPORT_COUNT_8: u8 = REPORT_COUNT | 1;
const COLLECTION_8: u8 = COLLECTION | 1;
const HIDINPUT_8: u8 = HIDINPUT | 1;
const FEATURE_8: u8 = FEATURE | 1;
const USAGE_PAGE_16: u8 = USAGE_PAGE | 2;
const LOGICAL_MAXIMUM_16: u8 = LOGICAL_MAXIMUM | 2;
const PHYSICAL_MAXIMUM_16: u8 = PHYSICAL_MAXIMUM | 2;
const REPORT_COUNT_16: u8 = REPORT_COUNT | 2;
const UNIT_16: u8 = UNIT | 2;
const LOGICAL_MAXIMUM_32: u8 = LOGICAL_MAXIMUM | 3;
const PHYSICAL_MAXIMUM_32: u8 = PHYSICAL_MAXIMUM | 3;

const fn lo_u16(value: u16) -> u8 {
    (value & 0x00ff) as u8
}

const fn hi_u16(value: u16) -> u8 {
    (value >> 8) as u8
}

/// PTP touchpad + configuration HID descriptor fragment.
///
/// Translated byte-for-byte from `imbushuo/mac-precision-touchpad`
/// `WellspringT2.h` + `Hid.h`.
#[rustfmt::skip]
const PTP_DESCRIPTOR: &[u8] = &[
    // =========================================================================
    // TLC 3: Digitizer / Touch Pad
    // =========================================================================
    USAGE_PAGE_8,   0x0d,                       // Digitizer
    USAGE_8,        0x05,                       // Touch Pad
    COLLECTION_8,   0x01,                       // Application

    REPORT_ID_8,    REPORTID_MULTITOUCH,

    // ----- Finger 1 (variant 1: with unit reset) ----------------------------
    USAGE_8, 0x22, COLLECTION_8, 0x02,
    LOGICAL_MAXIMUM_8, 0x01, USAGE_8, 0x47, USAGE_8, 0x42,
    REPORT_COUNT_8, 0x02, REPORT_SIZE_8, 0x01, HIDINPUT_8, 0x02,
    REPORT_SIZE_8, 0x01, REPORT_COUNT_8, 0x06, HIDINPUT_8, 0x03,
    REPORT_COUNT_8, 0x01, REPORT_SIZE_8, 0x20,
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0xff, 0xff,
    USAGE_8, 0x51, HIDINPUT_8, 0x02,
    USAGE_PAGE_8, 0x01, LOGICAL_MAXIMUM_16, lo_u16(ptp::LOGICAL_X_MAX), hi_u16(ptp::LOGICAL_X_MAX), REPORT_SIZE_8, 0x10,
    UNIT_EXPONENT_8, 0x0e, UNIT_8, 0x11,
    USAGE_8, 0x30, PHYSICAL_MAXIMUM_16, lo_u16(ptp::PHYSICAL_X_MAX), hi_u16(ptp::PHYSICAL_X_MAX), REPORT_COUNT_8, 0x01, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_16, lo_u16(ptp::PHYSICAL_Y_MAX), hi_u16(ptp::PHYSICAL_Y_MAX), LOGICAL_MAXIMUM_16, lo_u16(ptp::LOGICAL_Y_MAX), hi_u16(ptp::LOGICAL_Y_MAX), USAGE_8, 0x31, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_8, 0x00, UNIT_EXPONENT_8, 0x00, UNIT_8, 0x00,
    END_COLLECTION,

    // ----- Finger 2 (variant 1: with unit reset) ----------------------------
    USAGE_PAGE_8, 0x0d, USAGE_8, 0x22, COLLECTION_8, 0x02,
    LOGICAL_MAXIMUM_8, 0x01, USAGE_8, 0x47, USAGE_8, 0x42,
    REPORT_COUNT_8, 0x02, REPORT_SIZE_8, 0x01, HIDINPUT_8, 0x02,
    REPORT_SIZE_8, 0x01, REPORT_COUNT_8, 0x06, HIDINPUT_8, 0x03,
    REPORT_COUNT_8, 0x01, REPORT_SIZE_8, 0x20,
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0xff, 0xff,
    USAGE_8, 0x51, HIDINPUT_8, 0x02,
    USAGE_PAGE_8, 0x01, LOGICAL_MAXIMUM_16, lo_u16(ptp::LOGICAL_X_MAX), hi_u16(ptp::LOGICAL_X_MAX), REPORT_SIZE_8, 0x10,
    UNIT_EXPONENT_8, 0x0e, UNIT_8, 0x11,
    USAGE_8, 0x30, PHYSICAL_MAXIMUM_16, lo_u16(ptp::PHYSICAL_X_MAX), hi_u16(ptp::PHYSICAL_X_MAX), REPORT_COUNT_8, 0x01, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_16, lo_u16(ptp::PHYSICAL_Y_MAX), hi_u16(ptp::PHYSICAL_Y_MAX), LOGICAL_MAXIMUM_16, lo_u16(ptp::LOGICAL_Y_MAX), hi_u16(ptp::LOGICAL_Y_MAX), USAGE_8, 0x31, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_8, 0x00, UNIT_EXPONENT_8, 0x00, UNIT_8, 0x00,
    END_COLLECTION,

    // ----- Finger 3 (variant 2: no unit reset) ------------------------------
    USAGE_PAGE_8, 0x0d, USAGE_8, 0x22, COLLECTION_8, 0x02,
    LOGICAL_MAXIMUM_8, 0x01, USAGE_8, 0x47, USAGE_8, 0x42,
    REPORT_COUNT_8, 0x02, REPORT_SIZE_8, 0x01, HIDINPUT_8, 0x02,
    REPORT_SIZE_8, 0x01, REPORT_COUNT_8, 0x06, HIDINPUT_8, 0x03,
    REPORT_COUNT_8, 0x01, REPORT_SIZE_8, 0x20,
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0xff, 0xff,
    USAGE_8, 0x51, HIDINPUT_8, 0x02,
    USAGE_PAGE_8, 0x01, LOGICAL_MAXIMUM_16, lo_u16(ptp::LOGICAL_X_MAX), hi_u16(ptp::LOGICAL_X_MAX), REPORT_SIZE_8, 0x10,
    UNIT_EXPONENT_8, 0x0e, UNIT_8, 0x11,
    USAGE_8, 0x30, PHYSICAL_MAXIMUM_16, lo_u16(ptp::PHYSICAL_X_MAX), hi_u16(ptp::PHYSICAL_X_MAX), REPORT_COUNT_8, 0x01, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_16, lo_u16(ptp::PHYSICAL_Y_MAX), hi_u16(ptp::PHYSICAL_Y_MAX), LOGICAL_MAXIMUM_16, lo_u16(ptp::LOGICAL_Y_MAX), hi_u16(ptp::LOGICAL_Y_MAX), USAGE_8, 0x31, HIDINPUT_8, 0x02,
    END_COLLECTION,

    // ----- Finger 4 (variant 1: with unit reset) ----------------------------
    USAGE_PAGE_8, 0x0d, USAGE_8, 0x22, COLLECTION_8, 0x02,
    LOGICAL_MAXIMUM_8, 0x01, USAGE_8, 0x47, USAGE_8, 0x42,
    REPORT_COUNT_8, 0x02, REPORT_SIZE_8, 0x01, HIDINPUT_8, 0x02,
    REPORT_SIZE_8, 0x01, REPORT_COUNT_8, 0x06, HIDINPUT_8, 0x03,
    REPORT_COUNT_8, 0x01, REPORT_SIZE_8, 0x20,
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0xff, 0xff,
    USAGE_8, 0x51, HIDINPUT_8, 0x02,
    USAGE_PAGE_8, 0x01, LOGICAL_MAXIMUM_16, lo_u16(ptp::LOGICAL_X_MAX), hi_u16(ptp::LOGICAL_X_MAX), REPORT_SIZE_8, 0x10,
    UNIT_EXPONENT_8, 0x0e, UNIT_8, 0x11,
    USAGE_8, 0x30, PHYSICAL_MAXIMUM_16, lo_u16(ptp::PHYSICAL_X_MAX), hi_u16(ptp::PHYSICAL_X_MAX), REPORT_COUNT_8, 0x01, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_16, lo_u16(ptp::PHYSICAL_Y_MAX), hi_u16(ptp::PHYSICAL_Y_MAX), LOGICAL_MAXIMUM_16, lo_u16(ptp::LOGICAL_Y_MAX), hi_u16(ptp::LOGICAL_Y_MAX), USAGE_8, 0x31, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_8, 0x00, UNIT_EXPONENT_8, 0x00, UNIT_8, 0x00,
    END_COLLECTION,

    // ----- Finger 5 (variant 2: no unit reset) ------------------------------
    USAGE_PAGE_8, 0x0d, USAGE_8, 0x22, COLLECTION_8, 0x02,
    LOGICAL_MAXIMUM_8, 0x01, USAGE_8, 0x47, USAGE_8, 0x42,
    REPORT_COUNT_8, 0x02, REPORT_SIZE_8, 0x01, HIDINPUT_8, 0x02,
    REPORT_SIZE_8, 0x01, REPORT_COUNT_8, 0x06, HIDINPUT_8, 0x03,
    REPORT_COUNT_8, 0x01, REPORT_SIZE_8, 0x20,
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0xff, 0xff,
    USAGE_8, 0x51, HIDINPUT_8, 0x02,
    USAGE_PAGE_8, 0x01, LOGICAL_MAXIMUM_16, lo_u16(ptp::LOGICAL_X_MAX), hi_u16(ptp::LOGICAL_X_MAX), REPORT_SIZE_8, 0x10,
    UNIT_EXPONENT_8, 0x0e, UNIT_8, 0x11,
    USAGE_8, 0x30, PHYSICAL_MAXIMUM_16, lo_u16(ptp::PHYSICAL_X_MAX), hi_u16(ptp::PHYSICAL_X_MAX), REPORT_COUNT_8, 0x01, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_16, lo_u16(ptp::PHYSICAL_Y_MAX), hi_u16(ptp::PHYSICAL_Y_MAX), LOGICAL_MAXIMUM_16, lo_u16(ptp::LOGICAL_Y_MAX), hi_u16(ptp::LOGICAL_Y_MAX), USAGE_8, 0x31, HIDINPUT_8, 0x02,
    END_COLLECTION,

    // ----- Scan Time --------------------------------------------------------
    USAGE_PAGE_8, 0x0d, UNIT_EXPONENT_8, 0x0c, UNIT_16, 0x01, 0x10,
    PHYSICAL_MAXIMUM_32, 0xff, 0xff, 0x00, 0x00,
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0x00, 0x00,
    USAGE_8, 0x56, HIDINPUT_8, 0x02,

    // ----- Contact Count + Button -------------------------------------------
    USAGE_8, 0x54, LOGICAL_MAXIMUM_8, 0x7f, REPORT_SIZE_8, 0x08, HIDINPUT_8, 0x02,
    USAGE_PAGE_8, 0x09, USAGE_8, 0x01, LOGICAL_MAXIMUM_8, 0x01, REPORT_SIZE_8, 0x01,
    HIDINPUT_8, 0x02, REPORT_COUNT_8, 0x07, HIDINPUT_8, 0x03,

    // ----- Feature: Device Capabilities (0x07) ------------------------------
    USAGE_PAGE_8, 0x0d, REPORT_ID_8, REPORTID_DEVICE_CAPS,
    USAGE_8, 0x55, USAGE_8, 0x59,
    LOGICAL_MINIMUM_8, 0x00, LOGICAL_MAXIMUM_16, 0xff, 0x00,
    REPORT_SIZE_8, 0x08, REPORT_COUNT_8, 0x02, FEATURE_8, 0x02,

    // ----- Feature: PTPHQA Certification (0x08) -----------------------------
    USAGE_PAGE_16, 0x00, 0xff, REPORT_ID_8, REPORTID_PTPHQA,
    USAGE_8, 0xc5, LOGICAL_MINIMUM_8, 0x00, LOGICAL_MAXIMUM_16, 0xff, 0x00,
    REPORT_SIZE_8, 0x08, REPORT_COUNT_16, 0x00, 0x01, FEATURE_8, 0x02,

    END_COLLECTION,

    // =========================================================================
    // TLC 4: Digitizer / Configuration
    // =========================================================================
    USAGE_PAGE_8, 0x0d, USAGE_8, 0x0e, COLLECTION_8, 0x01,

    REPORT_ID_8, REPORTID_REPORTMODE,
    USAGE_8, 0x22, COLLECTION_8, 0x02,
    USAGE_8, 0x52, LOGICAL_MINIMUM_8, 0x00, LOGICAL_MAXIMUM_8, ptp::MAX_CONTACTS,
    REPORT_SIZE_8, 0x08, REPORT_COUNT_8, 0x01, FEATURE_8, 0x02,
    END_COLLECTION,

    COLLECTION_8, 0x00, REPORT_ID_8, REPORTID_FUNCSWITCH,
    USAGE_8, 0x57, USAGE_8, 0x58,
    REPORT_SIZE_8, 0x01, REPORT_COUNT_8, 0x02, LOGICAL_MAXIMUM_8, 0x01, FEATURE_8, 0x02,
    REPORT_COUNT_8, 0x06, FEATURE_8, 0x03,
    END_COLLECTION,

    END_COLLECTION,
];

// ---------------------------------------------------------------------------
// Composite descriptor (concatenated at compile time)
// ---------------------------------------------------------------------------

/// Full composite HID report descriptor: keyboard + consumer + PTP + config.
pub const COMPOSITE_REPORT_DESCRIPTOR: &[u8] = &{
    const KB_LEN: usize = KEYBOARD_CONSUMER_DESCRIPTOR.len();
    const PTP_LEN: usize = PTP_DESCRIPTOR.len();
    const TOTAL: usize = KB_LEN + PTP_LEN;

    let mut buf = [0u8; TOTAL];
    let mut i = 0;
    while i < KB_LEN {
        buf[i] = KEYBOARD_CONSUMER_DESCRIPTOR[i];
        i += 1;
    }
    while i < TOTAL {
        buf[i] = PTP_DESCRIPTOR[i - KB_LEN];
        i += 1;
    }
    buf
};
