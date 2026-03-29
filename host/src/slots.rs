//! Host-side slot table. Maps BLE addresses to numbered slots.
//! The firmware has no concept of slots — this is entirely host-side.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::serial;

pub const MAX_SLOTS: usize = 4;

/// Thread-safe slot table. Addresses are assigned on connect, cleared on
/// disconnect. Active slot is switched by hotkey.
pub struct SlotTable {
    slots: [Option<[u8; 6]>; MAX_SLOTS],
    active: AtomicUsize,
}

impl SlotTable {
    pub fn new() -> Self {
        Self {
            slots: [None; MAX_SLOTS],
            active: AtomicUsize::new(0),
        }
    }

    /// Assign an address to the first empty slot, or return its existing slot.
    pub fn connect(&mut self, addr: [u8; 6]) -> usize {
        if let Some(i) = self.slots.iter().position(|s| *s == Some(addr)) {
            return i;
        }
        if let Some(i) = self.slots.iter().position(|s| s.is_none()) {
            self.slots[i] = Some(addr);
            return i;
        }
        let active = self.active.load(Ordering::Relaxed);
        self.slots[active] = Some(addr);
        active
    }

    /// Clear the slot for a disconnected address.
    pub fn disconnect(&mut self, addr: [u8; 6]) {
        if let Some(slot) = self.slots.iter_mut().find(|s| **s == Some(addr)) {
            *slot = None;
        }
    }

    /// Get the active slot index.
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    /// Switch active slot. Returns false if out of range.
    pub fn set_active(&self, slot: usize) -> bool {
        if slot < MAX_SLOTS {
            self.active.store(slot, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Print all slots to stdout, marking the active one.
    pub fn print_status(&self) {
        for (i, slot) in self.slots.iter().enumerate() {
            let marker = if i == self.active() { "▶" } else { " " };
            match slot {
                Some(addr) => eprintln!("{marker} slot {i}: {}", serial::format_addr(addr)),
                None => eprintln!("{marker} slot {i}: ---"),
            }
        }
    }
}
