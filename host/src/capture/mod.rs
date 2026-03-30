//! Real capture mode: keyboard + trackpad → serial → firmware → BLE.

mod cursor;
mod keyboard;
mod keymap;
mod outbox;
mod trackpad;

use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use esp32_uc_protocol::wire::{FirmwareMsg, HostMsg};
use log::info;

use self::cursor::CursorController;
use self::outbox::Outbox;
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
    let cursor = Arc::new(Mutex::new(CursorController::default()));
    let initial_forwarding = {
        let table = slots.lock().expect("poisoned");
        let forwarding = table.is_forwarding();
        table.print_status();
        forwarding
    };
    sync_cursor(&cursor, initial_forwarding);

    let outbox = Arc::new(Outbox::new());

    // Serial writer thread. On disconnect, forces back to Mac.
    let mut writer = write_port;
    let writer_slots = Arc::clone(&slots);
    let writer_cursor = Arc::clone(&cursor);
    let writer_outbox = Arc::clone(&outbox);
    std::thread::Builder::new()
        .name("serial-writer".into())
        .spawn(move || {
            loop {
                let msg: HostMsg = writer_outbox.recv();
                if let Err(e) = serial::send(&mut writer, &msg) {
                    log::error!("Serial disconnected ({e}), falling back to Mac");
                    let forwarding = {
                        let mut table = writer_slots.lock().expect("poisoned");
                        table.set_active(None);
                        table.print_status();
                        table.is_forwarding()
                    };
                    sync_cursor(&writer_cursor, forwarding);
                    break;
                }
            }
        })?;

    // Firmware event thread: updates slot table and prints status.
    let fw_slots = Arc::clone(&slots);
    let fw_cursor = Arc::clone(&cursor);
    std::thread::Builder::new()
        .name("fw-events".into())
        .spawn(move || {
            while let Ok(msg) = fw_rx.recv() {
                match msg {
                    FirmwareMsg::PeerSnapshot(snapshot) => {
                        let forwarding = {
                            let mut table = fw_slots.lock().expect("slot table poisoned");
                            table.apply_snapshot(&snapshot);
                            table.print_status();
                            table.is_forwarding()
                        };
                        sync_cursor(&fw_cursor, forwarding);
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
                        let forwarding = {
                            let mut table = fw_slots.lock().expect("slot table poisoned");
                            table.disconnect(slot);
                            table.print_status();
                            table.is_forwarding()
                        };
                        sync_cursor(&fw_cursor, forwarding);
                        info!("BLE slot {slot}: disconnected");
                    }
                    FirmwareMsg::ActivePeerChanged(active_slot) => {
                        let (forwarding, active) = {
                            let mut table = fw_slots.lock().expect("slot table poisoned");
                            table.set_active(active_slot);
                            table.print_status();
                            (table.is_forwarding(), table.active())
                        };
                        sync_cursor(&fw_cursor, forwarding);
                        if let Some(slot) = active {
                            info!("Switched to remote slot {slot}");
                        } else {
                            info!("Switched to Mac (local)");
                        }
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
    let click = Arc::clone(&click_state);
    let kb_fwd = Arc::clone(&forwarding);
    let kb_slots = Arc::clone(&slots);
    let kb_outbox = Arc::clone(&outbox);
    std::thread::Builder::new()
        .name("keyboard".into())
        .spawn(move || {
            if let Err(e) = keyboard::run(kb_outbox, click, kb_fwd, kb_slots) {
                log::error!("Keyboard capture failed: {e}");
            }
        })?;

    // Trackpad capture on this thread.
    let tp_fwd = Arc::clone(&forwarding);
    trackpad::run(outbox, click_state, tp_fwd)?;

    Ok(())
}

fn sync_cursor(cursor: &Mutex<CursorController>, forwarding: bool) {
    cursor
        .lock()
        .expect("cursor controller poisoned")
        .sync_forwarding(forwarding);
}
