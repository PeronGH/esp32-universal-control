//! Semantic input snapshots shared between the macOS host and firmware.

use serde::{Deserialize, Serialize};

use crate::ptp;

/// Maximum simultaneous non-modifier keys tracked in a keyboard snapshot.
pub const MAX_KEYS: usize = 6;
/// Maximum simultaneous touch contacts carried in a semantic touch frame.
pub const MAX_TOUCH_CONTACTS: usize = ptp::MAX_CONTACTS as usize;

/// Consumer control bitfield used by the semantic protocol.
pub type ConsumerState = u16;

/// Consumer usage bit: Scan Next Track.
pub const CONSUMER_NEXT_TRACK: ConsumerState = 1 << 0;
/// Consumer usage bit: Scan Previous Track.
pub const CONSUMER_PREVIOUS_TRACK: ConsumerState = 1 << 1;
/// Consumer usage bit: Stop.
pub const CONSUMER_STOP: ConsumerState = 1 << 2;
/// Consumer usage bit: Play/Pause.
pub const CONSUMER_PLAY_PAUSE: ConsumerState = 1 << 3;
/// Consumer usage bit: Mute.
pub const CONSUMER_MUTE: ConsumerState = 1 << 4;
/// Consumer usage bit: Volume Up.
pub const CONSUMER_VOLUME_UP: ConsumerState = 1 << 5;
/// Consumer usage bit: Volume Down.
pub const CONSUMER_VOLUME_DOWN: ConsumerState = 1 << 6;

/// Full keyboard state snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardSnapshot {
    /// USB HID modifier bitfield.
    pub modifiers: u8,
    /// Up to six simultaneous non-modifier HID usages.
    pub keys: [u8; MAX_KEYS],
}

/// One semantic touch contact in the current frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TouchContact {
    /// Stable contact identifier for the current finger lifecycle.
    pub contact_id: u32,
    /// X coordinate in PTP logical units (0..=20000).
    pub x: u16,
    /// Y coordinate in PTP logical units (0..=12000).
    pub y: u16,
    /// Whether the contact is touching the surface.
    pub touching: bool,
    /// Whether the contact is intentional / confident.
    pub confident: bool,
}

/// Semantic touch frame captured on the host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TouchFrame {
    /// Up to five current contacts.
    pub contacts: [TouchContact; MAX_TOUCH_CONTACTS],
    /// Number of populated contacts in `contacts`.
    pub contact_count: u8,
    /// Whether the clickpad button is pressed.
    pub button: bool,
}

impl TouchFrame {
    /// Returns the currently populated contacts in this frame.
    pub fn contacts(&self) -> &[TouchContact] {
        let len = usize::from(self.contact_count).min(MAX_TOUCH_CONTACTS);
        &self.contacts[..len]
    }
}
