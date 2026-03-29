//! Wire protocol message types for host ↔ firmware UART communication.
//!
//! Messages are serialized with `postcard` and framed with COBS (`0x00`
//! delimiter). Both sides use `postcard::to_slice_cobs` to encode and
//! `postcard::CobsAccumulator` to stream-decode.
//!
//! The firmware is stateless: it knows which BLE devices are connected but
//! has no concept of slots or active targets. All routing decisions (slot
//! assignment, active device selection) are made by the host.

use serde::{Deserialize, Serialize};

use crate::keyboard::KeyboardReport;
use crate::ptp::PtpReport;

/// Messages from host to firmware.
#[derive(Serialize, Deserialize, Debug)]
pub enum HostMsg {
    /// Forward a keyboard HID report to all connected BLE devices.
    Keyboard(KeyboardReport),

    /// Forward a consumer control report (16-bit media key bitfield).
    Consumer(u16),

    /// Forward a PTP touch report to all connected BLE devices.
    Touch(PtpReport),

    /// Request the firmware to report all active BLE connections.
    QueryConnections,

    /// Handshake: host sends Ping, firmware responds with Pong.
    Ping,
}

/// Messages from firmware to host.
#[derive(Serialize, Deserialize, Debug)]
pub enum FirmwareMsg {
    /// Handshake response to `HostMsg::Ping`.
    Pong,

    /// Keyboard LED state changed (Caps/Num/Scroll Lock bits).
    LedState(u8),

    /// A BLE connection status change, or response to `QueryConnections`.
    /// The firmware reports the peer address and whether it connected or
    /// disconnected. Slot assignment is the host's responsibility.
    ConnectionStatus { addr: [u8; 6], connected: bool },
}
