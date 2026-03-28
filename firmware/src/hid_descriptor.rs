//! Windows Precision Touchpad HID report descriptor and report ID constants.
//!
//! Translated byte-for-byte from `imbushuo/mac-precision-touchpad`:
//! - `WellspringT2.h` (touchpad TLC with 5 finger collections)
//! - `Hid.h` (configuration TLC with input mode and function switch)
//!
//! Two top-level collections:
//! 1. **Digitizer / Touch Pad** (report ID 0x05) — 5 finger slots, scan time,
//!    button, plus feature reports for device caps (0x07) and PTPHQA cert (0x08).
//! 2. **Digitizer / Configuration** — feature reports for input mode (0x04) and
//!    function switch (0x06).

use crate::ptp;

// ---------------------------------------------------------------------------
// HID item tag constants
// ---------------------------------------------------------------------------
//
// HID short-item byte format: bits 0–1 = bSize (0→0B, 1→1B, 2→2B, 3→4B),
// bits 2–3 = bType, bits 4–7 = bTag.
//
// Base tags from esp32_nimble::hid have bSize = 0. The sized variants
// OR in the data length for use in a raw byte array.

use esp32_nimble::hid::{
    COLLECTION, END_COLLECTION, FEATURE, HIDINPUT, LOGICAL_MAXIMUM, LOGICAL_MINIMUM,
    PHYSICAL_MAXIMUM, REPORT_COUNT, REPORT_ID, REPORT_SIZE, UNIT, UNIT_EXPONENT, USAGE, USAGE_PAGE,
};

// 1-byte data (bSize = 1)
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

// 2-byte data (bSize = 2)
const USAGE_PAGE_16: u8 = USAGE_PAGE | 2;
const LOGICAL_MAXIMUM_16: u8 = LOGICAL_MAXIMUM | 2;
const PHYSICAL_MAXIMUM_16: u8 = PHYSICAL_MAXIMUM | 2;
const REPORT_COUNT_16: u8 = REPORT_COUNT | 2;
const UNIT_16: u8 = UNIT | 2;

// 4-byte data (bSize = 3 — HID encodes "4 data bytes" as bSize value 3)
const LOGICAL_MAXIMUM_32: u8 = LOGICAL_MAXIMUM | 3;
const PHYSICAL_MAXIMUM_32: u8 = PHYSICAL_MAXIMUM | 3;

// ---------------------------------------------------------------------------
// Report IDs — must match the descriptor below.
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
// Descriptor
// ---------------------------------------------------------------------------

