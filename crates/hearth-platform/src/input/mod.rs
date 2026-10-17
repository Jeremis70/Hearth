//! Controller input, reduced to a small set of UI actions.
//!
//! Every backend feeds [`Action`]s into one channel; the shell turns them into key events.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

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

/// Stick or trigger travel below which it counts as released. Smaller than the navigation
/// threshold: a game reacts to much less travel than Hearth's menus do.
pub const RELEASE_DEADZONE: f32 = 0.25;

/// Whether any controller is being touched: a button pressed, or a stick or trigger outside
/// [`RELEASE_DEADZONE`]. Written by the input thread, read by the UI.
#[derive(Debug, Clone, Default)]
pub struct PadActivity(Arc<AtomicBool>);

impl PadActivity {
    /// A new handle; idle until the input thread reports otherwise.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether every button is released and every stick and trigger is at rest.
    pub fn is_idle(&self) -> bool {
        !self.0.load(Ordering::Acquire)
    }

    /// Records whether something is held.
    pub fn set_held(&self, held: bool) {
        self.0.store(held, Ordering::Release);
    }
}

/// Whether nothing is held, given every button's pressed flag and every stick or trigger value.
pub fn is_at_rest(
    buttons: impl IntoIterator<Item = bool>,
    axes: impl IntoIterator<Item = f32>,
) -> bool {
    !buttons.into_iter().any(|pressed| pressed)
        && axes.into_iter().all(|v| v.abs() < RELEASE_DEADZONE)
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
    fn at_rest_needs_no_button_and_centered_axes() {
        assert!(is_at_rest([false, false], [0.0, 0.1, -0.2]));
        assert!(!is_at_rest([false, true], [0.0]));
        assert!(!is_at_rest([], [0.0, -0.6]));
        assert!(!is_at_rest([], [0.3]));
        assert!(is_at_rest([], []));
    }

    #[test]
    fn activity_starts_idle() {
        let activity = PadActivity::new();
        assert!(activity.is_idle());
        activity.clone().set_held(true);
        assert!(!activity.is_idle());
    }
}
