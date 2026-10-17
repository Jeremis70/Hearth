//! [`ProcessRuntime`]: games we spawn ourselves (emulators, native ports).
//!
//! Each game runs in its own process group (pgid = pid of the spawned process). Signals only
//! ever go to that group, never to anything else.
//!
//! A game never outlives Hearth: the kernel sends it SIGKILL when the thread that launched it
//! exits (`PR_SET_PDEATHSIG`). Launch games from a thread that lives as long as Hearth (the UI
//! thread). Normal shutdown closes the game properly first ([`Runtime::close`]).

use std::fs::OpenOptions;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use nix::sys::prctl;
use nix::sys::signal::{SigSet, SigmaskHow, Signal, killpg, pthread_sigmask};
use nix::unistd::{Pid, getpgrp};

use super::{LaunchCtx, Runtime, RuntimeError, RuntimeEvent, Session, SessionId};
use crate::model::Game;

/// How long `close` waits after SIGTERM before SIGKILL.
pub const CLOSE_GRACE: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(50);

/// Runs games as child processes in their own process group.
#[derive(Debug, Default)]
pub struct ProcessRuntime {
    next_id: AtomicU64,
}

impl ProcessRuntime {
    /// Creates the runtime.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Sends `signal` (or checks existence with `None`) to a game's process group.
/// Refuses pgid ≤ 1 and Hearth's own group.
fn signal_group(pgid: u32, signal: Option<Signal>) -> Result<(), RuntimeError> {
    let raw = i32::try_from(pgid).map_err(|_| RuntimeError::ForeignGroup(pgid))?;
    if raw <= 1 || Pid::from_raw(raw) == getpgrp() {
        return Err(RuntimeError::ForeignGroup(pgid));
    }
    killpg(Pid::from_raw(raw), signal).map_err(|source| RuntimeError::Signal { pgid, source })
}

fn group_alive(pgid: u32) -> bool {
    signal_group(pgid, None).is_ok()
}

fn wait_until_gone(pgid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !group_alive(pgid) {
            return true;
        }
        thread::sleep(POLL);
    }
    !group_alive(pgid)
}

impl Runtime for ProcessRuntime {
    fn launch(&self, game: &Game, ctx: &LaunchCtx) -> Result<Session, RuntimeError> {
        let spec = &ctx.command;
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args).process_group(0).stdin(Stdio::null());
        // SAFETY: runs in the forked child before exec and only makes two async-signal-safe
        // syscalls (prctl, sigprocmask); no allocation or locking.
        #[allow(unsafe_code)]
        unsafe {
            cmd.pre_exec(|| {
                // Die with Hearth: a frozen (SIGSTOPped) game would otherwise linger forever.
                prctl::set_pdeathsig(Signal::SIGKILL).map_err(std::io::Error::from)?;
                // Hearth blocks SIGINT/SIGTERM/SIGHUP for its signal thread; games must not
                // inherit that.
                pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&SigSet::empty()), None)
                    .map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        for key in &spec.env_remove {
            cmd.env_remove(key);
        }
        cmd.envs(spec.env.iter().map(|(k, v)| (k, v)));
        match &spec.log {
            Some(path) => {
                let file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|source| RuntimeError::Log {
                        path: path.clone(),
                        source,
                    })?;
                let err = file.try_clone().map_err(|source| RuntimeError::Log {
                    path: path.clone(),
                    source,
                })?;
                cmd.stdout(file).stderr(err);
            }
            None => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        let mut child = cmd.spawn().map_err(|source| RuntimeError::Spawn {
            program: spec.program.to_string_lossy().into_owned(),
            source,
        })?;

        let session = Session {
            id: SessionId(self.next_id.fetch_add(1, Ordering::Relaxed)),
            game: game.id.clone(),
            pgid: child.id(),
            exited: Arc::new(AtomicBool::new(false)),
        };
        tracing::info!(game = %game.id, pgid = session.pgid, "game started");

        let exited = session.exited.clone();
        let events = ctx.events.clone();
        let (id, game_id) = (session.id, session.game.clone());
        thread::Builder::new()
            .name("hearth-game-watch".into())
            .spawn(move || {
                let success = match child.wait() {
                    Ok(status) => {
                        tracing::info!(game = %game_id, %status, "game exited");
                        status.success()
                    }
                    Err(err) => {
                        tracing::warn!(game = %game_id, "waiting for game failed: {err}");
                        false
                    }
                };
                exited.store(true, Ordering::Release);
                let _ = events.send(RuntimeEvent::Exited {
                    session: id,
                    game: game_id,
                    success,
                });
            })
            .map_err(|source| RuntimeError::Spawn {
                program: "watcher thread".into(),
                source,
            })?;
        Ok(session)
    }

