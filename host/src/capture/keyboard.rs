//! Keyboard + click capture via CGEventTap.
//!
//! Creates an event tap at the HID level that observes key events and
//! trackpad click events. Key events are translated to USB HID and sent
//! as `HostMsg::Keyboard`. Click state is shared with the trackpad module
//! via an `AtomicBool`. Ctrl+Shift+F1-F4 switches the active slot.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::*;
use log::{info, warn};

use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::wire::HostMsg;

use super::keymap;
use crate::slots::SlotTable;

const MAX_KEYS: usize = 6;

/// macOS virtual keycodes for F1-F4.
const MAC_F1: u16 = 0x7A;
const MAC_F2: u16 = 0x78;
const MAC_F3: u16 = 0x63;
const MAC_F4: u16 = 0x76;

/// Start keyboard + click capture. Blocks the calling thread (runs CFRunLoop).
pub fn run(
    tx: mpsc::Sender<HostMsg>,
    click_state: Arc<AtomicBool>,
    slots: Arc<std::sync::Mutex<SlotTable>>,
) -> anyhow::Result<()> {
    info!("Starting keyboard + click capture (CGEventTap)");
    info!("Ctrl+Shift+F1-F4 to switch active slot");

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
        ],
        move |_proxy, event_type, event| {
            match event_type {
                CGEventType::LeftMouseDown => {
                    click_state.store(true, Ordering::Release);
                }
                CGEventType::LeftMouseUp => {
                    click_state.store(false, Ordering::Release);
                }
                CGEventType::KeyDown => {
                    if handle_slot_hotkey(event, &slots) {
                        // Consumed — don't forward to firmware.
                        return CallbackResult::Keep;
                    }
                    if let Some(msg) = translate_key_event(event_type, event)
                        && tx.send(msg).is_err()
                    {
                        warn!("Keyboard channel closed");
                    }
                }
                _ => {
                    if let Some(msg) = translate_key_event(event_type, event)
                        && tx.send(msg).is_err()
                    {
                        warn!("Keyboard channel closed");
                    }
                }
            }
            CallbackResult::Keep
        },
    )
    .map_err(|()| {
        anyhow::anyhow!("Failed to create CGEventTap — is Accessibility permission granted?")
    })?;

    let loop_source = tap
        .mach_port()
        .create_runloop_source(0)
        .expect("Failed to create run loop source from event tap");
    CFRunLoop::get_current().add_source(&loop_source, unsafe { kCFRunLoopCommonModes });

    tap.enable();
    info!("CGEventTap enabled, running CFRunLoop");
    CFRunLoop::run_current();

    Ok(())
}

/// Check if a KeyDown event is Ctrl+Shift+F1-F4. If so, switch the active
/// slot and return true (consumed). Otherwise return false (forward normally).
fn handle_slot_hotkey(event: &CGEvent, slots: &std::sync::Mutex<SlotTable>) -> bool {
    let flags = event.get_flags();
    let has_ctrl_shift =
        flags.contains(CGEventFlags::CGEventFlagControl | CGEventFlags::CGEventFlagShift);
    if !has_ctrl_shift {
        return false;
    }

    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let slot = match keycode {
        MAC_F1 => 0,
        MAC_F2 => 1,
        MAC_F3 => 2,
        MAC_F4 => 3,
        _ => return false,
    };

    let table = slots.lock().expect("slot table poisoned");
    if table.set_active(slot) {
        info!("Switched to slot {slot}");
        table.print_status();
    }
    true
}

/// Translate a macOS keyboard event to a `HostMsg::Keyboard`.
fn translate_key_event(event_type: CGEventType, event: &CGEvent) -> Option<HostMsg> {
    let mac_keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let flags = event.get_flags();
    let modifiers = keymap::flags_to_hid_modifiers(flags.bits());

    match event_type {
        CGEventType::KeyDown => {
            let hid_key = keymap::mac_to_hid(mac_keycode);
            if hid_key >= 0xE0 {
                return None;
            }
            Some(HostMsg::Keyboard(KeyboardReport {
                modifiers,
                reserved: 0,
                keycodes: [hid_key, 0, 0, 0, 0, 0],
            }))
        }
        CGEventType::KeyUp => {
            let hid_key = keymap::mac_to_hid(mac_keycode);
            if hid_key >= 0xE0 {
                return None;
            }
            Some(HostMsg::Keyboard(KeyboardReport {
                modifiers,
                reserved: 0,
                keycodes: [0; MAX_KEYS],
            }))
        }
        CGEventType::FlagsChanged => Some(HostMsg::Keyboard(KeyboardReport {
            modifiers,
            reserved: 0,
            keycodes: [0; MAX_KEYS],
        })),
        _ => None,
    }
}
