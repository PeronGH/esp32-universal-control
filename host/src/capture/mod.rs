//! Real capture mode: keyboard + trackpad → serial → firmware → BLE.

mod keyboard;
mod keymap;
mod trackpad;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use anyhow::Context;
use esp32_uc_protocol::wire::{FirmwareMsg, HostMsg};
use log::info;

use crate::serial;

/// Run real capture mode: capture Mac keyboard and trackpad input,
/// forward as HID reports to the firmware.
pub fn run(port_name: &str) -> anyhow::Result<()> {
    let port = serial::open_port(port_name)?;
    let mut write_port = port.try_clone().context("clone serial port")?;

    let (fw_tx, fw_rx) = mpsc::channel::<FirmwareMsg>();
    serial::spawn_reader(port, fw_tx);
    serial::handshake(&mut write_port, &fw_rx)?;

    info!("Real capture mode — forwarding keyboard + trackpad to firmware");

    // Channel for captured input events → serial writer.
    let (input_tx, input_rx) = mpsc::channel::<HostMsg>();

    // Serial writer thread: drains input events and writes to UART.
    let mut writer = write_port;
    std::thread::Builder::new()
        .name("serial-writer".into())
        .spawn(move || {
            while let Ok(msg) = input_rx.recv() {
                if let Err(e) = serial::send(&mut writer, &msg) {
                    log::warn!("Serial send error: {e}");
                }
            }
        })?;

    // Firmware event printer thread: shows BLE events on stdout.
    std::thread::Builder::new()
        .name("fw-events".into())
        .spawn(move || {
            while let Ok(msg) = fw_rx.recv() {
                match msg {
                    FirmwareMsg::ConnectionStatus { addr, connected } => {
                        let status = if connected {
                            "connected"
                        } else {
                            "disconnected"
                        };
                        info!("BLE {status}: {}", serial::format_addr(&addr));
                    }
                    FirmwareMsg::LedState(bits) => {
                        info!("LED state: {bits:#04x}");
                    }
                    FirmwareMsg::Pong => {}
                }
            }
        })?;

    // Shared click state: CGEventTap detects trackpad clicks,
    // trackpad callback reads it to set the PTP button field.
    let click_state = Arc::new(AtomicBool::new(false));

    // Keyboard + click capture thread (needs its own CFRunLoop).
    let kb_tx = input_tx.clone();
    let click = Arc::clone(&click_state);
    std::thread::Builder::new()
        .name("keyboard".into())
        .spawn(move || {
            if let Err(e) = keyboard::run(kb_tx, click) {
                log::error!("Keyboard capture failed: {e}");
            }
        })?;

    // Trackpad capture on this thread (runs its own CFRunLoop).
    trackpad::run(input_tx, click_state)?;

    Ok(())
}
