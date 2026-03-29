//! Keyboard + mouse event capture via CGEventTap.
//!
//! Uses `Default` mode to suppress keyboard and mouse events when forwarding
//! to a remote device. The hot path is lock-free (reads a single AtomicBool).
//! The mutex is only locked on the rare hotkey press (`Esc+1-5`).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};

use core_foundation::base::TCFType;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::*;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use log::info;

use esp32_uc_protocol::input::{ConsumerState, KeyboardSnapshot};
use esp32_uc_protocol::wire::HostMsg;

use super::keymap;
use super::outbox::Outbox;
use crate::slots::SlotTable;

/// macOS virtual keycodes for number keys 1-5.
const MAC_1: u16 = 0x12;
const MAC_2: u16 = 0x13;
const MAC_3: u16 = 0x14;
const MAC_4: u16 = 0x15;
const MAC_5: u16 = 0x17;
const MAC_ESCAPE: u16 = 0x35;

/// Stored mach port ref for re-enabling the tap on TapDisabledByTimeout.
static TAP_MACH_PORT: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

unsafe extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGDisplayHideCursor(display: u32) -> i32;
    fn CGDisplayShowCursor(display: u32) -> i32;
    fn CGSSetConnectionProperty(
        connection: i32,
        target: i32,
        key: core_foundation::string::CFStringRef,
        value: core_foundation::base::CFTypeRef,
    ) -> i32;
    fn _CGSDefaultConnection() -> i32;
}

const MAIN_DISPLAY: u32 = 0;

/// Enable hiding cursor from a background process (private CG API,
/// same approach as Barrier KVM) and hide the cursor.
pub fn hide_mac_cursor() {
    unsafe {
        // Allow cursor hide from a non-foreground app.
        let key = core_foundation::string::CFString::new("SetsCursorInBackground");
        CGSSetConnectionProperty(
            _CGSDefaultConnection(),
            _CGSDefaultConnection(),
            key.as_concrete_TypeRef(),
            core_foundation::boolean::CFBoolean::true_value().as_CFTypeRef(),
        );
        CGDisplayHideCursor(MAIN_DISPLAY);
    }
}

/// Show the local macOS cursor.
pub fn show_mac_cursor() {
    unsafe {
        CGDisplayShowCursor(MAIN_DISPLAY);
    }
}

/// Re-enable the event tap after macOS disabled it due to timeout.
fn reenable_tap() {
    let ptr = TAP_MACH_PORT.load(Ordering::Acquire);
    if !ptr.is_null() {
        // SAFETY: ptr is a valid CFMachPortRef stored after tap creation.
        unsafe { CGEventTapEnable(ptr as CFMachPortRef, true) };
    }
}

#[derive(Debug, Default)]
struct CaptureState {
    keyboard: KeyboardSnapshot,
    consumer: ConsumerState,
    hotkey_keyup: Option<u16>,
    escape_state: EscapeState,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum EscapeState {
    #[default]
    Idle,
    Pending,
    Forwarded,
    ChordUsed,
}

impl CaptureState {
    fn reset_remote_input(&mut self) {
        self.keyboard = KeyboardSnapshot::default();
        self.consumer = 0;
    }

    fn arm_hotkey_keyup(&mut self, keycode: u16) {
        self.hotkey_keyup = Some(keycode);
    }

    fn consume_hotkey_keyup(&mut self, keycode: u16) -> bool {
        if self.hotkey_keyup == Some(keycode) {
            self.hotkey_keyup = None;
            true
        } else {
            false
        }
    }

    fn handle_key_down(&mut self, event: &CGEvent) -> Option<HostMsg> {
        let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
        if let Some(bit) = keymap::mac_to_consumer(keycode) {
            let next = self.consumer | bit;
            if next != self.consumer {
                self.consumer = next;
                return Some(HostMsg::ConsumerState(self.consumer));
            }
            return None;
        }

        let hid = keymap::mac_to_hid(keycode);
        self.update_hid_key_state(hid, true)
    }

    fn handle_key_up(&mut self, event: &CGEvent) -> Option<HostMsg> {
        let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
        if let Some(bit) = keymap::mac_to_consumer(keycode) {
            let next = self.consumer & !bit;
            if next != self.consumer {
                self.consumer = next;
                return Some(HostMsg::ConsumerState(self.consumer));
            }
            return None;
        }

        let hid = keymap::mac_to_hid(keycode);
        self.update_hid_key_state(hid, false)
    }

