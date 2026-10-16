//! Controller input, reduced to a small set of UI actions.
//!
//! Every backend feeds [`Action`]s into one channel; the shell turns them into key events.

use std::sync::mpsc::Sender;

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
