//! What controller inputs mean to Hearth's menus, shared by every backend. Inputs are named by
//! Hearth pad position (`hearth_core::controls::PadInput` names: "south", "dpad_up", "guide").

use std::time::Instant;

use super::repeat::stick_direction;
use super::repeat::{DirectionSource, GuidePress, HeldDirections, KeyRepeat, STICK_DEADZONE};
use super::{Action, Direction};

/// The input events the state machine cares about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PadEvent {
    /// A button went down.
    Pressed(&'static str),
    /// A button went up.
    Released(&'static str),
    /// The left stick moved (y positive upwards).
    Stick(f32, f32),
    /// A controller went away: forget everything held.
    Disconnected,
}

/// What a button means to Hearth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonRole {
    Direction(Direction),
    Action(Action),
    Guide,
}

fn button_role(name: &str) -> Option<ButtonRole> {
    use ButtonRole as R;
    Some(match name {
        "dpad_up" => R::Direction(Direction::Up),
        "dpad_down" => R::Direction(Direction::Down),
        "dpad_left" => R::Direction(Direction::Left),
        "dpad_right" => R::Direction(Direction::Right),
        // TODO(M1): Nintendo/Xbox A-B swap setting.
        "south" => R::Action(Action::Accept),
        "east" => R::Action(Action::Back),
        "l1" => R::Action(Action::PrevTab),
        "r1" => R::Action(Action::NextTab),
        "start" => R::Action(Action::Options),
        "guide" => R::Guide,
        _ => return None,
    })
}

/// Merged state of every connected controller.
#[derive(Debug, Default)]
pub(crate) struct PadState {
    held: HeldDirections,
    repeat: KeyRepeat,
    guide: GuidePress,
    /// Action buttons held down, with the disarm count at the time they were pressed.
    armed: Vec<(&'static str, u64)>,
    /// Capture buttons pressed and not yet released.
    capture_held: Vec<&'static str>,
}

impl PadState {
    /// Action buttons fire when released, and only if nothing disarmed them in between: the
    /// press that resumes a game is over before the game sees anything, and a button pressed
    /// in a game can't act in Hearth when it is released there.
    ///
    /// `capture` lists the inputs that are Hearth's Capture button right now: releasing one
    /// sends [`Action::Capture`] (whatever else it does).
    pub(crate) fn handle(
        &mut self,
        event: PadEvent,
        now: Instant,
        disarms: u64,
        capture: &[&'static str],
        out: &mut Vec<Action>,
    ) {
        match event {
            PadEvent::Pressed(name) => {
                if capture.contains(&name) && !self.capture_held.contains(&name) {
                    self.capture_held.push(name);
                }
                match button_role(name) {
                    Some(ButtonRole::Direction(dir)) => {
                        self.held.press(DirectionSource::DPadButton, dir);
                        out.extend(self.repeat.set(self.held.current(), now));
                    }
                    Some(ButtonRole::Action(_)) => {
                        self.armed.retain(|(b, _)| *b != name);
                        self.armed.push((name, disarms));
                    }
                    Some(ButtonRole::Guide) => self.guide.press(now),
                    None => {}
                }
            }
            PadEvent::Released(name) => {
                if let Some(i) = self.capture_held.iter().position(|n| *n == name) {
                    self.capture_held.remove(i);
                    out.push(Action::Capture);
                }
                match button_role(name) {
                    Some(ButtonRole::Direction(dir)) => {
                        self.held.release(DirectionSource::DPadButton, dir);
                        out.extend(self.repeat.set(self.held.current(), now));
                    }
                    Some(ButtonRole::Action(action)) => {
                        let pressed_at = self.armed.iter().position(|(b, _)| *b == name);
                        if let Some(i) = pressed_at
                            && self.armed.swap_remove(i).1 == disarms
                        {
                            out.push(action);
                        }
                    }
                    Some(ButtonRole::Guide) => out.extend(self.guide.release(now)),
                    None => {}
                }
            }
            PadEvent::Stick(x, y) => {
                let dir = stick_direction(x, y, STICK_DEADZONE);
                self.held.set_axis(DirectionSource::Stick, dir);
                out.extend(self.repeat.set(self.held.current(), now));
            }
            PadEvent::Disconnected => *self = Self::default(),
        }
    }

    /// Fires due repeats and the Guide long press.
    pub(crate) fn tick(&mut self, now: Instant, out: &mut Vec<Action>) {
        out.extend(self.repeat.tick(now));
        out.extend(self.guide.tick(now));
    }

    /// When [`PadState::tick`] has something to do next.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        match (self.repeat.deadline(), self.guide.deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn feed(state: &mut PadState, event: PadEvent, now: Instant) -> Vec<Action> {
        feed_with(state, event, now, 0, &[])
    }

