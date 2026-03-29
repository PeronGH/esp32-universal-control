//! Real capture mode: keyboard + trackpad → serial → firmware → BLE.

mod keyboard;
mod keymap;
mod trackpad;

use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use esp32_uc_protocol::wire::{FirmwareMsg, HostMsg};
use log::info;

use crate::serial;
use crate::slots::SlotTable;

/// Run real capture mode: capture Mac keyboard and trackpad input,
/// forward as HID reports to the firmware.
pub fn run(port_name: &str) -> anyhow::Result<()> {
    let port = serial::open_port(port_name)?;
    let mut write_port = port.try_clone().context("clone serial port")?;

    let (fw_tx, fw_rx) = mpsc::channel::<FirmwareMsg>();
    serial::spawn_reader(port, fw_tx);
    let snapshot = serial::handshake(&mut write_port, &fw_rx)?;

    info!("Real capture mode: forwarding keyboard + trackpad to firmware");

    let slots = Arc::new(Mutex::new(SlotTable::from_snapshot(&snapshot)));
    {
        let table = slots.lock().expect("poisoned");
        if table.is_forwarding() {
            keyboard::hide_mac_cursor();
        }
        table.print_status();
    }

    // Channel for captured input events → serial writer.
    let (input_tx, input_rx) = mpsc::channel::<HostMsg>();

    // Serial writer thread. On disconnect, forces back to Mac.
    let mut writer = write_port;
    let writer_slots = Arc::clone(&slots);
    std::thread::Builder::new()
        .name("serial-writer".into())
        .spawn(move || {
            while let Ok(msg) = input_rx.recv() {
                if let Err(e) = serial::send(&mut writer, &msg) {
                    log::error!("Serial disconnected ({e}), falling back to Mac");
                    writer_slots.lock().expect("poisoned").set_active(None);
                    keyboard::show_mac_cursor();
                    writer_slots.lock().expect("poisoned").print_status();
                    break;
                }
            }
        })?;

    // Firmware event thread: updates slot table and prints status.
    let fw_slots = Arc::clone(&slots);
    std::thread::Builder::new()
        .name("fw-events".into())
        .spawn(move || {
            while let Ok(msg) = fw_rx.recv() {
                match msg {
                    FirmwareMsg::PeerSnapshot(snapshot) => {
                        let mut table = fw_slots.lock().expect("slot table poisoned");
                        table.apply_snapshot(&snapshot);
                        if table.is_forwarding() {
                            keyboard::hide_mac_cursor();
                        } else {
                            keyboard::show_mac_cursor();
                        }
                        table.print_status();
                    }
                    FirmwareMsg::PeerConnected(peer) => {
                        let mut table = fw_slots.lock().expect("slot table poisoned");
                        table.connect(peer);
                        info!(
                            "BLE slot {}: connected {}",
                            peer.slot,
                            serial::format_addr(&peer.addr)
                        );
                        table.print_status();
                    }
                    FirmwareMsg::PeerDisconnected { slot } => {
                        let mut table = fw_slots.lock().expect("slot table poisoned");
                        table.disconnect(slot);
                        keyboard::show_mac_cursor();
                        info!("BLE slot {slot}: disconnected");
                        table.print_status();
                    }
                    FirmwareMsg::ActivePeerChanged(active_slot) => {
                        let mut table = fw_slots.lock().expect("slot table poisoned");
                        table.set_active(active_slot);
                        if table.is_forwarding() {
                            keyboard::hide_mac_cursor();
                            if let Some(slot) = table.active() {
                                info!("Switched to remote slot {slot}");
                            }
                        } else {
                            keyboard::show_mac_cursor();
                            info!("Switched to Mac (local)");
                        }
                        table.print_status();
                    }
                    FirmwareMsg::LedState(bits) => {
                        info!("LED state: {bits:#04x}");
                    }
                    FirmwareMsg::HelloAck(_) => {}
                    FirmwareMsg::ProtocolError(err) => {
                        log::warn!("Firmware protocol error: {err:?}");
                    }
                }
            }
        })?;

    // Lock-free forwarding flag for the CGEventTap callback.
    let forwarding = slots.lock().expect("poisoned").forwarding_flag();

    // Shared click state.
    let click_state = Arc::new(AtomicBool::new(false));

    // Keyboard + mouse capture thread.
    let kb_tx = input_tx.clone();
    let click = Arc::clone(&click_state);
    let kb_fwd = Arc::clone(&forwarding);
    let kb_slots = Arc::clone(&slots);
    std::thread::Builder::new()
        .name("keyboard".into())
        .spawn(move || {
            if let Err(e) = keyboard::run(kb_tx, click, kb_fwd, kb_slots) {
                log::error!("Keyboard capture failed: {e}");
            }
        })?;

    // Trackpad capture on this thread.
    let tp_fwd = Arc::clone(&forwarding);
    trackpad::run(input_tx, click_state, tp_fwd)?;

    Ok(())
}
