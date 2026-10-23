//! Development input backend: actions sent as text to a Unix datagram socket, e.g.
//! `home_short`, `accept`, `down`. Lets scripts drive Hearth exactly like a controller (the
//! keyboard can't reach Hearth while a game has focus inside gamescope). Enabled only with
//! `--dev-socket`.
//!
//! Words that aren't actions go to an optional command handler (e.g. `sound:move`,
//! `audio_state`); a reply is sent back to the sender when it bound an address.

use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::thread;

use super::{Action, InputBackend, InputError};

/// Parses an action name (`home_short`, `prev_tab`, …; case-insensitive).
pub fn parse_action(name: &str) -> Option<Action> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "up" => Action::Up,
        "down" => Action::Down,
        "left" => Action::Left,
        "right" => Action::Right,
        "accept" => Action::Accept,
        "back" => Action::Back,
        "prev_tab" => Action::PrevTab,
        "next_tab" => Action::NextTab,
        "options" => Action::Options,
        "home_short" => Action::HomeShort,
        "home_long" => Action::HomeLong,
        "capture" => Action::Capture,
        _ => return None,
    })
}

/// Handles a non-action word; `None`: not a command it knows. Runs on the socket's thread.
pub type CommandHandler = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Reads actions from a Unix datagram socket at `path` (replaced if it exists).
pub struct DevSocketBackend {
    path: PathBuf,
    started: bool,
    commands: Option<CommandHandler>,
}

impl std::fmt::Debug for DevSocketBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevSocketBackend")
            .field("path", &self.path)
            .field("started", &self.started)
            .finish_non_exhaustive()
    }
}

impl DevSocketBackend {
    /// Backend listening at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            started: false,
            commands: None,
        }
    }

    /// Also handles other words with `handler` (its reply goes back to the sender).
    pub fn with_commands(mut self, handler: CommandHandler) -> Self {
        self.commands = Some(handler);
        self
    }
}

impl InputBackend for DevSocketBackend {
    fn start(&mut self, tx: Sender<Action>) -> Result<(), InputError> {
        if self.started {
            return Err(InputError::AlreadyStarted);
        }
        if self.path.exists() {
            std::fs::remove_file(&self.path).map_err(|e| InputError::Start(e.to_string()))?;
        }
        let socket =
            UnixDatagram::bind(&self.path).map_err(|e| InputError::Start(e.to_string()))?;
        tracing::info!(path = %self.path.display(), "dev input socket listening");
        let commands = self.commands.clone();
        thread::Builder::new()
            .name("hearth-dev-socket".into())
            .spawn(move || {
                let mut buf = [0u8; 256];
                while let Ok((n, from)) = socket.recv_from(&mut buf) {
                    let text = String::from_utf8_lossy(&buf[..n]);
                    for name in text.split_whitespace() {
                        if let Some(action) = parse_action(name) {
                            if tx.send(action).is_err() {
                                return;
                            }
                            continue;
                        }
                        match commands.as_ref().and_then(|handle| handle(name)) {
                            Some(reply) => {
                                if let Some(path) = from.as_pathname() {
                                    let _ = socket.send_to(reply.as_bytes(), path);
                                }
                            }
                            None => tracing::warn!(name, "dev socket: unknown action"),
                        }
                    }
                }
            })
            .map_err(|e| InputError::Start(e.to_string()))?;
        self.started = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_action_names() {
        assert_eq!(parse_action("home_short"), Some(Action::HomeShort));
        assert_eq!(parse_action(" ACCEPT\n"), Some(Action::Accept));
        assert_eq!(parse_action("jump"), None);
    }
}
