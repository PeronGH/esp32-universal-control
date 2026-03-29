//! Shared protocol types for esp32-universal-control.
//!
//! Platform-independent (`no_std`) crate defining the semantic host↔firmware
//! protocol plus the shared HID report types/state machines used by the
//! ESP32-S3 firmware and the macOS host app.

#![no_std]

pub mod input;
pub mod keyboard;
pub mod ptp;
pub mod wire;
