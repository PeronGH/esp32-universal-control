//! BLE HID device using NimBLE: advertising, HOGP setup, and report sending.

use std::sync::mpsc;
use std::sync::Arc;

use esp32_nimble::enums::*;
use esp32_nimble::utilities::mutex::Mutex;
use esp32_nimble::{BLEAdvertisementData, BLECharacteristic, BLEDevice, BLEHIDDevice};
use log::{info, warn};
use zerocopy::IntoBytes;

use esp32_uc_protocol::keyboard::{KeyboardReport, REPORTID_CONSUMER, REPORTID_KEYBOARD};
use esp32_uc_protocol::ptp::{
    self, PtpReport, REPORTID_DEVICE_CAPS, REPORTID_FUNCSWITCH, REPORTID_MULTITOUCH,
    REPORTID_PTPHQA, REPORTID_REPORTMODE,
};

use crate::hid_descriptor;

const DEVICE_NAME: &str = "ESP32 UC";
/// BLE appearance: Generic HID (Bluetooth SIG assigned number).
const APPEARANCE_HID: u16 = 0x03C0;
/// PnP vendor ID source: USB Implementers Forum.
const PNP_SIG_USB: u8 = 0x02;
const VENDOR_ID: u16 = 0x05AC;
const PRODUCT_ID: u16 = 0x0001;
const DEVICE_VERSION: u16 = 0x0100;
/// HID country code: not localized.
const HID_COUNTRY_NOT_LOCALIZED: u8 = 0x00;
/// HID flags: normally connectable.
const HID_FLAGS_NORMALLY_CONNECTABLE: u8 = 0x01;
const BATTERY_LEVEL_FULL: u8 = 100;
/// Request a 7.5 ms BLE connection interval for high-rate pointer/touch input.
const FAST_CONN_INTERVAL: u16 = 6;
/// No slave latency for interactive input devices.
const FAST_CONN_LATENCY: u16 = 0;
/// 600 ms supervision timeout in 10 ms units.
const FAST_CONN_TIMEOUT: u16 = 60;

/// Device Capabilities feature report (report ID 0x07).
const DEVICE_CAPS: [u8; 2] = [ptp::MAX_CONTACTS, 0x00 /* clickpad */];

