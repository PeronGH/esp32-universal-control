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
    serial::handshake(&mut write_port, &fw_rx)?;

    info!("Real capture mode — forwarding keyboard + trackpad to firmware");

    // Shared slot table — updated by fw-events thread, read by keyboard hotkey.
    let slots = Arc::new(Mutex::new(SlotTable::new()));

    // Query existing connections so we know about devices that connected
    // before the host started.
    serial::send(&mut write_port, &HostMsg::QueryConnections)?;
    std::thread::sleep(std::time::Duration::from_millis(300));
    while let Ok(msg) = fw_rx.try_recv() {
        if let FirmwareMsg::ConnectionStatus { addr, connected } = msg
            && connected
        {
            let mut table = slots.lock().expect("poisoned");
            let slot = table.connect(addr);
            info!(
                "BLE slot {slot}: {} (already connected)",
                serial::format_addr(&addr)
            );
        }
    }
    slots.lock().expect("poisoned").print_status();

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
                    log::error!("Serial disconnected: {e} — falling back to Mac");
                    writer_slots.lock().expect("poisoned").switch_to_mac();
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
                    FirmwareMsg::ConnectionStatus { addr, connected } => {
                        let mut table = fw_slots.lock().expect("slot table poisoned");
                        if connected {
                            let slot = table.connect(addr);
                            info!("BLE slot {slot}: connected {}", serial::format_addr(&addr));
                        } else {
                            table.disconnect(addr);
                            info!("BLE disconnected: {}", serial::format_addr(&addr));
                        }
                        table.print_status();
                    }
                    FirmwareMsg::LedState(bits) => {
                        info!("LED state: {bits:#04x}");
                    }
                    FirmwareMsg::Pong => {}
                }
            }
        })?;

    // Shared click state.
    let click_state = Arc::new(AtomicBool::new(false));

    // Keyboard + click capture thread.
    let kb_tx = input_tx.clone();
    let click = Arc::clone(&click_state);
    let kb_slots = Arc::clone(&slots);
    std::thread::Builder::new()
        .name("keyboard".into())
        .spawn(move || {
            if let Err(e) = keyboard::run(kb_tx, click, kb_slots) {
                log::error!("Keyboard capture failed: {e}");
            }
        })?;

    // Trackpad capture on this thread.
    trackpad::run(input_tx, click_state, slots)?;

    Ok(())
}
