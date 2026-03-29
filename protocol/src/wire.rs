//! Wire protocol message types for host ↔ firmware UART communication.
//!
//! Messages are serialized with `postcard` and framed with COBS (`0x00`
//! delimiter). Both sides use `postcard::to_slice_cobs` to encode and
//! `postcard::CobsAccumulator` to stream-decode.
//!
//! The protocol is semantic and versioned: the host sends normalized input
//! snapshots while firmware owns BLE peer state and final HID report emission.

use serde::{Deserialize, Serialize};

use crate::input::{ConsumerState, KeyboardSnapshot, TouchFrame};

/// Current host↔firmware protocol version.
pub const PROTOCOL_VERSION: u16 = 1;
/// Maximum remote peers tracked by the semantic session layer.
pub const MAX_PEERS: usize = 4;

/// Host hello message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Requested protocol version.
    pub protocol_version: u16,
}

/// Firmware acknowledgement for a successful hello handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAck {
    /// Negotiated protocol version.
    pub protocol_version: u16,
    /// Maximum peer slots exposed by firmware.
    pub max_peers: u8,
}

/// One visible peer in the host-facing slot table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerDescriptor {
    /// Slot index assigned by firmware.
    pub slot: u8,
    /// Peer BLE address in little-endian byte order.
    pub addr: [u8; 6],
}

/// Full firmware peer snapshot sent after a successful handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerSnapshot {
    /// Current peer table.
    pub peers: [Option<PeerDescriptor>; MAX_PEERS],
    /// Currently active slot, or `None` for local/Mac mode.
    pub active_slot: Option<u8>,
}

/// Recoverable protocol error reported by firmware.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolError {
    /// The host requested an unsupported protocol version.
    UnsupportedProtocolVersion {
        /// Protocol version supported by firmware.
        expected: u16,
        /// Protocol version sent by the host.
        received: u16,
    },
    /// The host selected a slot that is out of range or currently empty.
    InvalidPeerSlot(u8),
}

/// Messages from host to firmware.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostMsg {
    /// Version handshake.
    Hello(Hello),

    /// Select the active peer slot, or `None` for local/Mac mode.
    SelectPeer(Option<u8>),

    /// Latest keyboard snapshot.
    KeyboardState(KeyboardSnapshot),

    /// Latest consumer control snapshot.
    ConsumerState(ConsumerState),

    /// Latest touch frame.
    TouchFrame(TouchFrame),
}

/// Messages from firmware to host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FirmwareMsg {
    /// Handshake response.
    HelloAck(HelloAck),

    /// Initial peer snapshot after handshake.
    PeerSnapshot(PeerSnapshot),

    /// A new peer connected and was assigned a slot.
    PeerConnected(PeerDescriptor),

    /// A peer disconnected and freed its slot.
    PeerDisconnected { slot: u8 },

    /// Active peer selection changed.
    ActivePeerChanged(Option<u8>),

    /// Keyboard LED state changed (Caps/Num/Scroll Lock bits).
    LedState(u8),

    /// Recoverable protocol error.
    ProtocolError(ProtocolError),
}