    fn update_hid_key_state(&mut self, hid: u8, pressed: bool) -> Option<HostMsg> {
        if hid == 0 {
            return None;
        }

        if let Some(mask) = modifier_mask_from_hid(hid) {
            let next = if pressed {
                self.keyboard.modifiers | mask
            } else {
                self.keyboard.modifiers & !mask
            };
            if next != self.keyboard.modifiers {
                self.keyboard.modifiers = next;
                return Some(HostMsg::KeyboardState(self.keyboard));
            }
            return None;
        }

        if pressed {
            if self.keyboard.keys.contains(&hid) {
                return None;
            }
            if let Some(slot) = self.keyboard.keys.iter_mut().find(|slot| **slot == 0) {
                *slot = hid;
                return Some(HostMsg::KeyboardState(self.keyboard));
            }
            return None;
        }

        if let Some(idx) = self.keyboard.keys.iter().position(|current| *current == hid) {
            self.keyboard.keys.copy_within((idx + 1).., idx);
            self.keyboard.keys[self.keyboard.keys.len() - 1] = 0;
            return Some(HostMsg::KeyboardState(self.keyboard));
        }

        None
    }

    fn handle_flags_changed(&mut self, event: &CGEvent) -> Option<HostMsg> {
        let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
        let mask = keymap::modifier_mask(keycode)?;
        let generic_pressed = event.get_flags().bits() & generic_modifier_flag(mask) != 0;
        let next = apply_modifier_transition(self.keyboard.modifiers, mask, generic_pressed);
        if next != self.keyboard.modifiers {
            self.keyboard.modifiers = next;
            Some(HostMsg::KeyboardState(self.keyboard))
        } else {
            None
        }
    }
}

fn modifier_mask_from_hid(hid: u8) -> Option<u8> {
    (0xE0..=0xE7).contains(&hid).then(|| 1 << (hid - 0xE0))
}

fn generic_modifier_flag(mask: u8) -> u64 {
    match mask {
        0x01 | 0x10 => CGEventFlags::CGEventFlagControl.bits(),
        0x02 | 0x20 => CGEventFlags::CGEventFlagShift.bits(),
        0x04 | 0x40 => CGEventFlags::CGEventFlagAlternate.bits(),
        0x08 | 0x80 => CGEventFlags::CGEventFlagCommand.bits(),
        _ => 0,
    }
}

fn modifier_pair_mask(mask: u8) -> u8 {
    match mask {
        0x01 | 0x10 => 0x11,
        0x02 | 0x20 => 0x22,
        0x04 | 0x40 => 0x44,
        0x08 | 0x80 => 0x88,
        _ => mask,
    }
}

fn apply_modifier_transition(current: u8, mask: u8, generic_pressed: bool) -> u8 {
    let exact_pressed = current & mask != 0;
    let sibling_pressed = current & (modifier_pair_mask(mask) & !mask) != 0;

    if generic_pressed {
        if exact_pressed && sibling_pressed {
            current & !mask
        } else {
            current | mask
        }
    } else {
        current & !mask
    }
}

fn emit_local_escape(keydown: bool) {
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
        return;
    };
    let Ok(event) = CGEvent::new_keyboard_event(source, MAC_ESCAPE, keydown) else {
        return;
    };
    event.post(CGEventTapLocation::HID);
}

fn flush_pending_escape(state: &mut CaptureState, tx: &Outbox, forwarding: bool) {
    if forwarding {
        if let Some(msg) = state.update_hid_key_state(keymap::mac_to_hid(MAC_ESCAPE), true) {
            tx.push(msg);
        }
    } else {
        emit_local_escape(true);
    }
    state.escape_state = EscapeState::Forwarded;
}

fn finish_escape(state: &mut CaptureState, tx: &Outbox, forwarding: bool) {
    match state.escape_state {
        EscapeState::Pending => {
            if forwarding {
                if let Some(msg) = state.update_hid_key_state(keymap::mac_to_hid(MAC_ESCAPE), true)
                {
                    tx.push(msg);
                }
                if let Some(msg) =
                    state.update_hid_key_state(keymap::mac_to_hid(MAC_ESCAPE), false)
                {
                    tx.push(msg);
                }
            } else {
                emit_local_escape(true);
                emit_local_escape(false);
            }
        }
        EscapeState::Forwarded => {
            if forwarding {
                if let Some(msg) = state.update_hid_key_state(keymap::mac_to_hid(MAC_ESCAPE), false)
                {
                    tx.push(msg);
                }
            } else {
                emit_local_escape(false);
            }
        }
        EscapeState::ChordUsed | EscapeState::Idle => {}
    }
    state.escape_state = EscapeState::Idle;
}

