//! Development input backend reading controllers directly through gilrs.
//!
//! Devices are opened without an exclusive grab and no virtual device is created, so a
//! running game still sees the controller. That is fine on the laptop; the appliance uses
//! InputPlumber instead (TODO(M3)).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use gilrs::ev::filter::{Filter, axis_dpad_to_button};
use gilrs::{Axis, Button, Event, EventType, Gilrs, GilrsBuilder, PowerInfo};

use super::pad_state::{PadEvent, PadState};
use super::repeat::STICK_DEADZONE;
use super::{Action, Analog, Connection, InputBackend, InputError, PadActivity, PadInfo, Power};

/// Longest time the input thread sleeps without an event or a pending deadline.
const IDLE_WAIT: Duration = Duration::from_millis(250);
/// How often the controller list (battery levels) is refreshed.
const PADS_REFRESH: Duration = Duration::from_secs(10);

/// Controller input via gilrs, on its own thread.
#[derive(Debug)]
pub struct GilrsBackend {
    started: bool,
    activity: PadActivity,
    /// Sends actions (an observer only keeps `activity` current).
    emits: bool,
    /// Set to end the thread (InputPlumber is back).
    stop: Arc<AtomicBool>,
}

/// gilrs has no SDL mapping for InputPlumber's virtual Elite pad and names its buttons by
/// evdev code, but xpad (and InputPlumber, like it) reports Xbox X as `BTN_X`, which is
/// `BTN_NORTH`, and Y as `BTN_Y`, which is `BTN_WEST`. SDL and RetroArch number buttons and
/// get it right; for gilrs the two are swapped back here.
fn swap_north_west(name: &'static str) -> &'static str {
    match name {
        "north" => "west",
        "west" => "north",
        other => other,
    }
}

impl Default for GilrsBackend {
    fn default() -> Self {
        Self {
            started: false,
            activity: PadActivity::new(),
            emits: true,
            stop: Arc::default(),
        }
    }
}

impl GilrsBackend {
    /// Creates a backend; nothing is opened until [`InputBackend::start`].
    pub fn new() -> Self {
        Self::default()
    }

    /// A backend writing to an existing [`PadActivity`] (InputPlumber falling back to gilrs
    /// keeps the session's).
    pub fn with_activity(activity: PadActivity) -> Self {
        Self {
            activity,
            ..Self::default()
        }
    }

    /// Setting it ends the input thread (within [`IDLE_WAIT`]).
    pub fn stopper(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// A backend that only keeps its [`PadActivity`] current (held inputs, raw stick and
    /// trigger values) and sends no actions: the button test reads InputPlumber's virtual pad
    /// through it (see [`swap_north_west`]).
    pub fn observer() -> Self {
        Self {
            emits: false,
            ..Self::default()
        }
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
        let emits = self.emits;
        let stop = self.stop.clone();
        thread::Builder::new()
            .name("hearth-gilrs".into())
            .spawn(move || {
                let gilrs = match open() {
                    Ok(gilrs) => gilrs,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                run(gilrs, &tx, &activity, emits, &stop);
            })
            .map_err(|err| InputError::Start(err.to_string()))?;
        ready_rx
            .recv()
            .map_err(|err| InputError::Start(err.to_string()))?
            .map_err(InputError::Start)?;
        self.started = true;
        Ok(())
    }
}

/// gilrs without its deadzone and jitter filters, so the button test sees raw values (drift);
/// Hearth's navigation has its own thresholds. Its state is updated by [`next_event`].
fn open() -> Result<Gilrs, String> {
    GilrsBuilder::new()
        .with_default_filters(false)
        .set_update_state(false)
        .build()
        .map_err(|err| err.to_string())
}

/// The next event, with D-pad hats turned into D-pad buttons (gilrs's own filter, the one
/// default filter kept). gilrs's state gets both the raw event (raw axis values) and the
/// converted one (D-pad buttons).
fn next_event(gilrs: &mut Gilrs, wait: Duration) -> Option<Event> {
    let raw = gilrs.next_event_blocking(Some(wait))?;
    let event = Some(raw).filter_ev(&axis_dpad_to_button, gilrs)?;
    gilrs.update(&raw);
    if event != raw && !event.is_dropped() {
        gilrs.update(&event);
    }
    (!event.is_dropped()).then_some(event)
}

/// Raw analog values of the first controller.
fn analog(gilrs: &Gilrs) -> Analog {
    let Some((_, pad)) = gilrs.gamepads().next() else {
        return Analog::default();
    };
    Analog {
        left: (pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY)),
        right: (pad.value(Axis::RightStickX), pad.value(Axis::RightStickY)),
        l2: trigger(pad, Button::LeftTrigger2, Axis::LeftZ),
        r2: trigger(pad, Button::RightTrigger2, Axis::RightZ),
    }
}

/// A trigger's pull, 0..1. Pads with an SDL mapping report it as an analog button; without
/// one (InputPlumber's virtual Elite pad) gilrs reports the raw `ABS_Z`/`ABS_RZ` as an axis
/// going from -1 (released) to 1.
fn trigger(pad: gilrs::Gamepad<'_>, button: Button, axis: Axis) -> f32 {
    match (pad.button_data(button), pad.axis_data(axis)) {
        (Some(data), _) => data.value(),
        (None, Some(data)) => f32::midpoint(data.value(), 1.0),
        (None, None) => 0.0,
    }
}

fn run(
    mut gilrs: Gilrs,
    tx: &Sender<Action>,
    activity: &PadActivity,
    emits: bool,
    stop: &AtomicBool,
) {
    for (_, pad) in gilrs.gamepads() {
        tracing::info!(name = pad.name(), "controller connected");
    }
    activity.set_pads(pad_infos(&gilrs));
    let mut state = PadState::default();
    let mut out = Vec::new();
    // Extra buttons held (gilrs has no `Button` for them), by Hearth name.
    let mut extras: Vec<&'static str> = Vec::new();
    let mut pads_refreshed = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        let wait = state.deadline().map_or(IDLE_WAIT, |d| {
            d.saturating_duration_since(now).min(IDLE_WAIT)
        });
        let event = next_event(&mut gilrs, wait);
        let now = Instant::now();
        if now >= pads_refreshed + PADS_REFRESH {
            activity.set_pads(pad_infos(&gilrs));
            pads_refreshed = now;
        }
        if let Some(event) = event {
            let input = match event.event {
                EventType::ButtonPressed(button, code) => {
                    let name = input_name(button, code.into_u32());
                    if button == Button::Unknown
                        && let Some(name) = name
                        && !extras.contains(&name)
                    {
                        extras.push(name);
                    }
                    name.map(PadEvent::Pressed)
                }
                EventType::ButtonReleased(button, code) => {
                    let name = input_name(button, code.into_u32());
                    if button == Button::Unknown {
                        extras.retain(|n| Some(*n) != name);
                    }
                    name.map(PadEvent::Released)
                }
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
                let capture = activity.capture_inputs();
                state.handle(input, now, activity.disarm_count(), &capture, &mut out);
            }
            let mut pressed = pressed_inputs(&gilrs);
            for name in &extras {
                if !pressed.contains(name) {
                    pressed.push(name);
                }
            }
            if !emits {
                // The observer only ever reads InputPlumber's virtual pad.
                pressed = pressed.into_iter().map(swap_north_west).collect();
            }
            activity.set_pressed(pressed);
            activity.set_analog(analog(&gilrs));
        }
        state.tick(now, &mut out);
        if !emits {
            out.clear();
        }
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

/// Hearth pad position name of a button event.
fn input_name(button: Button, code: u32) -> Option<&'static str> {
    if button == Button::Unknown {
        extra_name(code)
    } else {
        position_name(button)
    }
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
        // xpad: Xbox Elite Series 2 paddles, numbered as SDL's mapping of the device does
        // (BTN_TRIGGER_HAPPY5–8 are paddles 1, 3, 2, 4; checked on InputPlumber's virtual
        // Elite 2 pad, which uses the same codes).
        0x2c4 => "paddle1",
        0x2c5 => "paddle3",
        0x2c6 => "paddle2",
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
        for (name, button, axis) in [
            ("l2", Button::LeftTrigger2, Axis::LeftZ),
            ("r2", Button::RightTrigger2, Axis::RightZ),
        ] {
            if trigger(pad, button, axis) > STICK_DEADZONE && !pressed.contains(&name) {
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
            connection: {
                // gilrs's id is SDL's GUID: the bus type comes first, little-endian.
                let uuid = pad.uuid();
                Connection::from_bus(u16::from_le_bytes([uuid[0], uuid[1]]))
            },
            power: match pad.power_info() {
                PowerInfo::Unknown => Power::Unknown,
                PowerInfo::Wired => Power::Wired,
                PowerInfo::Discharging(p) => Power::Discharging(p),
                PowerInfo::Charging(p) => Power::Charging(p),
                PowerInfo::Charged => Power::Charged,
            },
        })
        .collect()
}

/// The controllers connected right now, without starting the input thread (for command-line
/// tools).
pub fn connected_pads() -> Result<Vec<PadInfo>, InputError> {
    let gilrs = Gilrs::new().map_err(|err| InputError::Start(err.to_string()))?;
    Ok(pad_infos(&gilrs))
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
        assert_eq!(extra_name(key(0x2c6)), Some("paddle2"));
        assert_eq!(extra_name(key(0x2c7)), Some("paddle4"));
        assert_eq!(extra_name(key(0x130)), None);
        // An absolute axis with the same number isn't a key.
        assert_eq!(extra_name((3 << 16) | 167), None);
    }
}
