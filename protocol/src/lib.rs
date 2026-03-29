//! Shared protocol types for esp32-universal-control.
//!
//! Platform-independent (`no_std`) crate defining HID report descriptors,
//! report IDs, PTP report structs, and feature report data. Used by both
//! the ESP32-S3 firmware and the macOS host app.

#![no_std]

pub mod keyboard;
pub mod ptp;
