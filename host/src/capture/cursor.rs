//! Local cursor lifecycle during remote forwarding sessions.

use core_graphics::display::CGDisplay;
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use super::keyboard;

#[derive(Clone, Copy, Debug, PartialEq)]
struct CursorPosition {
    x: f64,
    y: f64,
}

impl From<CGPoint> for CursorPosition {
    fn from(point: CGPoint) -> Self {
        Self {
            x: point.x,
            y: point.y,
        }
    }
}

impl From<CursorPosition> for CGPoint {
    fn from(point: CursorPosition) -> Self {
        CGPoint::new(point.x, point.y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CursorAction {
    None,
    Hide,
    RestoreAndShow(CursorPosition),
    Show,
}

#[derive(Debug, Default)]
struct CursorSession {
    forwarding: bool,
    restore_point: Option<CursorPosition>,
}

impl CursorSession {
    fn transition(
        &mut self,
        next_forwarding: bool,
        current_position: Option<CursorPosition>,
    ) -> CursorAction {
        if self.forwarding == next_forwarding {
            return CursorAction::None;
        }

        self.forwarding = next_forwarding;

        if next_forwarding {
            if self.restore_point.is_none() {
                self.restore_point = current_position;
            }
            return CursorAction::Hide;
        }

        self.restore_point
            .take()
            .map_or(CursorAction::Show, CursorAction::RestoreAndShow)
    }
}

#[derive(Debug, Default)]
pub(super) struct CursorController {
    session: CursorSession,
}

impl CursorController {
    pub(super) fn sync_forwarding(&mut self, next_forwarding: bool) {
        let action = if next_forwarding {
            let current_position = current_cursor_position().inspect_err(|error| {
                log::warn!("Failed to capture Mac cursor position: {error:#}");
            });
            self.session
                .transition(next_forwarding, current_position.ok())
        } else {
            self.session.transition(next_forwarding, None)
        };

        match action {
            CursorAction::None => {}
            CursorAction::Hide => keyboard::hide_mac_cursor(),
            CursorAction::RestoreAndShow(position) => {
                if let Err(error) = warp_cursor_position(position) {
                    log::warn!("Failed to restore Mac cursor position: {error:#}");
                }
                keyboard::show_mac_cursor();
            }
            CursorAction::Show => keyboard::show_mac_cursor(),
        }
    }
}

fn current_cursor_position() -> anyhow::Result<CursorPosition> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| anyhow::anyhow!("CGEventSource::new failed"))?;
    let event = CGEvent::new(source).map_err(|()| anyhow::anyhow!("CGEvent::new failed"))?;
    Ok(event.location().into())
}

fn warp_cursor_position(position: CursorPosition) -> anyhow::Result<()> {
    CGDisplay::warp_mouse_cursor_position(position.into())
        .map_err(|error| anyhow::anyhow!("CGWarpMouseCursorPosition failed with code {error}"))
}

#[cfg(test)]
mod tests {
    use super::{CursorAction, CursorPosition, CursorSession};

    fn position(x: f64, y: f64) -> CursorPosition {
        CursorPosition { x, y }
    }

    #[test]
    fn entering_forwarding_saves_position_and_hides_cursor() {
        let mut session = CursorSession::default();

        let action = session.transition(true, Some(position(100.0, 200.0)));

        assert_eq!(action, CursorAction::Hide);
        assert_eq!(session.restore_point, Some(position(100.0, 200.0)));
    }

    #[test]
    fn switching_between_remote_targets_keeps_original_restore_point() {
        let mut session = CursorSession::default();
        session.transition(true, Some(position(10.0, 20.0)));

        let action = session.transition(true, Some(position(30.0, 40.0)));

        assert_eq!(action, CursorAction::None);
        assert_eq!(session.restore_point, Some(position(10.0, 20.0)));
    }

    #[test]
    fn returning_to_local_restores_once_and_clears_saved_position() {
        let mut session = CursorSession::default();
        session.transition(true, Some(position(1.0, 2.0)));

        let action = session.transition(false, None);

        assert_eq!(action, CursorAction::RestoreAndShow(position(1.0, 2.0)));
        assert_eq!(session.restore_point, None);
        assert_eq!(session.transition(false, None), CursorAction::None);
    }

    #[test]
    fn non_transition_while_forwarding_does_not_restore_cursor() {
        let mut session = CursorSession::default();
        session.transition(true, Some(position(5.0, 6.0)));

        let action = session.transition(true, Some(position(7.0, 8.0)));

        assert_eq!(action, CursorAction::None);
        assert_eq!(session.restore_point, Some(position(5.0, 6.0)));
    }

    #[test]
    fn startup_forwarding_seeds_restore_point() {
        let mut session = CursorSession::default();

        session.transition(true, Some(position(42.0, 24.0)));

        assert_eq!(session.restore_point, Some(position(42.0, 24.0)));
    }
}