    fn feed_with(
        state: &mut PadState,
        event: PadEvent,
        now: Instant,
        disarms: u64,
        capture: &[&'static str],
    ) -> Vec<Action> {
        let mut out = Vec::new();
        state.handle(event, now, disarms, capture, &mut out);
        state.tick(now, &mut out);
        out
    }

    #[test]
    fn face_buttons_fire_on_release() {
        let mut state = PadState::default();
        let now = Instant::now();
        assert!(feed(&mut state, PadEvent::Pressed("south"), now).is_empty());
        assert_eq!(
            feed(&mut state, PadEvent::Released("south"), now),
            [Action::Accept]
        );
        assert!(feed(&mut state, PadEvent::Pressed("r1"), now).is_empty());
        assert_eq!(
            feed(&mut state, PadEvent::Released("r1"), now),
            [Action::NextTab]
        );
        // A release without a press seen (held since before Hearth started) does nothing.
        assert!(feed(&mut state, PadEvent::Released("east"), now).is_empty());
    }

    #[test]
    fn disarmed_press_does_not_fire() {
        let mut state = PadState::default();
        let now = Instant::now();
        assert!(feed_with(&mut state, PadEvent::Pressed("south"), now, 3, &[]).is_empty());
        // Hearth took the controller back from the game while A was held.
        assert!(feed_with(&mut state, PadEvent::Released("south"), now, 4, &[]).is_empty());
        assert!(feed_with(&mut state, PadEvent::Pressed("south"), now, 4, &[]).is_empty());
        assert_eq!(
            feed_with(&mut state, PadEvent::Released("south"), now, 4, &[]),
            [Action::Accept]
        );
    }

    #[test]
    fn capture_fires_on_release_of_a_capture_input() {
        let mut state = PadState::default();
        let now = Instant::now();
        let capture = ["select"];
        assert!(feed_with(&mut state, PadEvent::Pressed("select"), now, 0, &capture).is_empty());
        assert_eq!(
            feed_with(&mut state, PadEvent::Released("select"), now, 0, &capture),
            [Action::Capture]
        );
        // Not a capture input: nothing.
        assert!(feed_with(&mut state, PadEvent::Pressed("select"), now, 0, &[]).is_empty());
        assert!(feed_with(&mut state, PadEvent::Released("select"), now, 0, &[]).is_empty());
    }

    #[test]
    fn stick_and_dpad_share_one_repeat() {
        let mut state = PadState::default();
        let t0 = Instant::now();
        assert_eq!(
            feed(&mut state, PadEvent::Stick(0.9, 0.0), t0),
            [Action::Right]
        );
        // Small wobble in the same direction doesn't retrigger.
        assert!(feed(&mut state, PadEvent::Stick(0.8, 0.1), t0).is_empty());
        let t1 = t0 + Duration::from_millis(100);
        assert_eq!(
            feed(&mut state, PadEvent::Pressed("dpad_down"), t1),
            [Action::Down]
        );
        // Releasing the D-pad falls back to the still-held stick.
        assert_eq!(
            feed(&mut state, PadEvent::Released("dpad_down"), t1),
            [Action::Right]
        );
        assert!(feed(&mut state, PadEvent::Stick(0.1, 0.0), t1).is_empty());
        assert_eq!(state.deadline(), None);
    }

    #[test]
    fn guide_short_press() {
        let mut state = PadState::default();
        let t0 = Instant::now();
        assert!(feed(&mut state, PadEvent::Pressed("guide"), t0).is_empty());
        assert_eq!(
            feed(
                &mut state,
                PadEvent::Released("guide"),
                t0 + Duration::from_millis(200)
            ),
            [Action::HomeShort]
        );
    }
}
