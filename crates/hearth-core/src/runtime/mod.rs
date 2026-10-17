//! The contract that hides consoles: every game is launched, frozen, resumed and closed the
//! same way (docs/plan.md §6.5).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use crate::model::{Game, GameId};

pub mod process;

pub use process::ProcessRuntime;

/// Errors from a runtime.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The game process could not be started.
    #[error("cannot start {program}: {source}")]
    Spawn {
        /// Program that failed.
        program: String,
        /// Underlying error.
        source: std::io::Error,
    },
    /// Signalling the game's process group failed.
    #[error("cannot signal process group {pgid}: {source}")]
    Signal {
        /// Target group.
        pgid: u32,
        /// Underlying error.
        source: nix::Error,
    },
    /// Refused to signal a group that isn't a game's (e.g. our own).
    #[error("refusing to signal process group {0}")]
    ForeignGroup(u32),
    /// Opening the game's log file failed.
    #[error("cannot open log file {path}: {source}")]
    Log {
        /// Log file.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
}

/// Identifies one launch of a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

/// A running (or finished) game session.
#[derive(Debug, Clone)]
pub struct Session {
    id: SessionId,
    game: GameId,
    pgid: u32,
    exited: Arc<AtomicBool>,
}

impl Session {
    /// The session id.
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// The game being played.
    pub fn game(&self) -> &GameId {
        &self.game
    }

    /// The game's process group (equal to the pid of the process we spawned).
    pub fn pgid(&self) -> u32 {
        self.pgid
    }

    /// Whether the main process has exited.
    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }
}

/// Something that happened to a session, reported from the runtime's watcher thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
    /// The game's main process exited (by itself or because it was closed).
    Exited {
        /// Which session.
        session: SessionId,
        /// Which game.
        game: GameId,
        /// Exit status was success.
        success: bool,
    },
}

/// A program to run, with its environment changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchCommand {
    /// Executable (looked up in PATH if relative).
    pub program: OsString,
    /// Arguments.
    pub args: Vec<OsString>,
    /// Variables to set.
    pub env: Vec<(OsString, OsString)>,
    /// Variables to remove.
    pub env_remove: Vec<OsString>,
    /// File receiving stdout and stderr (appended); discarded if `None`.
    pub log: Option<PathBuf>,
}

impl LaunchCommand {
    /// Sets an environment variable, replacing an earlier setting.
    pub fn set_env(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) {
        let key = key.into();
        self.env_remove.retain(|k| k != &key);
        self.env.retain(|(k, _)| k != &key);
        self.env.push((key, value.into()));
    }

    /// Removes an environment variable from the child's environment.
    pub fn remove_env(&mut self, key: impl Into<OsString>) {
        let key = key.into();
        self.env.retain(|(k, _)| k != &key);
        if !self.env_remove.contains(&key) {
            self.env_remove.push(key);
        }
    }

    /// Launcher rule inside gamescope: every game must use gamescope's Xwayland (`DISPLAY`)
    /// and never the host's Wayland session, or it would open on the desktop behind
    /// gamescope. Removes the Wayland socket variables and asks common toolkits for X11.
    pub fn force_x11(&mut self) {
        self.remove_env("WAYLAND_DISPLAY");
        self.remove_env("WAYLAND_SOCKET");
        self.set_env("XDG_SESSION_TYPE", "x11");
        self.set_env("SDL_VIDEODRIVER", "x11");
        self.set_env("GDK_BACKEND", "x11");
        self.set_env("QT_QPA_PLATFORM", "xcb");
    }
}

/// Everything a runtime needs to start a game.
#[derive(Debug, Clone)]
pub struct LaunchCtx {
    /// What to run.
    pub command: LaunchCommand,
    /// Where to report the session's events.
    pub events: Sender<RuntimeEvent>,
}

/// Launches and controls games of one kind.
pub trait Runtime: Send + Sync {
    /// Starts `game`.
    fn launch(&self, game: &Game, ctx: &LaunchCtx) -> Result<Session, RuntimeError>;
    /// Whether any of the session's processes are still alive.
    fn is_running(&self, s: &Session) -> bool;
    /// Freezes the game (used while Hearth is in front).
    fn suspend(&self, s: &Session) -> Result<(), RuntimeError>;
    /// Unfreezes the game.
    fn resume(&self, s: &Session) -> Result<(), RuntimeError>;
    /// Ends the game; blocks until it is gone (a few seconds at most).
    fn close(&self, s: &Session) -> Result<(), RuntimeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_x11_removes_wayland_and_sets_x11_hints() {
        let mut cmd = LaunchCommand::default();
        cmd.set_env("WAYLAND_DISPLAY", "wayland-0");
        cmd.force_x11();
        assert!(cmd.env_remove.contains(&"WAYLAND_DISPLAY".into()));
        assert!(!cmd.env.iter().any(|(k, _)| k == "WAYLAND_DISPLAY"));
        assert!(cmd.env.contains(&("SDL_VIDEODRIVER".into(), "x11".into())));
        assert!(cmd.env.contains(&("XDG_SESSION_TYPE".into(), "x11".into())));
    }
}
