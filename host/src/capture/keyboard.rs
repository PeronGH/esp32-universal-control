//! Keyboard capture via CGEventTap.
//!
//! Creates an event tap at the HID level that observes all key events.
//! Translates macOS virtual keycodes to USB HID keycodes and sends
//! `HostMsg::Keyboard` reports over the serial channel.
//!
//! Must run on a thread with an active CFRunLoop.

use std::sync::mpsc;

use core_foundation::runloop::CFRunLoop;
use core_graphics::event::*;
use log::{info, warn};

use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::wire::HostMsg;

use super::keymap;

/// Maximum simultaneous non-modifier keys in a USB HID keyboard report.
const MAX_KEYS: usize = 6;

/// Start keyboard capture. Blocks the calling thread (runs CFRunLoop).
/// Key events are translated and sent to `tx`.
pub fn run(tx: mpsc::Sender<HostMsg>) -> anyhow::Result<()> {
    info!("Starting keyboard capture (CGEventTap)");

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
        ],
        move |_proxy, event_type, event| {
            if let Some(msg) = translate_event(event_type, event)
                && tx.send(msg).is_err()
            {
                warn!("Keyboard channel closed");
            }
            CallbackResult::Keep
        },
    )
    .map_err(|()| {
        anyhow::anyhow!("Failed to create CGEventTap — is Accessibility permission granted?")
    })?;

    tap.enable();
    info!("CGEventTap enabled, running CFRunLoop");
    CFRunLoop::run_current();

    Ok(())
}

/// Translate a macOS keyboard event to a `HostMsg::Keyboard`.
fn translate_event(event_type: CGEventType, event: &CGEvent) -> Option<HostMsg> {
    let mac_keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let flags = event.get_flags();
    let modifiers = keymap::flags_to_hid_modifiers(flags.bits());

    match event_type {
        CGEventType::KeyDown => {
            let hid_key = keymap::mac_to_hid(mac_keycode);
            // Modifier-only keys (0xE0+) are handled via FlagsChanged, skip here.
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
            // Key release: send report with the key removed.
            // For simplicity, send an empty keycodes array with current modifiers.
            Some(HostMsg::Keyboard(KeyboardReport {
                modifiers,
                reserved: 0,
                keycodes: [0; MAX_KEYS],
            }))
        }
        CGEventType::FlagsChanged => {
            // Modifier change only — send report with updated modifiers, no keys.
            Some(HostMsg::Keyboard(KeyboardReport {
                modifiers,
                reserved: 0,
                keycodes: [0; MAX_KEYS],
            }))
        }
        _ => None,
    }
}
