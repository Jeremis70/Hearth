//! Controller input, reduced to a small set of UI actions.
//!
//! Every backend feeds [`Action`]s into one channel; the shell turns them into key events.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub mod dev_socket;
pub mod gilrs;
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

/// How far a stick or trigger may sit from its resting value and still count as released.
pub const REST_TOLERANCE: f32 = 0.15;

/// Live controller state shared between the input thread (writer) and the UI (reader):
/// since when every controller has been at rest, and a counter that cancels pending
/// release-fired actions.
#[derive(Debug, Clone, Default)]
pub struct PadActivity(Arc<ActivityState>);

#[derive(Debug, Default)]
struct ActivityState {
    /// `None` while something is held.
    rest_since: Mutex<Option<Instant>>,
    disarms: AtomicU64,
    pads: Mutex<Vec<PadInfo>>,
}

/// What the input backend knows about a connected controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PadInfo {
    /// Name from the SDL mapping database ("PS5 Controller"), or the kernel name.
    pub name: String,
    /// Kernel (evdev) name.
    pub os_name: String,
    /// USB vendor id.
    pub vendor_id: Option<u16>,
    /// USB product id.
    pub product_id: Option<u16>,
}

impl PadActivity {
    /// A new handle; at rest until the input thread reports otherwise.
    pub fn new() -> Self {
        let activity = Self::default();
        activity.set_at_rest(true, Instant::now());
        activity
    }

    /// How long every controller has been at rest, or `None` while something is held.
    pub fn rest_duration(&self, now: Instant) -> Option<Duration> {
        let since = *self
            .0
            .rest_since
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        since.map(|t| now.saturating_duration_since(t))
    }

    /// Records whether every controller is at rest; keeps the earliest rest time.
    pub fn set_at_rest(&self, at_rest: bool, now: Instant) {
        let mut since = self
            .0
            .rest_since
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match (at_rest, *since) {
            (true, None) => *since = Some(now),
            (false, _) => *since = None,
            (true, Some(_)) => {}
        }
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

    /// Increases with every [`PadActivity::disarm`].
    pub fn disarm_count(&self) -> u64 {
        self.0.disarms.load(Ordering::Acquire)
    }
}

/// Whether nothing is held: no button pressed and every analog value within
/// [`REST_TOLERANCE`] of its resting value, given as `(value, resting value)` pairs.
pub fn is_at_rest(
    buttons: impl IntoIterator<Item = bool>,
    analog: impl IntoIterator<Item = (f32, f32)>,
) -> bool {
    !buttons.into_iter().any(|pressed| pressed)
        && analog
            .into_iter()
            .all(|(value, rest)| (value - rest).abs() < REST_TOLERANCE)
}

/// Who should receive controller input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// The shell receives everything.
    #[default]
    Shell,
    /// A game receives everything except the Guide button.
    Game,
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

    /// Switches who receives input. Backends that can't hide input from games ignore it.
    fn set_mode(&mut self, mode: InputMode);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_rest_compares_with_resting_values() {
        assert!(is_at_rest([false, false], [(0.0, 0.0), (0.1, 0.0)]));
        // A trigger resting at -1 (raw axis) is at rest there, not at 0.
        assert!(is_at_rest([], [(-1.0, -1.0), (-0.9, -1.0)]));
        assert!(!is_at_rest([], [(-0.5, -1.0)]));
        assert!(!is_at_rest([false, true], [(0.0, 0.0)]));
        assert!(!is_at_rest([], [(0.3, 0.0)]));
        assert!(is_at_rest([], []));
    }

    #[test]
    fn rest_duration_keeps_the_earliest_time() {
        let t0 = Instant::now();
        let activity = PadActivity::new();
        activity.set_at_rest(false, t0);
        assert_eq!(activity.rest_duration(t0), None);
        activity.set_at_rest(true, t0);
        activity.set_at_rest(true, t0 + Duration::from_millis(50));
        assert_eq!(
            activity.rest_duration(t0 + Duration::from_millis(120)),
            Some(Duration::from_millis(120))
        );
        let before = activity.disarm_count();
        activity.clone().disarm();
        assert_eq!(activity.disarm_count(), before + 1);
    }
}
