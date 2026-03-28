mod ble_hid;
mod esp_hid_ffi;
mod feature_reports;
mod hid_descriptor;

use std::time::Duration;

use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::*;
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

fn run() -> Result<(), EspError> {
    info!("BLE PTP PoC starting");

    // NVS is required by the Bluetooth stack.
    let _nvs = EspDefaultNvsPartition::take()?;

    // Peripheral access — needed to prove we own the hardware.
    let _peripherals = Peripherals::take()?;

    // Initialise the Bluetooth controller and Bluedroid stack.
    bt_init()?;

    // Set up GAP advertising + create the HOGP HID device.
    ble_hid::init()?;

    info!("Waiting for BLE connection…");

    // After a host connects, send a hardcoded diagonal touch to move the cursor.
    let mut x: u16 = 5000;
    loop {
        std::thread::sleep(Duration::from_millis(50));

        if !ble_hid::is_connected() {
            continue;
        }

        // Finger down — sweep X from 5000 to 15000.
        if x <= 15000 {
            if let Err(e) = ble_hid::send_touch_report(x, 6000, 1, true) {
                error!("send_touch_report: {e}");
            }
            x += 200;
        } else {
            // Lift the finger, then pause before repeating.
            let _ = ble_hid::send_touch_report(0, 0, 1, false);
            std::thread::sleep(Duration::from_secs(2));
            x = 5000;
        }
    }
}

/// Initialise the Bluetooth controller (BLE-only) and Bluedroid stack.
fn bt_init() -> Result<(), EspError> {
    unsafe {
        // Free Classic-BT memory — we only use BLE.
        esp!(esp_bt_controller_mem_release(
            esp_bt_mode_t_ESP_BT_MODE_CLASSIC_BT
        ))?;

        let mut cfg = esp_bt_controller_config_t::default();
        esp!(esp_bt_controller_init(&mut cfg))?;
        esp!(esp_bt_controller_enable(esp_bt_mode_t_ESP_BT_MODE_BLE))?;

        esp!(esp_bluedroid_init())?;
        esp!(esp_bluedroid_enable())?;
    }

    info!("Bluetooth stack initialised (BLE)");
    Ok(())
}
