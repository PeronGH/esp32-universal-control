//! Semantic host session and active-peer routing.

use std::sync::mpsc;

use esp32_uc_protocol::input::{ConsumerState, KeyboardSnapshot, TouchFrame};
use esp32_uc_protocol::keyboard::KeyboardReport;
use esp32_uc_protocol::ptp::{PtpReport, TouchReportEncoder};
use esp32_uc_protocol::wire::{
    FirmwareMsg, HelloAck, HostMsg, PeerDescriptor, PeerSnapshot, ProtocolError, MAX_PEERS,
    PROTOCOL_VERSION,
};
use log::warn;

use crate::ble_hid::{BleEvent, BleHid};

/// Target BLE touch forwarding cadence (125 Hz).
const TOUCH_REPORT_INTERVAL_US: u64 = 8_000;
/// Retry backpressured touch notifications quickly without flooding the stack.
const TOUCH_RETRY_INTERVAL_US: u64 = 1_000;
/// Throttle repeated backpressure logs to keep the monitor readable.
const TOUCH_BACKPRESSURE_LOG_INTERVAL_US: u64 = 250_000;

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
    latest_touch: TouchFrame,
    touch_dirty: bool,
    touch: TouchReportEncoder,
    next_touch_send_at_us: u64,
    last_touch_backpressure_log_at_us: u64,
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
            latest_touch: TouchFrame {
                contacts: [esp32_uc_protocol::input::TouchContact {
                    contact_id: 0,
                    x: 0,
                    y: 0,
                    touching: false,
                    confident: false,
                }; esp32_uc_protocol::input::MAX_TOUCH_CONTACTS],
                contact_count: 0,
                button: false,
            },
            touch_dirty: false,
            touch: TouchReportEncoder::new(),
            next_touch_send_at_us: 0,
            last_touch_backpressure_log_at_us: 0,
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
                if self.active_conn_handle().is_some() {
                    self.latest_touch = frame;
                    self.touch_dirty = true;
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
        let _ = ble.send_touch_to(
            conn_handle,
            &PtpReport {
                scan_time: current_scan_time(),
                ..PtpReport::default()
            },
        );
        self.latest_touch = TouchFrame::default();
        self.touch_dirty = false;
        self.next_touch_send_at_us = 0;
    }

    fn reset_input_state(&mut self) {
        self.keyboard = KeyboardSnapshot::default();
        self.consumer = 0;
        self.touch.reset();
        self.latest_touch = TouchFrame::default();
        self.touch_dirty = false;
        self.next_touch_send_at_us = 0;
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

    /// Returns whether there is a touch frame waiting to be sent.
    pub fn has_pending_touch(&self) -> bool {
        self.touch_dirty
    }

    /// Send the latest touch frame if the BLE pacing window allows it.
    pub fn flush_touch_if_due(&mut self, ble: &BleHid) {
        let Some(conn_handle) = self.active_conn_handle() else {
            return;
        };
        if !self.touch_dirty {
            return;
        }

        let now_us = current_time_us();
        if now_us < self.next_touch_send_at_us {
            return;
        }

        let Some(report) = self
            .touch
            .encode(&self.latest_touch, scan_time_from_us(now_us))
        else {
            self.touch_dirty = false;
            self.next_touch_send_at_us = 0;
            return;
        };

        match ble.send_touch_to(conn_handle, &report) {
            Ok(()) => {
                self.touch_dirty = false;
                self.next_touch_send_at_us = now_us + TOUCH_REPORT_INTERVAL_US;
            }
            Err(err)
                if matches!(
                    err.code(),
                    esp_idf_svc::sys::BLE_HS_ENOMEM | esp_idf_svc::sys::BLE_HS_EBUSY
                ) =>
            {
                self.next_touch_send_at_us = now_us + TOUCH_RETRY_INTERVAL_US;
                if now_us.saturating_sub(self.last_touch_backpressure_log_at_us)
                    >= TOUCH_BACKPRESSURE_LOG_INTERVAL_US
                {
                    warn!("touch notify backpressured: {err:?}");
                    self.last_touch_backpressure_log_at_us = now_us;
                }
            }
            Err(err) => {
                warn!("touch notify failed: {err:?}");
                self.next_touch_send_at_us = now_us + TOUCH_RETRY_INTERVAL_US;
            }
        }
    }
}

fn current_scan_time() -> u16 {
    scan_time_from_us(current_time_us())
}

fn current_time_us() -> u64 {
    // SAFETY: esp_timer_get_time is always safe to call on ESP-IDF.
    unsafe { esp_idf_svc::sys::esp_timer_get_time() as u64 }
}

fn scan_time_from_us(us: u64) -> u16 {
    (us / 100) as u16
}
