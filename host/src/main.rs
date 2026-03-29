use std::io::{self, BufRead, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, bail};
use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::ptp::{PtpContact, PtpReport};
use esp32_uc_protocol::wire::{FirmwareMsg, HostMsg};
use serialport5::SerialPort;

const MAX_SLOTS: usize = 4;

/// Host-side slot table. Maps slots to BLE addresses.
/// The firmware has no concept of slots — this is entirely host-side.
struct SlotTable {
    slots: [Option<[u8; 6]>; MAX_SLOTS],
    active: usize,
}

impl SlotTable {
    fn new() -> Self {
        Self {
            slots: [None; MAX_SLOTS],
            active: 0,
        }
    }

    /// Assign an address to the first empty slot, or return its existing slot.
    fn assign(&mut self, addr: [u8; 6]) -> usize {
        // Already assigned?
        if let Some(i) = self.slots.iter().position(|s| *s == Some(addr)) {
            return i;
        }
        // First empty slot.
        if let Some(i) = self.slots.iter().position(|s| s.is_none()) {
            self.slots[i] = Some(addr);
            return i;
        }
        // Full — overwrite active slot.
        self.slots[self.active] = Some(addr);
        self.active
    }

    /// Mark an address as disconnected (clear its slot).
    fn disconnect(&mut self, addr: [u8; 6]) {
        if let Some(slot) = self.slots.iter_mut().find(|s| **s == Some(addr)) {
            *slot = None;
        }
    }

    fn print(&self) {
        for (i, slot) in self.slots.iter().enumerate() {
            let marker = if i == self.active { "* " } else { "  " };
            match slot {
                Some(addr) => println!("{marker}slot {i}: {}", format_addr(addr)),
                None => println!("{marker}slot {i}: (empty)"),
            }
        }
    }
}

/// Format a BLE address (little-endian bytes) as a colon-separated string.
fn format_addr(addr: &[u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        addr[5], addr[4], addr[3], addr[2], addr[1], addr[0]
    )
}

/// Encode a `HostMsg` and write it to the serial port.
fn send(port: &mut SerialPort, msg: &HostMsg) -> anyhow::Result<()> {
    let mut buf = [0u8; 128];
    let encoded = postcard::to_slice_cobs(msg, &mut buf).context("postcard encode")?;
    port.write_all(encoded).context("serial write")?;
    Ok(())
}

/// Background reader: decodes `FirmwareMsg` from the serial port.
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
            Ok(other) => eprintln!(" unexpected: {other:?}"),
            Err(_) => eprintln!(" timeout"),
        }
    }
    bail!("firmware did not respond to Ping — wrong port or firmware not running");
}

/// Handle a firmware event: update slot table and print.
fn handle_fw_event(msg: FirmwareMsg, slots: &mut SlotTable) {
    match msg {
        FirmwareMsg::Pong => {}
        FirmwareMsg::ConnectionStatus { addr, connected } => {
            if connected {
                let slot = slots.assign(addr);
                println!("  [event] slot {slot}: connected {}", format_addr(&addr));
            } else {
                slots.disconnect(addr);
                println!("  [event] disconnected {}", format_addr(&addr));
            }
        }
        FirmwareMsg::LedState(bits) => {
            println!("  [event] LED state: {bits:#04x}");
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

    let (fw_tx, fw_rx) = mpsc::channel::<FirmwareMsg>();
    spawn_reader(port, fw_tx);

    handshake(&mut write_port, &fw_rx)?;

    let mut slots = SlotTable::new();
    let mut scan_time: u16 = 0;

    println!("Connected to {port_name}");
    println!("  t       = touch sweep");
    println!("  k       = random key");
    println!("  l       = list connections");
    println!("  s <N>   = switch active slot");
    println!("  q       = quit");

    let stdin = io::stdin();

    loop {
        // Drain pending firmware events.
        while let Ok(msg) = fw_rx.try_recv() {
            handle_fw_event(msg, &mut slots);
        }

        print!("[{}] > ", slots.active);
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        let cmd = parts.first().copied().unwrap_or("");

        match cmd {
            "t" => {
                let mut x: u16 = 5000;
                while x <= 15_000 {
                    let report = PtpReport {
                        contacts: {
                            let mut c = [PtpContact::default(); 5];
                            c[0] = PtpContact {
                                flags: PtpContact::FINGER_DOWN,
                                contact_id: 1,
                                x,
                                y: 6000,
                            };
                            c
                        },
                        scan_time,
                        contact_count: 1,
                        button: 0,
                    };
                    scan_time = scan_time.wrapping_add(50);
                    send(&mut write_port, &HostMsg::Touch(report))?;
                    std::thread::sleep(Duration::from_millis(16));
                    x += 200;
                }
                let report = PtpReport {
                    scan_time,
                    ..PtpReport::default()
                };
                scan_time = scan_time.wrapping_add(50);
                send(&mut write_port, &HostMsg::Touch(report))?;
                println!("  touch sweep done");
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
                send(&mut write_port, &HostMsg::QueryConnections)?;
                // Collect responses with timeout instead of fixed sleep.
                while let Ok(msg) = fw_rx.recv_timeout(Duration::from_millis(300)) {
                    handle_fw_event(msg, &mut slots);
                }
                slots.print();
            }

            "s" => {
                if let Some(n) = parts.get(1).and_then(|s| s.parse::<usize>().ok()) {
                    if n < MAX_SLOTS {
                        slots.active = n;
                        println!("  active slot: {n}");
                    } else {
                        println!("  slot must be 0..{}", MAX_SLOTS - 1);
                    }
                } else {
                    println!("  usage: s <0-{}>", MAX_SLOTS - 1);
                }
            }

            "q" => {
                println!("quit");
                break;
            }

            "" => {}

            other => println!("  unknown command: {other:?}"),
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
