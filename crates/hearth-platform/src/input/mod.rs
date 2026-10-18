//! Controller input, reduced to a small set of UI actions.
//!
//! Every backend feeds [`Action`]s into one channel; the shell turns them into key events.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

pub mod dev_socket;
pub mod gilrs;
pub mod inputplumber;
mod pad_state;
pub mod repeat;

/// Everything the UI can be asked to do by a controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Move focus up.
    Up,
    /// Move focus down.
    Down,
    /// Move focus left.
    Left,
    /// Move focus right.
    Right,
    /// Confirm / activate.
    Accept,
    /// Cancel / go back.
    Back,
    /// Previous tab (L1).
    PrevTab,
    /// Next tab (R1).
    NextTab,
    /// Context menu for the focused item.
    Options,
    /// Guide released before the long-press threshold.
    HomeShort,
    /// Guide held up to the long-press threshold.
    HomeLong,
    /// A button bound to Hearth's Capture was released (see
    /// [`PadActivity::set_capture_inputs`]). Never injected into the UI.
    Capture,
}

/// A navigation direction, from the D-pad or the left stick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Up.
    Up,
    /// Down.
    Down,
    /// Left.
    Left,
    /// Right.
    Right,
}

impl From<Direction> for Action {
    fn from(dir: Direction) -> Self {
        match dir {
            Direction::Up => Action::Up,
            Direction::Down => Action::Down,
            Direction::Left => Action::Left,
            Direction::Right => Action::Right,
        }
    }
}

/// Live controller state shared between the input thread (writer) and the UI (reader): the
/// connected controllers, and a counter that cancels pending release-fired actions.
#[derive(Debug, Clone, Default)]
pub struct PadActivity(Arc<ActivityState>);

#[derive(Debug, Default)]
struct ActivityState {
    disarms: AtomicU64,
    pads: Mutex<Vec<PadInfo>>,
    pressed: Mutex<Vec<&'static str>>,
    capture: Mutex<Vec<&'static str>>,
    analog: Mutex<Analog>,
}

/// What the input backend knows about a connected controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PadInfo {
    /// Stays the same while the controller reconnects (InputPlumber's persistent id, the
    /// Bluetooth address of a DualSense; gilrs: its SDL GUID and kernel name).
    pub id: String,
    /// Name from the SDL mapping database ("PS5 Controller"), or the kernel name.
    pub name: String,
    /// Kernel (evdev) name.
    pub os_name: String,
    /// USB vendor id.
    pub vendor_id: Option<u16>,
    /// USB product id.
    pub product_id: Option<u16>,
    /// How it is connected.
    pub connection: Connection,
    /// Battery, as the kernel reports it.
    pub power: Power,
}

/// How a controller is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Connection {
    /// USB cable.
    Usb,
    /// Bluetooth.
    Bluetooth,
    /// Unknown or something else.
    #[default]
    Other,
}

impl Connection {
    /// From a Linux input bus type (`BUS_USB`, `BUS_BLUETOOTH`).
    pub fn from_bus(bus: u16) -> Self {
        match bus {
            0x03 => Self::Usb,
            0x05 => Self::Bluetooth,
            _ => Self::Other,
        }
    }
}

/// A controller's battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Power {
    /// Not reported.
    #[default]
    Unknown,
    /// No battery (wired-only pad).
    Wired,
    /// On battery, percent left.
    Discharging(u8),
    /// Charging, percent.
    Charging(u8),
    /// Full and plugged in.
    Charged,
}

/// Raw analog values of the first controller: sticks -1..1 (up is positive y, as gilrs reports
/// it) and triggers 0..1, with no deadzone, so drift shows.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Analog {
    /// Left stick x, y.
    pub left: (f32, f32),
    /// Right stick x, y.
    pub right: (f32, f32),
    /// Left trigger.
    pub l2: f32,
    /// Right trigger.
    pub r2: f32,
}

impl PadActivity {
    /// A new handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Buttons held right now won't fire their action when released. Call whenever Hearth
    /// takes the controller back from a game, so a game's button press can't act in Hearth.
    pub fn disarm(&self) {
        self.0.disarms.fetch_add(1, Ordering::AcqRel);
    }

