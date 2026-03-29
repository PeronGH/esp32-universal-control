//! Keyboard + mouse event capture via CGEventTap.
//!
//! Uses `Default` mode to suppress keyboard and mouse events when forwarding
//! to a remote device. The hot path is lock-free (reads a single AtomicBool).
//! The mutex is only locked on the rare hotkey press (Ctrl+Opt+1-5).

use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use core_foundation::base::TCFType;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::*;
use log::info;

use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::wire::HostMsg;

use super::keymap;
use crate::slots::SlotTable;

const MAX_KEYS: usize = 6;

/// macOS virtual keycodes for number keys 1-5.
const MAC_1: u16 = 0x12;
const MAC_2: u16 = 0x13;
const MAC_3: u16 = 0x14;
const MAC_4: u16 = 0x15;
const MAC_5: u16 = 0x17;

/// Stored mach port ref for re-enabling the tap on TapDisabledByTimeout.
static TAP_MACH_PORT: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

unsafe extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGDisplayHideCursor(display: u32) -> i32;
    fn CGDisplayShowCursor(display: u32) -> i32;
}

/// Main display ID (kCGDirectMainDisplay).
const MAIN_DISPLAY: u32 = 0;

fn hide_mac_cursor() {
    // SAFETY: CGDisplayHideCursor is safe to call with a valid display ID.
    unsafe { CGDisplayHideCursor(MAIN_DISPLAY) };
}

pub fn show_mac_cursor() {
    // SAFETY: CGDisplayShowCursor is safe to call with a valid display ID.
    unsafe { CGDisplayShowCursor(MAIN_DISPLAY) };
}

/// Re-enable the event tap after macOS disabled it due to timeout.
fn reenable_tap() {
    let ptr = TAP_MACH_PORT.load(Ordering::Acquire);
    if !ptr.is_null() {
        // SAFETY: ptr is a valid CFMachPortRef stored after tap creation.
        unsafe { CGEventTapEnable(ptr as CFMachPortRef, true) };
    }
}

/// Start keyboard + mouse capture. Blocks the calling thread (runs CFRunLoop).
pub fn run(
    tx: mpsc::Sender<HostMsg>,
    click_state: Arc<AtomicBool>,
    forwarding: Arc<AtomicBool>,
    slots: Arc<Mutex<SlotTable>>,
) -> anyhow::Result<()> {
    info!("Starting keyboard + mouse capture (CGEventTap)");

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![
            // Keyboard
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
        ],
        move |_proxy, event_type, event| {
            // Handle tap timeout/disable by re-enabling.
            match event_type {
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                    info!("CGEventTap was disabled, re-enabling");
                    reenable_tap();
                    return CallbackResult::Keep;
                }
                _ => {}
            }

            // Lock-free: single atomic read, no mutex.
            let fwd = forwarding.load(Ordering::Acquire);

            match event_type {
                CGEventType::KeyDown => {
                    // Hotkeys always processed (locks mutex, but rare).
                    if handle_slot_hotkey(event, &slots) {
                        return CallbackResult::Keep;
                    }
                    if fwd {
                        if let Some(msg) = translate_key_event(event_type, event) {
                            let _ = tx.send(msg);
                        }
                        CallbackResult::Drop
                    } else {
                        CallbackResult::Keep
                    }
                }
                CGEventType::KeyUp => {
                    if fwd {
                        if let Some(msg) = translate_key_event(event_type, event) {
                            let _ = tx.send(msg);
                        }
                        CallbackResult::Drop
                    } else {
                        CallbackResult::Keep
                    }
                }
                CGEventType::FlagsChanged => {
                    if fwd && let Some(msg) = translate_key_event(event_type, event) {
                        let _ = tx.send(msg);
                    }
                    // Always keep modifier changes so Mac stays in sync.
                    CallbackResult::Keep
                }

                // Click detection for PTP button field.
                CGEventType::LeftMouseDown => {
                    if fwd {
                        click_state.store(true, Ordering::Release);
                    }
                    CallbackResult::Keep
                }
                CGEventType::LeftMouseUp => {
                    click_state.store(false, Ordering::Release);
                    CallbackResult::Keep
                }
                _ => CallbackResult::Keep,
            }
        },
    )
    .map_err(|()| {
        anyhow::anyhow!("Failed to create CGEventTap. Is Accessibility permission granted?")
    })?;

    // Store the mach port for re-enabling on timeout.
    let port_ref = tap.mach_port().as_concrete_TypeRef();
    TAP_MACH_PORT.store(port_ref as *mut _, Ordering::Release);

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

/// Check if a KeyDown is Ctrl+Opt+1-5. If so, switch target and return true.
fn handle_slot_hotkey(event: &CGEvent, slots: &Mutex<SlotTable>) -> bool {
    let flags = event.get_flags();
    let ctrl_opt = CGEventFlags::CGEventFlagControl | CGEventFlags::CGEventFlagAlternate;
    if !flags.contains(ctrl_opt) {
        return false;
    }

    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let table = slots.lock().expect("poisoned");

    let switched = match keycode {
        MAC_1 => {
            table.switch_to_mac();
            show_mac_cursor();
            info!("Switched to Mac (local)");
            true
        }
        MAC_2 => {
            table.switch_to_remote(0);
            true
        }
        MAC_3 => {
            table.switch_to_remote(1);
            true
        }
        MAC_4 => {
            table.switch_to_remote(2);
            true
        }
        MAC_5 => {
            table.switch_to_remote(3);
            true
        }
        _ => false,
    };

    if switched && table.is_forwarding() {
        hide_mac_cursor();
        info!("Switched to remote slot {}", table.active());
    }

    if switched {
        table.print_status();
    }

    switched
}

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
