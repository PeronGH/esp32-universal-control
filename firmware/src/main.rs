mod ble_hid;
mod hid_descriptor;

use std::sync::mpsc;

use esp32_uc_protocol::wire::{FirmwareMsg, HostMsg};
use esp_idf_svc::hal::gpio;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::uart::{self, UartDriver, UartTxDriver};
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
        &uart::config::Config::new(),
    )?;

    // Split into independent TX/RX so writing responses never blocks the
    // receive path (critical at 70 Hz touch input).
    let (tx, rx) = uart.into_split();

    // TX thread: drains BLE events and response messages, writes to UART.
    let (resp_tx, resp_rx) = mpsc::channel::<FirmwareMsg>();
    let ble_event_rx = ble.take_event_rx();
    std::thread::Builder::new()
        .name("uart-tx".into())
        .stack_size(4096)
        .spawn(move || uart_tx_task(tx, ble_event_rx, resp_rx))?;

    info!("UART0 ready, waiting for host messages…");

    // RX loop: reads UART, decodes COBS, dispatches to BLE or queues response.
    let mut cobs_buf: CobsAccumulator<COBS_BUF_SIZE> = CobsAccumulator::new();
    let mut read_buf = [0u8; READ_BUF_SIZE];

    loop {
        let n = match rx.read(&mut read_buf, READ_TIMEOUT_TICKS) {
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
                    handle_msg(&ble, &resp_tx, data);
                    remaining
                }
            };
        }
    }
}

/// TX thread: sends BLE events and command responses to the host.
///
/// Drains from two sources:
/// - `ble_rx`: proactive BLE events (connect/disconnect, LED state)
/// - `resp_rx`: responses to host commands (Pong, ConnectionStatus)
fn uart_tx_task(
    mut tx: UartTxDriver<'static>,
    ble_rx: mpsc::Receiver<FirmwareMsg>,
    resp_rx: mpsc::Receiver<FirmwareMsg>,
) {
    loop {
        let mut sent = false;

        while let Ok(msg) = ble_rx.try_recv() {
            send(&mut tx, &msg);
            sent = true;
        }
        while let Ok(msg) = resp_rx.try_recv() {
            send(&mut tx, &msg);
            sent = true;
        }

        if !sent {
            // Nothing pending, sleep briefly to avoid busy-spinning.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
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

/// Handle a decoded host message. HID reports go directly to BLE.
/// Responses are queued for the TX thread.
fn handle_msg(ble: &ble_hid::BleHid, resp_tx: &mpsc::Sender<FirmwareMsg>, msg: HostMsg) {
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
        HostMsg::Touch(mut report) => {
            if ble.connected() {
                // Rewrite scan_time with firmware-side delivery timing.
                // Windows uses scan_time to compute contact velocity for
                // scroll inertia. Using the Mac-side timestamp causes jitter
                // because transport latency is variable.
                // Match imbushuo/mac-precision-touchpad: scan_time = delta
                // between reports in 100us units, capped at 0xFF.
                // esp_timer_get_time() returns i64 microseconds. We only need
                // the delta in 100us units (max 0xFF = 25.5ms), so truncating
                // to u32 is safe (wraps every ~71 minutes, delta is always small).
                static LAST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                // SAFETY: esp_timer_get_time is always safe to call.
                let now = (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 100) as u32;
                let last = LAST.swap(now, std::sync::atomic::Ordering::Relaxed);
                let delta = if last > 0 {
                    now.wrapping_sub(last).min(0xFF) as u16
                } else {
                    0
                };
                report.scan_time = delta;
                ble.send_touch(&report);
            }
        }
        HostMsg::QueryConnections => {
            for desc in ble.connections() {
                let _ = resp_tx.send(FirmwareMsg::ConnectionStatus {
                    addr: desc.address().as_le_bytes(),
                    connected: true,
                });
            }
        }
        HostMsg::Ping => {
            let _ = resp_tx.send(FirmwareMsg::Pong);
        }
    }
}
