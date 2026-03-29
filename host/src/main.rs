use std::io::{self, BufRead, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, bail};
use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::ptp::{PtpContact, PtpReport};
use esp32_uc_protocol::wire::{FirmwareMsg, HostMsg};
use serialport5::SerialPort;

/// Encode a `HostMsg` and write it to the serial port.
fn send(port: &mut SerialPort, msg: &HostMsg) -> anyhow::Result<()> {
    let mut buf = [0u8; 128];
    let encoded = postcard::to_slice_cobs(msg, &mut buf).context("postcard encode")?;
    port.write_all(encoded).context("serial write")?;
    Ok(())
}

/// Background reader: decodes `FirmwareMsg` from the serial port and sends
/// them to the main thread via a channel.
fn spawn_reader(mut port: SerialPort, tx: mpsc::Sender<FirmwareMsg>) {
    std::thread::spawn(move || {
        use postcard::accumulator::{CobsAccumulator, FeedResult};
        let mut cobs_buf: CobsAccumulator<128> = CobsAccumulator::new();
        let mut read_buf = [0u8; 64];

        loop {
            let n = match port.read(&mut read_buf) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::TimedOut => continue,
                Err(_) => break,
            };

            let mut window = &read_buf[..n];
            while !window.is_empty() {
                window = match cobs_buf.feed::<FirmwareMsg>(window) {
                    FeedResult::Consumed => break,
                    FeedResult::OverFull(rem) | FeedResult::DeserError(rem) => rem,
                    FeedResult::Success { data, remaining } => {
                        let _ = tx.send(data);
                        remaining
                    }
                };
            }
        }
    });
}

/// Format a BLE address (little-endian bytes) as a colon-separated string.
fn format_addr(addr: &[u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        addr[5], addr[4], addr[3], addr[2], addr[1], addr[0]
    )
}

/// Send Ping and wait for Pong. Retries a few times.
fn handshake(
    write_port: &mut SerialPort,
    fw_rx: &mpsc::Receiver<FirmwareMsg>,
) -> anyhow::Result<()> {
    for attempt in 1..=5 {
        eprint!("Handshake attempt {attempt}/5...");
        send(write_port, &HostMsg::Ping)?;

        match fw_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(FirmwareMsg::Pong) => {
                eprintln!(" ok");
                return Ok(());
            }
            Ok(other) => {
                eprintln!(" unexpected: {other:?}");
            }
            Err(_) => {
                eprintln!(" timeout");
            }
        }
    }
    bail!("firmware did not respond to Ping — wrong port or firmware not running");
}

fn handle_fw_msg(msg: FirmwareMsg) {
    match msg {
        FirmwareMsg::Pong => {} // handled by handshake
        FirmwareMsg::SlotStatus {
            slot,
            addr,
            connected,
        } => {
            let status = if connected {
                "connected"
            } else {
                "disconnected"
            };
            println!("  slot {slot}: {status} {}", format_addr(&addr));
        }
        FirmwareMsg::LedState(bits) => {
            println!("  LED state: {bits:#04x}");
        }
    }
}

fn run() -> anyhow::Result<()> {
    let port_name = std::env::args()
        .nth(1)
        .context("usage: esp32-uc-host <serial-port>")?;

    let port = SerialPort::builder()
        .baud_rate(115_200)
        .read_timeout(Some(Duration::from_millis(100)))
        .open(&port_name)
        .with_context(|| format!("open {port_name}"))?;

    let mut write_port = port.try_clone().context("clone serial port")?;

    // Background reader for firmware responses.
    let (fw_tx, fw_rx) = mpsc::channel::<FirmwareMsg>();
    spawn_reader(port, fw_tx);

    // Verify we're talking to the right device.
    handshake(&mut write_port, &fw_rx)?;

    println!("Connected to {port_name}");
    println!("  t = touch sweep step");
    println!("  k = random key");
    println!("  l = list connected devices");
    println!("  q = quit");

    let mut touch_x: u16 = 5000;
    let mut scan_time: u16 = 0;

    let stdin = io::stdin();

    loop {
        // Drain any pending firmware messages.
        while let Ok(msg) = fw_rx.try_recv() {
            handle_fw_msg(msg);
        }

        print!("> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }

        match line.trim() {
            "t" => {
                let mut report = PtpReport {
                    scan_time,
                    ..PtpReport::default()
                };
                scan_time = scan_time.wrapping_add(50);

                if touch_x <= 15_000 {
                    report.contacts[0] = PtpContact {
                        flags: PtpContact::FINGER_DOWN,
                        contact_id: 1,
                        x: touch_x,
                        y: 6000,
                    };
                    report.contact_count = 1;
                    touch_x += 500;
                } else {
                    touch_x = 5000;
                }

                send(&mut write_port, &HostMsg::Touch(report))?;
                println!("  touch x={}", touch_x.saturating_sub(500));
            }

            "k" => {
                let letter = b'a' + (scan_time as u8 % 26);
                let keycode = 0x04 + (letter - b'a');

                send(
                    &mut write_port,
                    &HostMsg::Keyboard(KeyboardReport {
                        keycodes: [keycode, 0, 0, 0, 0, 0],
                        ..KeyboardReport::default()
                    }),
                )?;
                std::thread::sleep(Duration::from_millis(10));
                send(
                    &mut write_port,
                    &HostMsg::Keyboard(KeyboardReport::default()),
                )?;

                println!("  key '{}'", letter as char);
            }

            "l" => {
                send(&mut write_port, &HostMsg::QuerySlots)?;
                // Give firmware time to respond.
                std::thread::sleep(Duration::from_millis(200));
                let mut found = false;
                while let Ok(msg) = fw_rx.try_recv() {
                    if let FirmwareMsg::SlotStatus {
                        slot,
                        addr,
                        connected,
                    } = msg
                    {
                        let status = if connected {
                            "connected"
                        } else {
                            "disconnected"
                        };
                        println!("  slot {slot}: {status} {}", format_addr(&addr));
                        found = true;
                    }
                }
                if !found {
                    println!("  no connected devices");
                }
            }

            "q" => {
                println!("quit");
                break;
            }

            "" => {}

            other => {
                println!("  unknown command: {other:?}");
            }
        }
    }

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
