//! Development input backend reading controllers directly through gilrs.
//!
//! Devices are opened without an exclusive grab and no virtual device is created, so a
//! running game still sees the controller. That is fine on the laptop; the appliance uses
//! InputPlumber instead (TODO(M3)).

use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use gilrs::{Axis, Button, EventType, Gilrs};

use super::repeat::stick_direction;
use super::repeat::{DirectionSource, GuidePress, HeldDirections, KeyRepeat, STICK_DEADZONE};
use super::{Action, Direction, InputBackend, InputError, InputMode, PadActivity, is_at_rest};

/// Longest time the input thread sleeps without an event or a pending deadline.
const IDLE_WAIT: Duration = Duration::from_millis(250);

/// Controller input via gilrs, on its own thread.
#[derive(Debug, Default)]
pub struct GilrsBackend {
    started: bool,
    activity: PadActivity,
}

impl GilrsBackend {
    /// Creates a backend; nothing is opened until [`InputBackend::start`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Live "is anything held" state of every controller, updated after each event.
    pub fn activity(&self) -> PadActivity {
        self.activity.clone()
    }
}

impl InputBackend for GilrsBackend {
    fn start(&mut self, tx: Sender<Action>) -> Result<(), InputError> {
        if self.started {
            return Err(InputError::AlreadyStarted);
        }
        // Gilrs isn't Send, so it is created on the thread that uses it.
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let activity = self.activity.clone();
        thread::Builder::new()
            .name("hearth-gilrs".into())
            .spawn(move || {
                let gilrs = match Gilrs::new() {
                    Ok(gilrs) => gilrs,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err.to_string()));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                run(gilrs, &tx, &activity);
            })
            .map_err(|err| InputError::Start(err.to_string()))?;
        ready_rx
            .recv()
            .map_err(|err| InputError::Start(err.to_string()))?
            .map_err(InputError::Start)?;
        self.started = true;
        Ok(())
    }

    fn set_mode(&mut self, _mode: InputMode) {
        // gilrs can't hide input from a game.
    }
}

fn run(mut gilrs: Gilrs, tx: &Sender<Action>, activity: &PadActivity) {
    for (_, pad) in gilrs.gamepads() {
        tracing::info!(name = pad.name(), "controller connected");
    }
    let mut state = PadState::default();
    let mut out = Vec::new();
    loop {
        let now = Instant::now();
        let wait = state.deadline().map_or(IDLE_WAIT, |d| {
            d.saturating_duration_since(now).min(IDLE_WAIT)
        });
        let event = gilrs.next_event_blocking(Some(wait));
        let now = Instant::now();
        if let Some(event) = event {
            let input = match event.event {
                EventType::ButtonPressed(button, _) => Some(PadEvent::Pressed(button)),
                EventType::ButtonReleased(button, _) => Some(PadEvent::Released(button)),
                EventType::AxisChanged(Axis::LeftStickX | Axis::LeftStickY, _, _) => {
                    let pad = gilrs.gamepad(event.id);
                    Some(PadEvent::Stick(
                        pad.value(Axis::LeftStickX),
                        pad.value(Axis::LeftStickY),
                    ))
                }
                EventType::Connected => {
                    tracing::info!(
                        name = gilrs.gamepad(event.id).name(),
                        "controller connected"
                    );
                    None
                }
                EventType::Disconnected => Some(PadEvent::Disconnected),
                _ => None,
            };
            if let Some(input) = input {
                state.handle(input, now, &mut out);
            }
            activity.set_held(!all_at_rest(&gilrs));
        }
        state.tick(now, &mut out);
        for action in out.drain(..) {
            if tx.send(action).is_err() {
                tracing::debug!("action receiver dropped; stopping gilrs thread");
                return;
            }
        }
    }
}

/// Whether no connected controller has anything held. Buttons are read from the raw state, so
/// unmapped ones (touchpad click, mic button) count too; analog triggers are buttons with a value.
fn all_at_rest(gilrs: &Gilrs) -> bool {
    gilrs.gamepads().all(|(_, pad)| {
        let state = pad.state();
        let buttons = state.buttons().map(|(_, data)| data.is_pressed());
        let triggers = state.buttons().map(|(_, data)| data.value());
        let sticks = [
            Axis::LeftStickX,
            Axis::LeftStickY,
            Axis::RightStickX,
            Axis::RightStickY,
        ]
        .map(|axis| pad.value(axis));
        is_at_rest(buttons, triggers.chain(sticks))
    })
}

