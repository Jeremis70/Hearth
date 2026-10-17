//! Development input backend: actions sent as text to a Unix datagram socket, e.g.
//! `home_short`, `accept`, `down`. Lets scripts drive Hearth exactly like a controller (the
//! keyboard can't reach Hearth while a game has focus inside gamescope). Enabled only with
//! `--dev-socket`.

use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use super::{Action, InputBackend, InputError, InputMode};

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
        _ => return None,
    })
}

/// Reads actions from a Unix datagram socket at `path` (replaced if it exists).
#[derive(Debug)]
pub struct DevSocketBackend {
    path: PathBuf,
    started: bool,
}

impl DevSocketBackend {
    /// Backend listening at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            started: false,
        }
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
        thread::Builder::new()
            .name("hearth-dev-socket".into())
            .spawn(move || {
                let mut buf = [0u8; 256];
                while let Ok(n) = socket.recv(&mut buf) {
                    let text = String::from_utf8_lossy(&buf[..n]);
                    for name in text.split_whitespace() {
                        match parse_action(name) {
                            Some(action) => {
                                if tx.send(action).is_err() {
                                    return;
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

    fn set_mode(&mut self, _mode: InputMode) {}
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
