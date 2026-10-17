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
use super::{Action, Direction, InputBackend, InputError, InputMode, PadActivity, PadInfo};

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
    activity.set_pads(pad_infos(&gilrs));
    let mut state = PadState::default();
    let mut out = Vec::new();
    // Extra buttons held (gilrs has no `Button` for them), by Hearth name.
    let mut extras: Vec<&'static str> = Vec::new();
    loop {
        let now = Instant::now();
        let wait = state.deadline().map_or(IDLE_WAIT, |d| {
            d.saturating_duration_since(now).min(IDLE_WAIT)
        });
        let event = gilrs.next_event_blocking(Some(wait));
        let now = Instant::now();
        if let Some(event) = event {
            let input = match event.event {
                EventType::ButtonPressed(Button::Unknown, code) => {
                    if let Some(name) = extra_name(code.into_u32())
                        && !extras.contains(&name)
                    {
                        extras.push(name);
                    }
                    None
                }
                EventType::ButtonReleased(Button::Unknown, code) => {
                    extras.retain(|n| Some(*n) != extra_name(code.into_u32()));
                    None
                }
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
                    let pad = gilrs.gamepad(event.id);
                    tracing::info!(name = pad.name(), "controller connected");
                    activity.set_pads(pad_infos(&gilrs));
                    None
                }
                EventType::Disconnected => {
                    activity.set_pads(pad_infos(&gilrs));
                    Some(PadEvent::Disconnected)
                }
                _ => None,
            };
            if let Some(input) = input {
                state.handle(input, now, activity.disarm_count(), &mut out);
            }
            let mut pressed = pressed_inputs(&gilrs);
            for name in &extras {
                if !pressed.contains(name) {
                    pressed.push(name);
                }
            }
            activity.set_pressed(pressed);
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

/// Hearth pad position name of a gilrs button (`hearth_core::controls::PadInput` names).
fn position_name(button: Button) -> Option<&'static str> {
    Some(match button {
        Button::South => "south",
        Button::East => "east",
        Button::West => "west",
        Button::North => "north",
        Button::LeftTrigger => "l1",
        Button::RightTrigger => "r1",
        Button::LeftTrigger2 => "l2",
        Button::RightTrigger2 => "r2",
        Button::LeftThumb => "l3",
        Button::RightThumb => "r3",
        Button::Start => "start",
        Button::Select => "select",
        Button::DPadUp => "dpad_up",
        Button::DPadDown => "dpad_down",
        Button::DPadLeft => "dpad_left",
        Button::DPadRight => "dpad_right",
        Button::Mode => "guide",
        _ => return None,
    })
}

/// Hearth name of a button gilrs reports as `Button::Unknown`, from its raw evdev code
/// (`EV_KEY << 16 | key`): the kernel drivers' codes for SDL's misc1 and the back paddles.
/// Unverified on hardware (no such pad here); the DualSense has none of these.
fn extra_name(code: u32) -> Option<&'static str> {
    const EV_KEY: u32 = 1;
    if code >> 16 != EV_KEY {
        return None;
    }
    Some(match code & 0xffff {
        // xpad: Xbox Series Share.
        167 => "share",
        // hid-nintendo: Switch Pro Capture.
        0x135 => "share",
        // xpad: Xbox Elite Series 2 paddles (BTN_TRIGGER_HAPPY5–8).
        0x2c4 => "paddle1",
        0x2c5 => "paddle2",
        0x2c6 => "paddle3",
        0x2c7 => "paddle4",
        _ => return None,
    })
}

const ALL_BUTTONS: [Button; 17] = [
    Button::South,
    Button::East,
    Button::West,
    Button::North,
    Button::LeftTrigger,
    Button::RightTrigger,
    Button::LeftTrigger2,
    Button::RightTrigger2,
    Button::LeftThumb,
    Button::RightThumb,
    Button::Start,
    Button::Select,
    Button::DPadUp,
    Button::DPadDown,
    Button::DPadLeft,
    Button::DPadRight,
    Button::Mode,
];

/// Inputs held on any controller, by position name; a stick counts when pushed past half.
fn pressed_inputs(gilrs: &Gilrs) -> Vec<&'static str> {
    let mut pressed = Vec::new();
    for (_, pad) in gilrs.gamepads() {
        for button in ALL_BUTTONS {
            if pad.is_pressed(button)
                && let Some(name) = position_name(button)
                && !pressed.contains(&name)
            {
                pressed.push(name);
            }
        }
        for (name, x, y) in [
            ("left_stick", Axis::LeftStickX, Axis::LeftStickY),
            ("right_stick", Axis::RightStickX, Axis::RightStickY),
        ] {
            if pad.value(x).hypot(pad.value(y)) > STICK_DEADZONE && !pressed.contains(&name) {
                pressed.push(name);
            }
        }
    }
    pressed
}

