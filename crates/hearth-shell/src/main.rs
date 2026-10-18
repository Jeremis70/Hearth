//! Hearth shell: the Slint UI and the glue to core and platform services.

mod bridge;
mod callouts;
mod controller_layout;
mod controls;
mod guard;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use hearth_core::paths::{self, DataDirs};
use hearth_core::profiles::{DolphinInstall, DolphinProfile, RetroArchProfile};
use hearth_core::systems::Systems;
use hearth_platform::gamescope::{self, Compositor};
use hearth_platform::input::dev_socket::DevSocketBackend;
use hearth_platform::input::gilrs::GilrsBackend;
use hearth_platform::input::inputplumber::InputPlumberBackend;
use hearth_platform::input::{InputBackend, InputControl, PadActivity};
use hearth_platform::network::MockNetworkBackend;
use hearth_platform::process;
use nix::sys::signal::{SigSet, Signal};
use slint::ComponentHandle;
use tracing_subscriber::EnvFilter;

slint::include_modules!();

/// Longest wait for InputPlumber to take the controllers back on exit.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(3);

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
    /// Print GAME's resolved controls (with the layer of each value) and the emulator config
    /// generated from them, then exit. GAME is a game id, a controls key (gc/GZ2E01) or part
    /// of a title.
    #[arg(long, value_name = "GAME")]
    print_controls: Option<String>,
    /// Internal: the guard process that hands the controllers back to InputPlumber if Hearth
    /// dies without doing it (see `guard`).
    #[arg(long, hide = true)]
    input_guard: bool,
    /// Where controller input comes from: InputPlumber (hides Hearth's menu input from games,
    /// remaps live), gilrs (reads controllers directly), or auto (InputPlumber when it runs
    /// and allows Hearth to use it, else gilrs).
    #[arg(long, value_enum, default_value_t = InputChoice::Auto)]
    input: InputChoice,
}

/// `--input`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum InputChoice {
    Auto,
    Inputplumber,
    Gilrs,
}

/// The controller backend, its live state and the session's handle on it.
struct Input {
    backend: Box<dyn InputBackend>,
    activity: PadActivity,
    control: InputControl,
}

/// Picks the controller backend. `auto` falls back to gilrs and says why.
fn select_input(choice: InputChoice, runtime_dir: &std::path::Path) -> anyhow::Result<Input> {
    let gilrs = || {
        let backend = GilrsBackend::new();
        Input {
            activity: backend.activity(),
            control: InputControl::detached(),
            backend: Box::new(backend),
        }
    };
    let inputplumber = || {
        InputPlumberBackend::connect(&runtime_dir.join("inputplumber")).map(|backend| Input {
            activity: backend.activity(),
            control: backend.control(),
            backend: Box::new(backend),
        })
    };
    Ok(match choice {
        InputChoice::Gilrs => gilrs(),
        InputChoice::Inputplumber => inputplumber().context("using InputPlumber")?,
        InputChoice::Auto => match inputplumber() {
            Ok(input) => {
                tracing::info!("controller input through InputPlumber");
                input
            }
            Err(err) => {
                tracing::info!("not using InputPlumber ({err}); reading controllers with gilrs");
                gilrs()
            }
        },
    })
}

