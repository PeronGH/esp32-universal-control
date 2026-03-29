//! macOS virtual keycode → USB HID keycode translation.
//!
//! macOS keycodes are from `core_graphics::event::KeyCode` (Carbon `Events.h`).
//! USB HID keycodes are from the USB HID Usage Tables (section 10, Keyboard/Keypad).

use esp32_uc_protocol::input::{
    CONSUMER_MUTE, CONSUMER_VOLUME_DOWN, CONSUMER_VOLUME_UP, ConsumerState,
};

/// Translate a macOS virtual keycode to a USB HID keycode.
/// Returns 0 (no key) for unmapped keycodes.
pub fn mac_to_hid(mac_keycode: u16) -> u8 {
    MAC_TO_HID.get(mac_keycode as usize).copied().unwrap_or(0)
}

/// Translate a macOS virtual keycode to a semantic consumer usage bit.
pub fn mac_to_consumer(mac_keycode: u16) -> Option<ConsumerState> {
    match mac_keycode {
        0x48 => Some(CONSUMER_VOLUME_UP),
        0x49 => Some(CONSUMER_VOLUME_DOWN),
        0x4A => Some(CONSUMER_MUTE),
        _ => None,
    }
}

/// Return the modifier bit represented by this macOS keycode, if any.
pub fn modifier_mask(mac_keycode: u16) -> Option<u8> {
    let hid = mac_to_hid(mac_keycode);
    (0xE0..=0xE7).contains(&hid).then(|| 1 << (hid - 0xE0))
}

/// macOS virtual keycode (index) → USB HID keycode (value).
/// 0 = unmapped.
///
/// Built by cross-referencing `core_graphics::event::KeyCode` constants
/// with USB HID Usage Tables section 10.
#[rustfmt::skip]
const MAC_TO_HID: [u8; 128] = [
    // 0x00 ANSI_A        0x01 ANSI_S        0x02 ANSI_D        0x03 ANSI_F
       0x04,              0x16,              0x07,              0x09,
    // 0x04 ANSI_H        0x05 ANSI_G        0x06 ANSI_Z        0x07 ANSI_X
       0x0B,              0x0A,              0x1D,              0x1B,
    // 0x08 ANSI_C        0x09 ANSI_V        0x0A ISO_Section   0x0B ANSI_B
       0x06,              0x19,              0x64,              0x05,
    // 0x0C ANSI_Q        0x0D ANSI_W        0x0E ANSI_E        0x0F ANSI_R
       0x14,              0x1A,              0x08,              0x15,
    // 0x10 ANSI_Y        0x11 ANSI_T        0x12 ANSI_1        0x13 ANSI_2
       0x1C,              0x17,              0x1E,              0x1F,
    // 0x14 ANSI_3        0x15 ANSI_4        0x16 ANSI_6        0x17 ANSI_5
       0x20,              0x21,              0x23,              0x22,
    // 0x18 ANSI_Equal    0x19 ANSI_9        0x1A ANSI_7        0x1B ANSI_Minus
       0x2E,              0x26,              0x24,              0x2D,
    // 0x1C ANSI_8        0x1D ANSI_0        0x1E ANSI_RBracket 0x1F ANSI_O
       0x25,              0x27,              0x30,              0x12,
    // 0x20 ANSI_U        0x21 ANSI_LBracket 0x22 ANSI_I        0x23 ANSI_P
       0x18,              0x2F,              0x0C,              0x13,
    // 0x24 Return        0x25 ANSI_L        0x26 ANSI_J        0x27 ANSI_Quote
       0x28,              0x0F,              0x0D,              0x34,
    // 0x28 ANSI_K        0x29 ANSI_Semi     0x2A ANSI_Backslash 0x2B ANSI_Comma
       0x0E,              0x33,              0x31,              0x36,
    // 0x2C ANSI_Slash    0x2D ANSI_N        0x2E ANSI_M        0x2F ANSI_Period
       0x38,              0x11,              0x10,              0x37,
    // 0x30 Tab           0x31 Space         0x32 ANSI_Grave    0x33 Delete(BS)
       0x2B,              0x2C,              0x35,              0x2A,
    // 0x34 (unused)      0x35 Escape        0x36 RightCommand  0x37 Command
       0x00,              0x29,              0xE7,              0xE3,
    // 0x38 Shift         0x39 CapsLock      0x3A Option        0x3B Control
       0xE1,              0x39,              0xE2,              0xE0,
    // 0x3C RightShift    0x3D RightOption   0x3E RightControl  0x3F Function
       0xE5,              0xE6,              0xE4,              0x00,
    // 0x40 F17           0x41 KP_Decimal    0x42 (unused)      0x43 KP_Multiply
       0x6C,              0x63,              0x00,              0x55,
    // 0x44 (unused)      0x45 KP_Plus       0x46 (unused)      0x47 KP_Clear
       0x00,              0x57,              0x00,              0x53,
    // 0x48 VolumeUp      0x49 VolumeDown    0x4A Mute          0x4B KP_Divide
       0x80,              0x81,              0x7F,              0x54,
    // 0x4C KP_Enter      0x4D (unused)      0x4E KP_Minus      0x4F F18
       0x58,              0x00,              0x56,              0x6D,
    // 0x50 F19           0x51 KP_Equal      0x52 KP_0          0x53 KP_1
       0x6E,              0x67,              0x62,              0x59,
    // 0x54 KP_2          0x55 KP_3          0x56 KP_4          0x57 KP_5
       0x5A,              0x5B,              0x5C,              0x5D,
    // 0x58 KP_6          0x59 KP_7          0x5A F20           0x5B KP_8
       0x5E,              0x5F,              0x6F,              0x60,
    // 0x5C KP_9          0x5D JIS_Yen       0x5E JIS_Underscore 0x5F JIS_KP_Comma
       0x61,              0x89,              0x87,              0x85,
    // 0x60 F5            0x61 F6            0x62 F7            0x63 F3
       0x3E,              0x3F,              0x40,              0x3C,
    // 0x64 F8            0x65 F9            0x66 JIS_Eisu      0x67 F11
       0x41,              0x42,              0x92,              0x44,
    // 0x68 JIS_Kana      0x69 F13           0x6A F16           0x6B F14
       0x90,              0x68,              0x6B,              0x69,
    // 0x6C (unused)      0x6D F10           0x6E (unused)      0x6F F12
       0x00,              0x43,              0x00,              0x45,
    // 0x70 (unused)      0x71 F15           0x72 Help/Insert   0x73 Home
       0x00,              0x6A,              0x49,              0x4A,
    // 0x74 PageUp        0x75 ForwardDelete 0x76 F4            0x77 End
       0x4B,              0x4C,              0x3D,              0x4D,
    // 0x78 F2            0x79 PageDown      0x7A F1            0x7B LeftArrow
       0x3B,              0x4E,              0x3A,              0x50,
    // 0x7C RightArrow    0x7D DownArrow     0x7E UpArrow       0x7F (unused)
       0x4F,              0x51,              0x52,              0x00,
];
