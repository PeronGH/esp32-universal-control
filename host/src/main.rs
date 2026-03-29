use std::io::{self, Read, Write};
use std::time::Duration;

use anyhow::Context;
use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::ptp::{PtpContact, PtpReport};
use esp32_uc_protocol::wire::HostMsg;
use serialport5::SerialPort;

/// Encode a `HostMsg` into a COBS-framed packet and write it to the serial port.
fn send(port: &mut SerialPort, msg: &HostMsg) -> anyhow::Result<()> {
    let mut buf = [0u8; 128];
    let encoded = postcard::to_slice_cobs(msg, &mut buf).context("postcard encode")?;
    port.write_all(encoded).context("serial write")?;
    Ok(())
}

fn run() -> anyhow::Result<()> {
    let port_name = std::env::args()
        .nth(1)
        .context("usage: esp32-uc-host <serial-port>")?;

    let mut port = SerialPort::builder()
        .baud_rate(115_200)
        .read_timeout(Some(Duration::from_millis(100)))
        .open(&port_name)
        .with_context(|| format!("open {port_name}"))?;

    println!("Connected to {port_name}");
    println!("  t = touch sweep step   k = random key   q = quit");

    let mut touch_x: u16 = 5000;
    let mut scan_time: u16 = 0;

    let stdin = io::stdin();
    let mut one = [0u8; 1];

    loop {
        print!("> ");
        io::stdout().flush()?;
        if stdin.lock().read(&mut one)? == 0 {
            break;
        }

        match one[0] {
            b't' => {
                // One step of horizontal touch sweep.
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
                    // Finger lift, reset for next sweep.
                    touch_x = 5000;
                }

                send(&mut port, &HostMsg::Touch(report))?;
                println!("  touch x={}", touch_x.saturating_sub(500));
            }

            b'k' => {
                // Random letter a-z.
                let letter = b'a' + (scan_time as u8 % 26);
                let keycode = 0x04 + (letter - b'a'); // USB HID: a=0x04

                // Key press
                send(
                    &mut port,
                    &HostMsg::Keyboard(KeyboardReport {
                        keycodes: [keycode, 0, 0, 0, 0, 0],
                        ..KeyboardReport::default()
                    }),
                )?;

                std::thread::sleep(Duration::from_millis(10));

                // Key release
                send(&mut port, &HostMsg::Keyboard(KeyboardReport::default()))?;

                println!("  key '{}'", letter as char);
            }

            b'q' => {
                println!("quit");
                break;
            }

            b'\n' | b'\r' => {}

            other => {
                println!("  unknown command: {:?}", other as char);
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