    fn is_running(&self, s: &Session) -> bool {
        !s.has_exited() || group_alive(s.pgid)
    }

    fn suspend(&self, s: &Session) -> Result<(), RuntimeError> {
        signal_group(s.pgid, Some(Signal::SIGSTOP))
    }

    fn resume(&self, s: &Session) -> Result<(), RuntimeError> {
        signal_group(s.pgid, Some(Signal::SIGCONT))
    }

    fn close(&self, s: &Session) -> Result<(), RuntimeError> {
        if !group_alive(s.pgid) {
            return Ok(());
        }
        // A stopped process only acts on SIGTERM once continued.
        let _ = signal_group(s.pgid, Some(Signal::SIGCONT));
        let _ = signal_group(s.pgid, Some(Signal::SIGTERM));
        if wait_until_gone(s.pgid, CLOSE_GRACE) {
            return Ok(());
        }
        tracing::warn!(pgid = s.pgid, "game ignored SIGTERM; sending SIGKILL");
        match signal_group(s.pgid, Some(Signal::SIGKILL)) {
            Ok(()) | Err(RuntimeError::Signal { .. }) => {}
            Err(err) => return Err(err),
        }
        wait_until_gone(s.pgid, Duration::from_secs(1));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc;

    use super::*;
    use crate::model::{GameId, SystemId};
    use crate::runtime::LaunchCommand;

    fn game() -> Game {
        Game {
            id: GameId::new("test:sleep"),
            title: "Sleep".into(),
            system: SystemId::new("test"),
            path: PathBuf::from("/dev/null"),
        }
    }

    fn ctx(args: &[&str]) -> (LaunchCtx, mpsc::Receiver<RuntimeEvent>) {
        let (tx, rx) = mpsc::channel();
        let command = LaunchCommand {
            program: "sleep".into(),
            args: args.iter().map(Into::into).collect(),
            ..LaunchCommand::default()
        };
        (
            LaunchCtx {
                command,
                events: tx,
            },
            rx,
        )
    }

    /// Process state letter from /proc/<pid>/stat ('S' sleeping, 'T' stopped, ...).
    fn state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat[stat.rfind(')')? + 1..].trim_start().chars().next()
    }

    fn wait_for_state(pid: u32, want: char) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if state(pid) == Some(want) {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn launch_suspend_resume_close() {
        let runtime = ProcessRuntime::new();
        let (ctx, rx) = ctx(&["30"]);
        let session = runtime.launch(&game(), &ctx).unwrap();
        assert_ne!(Pid::from_raw(session.pgid() as i32), getpgrp());
        assert!(runtime.is_running(&session));

        runtime.suspend(&session).unwrap();
        assert!(wait_for_state(session.pgid(), 'T'), "not stopped");
        runtime.resume(&session).unwrap();
        assert!(wait_for_state(session.pgid(), 'S'), "not resumed");

        // Close while frozen: SIGCONT + SIGTERM must still end it quickly.
        runtime.suspend(&session).unwrap();
        let started = Instant::now();
        runtime.close(&session).unwrap();
        assert!(started.elapsed() < CLOSE_GRACE);
        let event = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(event, RuntimeEvent::Exited { success: false, .. }));
        assert!(!runtime.is_running(&session));
    }

