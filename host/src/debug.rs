//! Debug CLI mode: interactive commands for testing firmware communication.

use std::io::{self, BufRead, Write};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context;
use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::ptp::{PtpContact, PtpReport};
use esp32_uc_protocol::wire::{FirmwareMsg, HostMsg};

use crate::serial;

const MAX_SLOTS: usize = 4;
const QUERY_RESPONSE_TIMEOUT: Duration = Duration::from_millis(300);
/// USB HID keycode for 'a' (usage table 0x04..0x1D = a..z).
const HID_KEY_A: u8 = 0x04;

/// Host-side slot table. Maps slots to BLE addresses.
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

    fn assign(&mut self, addr: [u8; 6]) -> usize {
        if let Some(i) = self.slots.iter().position(|s| *s == Some(addr)) {
            return i;
        }
        if let Some(i) = self.slots.iter().position(|s| s.is_none()) {
            self.slots[i] = Some(addr);
            return i;
        }
        self.slots[self.active] = Some(addr);
        self.active
    }

    fn disconnect(&mut self, addr: [u8; 6]) {
        if let Some(slot) = self.slots.iter_mut().find(|s| **s == Some(addr)) {
            *slot = None;
        }
    }

    fn print(&self) {
        for (i, slot) in self.slots.iter().enumerate() {
            let marker = if i == self.active { "* " } else { "  " };
            match slot {
                Some(addr) => println!("{marker}slot {i}: {}", serial::format_addr(addr)),
                None => println!("{marker}slot {i}: (empty)"),
            }
        }
    }
}

fn handle_fw_event(msg: FirmwareMsg, slots: &mut SlotTable) {
    match msg {
        FirmwareMsg::Pong => {}
        FirmwareMsg::ConnectionStatus { addr, connected } => {
            if connected {
                let slot = slots.assign(addr);
                println!(
                    "  [event] slot {slot}: connected {}",
                    serial::format_addr(&addr)
                );
            } else {
                slots.disconnect(addr);
                println!("  [event] disconnected {}", serial::format_addr(&addr));
            }
        }
        FirmwareMsg::LedState(bits) => {
            println!("  [event] LED state: {bits:#04x}");
        }
    }
}

/// Run the debug CLI on the given serial port.
pub fn run(port_name: &str) -> anyhow::Result<()> {
    let port = serial::open_port(port_name)?;
    let mut write_port = port.try_clone().context("clone serial port")?;

    let (fw_tx, fw_rx) = mpsc::channel::<FirmwareMsg>();
    serial::spawn_reader(port, fw_tx);
    serial::handshake(&mut write_port, &fw_rx)?;

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
                    serial::send(&mut write_port, &HostMsg::Touch(report))?;
                    std::thread::sleep(Duration::from_millis(16));
                    x += 200;
                }
                let report = PtpReport {
                    scan_time,
                    ..PtpReport::default()
                };
                scan_time = scan_time.wrapping_add(50);
                serial::send(&mut write_port, &HostMsg::Touch(report))?;
                println!("  touch sweep done");
            }

            "k" => {
                let letter = b'a' + (scan_time as u8 % 26);
                let keycode = HID_KEY_A + (letter - b'a');
                serial::send(
                    &mut write_port,
                    &HostMsg::Keyboard(KeyboardReport {
                        keycodes: [keycode, 0, 0, 0, 0, 0],
                        ..KeyboardReport::default()
                    }),
                )?;
                std::thread::sleep(Duration::from_millis(10));
                serial::send(
                    &mut write_port,
                    &HostMsg::Keyboard(KeyboardReport::default()),
                )?;
                println!("  key '{}'", letter as char);
            }

            "l" => {
                serial::send(&mut write_port, &HostMsg::QueryConnections)?;
                while let Ok(msg) = fw_rx.recv_timeout(QUERY_RESPONSE_TIMEOUT) {
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
