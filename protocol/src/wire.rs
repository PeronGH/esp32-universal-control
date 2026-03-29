//! Wire protocol message types for host ↔ firmware UART communication.
//!
//! Messages are serialized with `postcard` and framed with COBS (`0x00`
//! delimiter). Both sides use `postcard::to_slice_cobs` to encode and
//! `postcard::CobsAccumulator` to stream-decode.

use serde::{Deserialize, Serialize};

use crate::keyboard::KeyboardReport;
use crate::ptp::PtpReport;

/// Maximum BLE device slots the firmware supports.
pub const MAX_SLOTS: usize = 4;

/// Messages from host to firmware.
#[derive(Serialize, Deserialize, Debug)]
pub enum HostMsg {
    /// Send a keyboard HID report to the active BLE slot.
    Keyboard(KeyboardReport),

    /// Send a consumer control report (16-bit media key bitfield).
    Consumer(u16),

    /// Send a PTP touch report to the active BLE slot.
    Touch(PtpReport),

    /// Switch which BLE slot receives subsequent HID reports.
    SwitchSlot(u8),

    /// Assign a bonded device address to a slot.
    /// The firmware will auto-reconnect to this device.
    SetSlotDevice { slot: u8, addr: [u8; 6] },

    /// Request the firmware to report all slot statuses.
    QuerySlots,

    /// Handshake: host sends Ping, firmware responds with Pong.
    Ping,
}

/// Messages from firmware to host.
#[derive(Serialize, Deserialize, Debug)]
pub enum FirmwareMsg {
    /// Keyboard LED state changed (Caps/Num/Scroll Lock bits).
    LedState(u8),

    /// Handshake response to `HostMsg::Ping`.
    Pong,

    /// A BLE slot's connection status changed.
    SlotStatus {
        slot: u8,
        /// Peer BLE address. The firmware cannot read the peer's device
        /// name — it is a HOGP server. Naming is host-side.
        addr: [u8; 6],
        connected: bool,
    },
}
