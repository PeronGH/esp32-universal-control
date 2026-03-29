mod ble_hid;
mod hid_descriptor;
mod session;

use std::sync::mpsc;

use esp32_uc_protocol::wire::{FirmwareMsg, HostMsg};
use esp_idf_svc::hal::gpio;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::uart::{self, UartDriver, UartTxDriver};
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{error, info, warn};
use postcard::accumulator::{CobsAccumulator, FeedResult};

use crate::session::Session;

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

/// UART read timeout while idle: 10 ms in FreeRTOS ticks (CONFIG_FREERTOS_HZ = 100).
const IDLE_READ_TIMEOUT_TICKS: u32 = 1;
/// Shorter UART read timeout while a touch frame is pending so BLE pacing can
/// drain the latest frame without waiting for another serial packet.
const ACTIVE_TOUCH_READ_TIMEOUT_TICKS: u32 = 1;
/// Host↔firmware UART baud rate.
const UART_BAUD_RATE: u32 = 921_600;
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

    let mut ble = ble_hid::BleHid::init()?;

    // UART0 (GPIO43 TX, GPIO44 RX) → CH343 → "USB Single Serial" port.
    // Console/logs go to USB-Serial-JTAG (CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG),
    // so UART0 is clean for host data.
    let uart = UartDriver::new(
        peripherals.uart0,
        peripherals.pins.gpio43,
        peripherals.pins.gpio44,
        Option::<gpio::AnyIOPin>::None,
        Option::<gpio::AnyIOPin>::None,
        &uart::config::Config::new().baudrate(Hertz(UART_BAUD_RATE)),
    )?;

    // Split into independent TX/RX so writing responses never blocks the
    // receive path (critical at 70 Hz touch input).
    let (tx, rx) = uart.into_split();

    // TX thread: writes host-visible firmware messages to UART.
    let (tx_msg, tx_rx) = mpsc::channel::<FirmwareMsg>();
    let ble_event_rx = ble.take_event_rx();
    std::thread::Builder::new()
        .name("uart-tx".into())
        .stack_size(4096)
        .spawn(move || uart_tx_task(tx, tx_rx))?;

    info!("UART0 ready, waiting for host messages…");

    let mut session = Session::new();

    // RX loop: reads UART, decodes COBS, dispatches to BLE or queues response.
    let mut cobs_buf: CobsAccumulator<COBS_BUF_SIZE> = CobsAccumulator::new();
    let mut read_buf = [0u8; READ_BUF_SIZE];

    loop {
        while let Ok(event) = ble_event_rx.try_recv() {
            session.handle_ble_event(&tx_msg, event);
        }

        session.flush_touch_if_due(&ble);
        let read_timeout_ticks = if session.has_pending_touch() {
            ACTIVE_TOUCH_READ_TIMEOUT_TICKS
        } else {
            IDLE_READ_TIMEOUT_TICKS
        };

        let n = match rx.read(&mut read_buf, read_timeout_ticks) {
            Ok(n) => n,
            Err(e) if e.code() == esp_idf_svc::sys::ESP_ERR_TIMEOUT => continue,
            Err(e) => return Err(e.into()),
        };

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
                    session.handle_host_msg(&ble, &tx_msg, data);
                    session.flush_touch_if_due(&ble);
                    remaining
                }
            };
        }
    }
}

/// TX thread: sends firmware events and responses to the host over UART.
fn uart_tx_task(mut tx: UartTxDriver<'static>, rx: mpsc::Receiver<FirmwareMsg>) {
    while let Ok(msg) = rx.recv() {
        send(&mut tx, &msg);
    }
}

/// Encode and write a `FirmwareMsg` to UART TX, handling partial writes.
fn send(tx: &mut UartTxDriver<'_>, msg: &FirmwareMsg) {
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
        match tx.write(&encoded[offset..]) {
            Ok(n) => offset += n,
            Err(e) => {
                warn!("UART write failed: {e}");
                return;
            }
        }
    }
}