/// The connected controllers, as [`PadInfo`]s.
fn pad_infos(gilrs: &Gilrs) -> Vec<PadInfo> {
    gilrs
        .gamepads()
        .map(|(_, pad)| PadInfo {
            name: pad.name().to_owned(),
            os_name: pad.os_name().to_owned(),
            vendor_id: pad.vendor_id(),
            product_id: pad.product_id(),
        })
        .collect()
}

/// The controllers connected right now, without starting the input thread (for command-line
/// tools).
pub fn connected_pads() -> Result<Vec<PadInfo>, InputError> {
    let gilrs = Gilrs::new().map_err(|err| InputError::Start(err.to_string()))?;
    Ok(pad_infos(&gilrs))
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
    /// Action buttons held down, with the disarm count at the time they were pressed.
    armed: Vec<(Button, u64)>,
}

impl PadState {
    /// Action buttons fire when released, and only if nothing disarmed them in between: the
    /// press that resumes a game is over before the game sees anything, and a button pressed
    /// in a game can't act in Hearth when it is released there.
    fn handle(&mut self, event: PadEvent, now: Instant, disarms: u64, out: &mut Vec<Action>) {
        match event {
            PadEvent::Pressed(button) => match button_role(button) {
                Some(ButtonRole::Direction(dir)) => {
                    self.held.press(DirectionSource::DPadButton, dir);
                    out.extend(self.repeat.set(self.held.current(), now));
                }
                Some(ButtonRole::Action(_)) => {
                    self.armed.retain(|(b, _)| *b != button);
                    self.armed.push((button, disarms));
                }
                Some(ButtonRole::Guide) => self.guide.press(now),
                None => {}
            },
            PadEvent::Released(button) => match button_role(button) {
                Some(ButtonRole::Direction(dir)) => {
                    self.held.release(DirectionSource::DPadButton, dir);
                    out.extend(self.repeat.set(self.held.current(), now));
                }
                Some(ButtonRole::Action(action)) => {
                    let pressed_at = self.armed.iter().position(|(b, _)| *b == button);
                    if let Some(i) = pressed_at
                        && self.armed.swap_remove(i).1 == disarms
                    {
                        out.push(action);
                    }
                }
                Some(ButtonRole::Guide) => out.extend(self.guide.release(now)),
                None => {}
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

    #[test]
    fn extra_buttons_by_raw_code() {
        let key = |code: u32| (1 << 16) | code;
        assert_eq!(extra_name(key(167)), Some("share"));
        assert_eq!(extra_name(key(0x135)), Some("share"));
        assert_eq!(extra_name(key(0x2c4)), Some("paddle1"));
        assert_eq!(extra_name(key(0x2c7)), Some("paddle4"));
        assert_eq!(extra_name(key(0x130)), None);
        // An absolute axis with the same number isn't a key.
        assert_eq!(extra_name((3 << 16) | 167), None);
    }

    fn feed(state: &mut PadState, event: PadEvent, now: Instant) -> Vec<Action> {
        feed_with(state, event, now, 0)
    }

    fn feed_with(state: &mut PadState, event: PadEvent, now: Instant, disarms: u64) -> Vec<Action> {
        let mut out = Vec::new();
        state.handle(event, now, disarms, &mut out);
        state.tick(now, &mut out);
        out
    }

    #[test]
    fn face_buttons_fire_on_release() {
        let mut state = PadState::default();
        let now = Instant::now();
        assert!(feed(&mut state, PadEvent::Pressed(Button::South), now).is_empty());
        assert_eq!(
            feed(&mut state, PadEvent::Released(Button::South), now),
            [Action::Accept]
        );
        assert!(feed(&mut state, PadEvent::Pressed(Button::RightTrigger), now).is_empty());
        assert_eq!(
            feed(&mut state, PadEvent::Released(Button::RightTrigger), now),
            [Action::NextTab]
        );
        // A release without a press seen (held since before Hearth started) does nothing.
        assert!(feed(&mut state, PadEvent::Released(Button::East), now).is_empty());
    }

    #[test]
    fn disarmed_press_does_not_fire() {
        let mut state = PadState::default();
        let now = Instant::now();
        assert!(feed_with(&mut state, PadEvent::Pressed(Button::South), now, 3).is_empty());
        // Hearth took the controller back from the game while A was held.
        assert!(feed_with(&mut state, PadEvent::Released(Button::South), now, 4).is_empty());
        assert!(feed_with(&mut state, PadEvent::Pressed(Button::South), now, 4).is_empty());
        assert_eq!(
            feed_with(&mut state, PadEvent::Released(Button::South), now, 4),
            [Action::Accept]
        );
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