/// The subset of gilrs events the state machine cares about.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PadEvent {
    Pressed(Button),
    Released(Button),
    Stick(f32, f32),
    Disconnected,
}

/// What a button means to Hearth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonRole {
    Direction(Direction),
    Action(Action),
    Guide,
}

fn button_role(button: Button) -> Option<ButtonRole> {
    use ButtonRole as R;
    Some(match button {
        Button::DPadUp => R::Direction(Direction::Up),
        Button::DPadDown => R::Direction(Direction::Down),
        Button::DPadLeft => R::Direction(Direction::Left),
        Button::DPadRight => R::Direction(Direction::Right),
        // TODO(M1): Nintendo/Xbox A-B swap setting.
        Button::South => R::Action(Action::Accept),
        Button::East => R::Action(Action::Back),
        Button::LeftTrigger => R::Action(Action::PrevTab),
        Button::RightTrigger => R::Action(Action::NextTab),
        Button::Start => R::Action(Action::Options),
        Button::Mode => R::Guide,
        _ => return None,
    })
}

/// Merged state of every connected controller.
#[derive(Debug, Default)]
struct PadState {
    held: HeldDirections,
    repeat: KeyRepeat,
    guide: GuidePress,
}

impl PadState {
    fn handle(&mut self, event: PadEvent, now: Instant, out: &mut Vec<Action>) {
        match event {
            PadEvent::Pressed(button) => match button_role(button) {
                Some(ButtonRole::Direction(dir)) => {
                    self.held.press(DirectionSource::DPadButton, dir);
                    out.extend(self.repeat.set(self.held.current(), now));
                }
                Some(ButtonRole::Action(action)) => out.push(action),
                Some(ButtonRole::Guide) => self.guide.press(now),
                None => {}
            },
            PadEvent::Released(button) => match button_role(button) {
                Some(ButtonRole::Direction(dir)) => {
                    self.held.release(DirectionSource::DPadButton, dir);
                    out.extend(self.repeat.set(self.held.current(), now));
                }
                Some(ButtonRole::Guide) => out.extend(self.guide.release(now)),
                _ => {}
            },
            PadEvent::Stick(x, y) => {
                let dir = stick_direction(x, y, STICK_DEADZONE);
                self.held.set_axis(DirectionSource::Stick, dir);
                out.extend(self.repeat.set(self.held.current(), now));
            }
            PadEvent::Disconnected => *self = Self::default(),
        }
    }

    fn tick(&mut self, now: Instant, out: &mut Vec<Action>) {
        out.extend(self.repeat.tick(now));
        out.extend(self.guide.tick(now));
    }

    fn deadline(&self) -> Option<Instant> {
        match (self.repeat.deadline(), self.guide.deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(state: &mut PadState, event: PadEvent, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        state.handle(event, now, &mut out);
        state.tick(now, &mut out);
        out
    }

    #[test]
    fn face_buttons_map_to_actions() {
        let mut state = PadState::default();
        let now = Instant::now();
        assert_eq!(
            feed(&mut state, PadEvent::Pressed(Button::South), now),
            [Action::Accept]
        );
        assert_eq!(
            feed(&mut state, PadEvent::Pressed(Button::RightTrigger), now),
            [Action::NextTab]
        );
        assert!(feed(&mut state, PadEvent::Released(Button::South), now).is_empty());
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
            feed(&mut state, PadEvent::Pressed(Button::DPadDown), t1),
            [Action::Down]
        );
        // Releasing the D-pad falls back to the still-held stick.
        assert_eq!(
            feed(&mut state, PadEvent::Released(Button::DPadDown), t1),
            [Action::Right]
        );
        assert!(feed(&mut state, PadEvent::Stick(0.1, 0.0), t1).is_empty());
        assert_eq!(state.deadline(), None);
    }

    #[test]
    fn guide_short_press() {
        let mut state = PadState::default();
        let t0 = Instant::now();
        assert!(feed(&mut state, PadEvent::Pressed(Button::Mode), t0).is_empty());
        assert_eq!(
            feed(
                &mut state,
                PadEvent::Released(Button::Mode),
                t0 + Duration::from_millis(200)
            ),
            [Action::HomeShort]
        );
    }
}
