mod ble_hid;
mod hid_descriptor;

use std::time::Duration;

use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::ptp::{PtpContact, PtpReport};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{error, info};

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    if let Err(e) = run() {
        error!("Fatal: {e}");
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

/// USB HID keycode for each letter in "hello\n".
const HELLO_KEYCODES: &[u8] = &[
    0x0b, // h
    0x08, // e
    0x0f, // l
    0x0f, // l
    0x12, // o
    0x28, // Enter
];

fn run() -> anyhow::Result<()> {
    info!("esp32-universal-control starting");

    let _nvs = EspDefaultNvsPartition::take()?;
    let _peripherals = Peripherals::take()?;

    let ble = ble_hid::BleHid::init()?;

    info!("Waiting for BLE connection…");

    let mut x: u16 = 5000;
    let mut scan_time: u16 = 0;

    loop {
        std::thread::sleep(Duration::from_millis(50));

        if !ble.connected() {
            continue;
        }

        // --- Touch: horizontal sweep ---
        let mut report = PtpReport {
            scan_time,
            ..PtpReport::default()
        };
        scan_time = scan_time.wrapping_add(50);

        if x <= 15_000 {
            report.contacts[0] = PtpContact {
                flags: PtpContact::FINGER_DOWN,
                contact_id: 1,
                x,
                y: 6000,
            };
            report.contact_count = 1;
            x += 200;
            ble.send_touch(&report);
        } else {
            // Lift finger
            ble.send_touch(&report);
            std::thread::sleep(Duration::from_millis(500));

            // --- Keyboard: type "hello\n" ---
            for &keycode in HELLO_KEYCODES {
                ble.send_keyboard(&KeyboardReport {
                    keycodes: [keycode, 0, 0, 0, 0, 0],
                    ..KeyboardReport::default()
                });
                std::thread::sleep(Duration::from_millis(20));
                // Key release
                ble.send_keyboard(&KeyboardReport::default());
                std::thread::sleep(Duration::from_millis(20));
            }

            std::thread::sleep(Duration::from_secs(2));
            x = 5000;
        }
    }
}