    /// Connected controllers, first connected first.
    pub fn pads(&self) -> Vec<PadInfo> {
        self.0
            .pads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the list of connected controllers.
    pub fn set_pads(&self, pads: Vec<PadInfo>) {
        *self.0.pads.lock().unwrap_or_else(PoisonError::into_inner) = pads;
    }

    /// Inputs held right now on any controller, by Hearth pad position name ("south",
    /// "left_stick", "guide"), for the button test.
    pub fn pressed(&self) -> Vec<&'static str> {
        self.0
            .pressed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the held inputs.
    pub fn set_pressed(&self, pressed: Vec<&'static str>) {
        *self
            .0
            .pressed
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = pressed;
    }

    /// The inputs (Hearth pad position names) that act as Hearth's Capture button right now:
    /// releasing one sends [`Action::Capture`]. They depend on what is in front (a game's
    /// mapping, or the controller type's default in menus), so the UI keeps them current.
    pub fn set_capture_inputs(&self, inputs: Vec<&'static str>) {
        *self
            .0
            .capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = inputs;
    }

    /// See [`PadActivity::set_capture_inputs`].
    pub fn capture_inputs(&self) -> Vec<&'static str> {
        self.0
            .capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Raw analog values of the first controller, for the button test.
    pub fn analog(&self) -> Analog {
        *self.0.analog.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the analog values.
    pub fn set_analog(&self, analog: Analog) {
        *self.0.analog.lock().unwrap_or_else(PoisonError::into_inner) = analog;
    }

    /// Increases with every [`PadActivity::disarm`].
    pub fn disarm_count(&self) -> u64 {
        self.0.disarms.load(Ordering::Acquire)
    }
}

/// Who should receive controller input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// The shell receives everything.
    #[default]
    Shell,
    /// A game receives everything except the Guide button.
    Game,
    /// The button test, with no game running: the controller's input goes to the virtual pad
    /// (where gilrs reads raw stick and trigger values), Guide and the extras to the shell.
    Test,
}

/// What the session can ask of the backend that owns the controllers.
#[derive(Debug)]
pub enum InputCommand {
    /// Switch who receives input.
    Mode(InputMode),
    /// The remapping profile to use while a game has the controller (`None`: no remapping).
    /// Takes effect the next time the mode becomes [`InputMode::Game`].
    GameProfile(Option<PathBuf>),
    /// Hand the controllers back to the system (Hearth is exiting); answers when done.
    Release(mpsc::SyncSender<()>),
    /// A game launched while the service was gone reads the physical controller: while `true`,
    /// a returning service must not take the controller (the game would lose it).
    KeepDirect(bool),
}

/// The session's handle on the input backend. Backends that can't hide input from games (gilrs)
/// hand out a detached handle, on which every call does nothing.
#[derive(Debug, Clone, Default)]
pub struct InputControl {
    tx: Option<Sender<InputCommand>>,
    state: Arc<ControlState>,
}

/// What happened to the service behind the input backend (InputPlumber).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputStatus {
    /// It went away: games lost their virtual pad; Hearth reads controllers with gilrs.
    Lost,
    /// Still gone after a few seconds (not just a restart).
    StillLost,
    /// It is back and has the controllers again.
    Back,
}

/// Called on the backend's thread with each [`InputStatus`].
type StatusListener = Box<dyn Fn(InputStatus) + Send>;

/// What the backend tells the session through its handle.
#[derive(Default)]
struct ControlState {
    /// The service is gone: no live remapping, input isn't hidden from games.
    lost: std::sync::atomic::AtomicBool,
    listener: Mutex<Option<StatusListener>>,
}

impl std::fmt::Debug for ControlState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlState")
            .field("lost", &self.lost)
            .finish_non_exhaustive()
    }
}

impl InputControl {
    /// A handle that does nothing.
    pub fn detached() -> Self {
        Self::default()
    }

    /// A handle sending to a backend's command channel.
    pub fn new(tx: Sender<InputCommand>) -> Self {
        Self {
            tx: Some(tx),
            state: Arc::default(),
        }
    }

    /// Whether a backend listens: games get remapped live and Hearth's input is hidden from them.
    /// False while the service behind it is gone ([`InputStatus::Lost`]).
    pub fn is_live(&self) -> bool {
        self.tx.is_some() && !self.state.lost.load(Ordering::Acquire)
    }

    /// Session side: `listener` hears every [`InputStatus`], on the backend's thread.
    pub fn on_status(&self, listener: impl Fn(InputStatus) + Send + 'static) {
        *self
            .state
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Box::new(listener));
    }

    /// Backend side: reports what happened to its service.
    pub fn report(&self, status: InputStatus) {
        match status {
            InputStatus::Lost => self.state.lost.store(true, Ordering::Release),
            InputStatus::Back => self.state.lost.store(false, Ordering::Release),
            InputStatus::StillLost => {}
        }
        if let Some(listener) = &*self
            .state
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            listener(status);
        }
    }

    /// See [`InputCommand::Mode`]. Always sent, even when unchanged: the backend re-applies it
    /// (InputPlumber switches a controller to Hearth by itself when Guide is pressed).
    pub fn set_mode(&self, mode: InputMode) {
        self.send(InputCommand::Mode(mode));
    }

    /// See [`InputCommand::GameProfile`].
    pub fn set_game_profile(&self, profile: Option<PathBuf>) {
        self.send(InputCommand::GameProfile(profile));
    }

    /// See [`InputCommand::KeepDirect`].
    pub fn keep_direct(&self, keep: bool) {
        self.send(InputCommand::KeepDirect(keep));
    }

    /// Hands the controllers back, waiting at most `timeout`. Returns whether it finished.
    pub fn release(&self, timeout: Duration) -> bool {
        let Some(tx) = &self.tx else {
            return true;
        };
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        if tx.send(InputCommand::Release(done_tx)).is_err() {
            return false;
        }
        done_rx.recv_timeout(timeout).is_ok()
    }

    fn send(&self, command: InputCommand) {
        if let Some(tx) = &self.tx
            && tx.send(command).is_err()
        {
            tracing::warn!("input backend gone; command dropped");
        }
    }
}

/// Errors from an input backend.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// The backend could not be initialised.
    #[error("input backend failed to start: {0}")]
    Start(String),
    /// [`InputBackend::start`] was called twice.
    #[error("input backend already started")]
    AlreadyStarted,
}

/// A source of controller [`Action`]s.
pub trait InputBackend: Send {
    /// Starts delivering actions to `tx` from a background thread.
    fn start(&mut self, tx: Sender<Action>) -> Result<(), InputError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disarm_counts_up() {
        let activity = PadActivity::new();
        let before = activity.disarm_count();
        activity.clone().disarm();
        assert_eq!(activity.disarm_count(), before + 1);
    }
}
