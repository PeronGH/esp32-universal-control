//! Host-side outbound queue with latest-frame coalescing for touch input.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

use esp32_uc_protocol::input::TouchFrame;
use esp32_uc_protocol::wire::HostMsg;

#[derive(Debug, Default)]
struct OutboxState {
    control: VecDeque<HostMsg>,
    latest_touch: Option<TouchFrame>,
}

/// Queue semantic host messages for serial delivery.
///
/// Control messages are kept in order. Touch frames are coalesced so only the
/// latest outstanding frame is retained.
#[derive(Debug, Default)]
pub struct Outbox {
    state: Mutex<OutboxState>,
    ready: Condvar,
}

impl Outbox {
    /// Create an empty outbox.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push one message for serial delivery.
    pub fn push(&self, msg: HostMsg) {
        let mut state = self.state.lock().expect("outbox poisoned");
        match msg {
            HostMsg::TouchFrame(frame) => {
                state.latest_touch = Some(frame);
            }
            HostMsg::SelectPeer(slot) => {
                state.latest_touch = None;
                state.control.push_back(HostMsg::SelectPeer(slot));
            }
            other => {
                state.control.push_back(other);
            }
        }
        self.ready.notify_one();
    }

    /// Block until the next outbound message is available.
    pub fn recv(&self) -> HostMsg {
        let mut state = self.state.lock().expect("outbox poisoned");
        loop {
            if let Some(msg) = state.control.pop_front() {
                return msg;
            }
            if let Some(frame) = state.latest_touch.take() {
                return HostMsg::TouchFrame(frame);
            }
            state = self.ready.wait(state).expect("outbox poisoned");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use esp32_uc_protocol::input::{KeyboardSnapshot, TouchContact};

    #[test]
    fn coalesces_touch_frames_to_latest_one() {
        let outbox = Outbox::new();
        let mut first_contacts = [TouchContact::default(); 5];
        first_contacts[0] = TouchContact {
            contact_id: 1,
            x: 10,
            y: 20,
            touching: true,
            confident: true,
        };
        let mut second_contacts = [TouchContact::default(); 5];
        second_contacts[0] = TouchContact {
            contact_id: 1,
            x: 30,
            y: 40,
            touching: true,
            confident: true,
        };

        outbox.push(HostMsg::TouchFrame(TouchFrame {
            contacts: first_contacts,
            contact_count: 1,
            button: false,
        }));
        outbox.push(HostMsg::TouchFrame(TouchFrame {
            contacts: second_contacts,
            contact_count: 1,
            button: true,
        }));

        let HostMsg::TouchFrame(frame) = outbox.recv() else {
            panic!("expected touch frame");
        };
        assert_eq!(frame.contacts[0].x, 30);
        assert!(frame.button);
    }

    #[test]
    fn prioritizes_control_messages_and_clears_touch_on_select() {
        let outbox = Outbox::new();
        outbox.push(HostMsg::TouchFrame(TouchFrame::default()));
        outbox.push(HostMsg::SelectPeer(Some(2)));
        outbox.push(HostMsg::KeyboardState(KeyboardSnapshot {
            modifiers: 1,
            keys: [4, 0, 0, 0, 0, 0],
        }));

        assert_eq!(outbox.recv(), HostMsg::SelectPeer(Some(2)));
        assert_eq!(
            outbox.recv(),
            HostMsg::KeyboardState(KeyboardSnapshot {
                modifiers: 1,
                keys: [4, 0, 0, 0, 0, 0],
            })
        );
    }
}