    #[test]
    fn natural_exit_is_reported() {
        let runtime = ProcessRuntime::new();
        let (ctx, rx) = ctx(&["0.2"]);
        let session = runtime.launch(&game(), &ctx).unwrap();
        let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            event,
            RuntimeEvent::Exited {
                session: session.id(),
                game: GameId::new("test:sleep"),
                success: true
            }
        );
        assert!(session.has_exited());
        assert!(!runtime.is_running(&session));
        // Closing a finished session is a no-op.
        runtime.close(&session).unwrap();
    }

    #[test]
    fn close_kills_a_game_that_ignores_sigterm() {
        let runtime = ProcessRuntime::new();
        let (tx, rx) = mpsc::channel();
        let command = LaunchCommand {
            program: "sh".into(),
            args: ["-c", "trap '' TERM; sleep 30 & wait"]
                .iter()
                .map(Into::into)
                .collect(),
            ..LaunchCommand::default()
        };
        let session = runtime
            .launch(
                &game(),
                &LaunchCtx {
                    command,
                    events: tx,
                },
            )
            .unwrap();
        thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        runtime.close(&session).unwrap();
        assert!(started.elapsed() >= CLOSE_GRACE);
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
        assert!(!group_alive(session.pgid()));
    }

    #[test]
    fn refuses_own_and_init_groups() {
        let own = u32::try_from(getpgrp().as_raw()).unwrap();
        assert!(matches!(
            signal_group(own, Some(Signal::SIGSTOP)),
            Err(RuntimeError::ForeignGroup(_))
        ));
        assert!(matches!(
            signal_group(1, None),
            Err(RuntimeError::ForeignGroup(1))
        ));
        assert!(matches!(
            signal_group(0, None),
            Err(RuntimeError::ForeignGroup(0))
        ));
    }

    /// Field of `/proc/<pid>/status`, e.g. "SigBlk".
    fn status_field(pid: u32, key: &str) -> Option<String> {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        status.lines().find_map(|l| {
            l.strip_prefix(&format!("{key}:"))
                .map(|v| v.trim().to_owned())
        })
    }

    #[test]
    fn game_dies_with_the_thread_that_launched_it() {
        let runtime = Arc::new(ProcessRuntime::new());
        let (ctx, rx) = ctx(&["30"]);
        let launcher = {
            let runtime = runtime.clone();
            thread::spawn(move || runtime.launch(&game(), &ctx).unwrap())
        };
        let session = launcher.join().unwrap();
        // The launching thread is gone: the kernel kills the game.
        let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(event, RuntimeEvent::Exited { success: false, .. }));
        assert!(!runtime.is_running(&session));
    }

    #[test]
    fn child_does_not_inherit_blocked_signals() {
        let runtime = ProcessRuntime::new();
        let (ctx, rx) = ctx(&["5"]);
        let mut blocked = SigSet::empty();
        blocked.add(Signal::SIGUSR2);
        blocked.thread_block().unwrap();
        let session = runtime.launch(&game(), &ctx).unwrap();
        blocked.thread_unblock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut sigblk = None;
        while Instant::now() < deadline {
            // Before exec, /proc still describes the forked copy of the test process.
            if std::fs::read_link(format!("/proc/{}/exe", session.pgid()))
                .is_ok_and(|exe| exe.ends_with("sleep"))
            {
                sigblk = status_field(session.pgid(), "SigBlk");
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(sigblk.as_deref(), Some("0000000000000000"));
        runtime.close(&session).unwrap();
        let _ = rx.recv_timeout(Duration::from_secs(2));
    }

    #[test]
    fn env_changes_reach_the_child() {
        let runtime = ProcessRuntime::new();
        let (tx, rx) = mpsc::channel();
        let mut command = LaunchCommand {
            program: "sleep".into(),
            args: vec!["5".into()],
            ..LaunchCommand::default()
        };
        command.set_env("HEARTH_TEST_VAR", "1");
        command.force_x11();
        let session = runtime
            .launch(
                &game(),
                &LaunchCtx {
                    command,
                    events: tx,
                },
            )
            .unwrap();
        // Until the child has exec'd, /proc still shows the parent's environment.
        let read_vars = || -> Vec<String> {
            std::fs::read(format!("/proc/{}/environ", session.pgid()))
                .unwrap_or_default()
                .split(|&b| b == 0)
                .map(|v| String::from_utf8_lossy(v).into_owned())
                .collect()
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut vars = read_vars();
        while !vars.iter().any(|v| v == "HEARTH_TEST_VAR=1") && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
            vars = read_vars();
        }
        assert!(vars.iter().any(|v| v == "HEARTH_TEST_VAR=1"));
        assert!(vars.iter().any(|v| v == "SDL_VIDEODRIVER=x11"));
        assert!(!vars.iter().any(|v| v.starts_with("WAYLAND_DISPLAY=")));
        runtime.close(&session).unwrap();
        let _ = rx.recv_timeout(Duration::from_secs(2));
    }
}