fn main() -> anyhow::Result<()> {
    // Before any thread exists, so every thread inherits the mask and only the signal thread
    // receives these.
    let shutdown_signals = block_shutdown_signals()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args = Args::parse();
    if args.input_guard {
        guard::run();
        return Ok(());
    }

    let dirs = DataDirs::from_env().context("resolving data directories")?;
    dirs.create_all()?;
    tracing::info!(data = %dirs.data().display(), "using data directories");
    let games_dir = args
        .games_dir
        .or_else(paths::default_games_dir)
        .context("no home directory; pass --games-dir")?;
    let systems = Systems::builtin()?;
    let controls_dir = hearth_core::controls::user_dir(dirs.config());

    if let Some(query) = args.print_controls {
        return print_controls(
            &query,
            &games_dir,
            &systems,
            &controls_dir,
            &dirs,
            args.input,
        );
    }

    // Before anything reads the controller or opens a window. (--print-controls only
    // reads, so it may run next to a running Hearth.)
    let _instance = match paths::InstanceLock::acquire(dirs.data()) {
        Ok(lock) => lock,
        Err(err @ paths::LockError::Running { .. }) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
        Err(err) => return Err(err).context("taking the single-instance lock"),
    };

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

    let dolphin = DolphinInstall::detect();
    match &dolphin {
        Some(install) => tracing::info!(?install, "Dolphin found"),
        None => tracing::info!("Dolphin not found: GameCube and Wii games won't launch"),
    }

    let input = select_input(args.input, dirs.runtime())?;
    // Hands the controllers back even if Hearth is killed (SIGKILL included).
    let guard = input.control.is_live().then(guard::Guard::spawn).flatten();
    bridge::session::install(bridge::session::Setup {
        app: app.as_weak(),
        overlay,
        compositor,
        systems,
        profile: RetroArchProfile::new(dirs.retroarch()),
        dolphin: DolphinProfile::new(dirs.dolphin(), dolphin),
        controls_dir,
        runtime_dir: dirs.runtime().to_path_buf(),
        activity: input.activity.clone(),
        input: input.control.clone(),
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

    let mut backends: Vec<Box<dyn InputBackend>> = vec![input.backend];
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
    if input.control.is_live() && !input.control.release(RELEASE_TIMEOUT) {
        tracing::warn!("the input thread didn't hand the controllers back; doing it directly");
        if let Err(err) = hearth_platform::input::inputplumber::release_all() {
            tracing::warn!("handing the controllers back failed: {err}");
        }
    }
    if let Some(guard) = guard {
        guard.stand_down();
    }
    Ok(())
}

/// `--print-controls`: no window, nothing written.
fn print_controls(
    query: &str,
    games_dir: &std::path::Path,
    systems: &Systems,
    controls_dir: &std::path::Path,
    dirs: &DataDirs,
    input: InputChoice,
) -> anyhow::Result<()> {
    let games = hearth_core::scan::scan_roms(games_dir, systems);
    let keys: Vec<_> = games
        .iter()
        .map(|g| hearth_core::controls::GameKey::for_rom(&g.system, &g.path, &g.title))
        .collect();
    let needle = query.to_lowercase();
    let exact: Vec<_> = games
        .iter()
        .zip(&keys)
        .filter(|(g, k)| g.id.as_str() == query || k.as_string() == query)
        .collect();
    let matches = if exact.is_empty() {
        games
            .iter()
            .zip(&keys)
            .filter(|(g, _)| g.title.to_lowercase().contains(&needle))
            .collect()
    } else {
        exact
    };
    let [(game, _)] = matches.as_slice() else {
        let list: Vec<_> = matches
            .iter()
            .map(|(g, k)| format!("  {} [{}]", g.title, k.as_string()))
            .collect();
        anyhow::bail!(
            "{} games match '{query}' under {}{}{}",
            matches.len(),
            games_dir.display(),
            if list.is_empty() { "" } else { ":\n" },
            list.join("\n")
        );
    };
    let system = systems
        .get(&game.system)
        .context("game of an unknown system")?;
    let kind = system.emulator().context("system has no emulator")?.kind;
    // Like a launch would: InputPlumber's virtual pad when Hearth would use it. Its managed
    // controllers are hidden from gilrs, so ask it about them.
    let client = (input != InputChoice::Gilrs)
        .then(|| hearth_platform::input::inputplumber::client::Client::connect().ok())
        .flatten();
    let mut pads: Vec<_> = client
        .iter()
        .flat_map(|c| {
            let devices = c.composite_devices().unwrap_or_default();
            devices
                .into_iter()
                .filter(|d| c.capabilities(d).is_ok_and(|caps| !caps.is_empty()))
                .filter_map(|d| c.source_pad(&d).ok())
                .collect::<Vec<_>>()
        })
        .collect();
    if pads.is_empty() {
        pads = hearth_platform::input::gilrs::connected_pads().unwrap_or_else(|err| {
            tracing::warn!("can't list controllers: {err}");
            Vec::new()
        });
    }
    let pad = if client.is_some() {
        controls::PadTarget::Virtual
    } else {
        controls::PadTarget::Physical
    };
    let device = controls::device(&pads, controls_dir);
    let retroarch = RetroArchProfile::new(dirs.retroarch());
    let input = controls::game_input(system, game, kind, controls_dir, device, &retroarch, pad);
    print!("{}", input.describe(game));
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