/// PTPHQA certification blob (report ID 0x08, 256 bytes).
/// From `imbushuo/mac-precision-touchpad` `Hid.h`.
#[rustfmt::skip]
const PTPHQA_BLOB: [u8; 256] = [
    0xfc, 0x28, 0xfe, 0x84, 0x40, 0xcb, 0x9a, 0x87, 0x0d, 0xbe, 0x57, 0x3c, 0xb6, 0x70, 0x09, 0x88,
    0x07, 0x97, 0x2d, 0x2b, 0xe3, 0x38, 0x34, 0xb6, 0x6c, 0xed, 0xb0, 0xf7, 0xe5, 0x9c, 0xf6, 0xc2,
    0x2e, 0x84, 0x1b, 0xe8, 0xb4, 0x51, 0x78, 0x43, 0x1f, 0x28, 0x4b, 0x7c, 0x2d, 0x53, 0xaf, 0xfc,
    0x47, 0x70, 0x1b, 0x59, 0x6f, 0x74, 0x43, 0xc4, 0xf3, 0x47, 0x18, 0x53, 0x1a, 0xa2, 0xa1, 0x71,
    0xc7, 0x95, 0x0e, 0x31, 0x55, 0x21, 0xd3, 0xb5, 0x1e, 0xe9, 0x0c, 0xba, 0xec, 0xb8, 0x89, 0x19,
    0x3e, 0xb3, 0xaf, 0x75, 0x81, 0x9d, 0x53, 0xb9, 0x41, 0x57, 0xf4, 0x6d, 0x39, 0x25, 0x29, 0x7c,
    0x87, 0xd9, 0xb4, 0x98, 0x45, 0x7d, 0xa7, 0x26, 0x9c, 0x65, 0x3b, 0x85, 0x68, 0x89, 0xd7, 0x3b,
    0xbd, 0xff, 0x14, 0x67, 0xf2, 0x2b, 0xf0, 0x2a, 0x41, 0x54, 0xf0, 0xfd, 0x2c, 0x66, 0x7c, 0xf8,
    0xc0, 0x8f, 0x33, 0x13, 0x03, 0xf1, 0xd3, 0xc1, 0x0b, 0x89, 0xd9, 0x1b, 0x62, 0xcd, 0x51, 0xb7,
    0x80, 0xb8, 0xaf, 0x3a, 0x10, 0xc1, 0x8a, 0x5b, 0xe8, 0x8a, 0x56, 0xf0, 0x8c, 0xaa, 0xfa, 0x35,
    0xe9, 0x42, 0xc4, 0xd8, 0x55, 0xc3, 0x38, 0xcc, 0x2b, 0x53, 0x5c, 0x69, 0x52, 0xd5, 0xc8, 0x73,
    0x02, 0x38, 0x7c, 0x73, 0xb6, 0x41, 0xe7, 0xff, 0x05, 0xd8, 0x2b, 0x79, 0x9a, 0xe2, 0x34, 0x60,
    0x8f, 0xa3, 0x32, 0x1f, 0x09, 0x78, 0x62, 0xbc, 0x80, 0xe3, 0x0f, 0xbd, 0x65, 0x20, 0x08, 0x13,
    0xc1, 0xe2, 0xee, 0x53, 0x2d, 0x86, 0x7e, 0xa7, 0x5a, 0xc5, 0xd3, 0x7d, 0x98, 0xbe, 0x31, 0x48,
    0x1f, 0xfb, 0xda, 0xaf, 0xa2, 0xa8, 0x6a, 0x89, 0xd6, 0xbf, 0xf2, 0xd3, 0x32, 0x2a, 0x9a, 0xe4,
    0xcf, 0x17, 0xb7, 0xb8, 0xf4, 0xe1, 0x33, 0x08, 0x24, 0x8b, 0xc4, 0x43, 0xa5, 0xe5, 0x24, 0xc2,
];

/// Internal BLE events emitted by the HID wrapper.
#[derive(Clone, Copy, Debug)]
pub enum BleEvent {
    /// A BLE peer connected.
    Connected { conn_handle: u16, addr: [u8; 6] },
    /// A BLE peer disconnected.
    Disconnected { conn_handle: u16 },
    /// Keyboard LED state changed.
    LedState(u8),
}

/// Owns the NimBLE HID device and its report characteristics.
pub struct BleHid {
    keyboard_input: Arc<Mutex<BLECharacteristic>>,
    consumer_input: Arc<Mutex<BLECharacteristic>>,
    touch_input: Arc<Mutex<BLECharacteristic>>,
    event_rx: Option<mpsc::Receiver<BleEvent>>,
}