/// Start keyboard + mouse capture. Blocks the calling thread (runs CFRunLoop).
pub fn run(
    tx: Arc<Outbox>,
    click_state: Arc<AtomicBool>,
    forwarding: Arc<AtomicBool>,
    slots: Arc<Mutex<SlotTable>>,
) -> anyhow::Result<()> {
    info!("Starting keyboard + mouse capture (CGEventTap)");
    let state = RefCell::new(CaptureState::default());

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGEventType::MouseMoved,
            CGEventType::LeftMouseDragged,
            CGEventType::RightMouseDragged,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGEventType::OtherMouseDragged,
            CGEventType::ScrollWheel,
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
            if !fwd {
                state.borrow_mut().reset_remote_input();
            }

            match event_type {
                CGEventType::KeyDown => {
                    let keycode =
                        event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                    if keycode == MAC_ESCAPE {
                        state.borrow_mut().escape_state = EscapeState::Pending;
                        return CallbackResult::Drop;
                    }

                    // Hotkeys always processed (locks mutex, but rare).
                    if handle_slot_hotkey(event, &state, &slots, &tx) {
                        state.borrow_mut().arm_hotkey_keyup(keycode);
                        state.borrow_mut().escape_state = EscapeState::ChordUsed;
                        return CallbackResult::Drop;
                    }

                    if state.borrow().escape_state == EscapeState::Pending {
                        flush_pending_escape(&mut state.borrow_mut(), &tx, fwd);
                    }

                    if fwd {
                        if let Some(msg) = state.borrow_mut().handle_key_down(event) {
                            tx.push(msg);
                        }
                        return CallbackResult::Drop;
                    }
                    CallbackResult::Keep
                }
                CGEventType::KeyUp => {
                    let keycode =
                        event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                    if keycode == MAC_ESCAPE {
                        finish_escape(&mut state.borrow_mut(), &tx, fwd);
                        return CallbackResult::Drop;
                    }
                    if state.borrow_mut().consume_hotkey_keyup(keycode) {
                        return CallbackResult::Drop;
                    }
                    if fwd {
                        if let Some(msg) = state.borrow_mut().handle_key_up(event) {
                            tx.push(msg);
                        }
                        CallbackResult::Drop
                    } else {
                        CallbackResult::Keep
                    }
                }
                CGEventType::FlagsChanged => {
                    if fwd && let Some(msg) = state.borrow_mut().handle_flags_changed(event) {
                        tx.push(msg);
                    }
                    // Always keep modifier changes so Mac stays in sync.
                    CallbackResult::Keep
                }

                // Click detection for PTP button field.
                CGEventType::LeftMouseDown => {
                    if fwd {
                        click_state.store(true, Ordering::Release);
                    }
                    if fwd {
                        CallbackResult::Drop
                    } else {
                        CallbackResult::Keep
                    }
                }
                CGEventType::LeftMouseUp => {
                    click_state.store(false, Ordering::Release);
                    if fwd {
                        CallbackResult::Drop
                    } else {
                        CallbackResult::Keep
                    }
                }
                // All other mouse/scroll/drag events: drop when forwarding.
                _ => {
                    if fwd {
                        CallbackResult::Drop
                    } else {
                        CallbackResult::Keep
                    }
                }
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

/// Check if a KeyDown is `Esc+1-5`. If so, switch target and return true.
fn handle_slot_hotkey(
    event: &CGEvent,
    state: &RefCell<CaptureState>,
    slots: &Mutex<SlotTable>,
    tx: &Outbox,
) -> bool {
    if state.borrow().escape_state != EscapeState::Pending {
        return false;
    }

    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let table = slots.lock().expect("poisoned");

    match keycode {
        MAC_1 => {
            tx.push(HostMsg::SelectPeer(None));
            info!("Requested switch to Mac (local)");
            true
        }
        MAC_2 if table.has_slot(0) => {
            tx.push(HostMsg::SelectPeer(Some(0)));
            info!("Requested remote slot 0");
            true
        }
        MAC_3 if table.has_slot(1) => {
            tx.push(HostMsg::SelectPeer(Some(1)));
            info!("Requested remote slot 1");
            true
        }
        MAC_4 if table.has_slot(2) => {
            tx.push(HostMsg::SelectPeer(Some(2)));
            info!("Requested remote slot 2");
            true
        }
        MAC_5 if table.has_slot(3) => {
            tx.push(HostMsg::SelectPeer(Some(3)));
            info!("Requested remote slot 3");
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::apply_modifier_transition;

    #[test]
    fn releasing_left_ctrl_keeps_right_ctrl_pressed() {
        let left_ctrl = 0x01;
        let right_ctrl = 0x10;
        let current = left_ctrl | right_ctrl;

        let next = apply_modifier_transition(current, left_ctrl, true);

        assert_eq!(next, right_ctrl);
    }

    #[test]
    fn pressing_right_shift_while_left_shift_is_down_keeps_both() {
        let left_shift = 0x02;
        let right_shift = 0x20;

        let next = apply_modifier_transition(left_shift, right_shift, true);

        assert_eq!(next, left_shift | right_shift);
    }

    #[test]
    fn releasing_last_command_clears_that_side() {
        let left_command = 0x08;

        let next = apply_modifier_transition(left_command, left_command, false);

        assert_eq!(next, 0);
    }
}
