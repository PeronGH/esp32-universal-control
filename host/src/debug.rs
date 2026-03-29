//! Debug CLI mode: interactive commands for testing semantic firmware traffic.

use std::io::{self, BufRead, Write};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context;
use esp32_uc_protocol::input::{KeyboardSnapshot, TouchContact, TouchFrame};
use esp32_uc_protocol::wire::{FirmwareMsg, HostMsg};

use crate::serial;
use crate::slots::{self, SlotTable};

const INPUT_PAUSE: Duration = Duration::from_millis(16);
const KEY_PAUSE: Duration = Duration::from_millis(10);
const HID_KEY_A: u8 = 0x04;

fn handle_fw_event(msg: FirmwareMsg, slots: &mut SlotTable) {
    match msg {
        FirmwareMsg::HelloAck(_) => {}
        FirmwareMsg::PeerSnapshot(snapshot) => {
            slots.apply_snapshot(&snapshot);
            println!("  [event] peer snapshot");
        }
        FirmwareMsg::PeerConnected(peer) => {
            slots.connect(peer);
            println!(
                "  [event] slot {}: connected {}",
                peer.slot,
                serial::format_addr(&peer.addr)
            );
        }
        FirmwareMsg::PeerDisconnected { slot } => {
            slots.disconnect(slot);
            println!("  [event] slot {slot}: disconnected");
        }
        FirmwareMsg::ActivePeerChanged(active_slot) => {
            slots.set_active(active_slot);
            println!("  [event] active slot: {:?}", slots.active());
        }
        FirmwareMsg::LedState(bits) => {
            println!("  [event] LED state: {bits:#04x}");
        }
        FirmwareMsg::ProtocolError(err) => {
            println!("  [event] protocol error: {err:?}");
        }
    }
}

/// Run the debug CLI on the given serial port.
pub fn run(port_name: &str) -> anyhow::Result<()> {
    let port = serial::open_port(port_name)?;
    let mut write_port = port.try_clone().context("clone serial port")?;

    let (fw_tx, fw_rx) = mpsc::channel::<FirmwareMsg>();
    serial::spawn_reader(port, fw_tx);
    let snapshot = serial::handshake(&mut write_port, &fw_rx)?;

    let mut slots = SlotTable::from_snapshot(&snapshot);
    let mut key_counter: u8 = 0;

    println!("Connected to {port_name}");
    println!("  t       = touch sweep");
    println!("  k       = random key");
    println!("  l       = list mirrored slots");
    println!("  s <N>   = select remote slot");
    println!("  m       = switch to Mac/local");
    println!("  q       = quit");

    let stdin = io::stdin();

    loop {
        while let Ok(msg) = fw_rx.try_recv() {
            handle_fw_event(msg, &mut slots);
        }

        print!("[{:?}] > ", slots.active());
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
                    let mut contacts = [TouchContact::default(); 5];
                    contacts[0] = TouchContact {
                        contact_id: 1,
                        x,
                        y: 6000,
                        touching: true,
                        confident: true,
                    };
                    serial::send(
                        &mut write_port,
                        &HostMsg::TouchFrame(TouchFrame {
                            contacts,
                            contact_count: 1,
                            button: false,
                        }),
                    )?;
                    std::thread::sleep(INPUT_PAUSE);
                    x += 200;
                }
                serial::send(&mut write_port, &HostMsg::TouchFrame(TouchFrame::default()))?;
                println!("  touch sweep done");
            }

            "k" => {
                let keycode = HID_KEY_A + (key_counter % 26);
                key_counter = key_counter.wrapping_add(1);
                serial::send(
                    &mut write_port,
                    &HostMsg::KeyboardState(KeyboardSnapshot {
                        modifiers: 0,
                        keys: [keycode, 0, 0, 0, 0, 0],
                    }),
                )?;
                std::thread::sleep(KEY_PAUSE);
                serial::send(
                    &mut write_port,
                    &HostMsg::KeyboardState(KeyboardSnapshot::default()),
                )?;
                println!("  keycode {keycode:#04x}");
            }

            "l" => {
                slots.print_status();
            }

            "m" => {
                serial::send(&mut write_port, &HostMsg::SelectPeer(None))?;
                println!("  requested local mode");
            }

            "s" => {
                if let Some(n) = parts.get(1).and_then(|s| s.parse::<usize>().ok()) {
                    if n < slots::MAX_SLOTS {
                        serial::send(&mut write_port, &HostMsg::SelectPeer(Some(n as u8)))?;
                        println!("  requested remote slot: {n}");
                    } else {
                        println!("  slot must be 0..{}", slots::MAX_SLOTS - 1);
                    }
                } else {
                    println!("  usage: s <0-{}>", slots::MAX_SLOTS - 1);
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
