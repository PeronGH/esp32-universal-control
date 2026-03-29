mod ble_hid;
mod hid_descriptor;

use esp32_uc_protocol::wire::HostMsg;
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
        let n = usb_serial.read(&mut read_buf, esp_idf_svc::hal::delay::BLOCK)?;
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
                    handle_msg(&ble, data);
                    remaining
                }
            };
        }
    }
}

fn handle_msg(ble: &ble_hid::BleHid, msg: HostMsg) {
    if !ble.connected() {
        return;
    }

    match msg {
        HostMsg::Keyboard(report) => {
            ble.send_keyboard(&report);
        }
        HostMsg::Touch(report) => {
            ble.send_touch(&report);
        }
        HostMsg::SwitchSlot(slot) => {
            info!("SwitchSlot({slot}) — not yet implemented");
        }
        HostMsg::SetSlotDevice { slot, addr } => {
            info!("SetSlotDevice(slot={slot}, addr={addr:02x?}) — not yet implemented");
        }
        HostMsg::QuerySlots => {
            info!("QuerySlots — not yet implemented");
        }
    }
}
