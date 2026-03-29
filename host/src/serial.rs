//! Shared serial I/O: port opening, handshake, reader thread, message sending.

use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, bail};
use esp32_uc_protocol::wire::{FirmwareMsg, Hello, HostMsg, PROTOCOL_VERSION, PeerSnapshot};
use postcard::accumulator::{CobsAccumulator, FeedResult};
use serialport5::SerialPort;

const BAUD_RATE: u32 = 115_200;
const SERIAL_READ_TIMEOUT: Duration = Duration::from_millis(100);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(1);
const HANDSHAKE_RETRIES: u32 = 5;
const COBS_BUF_SIZE: usize = 128;
const ENCODE_BUF_SIZE: usize = 128;

/// Open a serial port by name.
pub fn open_port(name: &str) -> anyhow::Result<SerialPort> {
    SerialPort::builder()
        .baud_rate(BAUD_RATE)
        .read_timeout(Some(SERIAL_READ_TIMEOUT))
        .open(name)
        .with_context(|| format!("open {name}"))
}

/// Encode a `HostMsg` and write it to the serial port.
pub fn send(port: &mut SerialPort, msg: &HostMsg) -> anyhow::Result<()> {
    let mut buf = [0u8; ENCODE_BUF_SIZE];
    let encoded = postcard::to_slice_cobs(msg, &mut buf).context("postcard encode")?;
    port.write_all(encoded).context("serial write")?;
    Ok(())
}

/// Spawn a background thread that decodes `FirmwareMsg` from the serial port
/// and sends them into the provided channel.
pub fn spawn_reader(
    mut port: SerialPort,
    tx: mpsc::Sender<FirmwareMsg>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut cobs_buf: CobsAccumulator<COBS_BUF_SIZE> = CobsAccumulator::new();
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
    })
}

/// Perform the semantic protocol handshake and return the initial peer
/// snapshot. Retries up to `HANDSHAKE_RETRIES` times.
pub fn handshake(
    write_port: &mut SerialPort,
    fw_rx: &mpsc::Receiver<FirmwareMsg>,
) -> anyhow::Result<PeerSnapshot> {
    for attempt in 1..=HANDSHAKE_RETRIES {
        eprint!("Handshake attempt {attempt}/{HANDSHAKE_RETRIES}...");
        send(
            write_port,
            &HostMsg::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
            }),
        )?;

        let mut hello_ok = false;

        loop {
            match fw_rx.recv_timeout(HANDSHAKE_TIMEOUT) {
                Ok(FirmwareMsg::HelloAck(ack)) => {
                    if ack.protocol_version != PROTOCOL_VERSION {
                        bail!(
                            "protocol mismatch: firmware={}, host={}",
                            ack.protocol_version,
                            PROTOCOL_VERSION
                        );
                    }
                    hello_ok = true;
                }
                Ok(FirmwareMsg::PeerSnapshot(snapshot)) if hello_ok => {
                    eprintln!(" ok");
                    return Ok(snapshot);
                }
                Ok(FirmwareMsg::ProtocolError(err)) => {
                    bail!("firmware rejected handshake: {err:?}");
                }
                Ok(other) => eprintln!(" unexpected: {other:?}"),
                Err(_) => {
                    eprintln!(" timeout");
                    break;
                }
            }
        }
    }
    bail!("firmware did not complete semantic handshake");
}

/// Format a BLE address (little-endian bytes) as a colon-separated string.
pub fn format_addr(addr: &[u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        addr[5], addr[4], addr[3], addr[2], addr[1], addr[0]
    )
}
