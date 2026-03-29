//! Semantic host session and active-peer routing.

use std::sync::mpsc;

use esp32_uc_protocol::input::{ConsumerState, KeyboardSnapshot};
use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::ptp::{PtpReport, TouchReportEncoder};
use esp32_uc_protocol::wire::{
    FirmwareMsg, HelloAck, HostMsg, PeerDescriptor, PeerSnapshot, ProtocolError, MAX_PEERS,
    PROTOCOL_VERSION,
};

use crate::ble_hid::{BleEvent, BleHid};

#[derive(Clone, Copy, Debug)]
struct PeerSlot {
    conn_handle: u16,
    addr: [u8; 6],
}

/// Firmware-owned semantic session state.
pub struct Session {
    host_ready: bool,
    peers: [Option<PeerSlot>; MAX_PEERS],
    active_slot: Option<u8>,
    keyboard: KeyboardSnapshot,
    consumer: ConsumerState,
    touch: TouchReportEncoder,
}

impl Session {
    /// Create a fresh firmware session with no host handshake and no peers.
    pub const fn new() -> Self {
        Self {
            host_ready: false,
            peers: [None; MAX_PEERS],
            active_slot: None,
            keyboard: KeyboardSnapshot {
                modifiers: 0,
                keys: [0; esp32_uc_protocol::input::MAX_KEYS],
            },
            consumer: 0,
            touch: TouchReportEncoder::new(),
        }
    }

    /// Handle one BLE event from the HID wrapper.
    pub fn handle_ble_event(&mut self, tx: &mpsc::Sender<FirmwareMsg>, event: BleEvent) {
        match event {
            BleEvent::Connected { conn_handle, addr } => {
                if let Some(slot) = self.first_free_slot() {
                    self.peers[slot] = Some(PeerSlot { conn_handle, addr });
                    if self.host_ready {
                        let _ = tx.send(FirmwareMsg::PeerConnected(PeerDescriptor {
                            slot: slot as u8,
                            addr,
                        }));
                    }
                }
            }
            BleEvent::Disconnected { conn_handle } => {
                if let Some(slot) = self.slot_for_conn_handle(conn_handle) {
                    self.peers[slot] = None;
                    let was_active = self.active_slot == Some(slot as u8);
                    if was_active {
                        self.active_slot = None;
                        self.reset_input_state();
                    }
                    if self.host_ready {
                        let _ = tx.send(FirmwareMsg::PeerDisconnected { slot: slot as u8 });
                        if was_active {
                            let _ = tx.send(FirmwareMsg::ActivePeerChanged(None));
                        }
                    }
                }
            }
            BleEvent::LedState(bits) => {
                if self.host_ready {
                    let _ = tx.send(FirmwareMsg::LedState(bits));
                }
            }
        }
    }

    /// Handle one decoded host message.
    pub fn handle_host_msg(&mut self, ble: &BleHid, tx: &mpsc::Sender<FirmwareMsg>, msg: HostMsg) {
        match msg {
            HostMsg::Hello(hello) => {
                if hello.protocol_version != PROTOCOL_VERSION {
                    let _ = tx.send(FirmwareMsg::ProtocolError(
                        ProtocolError::UnsupportedProtocolVersion {
                            expected: PROTOCOL_VERSION,
                            received: hello.protocol_version,
                        },
                    ));
                    return;
                }

                self.host_ready = true;
                let _ = tx.send(FirmwareMsg::HelloAck(HelloAck {
                    protocol_version: PROTOCOL_VERSION,
                    max_peers: MAX_PEERS as u8,
                }));
                let _ = tx.send(FirmwareMsg::PeerSnapshot(self.snapshot()));
            }
            HostMsg::SelectPeer(slot) => {
                self.select_peer(ble, tx, slot);
            }
            HostMsg::KeyboardState(snapshot) => {
                if let Some(conn_handle) = self.active_conn_handle() {
                    self.keyboard = snapshot;
                    ble.send_keyboard_to(conn_handle, &KeyboardReport::from(snapshot));
                }
            }
            HostMsg::ConsumerState(bits) => {
                if let Some(conn_handle) = self.active_conn_handle() {
                    self.consumer = bits;
                    ble.send_consumer_to(conn_handle, bits);
                }
            }
            HostMsg::TouchFrame(frame) => {
                if let Some(conn_handle) = self.active_conn_handle() {
                    if let Some(report) = self.touch.encode(&frame, current_scan_time()) {
                        ble.send_touch_to(conn_handle, &report);
                    }
                }
            }
        }
    }

    fn select_peer(&mut self, ble: &BleHid, tx: &mpsc::Sender<FirmwareMsg>, slot: Option<u8>) {
        let Some(next_slot) = slot else {
            if let Some(conn_handle) = self.active_conn_handle() {
                self.clear_outputs_for(ble, conn_handle);
            }
            self.active_slot = None;
            self.reset_input_state();
            if self.host_ready {
                let _ = tx.send(FirmwareMsg::ActivePeerChanged(None));
            }
            return;
        };

        let next_idx = usize::from(next_slot);
        if next_idx >= MAX_PEERS || self.peers[next_idx].is_none() {
            if self.host_ready {
                let _ = tx.send(FirmwareMsg::ProtocolError(ProtocolError::InvalidPeerSlot(
                    next_slot,
                )));
            }
            return;
        }

        if self.active_slot == Some(next_slot) {
            return;
        }

        if let Some(conn_handle) = self.active_conn_handle() {
            self.clear_outputs_for(ble, conn_handle);
        }

        self.active_slot = Some(next_slot);
        self.reset_input_state();
        if self.host_ready {
            let _ = tx.send(FirmwareMsg::ActivePeerChanged(Some(next_slot)));
        }
    }

    fn clear_outputs_for(&mut self, ble: &BleHid, conn_handle: u16) {
        ble.send_keyboard_to(conn_handle, &KeyboardReport::default());
        ble.send_consumer_to(conn_handle, 0);
        self.touch.reset();
        ble.send_touch_to(
            conn_handle,
            &PtpReport {
                scan_time: current_scan_time(),
                ..PtpReport::default()
            },
        );
    }

    fn reset_input_state(&mut self) {
        self.keyboard = KeyboardSnapshot::default();
        self.consumer = 0;
        self.touch.reset();
    }

    fn snapshot(&self) -> PeerSnapshot {
        let mut peers = [None; MAX_PEERS];
        let mut slot = 0usize;
        while slot < MAX_PEERS {
            if let Some(peer) = self.peers[slot] {
                peers[slot] = Some(PeerDescriptor {
                    slot: slot as u8,
                    addr: peer.addr,
                });
            }
            slot += 1;
        }
        PeerSnapshot {
            peers,
            active_slot: self.active_slot,
        }
    }

    fn first_free_slot(&self) -> Option<usize> {
        self.peers.iter().position(Option::is_none)
    }

    fn slot_for_conn_handle(&self, conn_handle: u16) -> Option<usize> {
        self.peers
            .iter()
            .position(|slot| slot.is_some_and(|peer| peer.conn_handle == conn_handle))
    }

    fn active_conn_handle(&self) -> Option<u16> {
        self.active_slot
            .and_then(|slot| self.peers[usize::from(slot)].map(|peer| peer.conn_handle))
    }
}

fn current_scan_time() -> u16 {
    // SAFETY: esp_timer_get_time is always safe to call on ESP-IDF.
    let us = unsafe { esp_idf_svc::sys::esp_timer_get_time() } as u64;
    (us / 100) as u16
}
