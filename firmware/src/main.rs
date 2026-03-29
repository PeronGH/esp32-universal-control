mod ble_hid;
mod hid_descriptor;

use esp32_uc_protocol::wire::{FirmwareMsg, HostMsg};
use esp_idf_svc::hal::gpio;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::uart::{self, UartDriver};
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

/// UART read timeout: 50 ms in FreeRTOS ticks (CONFIG_FREERTOS_HZ = 100).
const READ_TIMEOUT_TICKS: u32 = 5;
/// Maximum postcard+COBS message size we can receive.
const COBS_BUF_SIZE: usize = 128;
/// UART read chunk size.
const READ_BUF_SIZE: usize = 64;
/// Encode buffer for outgoing FirmwareMsg.
const ENCODE_BUF_SIZE: usize = 64;

fn run() -> anyhow::Result<()> {
    info!("esp32-universal-control starting");

    let _nvs = EspDefaultNvsPartition::take()?;
    let peripherals = Peripherals::take()?;

    let ble = ble_hid::BleHid::init()?;

    // UART0 (GPIO43 TX, GPIO44 RX) → CH343 → "USB Single Serial" port.
    // Console/logs go to USB-Serial-JTAG (CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG),
    // so UART0 is clean for host data.
    let uart = UartDriver::new(
        peripherals.uart0,
        peripherals.pins.gpio43,
        peripherals.pins.gpio44,
        Option::<gpio::AnyIOPin>::None,
        Option::<gpio::AnyIOPin>::None,
        &uart::config::Config::new(),
    )?;

    info!("UART0 ready, waiting for host messages…");

    let mut cobs_buf: CobsAccumulator<COBS_BUF_SIZE> = CobsAccumulator::new();
    let mut read_buf = [0u8; READ_BUF_SIZE];

    loop {
        let n = match uart.read(&mut read_buf, READ_TIMEOUT_TICKS) {
            Ok(n) => n,
            Err(e) if e.code() == esp_idf_svc::sys::ESP_ERR_TIMEOUT => 0,
            Err(e) => return Err(e.into()),
        };

        // Forward BLE events (connect/disconnect, LED state) to host.
        while let Some(msg) = ble.try_recv_event() {
            send_to_host(&uart, &msg);
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
                    handle_msg(&ble, &uart, data);
                    remaining
                }
            };
        }
    }
}

/// Send a `FirmwareMsg` to the host over UART0, handling partial writes.
fn send_to_host(uart: &UartDriver<'_>, msg: &FirmwareMsg) {
    let mut buf = [0u8; ENCODE_BUF_SIZE];
    let encoded = match postcard::to_slice_cobs(msg, &mut buf) {
        Ok(encoded) => encoded,
        Err(e) => {
            warn!("postcard encode failed: {e}");
            return;
        }
    };

    let mut offset = 0;
    while offset < encoded.len() {
        match uart.write(&encoded[offset..]) {
            Ok(n) => offset += n,
            Err(e) => {
                warn!("UART write failed: {e}");
                return;
            }
        }
    }
}

fn handle_msg(ble: &ble_hid::BleHid, uart: &UartDriver<'_>, msg: HostMsg) {
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
        HostMsg::QueryConnections => {
            for desc in ble.connections() {
                send_to_host(
                    uart,
                    &FirmwareMsg::ConnectionStatus {
                        addr: desc.address().as_le_bytes(),
                        connected: true,
                    },
                );
            }
        }
        HostMsg::Ping => {
            send_to_host(uart, &FirmwareMsg::Pong);
        }
    }
}