/// Windows Precision Touchpad HID report descriptor.
///
/// The raw bytes are identical to the C original. Named constants are used for
/// HID item tags; data bytes remain numeric where they represent
/// protocol-defined values (usage IDs, collection types, input/feature flags).
#[rustfmt::skip]
pub const PTP_REPORT_DESCRIPTOR: &[u8] = &[
    // =========================================================================
    // TLC 1: Digitizer / Touch Pad
    // =========================================================================
    USAGE_PAGE_8,   0x0d,                       // Usage Page: Digitizer
    USAGE_8,        0x05,                       // Usage: Touch Pad
    COLLECTION_8,   0x01,                       // Collection: Application

    REPORT_ID_8,    REPORTID_MULTITOUCH,

    // ----- Finger 1 (variant 1: with unit reset) ----------------------------
    USAGE_8,        0x22,                       // Usage: Finger
    COLLECTION_8,   0x02,                       // Collection: Logical
    LOGICAL_MAXIMUM_8, 0x01,
    USAGE_8,        0x47,                       //   Confidence
    USAGE_8,        0x42,                       //   Tip Switch
    REPORT_COUNT_8, 0x02,
    REPORT_SIZE_8,  0x01,
    HIDINPUT_8,     0x02,                       //   Data, Var, Abs
    REPORT_SIZE_8,  0x01,
    REPORT_COUNT_8, 0x06,
    HIDINPUT_8,     0x03,                       //   Const (padding)
    REPORT_COUNT_8, 0x01,
    REPORT_SIZE_8,  0x20,                       //   32 bits
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0xff, 0xff, //   Contact ID max
    USAGE_8,        0x51,                       //   Contact Identifier
    HIDINPUT_8,     0x02,
    USAGE_PAGE_8,   0x01,                       //   Generic Desktop
    LOGICAL_MAXIMUM_16, 0x20, 0x4e,             //   20000
    REPORT_SIZE_8,  0x10,                       //   16 bits
    UNIT_EXPONENT_8, 0x0e,                      //   -2
    UNIT_8,         0x11,                       //   cm
    USAGE_8,        0x30,                       //   X
    PHYSICAL_MAXIMUM_16, 0x14, 0x05,            //   1300 (13.00 cm)
    REPORT_COUNT_8, 0x01,
    HIDINPUT_8,     0x02,
    PHYSICAL_MAXIMUM_16, 0x52, 0x03,            //   850 (8.50 cm)
    LOGICAL_MAXIMUM_16, 0xe0, 0x2e,             //   12000
    USAGE_8,        0x31,                       //   Y
    HIDINPUT_8,     0x02,
    PHYSICAL_MAXIMUM_8, 0x00,                   //   reset
    UNIT_EXPONENT_8, 0x00,                      //   reset
    UNIT_8,         0x00,                       //   reset
    END_COLLECTION,

    // ----- Finger 2 (variant 1: with unit reset) ----------------------------
    USAGE_PAGE_8, 0x0d,
    USAGE_8, 0x22,
    COLLECTION_8, 0x02,
    LOGICAL_MAXIMUM_8, 0x01,
    USAGE_8, 0x47, USAGE_8, 0x42,
    REPORT_COUNT_8, 0x02, REPORT_SIZE_8, 0x01, HIDINPUT_8, 0x02,
    REPORT_SIZE_8, 0x01, REPORT_COUNT_8, 0x06, HIDINPUT_8, 0x03,
    REPORT_COUNT_8, 0x01, REPORT_SIZE_8, 0x20,
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0xff, 0xff,
    USAGE_8, 0x51, HIDINPUT_8, 0x02,
    USAGE_PAGE_8, 0x01,
    LOGICAL_MAXIMUM_16, 0x20, 0x4e, REPORT_SIZE_8, 0x10,
    UNIT_EXPONENT_8, 0x0e, UNIT_8, 0x11,
    USAGE_8, 0x30, PHYSICAL_MAXIMUM_16, 0x14, 0x05, REPORT_COUNT_8, 0x01, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_16, 0x52, 0x03, LOGICAL_MAXIMUM_16, 0xe0, 0x2e, USAGE_8, 0x31, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_8, 0x00, UNIT_EXPONENT_8, 0x00, UNIT_8, 0x00,
    END_COLLECTION,

    // ----- Finger 3 (variant 2: no unit reset) ------------------------------
    USAGE_PAGE_8, 0x0d,
    USAGE_8, 0x22,
    COLLECTION_8, 0x02,
    LOGICAL_MAXIMUM_8, 0x01,
    USAGE_8, 0x47, USAGE_8, 0x42,
    REPORT_COUNT_8, 0x02, REPORT_SIZE_8, 0x01, HIDINPUT_8, 0x02,
    REPORT_SIZE_8, 0x01, REPORT_COUNT_8, 0x06, HIDINPUT_8, 0x03,
    REPORT_COUNT_8, 0x01, REPORT_SIZE_8, 0x20,
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0xff, 0xff,
    USAGE_8, 0x51, HIDINPUT_8, 0x02,
    USAGE_PAGE_8, 0x01,
    LOGICAL_MAXIMUM_16, 0x20, 0x4e, REPORT_SIZE_8, 0x10,
    UNIT_EXPONENT_8, 0x0e, UNIT_8, 0x11,
    USAGE_8, 0x30, PHYSICAL_MAXIMUM_16, 0x14, 0x05, REPORT_COUNT_8, 0x01, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_16, 0x52, 0x03, LOGICAL_MAXIMUM_16, 0xe0, 0x2e, USAGE_8, 0x31, HIDINPUT_8, 0x02,
    END_COLLECTION, // no unit reset — matches WellspringT2.h collection 2

    // ----- Finger 4 (variant 1: with unit reset) ----------------------------
    USAGE_PAGE_8, 0x0d,
    USAGE_8, 0x22,
    COLLECTION_8, 0x02,
    LOGICAL_MAXIMUM_8, 0x01,
    USAGE_8, 0x47, USAGE_8, 0x42,
    REPORT_COUNT_8, 0x02, REPORT_SIZE_8, 0x01, HIDINPUT_8, 0x02,
    REPORT_SIZE_8, 0x01, REPORT_COUNT_8, 0x06, HIDINPUT_8, 0x03,
    REPORT_COUNT_8, 0x01, REPORT_SIZE_8, 0x20,
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0xff, 0xff,
    USAGE_8, 0x51, HIDINPUT_8, 0x02,
    USAGE_PAGE_8, 0x01,
    LOGICAL_MAXIMUM_16, 0x20, 0x4e, REPORT_SIZE_8, 0x10,
    UNIT_EXPONENT_8, 0x0e, UNIT_8, 0x11,
    USAGE_8, 0x30, PHYSICAL_MAXIMUM_16, 0x14, 0x05, REPORT_COUNT_8, 0x01, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_16, 0x52, 0x03, LOGICAL_MAXIMUM_16, 0xe0, 0x2e, USAGE_8, 0x31, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_8, 0x00, UNIT_EXPONENT_8, 0x00, UNIT_8, 0x00,
    END_COLLECTION,

    // ----- Finger 5 (variant 2: no unit reset) ------------------------------
    USAGE_PAGE_8, 0x0d,
    USAGE_8, 0x22,
    COLLECTION_8, 0x02,
    LOGICAL_MAXIMUM_8, 0x01,
    USAGE_8, 0x47, USAGE_8, 0x42,
    REPORT_COUNT_8, 0x02, REPORT_SIZE_8, 0x01, HIDINPUT_8, 0x02,
    REPORT_SIZE_8, 0x01, REPORT_COUNT_8, 0x06, HIDINPUT_8, 0x03,
    REPORT_COUNT_8, 0x01, REPORT_SIZE_8, 0x20,
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0xff, 0xff,
    USAGE_8, 0x51, HIDINPUT_8, 0x02,
    USAGE_PAGE_8, 0x01,
    LOGICAL_MAXIMUM_16, 0x20, 0x4e, REPORT_SIZE_8, 0x10,
    UNIT_EXPONENT_8, 0x0e, UNIT_8, 0x11,
    USAGE_8, 0x30, PHYSICAL_MAXIMUM_16, 0x14, 0x05, REPORT_COUNT_8, 0x01, HIDINPUT_8, 0x02,
    PHYSICAL_MAXIMUM_16, 0x52, 0x03, LOGICAL_MAXIMUM_16, 0xe0, 0x2e, USAGE_8, 0x31, HIDINPUT_8, 0x02,
    END_COLLECTION,

    // ----- Scan Time --------------------------------------------------------
    USAGE_PAGE_8,   0x0d,                       // Digitizer
    UNIT_EXPONENT_8, 0x0c,                      // -4 (100 µs)
    UNIT_16,        0x01, 0x10,                 // Second
    PHYSICAL_MAXIMUM_32, 0xff, 0xff, 0x00, 0x00,// 65535
    LOGICAL_MAXIMUM_32, 0xff, 0xff, 0x00, 0x00, // 65535
    USAGE_8,        0x56,                       // Scan Time
    HIDINPUT_8,     0x02,                       // 16-bit, inherited

    // ----- Contact Count ----------------------------------------------------
    USAGE_8,        0x54,                       // Contact Count
    LOGICAL_MAXIMUM_8, 0x7f,                    // 127
    REPORT_SIZE_8,  0x08,
    HIDINPUT_8,     0x02,

    // ----- Button -----------------------------------------------------------
    USAGE_PAGE_8,   0x09,                       // Button
    USAGE_8,        0x01,                       // Button 1
    LOGICAL_MAXIMUM_8, 0x01,
    REPORT_SIZE_8,  0x01,
    HIDINPUT_8,     0x02,                       // 1-bit button
    REPORT_COUNT_8, 0x07,
    HIDINPUT_8,     0x03,                       // 7-bit padding

    // ----- Feature: Device Capabilities (report ID 0x07) --------------------
    USAGE_PAGE_8,   0x0d,                       // Digitizer
    REPORT_ID_8,    REPORTID_DEVICE_CAPS,
    USAGE_8,        0x55,                       // Maximum Contacts
    USAGE_8,        0x59,                       // Touchpad Button Type
    LOGICAL_MINIMUM_8, 0x00,
    LOGICAL_MAXIMUM_16, 0xff, 0x00,             // 255
    REPORT_SIZE_8,  0x08,
    REPORT_COUNT_8, 0x02,
    FEATURE_8,      0x02,

    // ----- Feature: PTPHQA Certification (report ID 0x08) -------------------
    USAGE_PAGE_16,  0x00, 0xff,                 // Vendor Defined
    REPORT_ID_8,    REPORTID_PTPHQA,
    USAGE_8,        0xc5,
    LOGICAL_MINIMUM_8, 0x00,
    LOGICAL_MAXIMUM_16, 0xff, 0x00,             // 255
    REPORT_SIZE_8,  0x08,
    REPORT_COUNT_16, 0x00, 0x01,                // 256
    FEATURE_8,      0x02,

    END_COLLECTION,                             // End Touch Pad application

    // =========================================================================
    // TLC 2: Digitizer / Configuration
    // =========================================================================
    USAGE_PAGE_8,   0x0d,                       // Digitizer
    USAGE_8,        0x0e,                       // Configuration
    COLLECTION_8,   0x01,                       // Application

    // ----- Feature: Input Mode (report ID 0x04) -----------------------------
    REPORT_ID_8,    REPORTID_REPORTMODE,
    USAGE_8,        0x22,                       // Finger
    COLLECTION_8,   0x02,                       // Logical
    USAGE_8,        0x52,                       //   Input Mode
    LOGICAL_MINIMUM_8, 0x00,
    LOGICAL_MAXIMUM_8, ptp::MAX_CONTACTS,
    REPORT_SIZE_8,  0x08,
    REPORT_COUNT_8, 0x01,
    FEATURE_8,      0x02,
    END_COLLECTION,

    // ----- Feature: Function Switch (report ID 0x06) ------------------------
    COLLECTION_8,   0x00,                       // Physical
    REPORT_ID_8,    REPORTID_FUNCSWITCH,
    USAGE_8,        0x57,                       // Surface Switch
    USAGE_8,        0x58,                       // Button Switch
    REPORT_SIZE_8,  0x01,
    REPORT_COUNT_8, 0x02,
    LOGICAL_MAXIMUM_8, 0x01,
    FEATURE_8,      0x02,
    REPORT_COUNT_8, 0x06,
    FEATURE_8,      0x03,                       // Const padding
    END_COLLECTION,

    END_COLLECTION,                             // End Configuration application
];
