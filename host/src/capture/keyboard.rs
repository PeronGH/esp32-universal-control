//! Keyboard + click capture via CGEventTap.
//!
//! Creates an event tap at the HID level that observes key events and
//! trackpad click events. Key events are translated to USB HID and sent
//! as `HostMsg::Keyboard`. Click state is shared with the trackpad module
//! via an `AtomicBool`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::*;
use log::{info, warn};

use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::wire::HostMsg;

use super::keymap;

const MAX_KEYS: usize = 6;

/// Start keyboard + click capture. Blocks the calling thread (runs CFRunLoop).
pub fn run(tx: mpsc::Sender<HostMsg>, click_state: Arc<AtomicBool>) -> anyhow::Result<()> {
    info!("Starting keyboard + click capture (CGEventTap)");

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
