//! Host-side slot table and forwarding state.
//!
//! Slots 0-3 track remote BLE devices. A separate `forwarding` flag
//! controls whether input is sent to a remote device or stays local (Mac).
//! The firmware has no concept of "Mac"; this is purely host-side.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::serial;

pub const MAX_SLOTS: usize = 4;

/// Thread-safe slot table with forwarding state.
pub struct SlotTable {
    slots: [Option<[u8; 6]>; MAX_SLOTS],
    /// Which remote slot is targeted (0-3). Only relevant when forwarding.
    active: AtomicUsize,
    /// When true, input is forwarded to the active remote slot.
    /// When false, input stays on Mac (local).
    forwarding: AtomicBool,
}

impl SlotTable {
    pub fn new() -> Self {
        Self {
            slots: [None; MAX_SLOTS],
            active: AtomicUsize::new(0),
            forwarding: AtomicBool::new(false),
        }
    }

    /// Whether input is being forwarded to a remote device.
    pub fn is_forwarding(&self) -> bool {
        self.forwarding.load(Ordering::Acquire)
    }

    /// Switch to Mac (local). Stops forwarding.
    pub fn switch_to_mac(&self) {
        self.forwarding.store(false, Ordering::Release);
    }

    /// Switch to a remote slot. Starts forwarding.
    /// Returns false if slot is out of range.
    pub fn switch_to_remote(&self, slot: usize) -> bool {
        if slot < MAX_SLOTS {
            self.active.store(slot, Ordering::Relaxed);
            self.forwarding.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Get the active remote slot index.
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
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

    /// For debug CLI compatibility.
    pub fn set_active(&self, slot: usize) -> bool {
        self.switch_to_remote(slot)
    }

    /// Print active target and connected slots to stderr.
    /// Only shows slots that have a device. Each slot shows its hotkey.
    pub fn print_status(&self) {
        let forwarding = self.is_forwarding();
        let active = self.active();
        let mac_marker = if !forwarding { "▶" } else { " " };
        eprintln!("{mac_marker} Mac (Ctrl+Opt+1)");
        for (i, slot) in self.slots.iter().enumerate() {
            let Some(addr) = slot else { continue };
            let marker = if forwarding && i == active {
                "▶"
            } else {
                " "
            };
            let num = i + 2; // slot 0 = Ctrl+Opt+2, slot 1 = Ctrl+Opt+3, ...
            eprintln!(
                "{marker} slot {i}: {} (Ctrl+Opt+{num})",
                serial::format_addr(addr)
            );
        }
    }
}
