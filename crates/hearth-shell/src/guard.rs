//! The input guard: a small child process (`hearth-guard`, its own binary so `pgrep -x
//! hearth-shell` only finds Hearth) that hands the controllers back to InputPlumber when
//! Hearth dies without doing it (crash, SIGKILL). InputPlumber doesn't notice a client going
//! away, so a controller left in `Always` would stay dead for games and the desktop.
//!
//! Hearth keeps the write end of a pipe to the guard's stdin. On a normal exit, after releasing
//! the controllers itself, Hearth writes [`STAND_DOWN`]. Any other end of the pipe (the kernel
//! closes it whatever kills Hearth) makes the guard release them. The pipe is close-on-exec, so
//! games never hold it open. The guard inherits Hearth's blocked SIGINT/SIGTERM/SIGHUP, so a
//! Ctrl+C or a terminal closing doesn't stop it before it has done its job.

use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};

/// What Hearth writes when it has released the controllers itself (`src/bin/hearth-guard.rs`
/// reads it).
const STAND_DOWN: &str = "released\n";

/// Hearth's side of the guard.
pub struct Guard {
    child: Child,
    stdin: ChildStdin,
}

impl Guard {
    /// Starts the guard (`hearth-guard`, next to this executable). Logs and returns `None` if
    /// it can't: Hearth still works, only a crash would leave the controller in Hearth's hands.
    pub fn spawn() -> Option<Self> {
        let spawned = std::env::current_exe().and_then(|exe| {
            Command::new(exe.with_file_name("hearth-guard"))
                .stdin(Stdio::piped())
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
