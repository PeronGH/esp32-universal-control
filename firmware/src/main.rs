mod ble_hid;
mod hid_descriptor;

use esp32_uc_protocol::wire::{FirmwareMsg, HostMsg};
use esp_idf_svc::hal::delay;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::usb_serial::UsbSerialDriver;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{error, info, warn};
use postcard::accumulator::{CobsAccumulator, FeedResult};

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    if let Err(e) = run() {
        error!("Fatal: {e}");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}

/// 50 ms timeout for USB serial reads, in FreeRTOS ticks.
/// ESP-IDF default: CONFIG_FREERTOS_HZ = 100 → 10ms per tick → 5 ticks = 50ms.
const READ_TIMEOUT_TICKS: u32 = 5;

fn run() -> anyhow::Result<()> {
    info!("esp32-universal-control starting");

    let _nvs = EspDefaultNvsPartition::take()?;
    let peripherals = Peripherals::take()?;

    let ble = ble_hid::BleHid::init()?;

    // USB-Serial-JTAG — the second serial port visible on the Mac through
    // the CH334 hub. Console logs go to UART0/CH343 (the first port);
    // this port is dedicated to host ↔ firmware data.
    let mut usb_serial = UsbSerialDriver::new(
        peripherals.usb_serial,
        peripherals.pins.gpio19,
        peripherals.pins.gpio20,
        &Default::default(),
    )?;

    info!("USB-Serial-JTAG ready, waiting for host messages…");

    let mut cobs_buf: CobsAccumulator<128> = CobsAccumulator::new();
    let mut read_buf = [0u8; 64];

    loop {
        // Non-blocking-ish read: timeout so we can drain BLE events.
        let n = usb_serial.read(&mut read_buf, READ_TIMEOUT_TICKS)?;

        // Drain BLE connection events and forward to host.
        while let Ok(msg) = ble.event_rx.try_recv() {
            send_to_host(&mut usb_serial, &msg);
        }

        if n == 0 {
            continue;
        }

        let mut window = &read_buf[..n];
        while !window.is_empty() {
            window = match cobs_buf.feed::<HostMsg>(window) {
                FeedResult::Consumed => break,
                FeedResult::OverFull(remaining) => {
                    warn!("COBS buffer overflow, discarding frame");
                    remaining
                }
                FeedResult::DeserError(remaining) => {
                    warn!("postcard deserialization error, discarding frame");
                    remaining
                }
                FeedResult::Success { data, remaining } => {
                    handle_msg(&ble, &mut usb_serial, data);
                    remaining
                }
            };
        }
    }
}

/// Send a `FirmwareMsg` to the host over USB-Serial-JTAG, handling partial writes.
fn send_to_host(usb: &mut UsbSerialDriver<'_>, msg: &FirmwareMsg) {
    let mut buf = [0u8; 64];
    let encoded = match postcard::to_slice_cobs(msg, &mut buf) {
        Ok(encoded) => encoded,
        Err(e) => {
            warn!("postcard encode failed: {e}");
            return;
        }
    };

    let mut offset = 0;
    while offset < encoded.len() {
        match usb.write(&encoded[offset..], delay::BLOCK) {
            Ok(n) => offset += n,
            Err(e) => {
                warn!("USB serial write failed: {e}");
                return;
            }
        }
    }
}

fn handle_msg(ble: &ble_hid::BleHid, usb: &mut UsbSerialDriver<'_>, msg: HostMsg) {
    match msg {
        HostMsg::Keyboard(report) => {
            if ble.connected() {
                ble.send_keyboard(&report);
            }
        }
        HostMsg::Consumer(bits) => {
            if ble.connected() {
                ble.send_consumer(bits);
            }
        }
        HostMsg::Touch(report) => {
            if ble.connected() {
                ble.send_touch(&report);
            }
        }
        HostMsg::SwitchSlot(slot) => {
            info!("SwitchSlot({slot}) — not yet implemented");
        }
        HostMsg::SetSlotDevice { slot, addr } => {
            info!("SetSlotDevice(slot={slot}, addr={addr:02x?}) — not yet implemented");
        }
        HostMsg::QuerySlots => {
            for (slot, desc) in ble.connections().enumerate() {
                send_to_host(
                    usb,
                    &FirmwareMsg::SlotStatus {
                        slot: slot as u8,
                        addr: desc.address().as_le_bytes(),
                        connected: true,
                    },
                );
            }
        }
    }
}
