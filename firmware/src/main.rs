mod ble_hid;
mod hid_descriptor;
mod ptp;

use std::time::Duration;

use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{error, info};

use ptp::{PtpContact, PtpReport};

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

fn run() -> anyhow::Result<()> {
    info!("esp32-universal-control starting");

    // NVS is required by the Bluetooth stack.
    let _nvs = EspDefaultNvsPartition::take()?;
    let _peripherals = Peripherals::take()?;

    let ble = ble_hid::BleHid::init()?;

    info!("Waiting for BLE connection…");

    // Demo: hardcoded single-finger horizontal sweep to verify PTP works.
    let mut x: u16 = 5000;
    let mut scan_time: u16 = 0;

    loop {
        std::thread::sleep(Duration::from_millis(50));

        if !ble.connected() {
            continue;
        }

        let mut report = PtpReport {
            scan_time,
            ..PtpReport::default()
        };
        scan_time = scan_time.wrapping_add(50);

        if x <= 15_000 {
            // Finger down — sweep X across the touchpad.
            report.contacts[0] = PtpContact {
                flags: PtpContact::FINGER_DOWN,
                contact_id: 1,
                x,
                y: 6000,
            };
            report.contact_count = 1;
            x += 200;
        } else {
            // Finger lifted — pause, then repeat.
            ble.send_report(&report);
            std::thread::sleep(Duration::from_secs(2));
            x = 5000;
            continue;
        }

        ble.send_report(&report);
    }
}
