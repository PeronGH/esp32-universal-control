//! Keyboard + click capture via CGEventTap.
//!
//! When forwarding to a remote slot, keyboard events are suppressed locally
//! (CallbackResult::Drop) and sent to the firmware. When targeting Mac,
//! events pass through normally. Ctrl+Shift+F1 = Mac, Ctrl+Shift+F2-F5 = remote.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::*;
use log::info;

use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::wire::HostMsg;

use super::keymap;
use crate::slots::SlotTable;

const MAX_KEYS: usize = 6;

/// macOS virtual keycodes for F1-F5.
const MAC_F1: u16 = 0x7A;
const MAC_F2: u16 = 0x78;
const MAC_F3: u16 = 0x63;
const MAC_F4: u16 = 0x76;
const MAC_F5: u16 = 0x60;

/// Start keyboard + click capture. Blocks the calling thread (runs CFRunLoop).
pub fn run(
    tx: mpsc::Sender<HostMsg>,
    click_state: Arc<AtomicBool>,
    slots: Arc<Mutex<SlotTable>>,
) -> anyhow::Result<()> {
    info!("Starting keyboard + click capture (CGEventTap)");

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        // Default (not ListenOnly) allows us to suppress events.
        CGEventTapOptions::Default,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
        ],
        move |_proxy, event_type, event| {
            let forwarding = slots.lock().expect("poisoned").is_forwarding();

            match event_type {
                CGEventType::LeftMouseDown => {
                    if forwarding {
                        click_state.store(true, Ordering::Release);
                    }
                    // Never suppress mouse clicks; Mac needs them for UI.
                    CallbackResult::Keep
                }
                CGEventType::LeftMouseUp => {
                    click_state.store(false, Ordering::Release);
                    CallbackResult::Keep
                }
                CGEventType::KeyDown => {
                    // Hotkeys are always processed, never suppressed.
                    if handle_slot_hotkey(event, &slots) {
                        return CallbackResult::Keep;
                    }
                    if forwarding {
                        if let Some(msg) = translate_key_event(event_type, event) {
                            let _ = tx.send(msg);
                        }
                        CallbackResult::Drop
                    } else {
                        CallbackResult::Keep
                    }
                }
                CGEventType::KeyUp => {
                    if forwarding {
                        if let Some(msg) = translate_key_event(event_type, event) {
                            let _ = tx.send(msg);
                        }
                        CallbackResult::Drop
                    } else {
                        CallbackResult::Keep
                    }
                }
                CGEventType::FlagsChanged => {
                    if forwarding && let Some(msg) = translate_key_event(event_type, event) {
                        let _ = tx.send(msg);
                    }
                    // Always let modifier changes through to keep Mac in sync.
                    CallbackResult::Keep
                }
                _ => CallbackResult::Keep,
            }
        },
    )
    .map_err(|()| {
        anyhow::anyhow!("Failed to create CGEventTap. Is Accessibility permission granted?")
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

/// Check if a KeyDown is Ctrl+Shift+F1-F5. If so, switch target and return true.
fn handle_slot_hotkey(event: &CGEvent, slots: &Mutex<SlotTable>) -> bool {
    let flags = event.get_flags();
    if !flags.contains(CGEventFlags::CGEventFlagControl | CGEventFlags::CGEventFlagShift) {
        return false;
    }

    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let table = slots.lock().expect("poisoned");

    match keycode {
        MAC_F1 => {
            table.switch_to_mac();
            info!("→ Mac (local)");
            table.print_status();
            true
        }
        MAC_F2 => {
            table.switch_to_remote(0);
            info!("→ Remote slot 0");
            table.print_status();
            true
        }
        MAC_F3 => {
            table.switch_to_remote(1);
            info!("→ Remote slot 1");
            table.print_status();
            true
        }
        MAC_F4 => {
            table.switch_to_remote(2);
            info!("→ Remote slot 2");
            table.print_status();
            true
        }
        MAC_F5 => {
            table.switch_to_remote(3);
            info!("→ Remote slot 3");
            table.print_status();
            true
        }
        _ => false,
    }
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
