//! The input guard: a small child process that hands the controllers back to InputPlumber when
//! Hearth dies without doing it (crash, SIGKILL). InputPlumber doesn't notice a client going
//! away, so a controller left in `Always` would stay dead for games and the desktop.
//!
//! Hearth keeps the write end of a pipe to the guard's stdin. On a normal exit, after releasing
//! the controllers itself, Hearth writes [`STAND_DOWN`]. Any other end of the pipe (the kernel
//! closes it whatever kills Hearth) makes the guard release them. The pipe is close-on-exec, so
//! games never hold it open. The guard inherits Hearth's blocked SIGINT/SIGTERM/SIGHUP, so a
//! Ctrl+C or a terminal closing doesn't stop it before it has done its job.

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

use hearth_platform::input::inputplumber;

/// What Hearth writes when it has released the controllers itself.
const STAND_DOWN: &str = "released\n";

/// Hearth's side of the guard.
pub struct Guard {
    child: Child,
    stdin: ChildStdin,
}

impl Guard {
    /// Starts the guard (this executable with `--input-guard`). Logs and returns `None` if it
    /// can't: Hearth still works, only a crash would leave the controller in Hearth's hands.
    pub fn spawn() -> Option<Self> {
        let spawned = std::env::current_exe().and_then(|exe| {
            Command::new(exe)
                .arg("--input-guard")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
        });
        match spawned {
            Ok(mut child) => {
                let stdin = child.stdin.take()?;
                tracing::info!(pid = child.id(), "input guard started");
                Some(Self { child, stdin })
            }
            Err(err) => {
                tracing::warn!("starting the input guard failed: {err}");
                None
            }
        }
    }

    /// Hearth released the controllers: the guard exits without touching them.
    pub fn stand_down(mut self) {
        if let Err(err) = self.stdin.write_all(STAND_DOWN.as_bytes()) {
            tracing::warn!("telling the input guard to stand down failed: {err}");
        }
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

/// The guard process (`--input-guard`): waits for Hearth to go away, then releases the
/// controllers unless Hearth said it already did.
pub fn run() {
    let mut message = String::new();
    // Ends when Hearth closes the pipe or dies.
    let _ = std::io::stdin().read_to_string(&mut message);
    if message == STAND_DOWN {
        return;
    }
    tracing::warn!("Hearth ended without handing the controllers back; doing it now");
    match inputplumber::release_all() {
        Ok(()) => tracing::info!("controllers handed back to InputPlumber"),
        Err(err) => tracing::error!("handing the controllers back failed: {err}"),
    }
}