impl BleHid {
    /// Initialise BLE security, create the composite HID device (keyboard +
    /// consumer + PTP), pre-load feature reports, and start advertising.
    pub fn init() -> anyhow::Result<Self> {
        let device = BLEDevice::take();
        BLEDevice::set_device_name(DEVICE_NAME)?;
        device
            .security()
            .set_auth(AuthReq::Bond)
            .set_io_cap(SecurityIOCap::NoInputNoOutput)
            .resolve_rpa();

        let (event_tx, event_rx) = mpsc::channel::<BleEvent>();

        let server = device.get_server();

        // --- BLE connection event callbacks -----------------------------------
        let tx = event_tx.clone();
        server.on_connect(move |server, desc| {
            info!("BLE connected: {:?}", desc.address());
            if let Err(err) = server.update_conn_params(
                desc.conn_handle(),
                FAST_CONN_INTERVAL,
                FAST_CONN_INTERVAL,
                FAST_CONN_LATENCY,
                FAST_CONN_TIMEOUT,
            ) {
                warn!(
                    "failed to request fast conn params for {}: {err:?}",
                    desc.conn_handle()
                );
            }
            let _ = tx.send(BleEvent::Connected {
                conn_handle: desc.conn_handle(),
                addr: desc.id_address().as_le_bytes(),
            });
        });
        let tx = event_tx.clone();
        server.on_disconnect(move |desc, _reason| {
            info!("BLE disconnected: {:?}", desc.address());
            let _ = tx.send(BleEvent::Disconnected {
                conn_handle: desc.conn_handle(),
            });
        });

        let mut hid = BLEHIDDevice::new(server);

        // --- Report characteristics ------------------------------------------
        let keyboard_input = hid.input_report(REPORTID_KEYBOARD);
        let keyboard_output = hid.output_report(REPORTID_KEYBOARD);
        let consumer_input = hid.input_report(REPORTID_CONSUMER);
        let touch_input = hid.input_report(REPORTID_MULTITOUCH);
        let input_mode = hid.feature_report(REPORTID_REPORTMODE);
        let _func_switch = hid.feature_report(REPORTID_FUNCSWITCH);
        let device_caps = hid.feature_report(REPORTID_DEVICE_CAPS);
        let ptphqa = hid.feature_report(REPORTID_PTPHQA);

        // --- Keyboard LED state from Windows ---------------------------------
        let tx = event_tx;
        keyboard_output.lock().on_write(move |args| {
            if let Some(&bits) = args.recv_data().first() {
                let _ = tx.send(BleEvent::LedState(bits));
            }
        });

        // --- Device metadata -------------------------------------------------
        hid.report_map(hid_descriptor::COMPOSITE_REPORT_DESCRIPTOR);
        hid.manufacturer("esp32-universal-control");
        hid.pnp(PNP_SIG_USB, VENDOR_ID, PRODUCT_ID, DEVICE_VERSION);
        hid.hid_info(HID_COUNTRY_NOT_LOCALIZED, HID_FLAGS_NORMALLY_CONNECTABLE);
        hid.set_battery_level(BATTERY_LEVEL_FULL);

        // --- Pre-load feature reports ----------------------------------------
        device_caps.lock().set_value(&DEVICE_CAPS);
        ptphqa.lock().set_value(&PTPHQA_BLOB);

        input_mode.lock().on_write(|args| {
            info!("Input Mode set to {:02x?}", args.recv_data());
        });

        // --- Advertising -----------------------------------------------------
        let adv = device.get_advertising();
        adv.lock().set_data(
            BLEAdvertisementData::new()
                .name(DEVICE_NAME)
                .appearance(APPEARANCE_HID)
                .add_service_uuid(hid.hid_service().lock().uuid()),
        )?;
        adv.lock().start()?;

        info!("BLE HID advertising as \"{DEVICE_NAME}\"");

        Ok(Self {
            keyboard_input,
            consumer_input,
            touch_input,
            event_rx: Some(event_rx),
        })
    }

    /// Take the BLE event receiver. Must be called exactly once, before
    /// passing to the TX thread.
    pub fn take_event_rx(&mut self) -> mpsc::Receiver<BleEvent> {
        self.event_rx.take().expect("event_rx already taken")
    }

    /// Send a keyboard input report to one connected BLE device.
    pub fn send_keyboard_to(&self, conn_handle: u16, report: &KeyboardReport) {
        let chr = self.keyboard_input.lock();
        if let Err(err) = chr.notify_with(report.as_bytes(), conn_handle) {
            warn!("keyboard notify({conn_handle}) failed: {err:?}");
        }
    }

    /// Send a consumer control input report to one connected BLE device.
    pub fn send_consumer_to(&self, conn_handle: u16, bits: u16) {
        let chr = self.consumer_input.lock();
        if let Err(err) = chr.notify_with(&bits.to_le_bytes(), conn_handle) {
            warn!("consumer notify({conn_handle}) failed: {err:?}");
        }
    }

    /// Send a PTP touch input report to one connected BLE device.
    pub fn send_touch_to(&self, conn_handle: u16, report: &PtpReport) {
        let chr = self.touch_input.lock();
        if let Err(err) = chr.notify_with(report.as_bytes(), conn_handle) {
            warn!("touch notify({conn_handle}) failed: {err:?}");
        }
    }
}
