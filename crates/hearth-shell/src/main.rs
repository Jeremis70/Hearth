//! Hearth shell: the Slint UI and the glue to core and platform services.

mod bridge;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use hearth_core::paths::{self, DataDirs};
use hearth_core::profiles::RetroArchProfile;
use hearth_core::systems::Systems;
use hearth_platform::gamescope::{self, Compositor};
use hearth_platform::input::InputBackend;
use hearth_platform::input::dev_socket::DevSocketBackend;
use hearth_platform::input::gilrs::GilrsBackend;
use hearth_platform::network::MockNetworkBackend;
use hearth_platform::process;
use nix::sys::signal::{SigSet, Signal};
use slint::ComponentHandle;
use tracing_subscriber::EnvFilter;

slint::include_modules!();

/// Hearth, a controller-first living-room console.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Run in a resizable window (development).
    #[arg(long, conflicts_with = "fullscreen")]
    windowed: bool,
    /// Run fullscreen (the default).
    #[arg(long)]
    fullscreen: bool,
    /// Also read actions (home_short, accept, ...) from a Unix datagram socket at PATH, for
    /// scripted testing.
    #[arg(long, value_name = "PATH")]
    dev_socket: Option<PathBuf>,
    /// Folder scanned (read-only) for games, laid out as <dir>/<system>/... [default: ~/Games]
    #[arg(long, value_name = "DIR")]
    games_dir: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    // Before any thread exists, so every thread inherits the mask and only the signal thread
    // receives these.
    let shutdown_signals = block_shutdown_signals()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args = Args::parse();

    let dirs = DataDirs::from_env().context("resolving data directories")?;
    dirs.create_all()?;
    tracing::info!(data = %dirs.data().display(), "using data directories");
    let games_dir = args
        .games_dir
        .or_else(paths::default_games_dir)
        .context("no home directory; pass --games-dir")?;
    let systems = Systems::builtin()?;

    if process::any_process_named("steam") {
        tracing::warn!(
            "Steam is running: it also reads the controller (Steam Input) and may react to \
             the Guide button; quit Steam while testing Hearth"
        );
    }

    // gamescope is the compositor on the appliance, so always use winit, never LinuxKMS.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .select()
        .context("selecting the winit backend")?;

    let compositor: Arc<dyn Compositor> = Arc::from(gamescope::detect());

    spawn_signal_thread(shutdown_signals)?;

    let app = AppWindow::new()?;
    // The overlay window keeps the event loop alive, so quit explicitly when Hearth is closed.
    app.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });
    app.window().set_fullscreen(!args.windowed);
    bridge::compositor::register_shell(&app, compositor.clone());

    bridge::models::install(&app);
    bridge::models::load_network(&app, MockNetworkBackend::new());

    // The overlay only exists inside gamescope (Steam mode), drawn on the overlay plane.
    let overlay = if compositor.controls_focus() {
        let overlay = OverlayWindow::new()?;
        overlay.window().set_fullscreen(true);
        overlay.show()?;
        bridge::compositor::register_overlay(&overlay);
        Some(overlay)
    } else {
        None
    };

    let gilrs = GilrsBackend::new();
    bridge::session::install(bridge::session::Setup {
        app: app.as_weak(),
        overlay,
        compositor,
        systems,
        profile: RetroArchProfile::new(dirs.retroarch()),
        activity: gilrs.activity(),
        library_db: dirs.library_db(),
        games_dir,
    });

    let clock = slint::Timer::default();
    update_clock(&app);
    let weak = app.as_weak();
    clock.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(1),
        move || {
            if let Some(app) = weak.upgrade() {
                update_clock(&app);
            }
        },
    );

    let mut backends: Vec<Box<dyn InputBackend>> = vec![Box::new(gilrs)];
    if let Some(path) = args.dev_socket {
        backends.push(Box::new(DevSocketBackend::new(path)));
    }
    if let Err(err) = bridge::input::start(backends) {
        tracing::warn!("controller input unavailable, keyboard only: {err:#}");
    }

    // run_event_loop: the overlay window being hidden must not end the app.
    app.show()?;
    slint::run_event_loop_until_quit()?;
    tracing::info!("shutting down");
    bridge::session::shutdown();
    Ok(())
}

fn block_shutdown_signals() -> anyhow::Result<SigSet> {
    let mut set = SigSet::empty();
    for signal in [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP] {
        set.add(signal);
    }
    set.thread_block().context("blocking shutdown signals")?;
    Ok(set)
}

/// Ctrl+C, SIGTERM or SIGHUP (terminal or gamescope going away) end the event loop, so the
/// running game is closed properly instead of being left behind.
fn spawn_signal_thread(set: SigSet) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("hearth-signals".into())
        .spawn(move || {
            if let Ok(signal) = set.wait() {
                tracing::info!(?signal, "quitting on signal");
                let _ = slint::invoke_from_event_loop(|| {
                    let _ = slint::quit_event_loop();
                });
            }
        })
        .context("spawning signal thread")?;
    Ok(())
}

fn update_clock(app: &AppWindow) {
    let now = chrono::Local::now().format("%-I:%M %p").to_string();
    app.global::<Status>().set_clock(now.as_str().into());
    bridge::session::tick(&now);
}
