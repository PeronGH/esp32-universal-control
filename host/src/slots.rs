//! Host-side mirror of firmware-managed peer slots and forwarding state.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use esp32_uc_protocol::wire::{MAX_PEERS, PeerDescriptor, PeerSnapshot};

use crate::serial;

pub const MAX_SLOTS: usize = MAX_PEERS;

/// Thread-safe slot table mirrored from firmware state.
pub struct SlotTable {
    slots: [Option<[u8; 6]>; MAX_SLOTS],
    active: Option<usize>,
    /// Shared forwarding flag. Capture callbacks read this via
    /// `Arc<AtomicBool>` without locking the `SlotTable` mutex.
    forwarding: Arc<AtomicBool>,
}

impl SlotTable {
    /// Create an empty local-only slot table.
    pub fn new() -> Self {
        Self {
            slots: [None; MAX_SLOTS],
            active: None,
            forwarding: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Build a slot table from a firmware snapshot.
    pub fn from_snapshot(snapshot: &PeerSnapshot) -> Self {
        let mut table = Self::new();
        table.apply_snapshot(snapshot);
        table
    }

    /// Replace the mirrored state with a fresh firmware snapshot.
    pub fn apply_snapshot(&mut self, snapshot: &PeerSnapshot) {
        self.slots = [None; MAX_SLOTS];
        for peer in snapshot.peers.iter().flatten() {
            let slot = usize::from(peer.slot);
            if slot < MAX_SLOTS {
                self.slots[slot] = Some(peer.addr);
            }
        }
        self.set_active(snapshot.active_slot);
    }

    /// Get a clone of the forwarding flag for lock-free reading in callbacks.
    pub fn forwarding_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.forwarding)
    }

    /// Whether input is currently forwarded to a remote peer.
    pub fn is_forwarding(&self) -> bool {
        self.forwarding.load(Ordering::Acquire)
    }

    /// Whether a slot is currently populated.
    pub fn has_slot(&self, slot: usize) -> bool {
        slot < MAX_SLOTS && self.slots[slot].is_some()
    }

    /// Update one slot from a firmware connect event.
    pub fn connect(&mut self, peer: PeerDescriptor) {
        let slot = usize::from(peer.slot);
        if slot < MAX_SLOTS {
            self.slots[slot] = Some(peer.addr);
        }
    }

    /// Clear one slot from a firmware disconnect event.
    pub fn disconnect(&mut self, slot: u8) {
        let slot = usize::from(slot);
        if slot >= MAX_SLOTS {
            return;
        }

        self.slots[slot] = None;
        if self.active == Some(slot) {
            self.active = None;
            self.forwarding.store(false, Ordering::Release);
        }
    }

    /// Mirror the firmware-selected active slot.
    pub fn set_active(&mut self, slot: Option<u8>) {
        let next_active = slot
            .map(usize::from)
            .filter(|slot| *slot < MAX_SLOTS && self.slots[*slot].is_some());
        self.active = next_active;
        self.forwarding
            .store(next_active.is_some(), Ordering::Release);
    }

    /// Returns the currently active slot, if any.
    pub fn active(&self) -> Option<usize> {
        self.active
    }

    /// Print current slot and forwarding status.
    pub fn print_status(&self) {
        let forwarding = self.is_forwarding();
        let active = self.active();
        let mac_marker = if !forwarding { "▶" } else { " " };
        eprintln!("{mac_marker} Mac (Ctrl+Opt+1)");
        for (i, slot) in self.slots.iter().enumerate() {
            let Some(addr) = slot else { continue };
            let marker = if forwarding && active == Some(i) {
                "▶"
            } else {
                " "
            };
            let num = i + 2;
            eprintln!(
                "{marker} slot {i}: {} (Ctrl+Opt+{num})",
                serial::format_addr(addr)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_snapshot_state() {
        let mut peers = [None; MAX_SLOTS];
        peers[1] = Some(PeerDescriptor {
            slot: 1,
            addr: [1, 2, 3, 4, 5, 6],
        });
        let table = SlotTable::from_snapshot(&PeerSnapshot {
            peers,
            active_slot: Some(1),
        });

        assert!(table.has_slot(1));
        assert!(table.is_forwarding());
        assert_eq!(table.active(), Some(1));
    }

    #[test]
    fn disconnecting_active_slot_returns_to_local() {
        let mut peers = [None; MAX_SLOTS];
        peers[0] = Some(PeerDescriptor {
            slot: 0,
            addr: [1, 2, 3, 4, 5, 6],
        });
        let mut table = SlotTable::from_snapshot(&PeerSnapshot {
            peers,
            active_slot: Some(0),
        });

        table.disconnect(0);

        assert!(!table.is_forwarding());
        assert_eq!(table.active(), None);
        assert!(!table.has_slot(0));
    }
}
