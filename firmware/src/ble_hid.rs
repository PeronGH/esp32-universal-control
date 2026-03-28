//! BLE HID device using NimBLE: advertising, HOGP setup, and report sending.

use std::mem::size_of;
use std::sync::Arc;

use esp32_nimble::enums::*;
use esp32_nimble::utilities::mutex::Mutex;
use esp32_nimble::{BLEAdvertisementData, BLECharacteristic, BLEDevice, BLEHIDDevice};
use log::info;

use crate::hid_descriptor::{
    self, REPORTID_DEVICE_CAPS, REPORTID_FUNCSWITCH, REPORTID_MULTITOUCH, REPORTID_PTPHQA,
    REPORTID_REPORTMODE,
};
use crate::ptp::{self, PtpReport};

const DEVICE_NAME: &str = "ESP32 UC PTP";

/// BLE HID appearance: HID Touchpad (Generic HID 0x03C0 + subtype 0x05).
const APPEARANCE_HID_TOUCHPAD: u16 = 0x03C5;

/// PnP signature: USB Implementers Forum assigned vendor ID.
const PNP_SIG_USB: u8 = 0x02;
const VENDOR_ID: u16 = 0x05AC;
const PRODUCT_ID: u16 = 0x0001;
const DEVICE_VERSION: u16 = 0x0100;

/// Owns the NimBLE HID device and its report characteristics.
pub struct BleHid {
    touch_input: Arc<Mutex<BLECharacteristic>>,
}

impl BleHid {
    /// Initialise BLE security, create the HID device with PTP report
    /// characteristics, pre-load feature reports, and start advertising.
    ///
    /// NimBLE manages the BT controller and GATT server lifecycle internally.
    pub fn init() -> anyhow::Result<Self> {
        let device = BLEDevice::take();
        device
            .security()
            .set_auth(AuthReq::Bond)
            .set_io_cap(SecurityIOCap::NoInputNoOutput)
            .resolve_rpa();

        let server = device.get_server();
        let mut hid = BLEHIDDevice::new(server);

        // --- Report characteristics ------------------------------------------
        let touch_input = hid.input_report(REPORTID_MULTITOUCH);
        let input_mode = hid.feature_report(REPORTID_REPORTMODE);
        let _func_switch = hid.feature_report(REPORTID_FUNCSWITCH);
        let device_caps = hid.feature_report(REPORTID_DEVICE_CAPS);
        let ptphqa = hid.feature_report(REPORTID_PTPHQA);

        // --- Device metadata -------------------------------------------------
        hid.report_map(hid_descriptor::PTP_REPORT_DESCRIPTOR);
        hid.manufacturer("esp32-universal-control");
        hid.pnp(PNP_SIG_USB, VENDOR_ID, PRODUCT_ID, DEVICE_VERSION);
        hid.hid_info(0x00, 0x01);
        hid.set_battery_level(100);

        // --- Pre-load feature reports ----------------------------------------
        device_caps.lock().set_value(&ptp::DEVICE_CAPS);
        ptphqa.lock().set_value(&ptp::PTPHQA_BLOB);

        // --- Detect host setting Input Mode = 3 (PTP) ------------------------
        input_mode.lock().on_write(|args| {
            let data = args.recv_data();
            if data.first() == Some(&ptp::INPUT_MODE_PTP) {
                info!("*** Host set Input Mode = 3 (PTP) — precision touchpad confirmed ***");
            } else {
                info!("Host set Input Mode = {data:02x?}");
            }
        });

        // --- Advertising -----------------------------------------------------
        let adv = device.get_advertising();
        adv.lock().set_data(
            BLEAdvertisementData::new()
                .name(DEVICE_NAME)
                .appearance(APPEARANCE_HID_TOUCHPAD)
                .add_service_uuid(hid.hid_service().lock().uuid()),
        )?;
        adv.lock().start()?;

        info!("BLE HID advertising as \"{DEVICE_NAME}\"");

        Ok(Self { touch_input })
    }

    /// Returns `true` when at least one BLE host is connected.
    pub fn connected(&self) -> bool {
        BLEDevice::take().get_server().connected_count() > 0
    }

    /// Send a PTP input report to the connected host.
    pub fn send_report(&self, report: &PtpReport) {
        let mut chr = self.touch_input.lock();
        // SAFETY: PtpReport is repr(C, packed) with no padding; its byte
        // representation is the exact HID report the host expects.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                report as *const PtpReport as *const u8,
                size_of::<PtpReport>(),
            )
        };
        chr.set_value(bytes);
        chr.notify();
    }
}
