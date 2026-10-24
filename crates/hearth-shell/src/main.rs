//! Hearth shell: the Slint UI and the glue to core and platform services.

mod bridge;
mod callouts;
mod controller_layout;
mod controls;
mod diagram;
mod guard;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use hearth_audio::{Audio, AudioOptions, Backend, Volumes};
use hearth_core::art::{
    ArtCache, DisplayClass, MetadataProvider, MockMetadataProvider, SteamGridDb,
};
use hearth_core::config::{self, Config};
use hearth_core::custom::CustomStore;
use hearth_core::packs::Resolver;
use hearth_core::paths::{self, DataDirs};
use hearth_core::profiles::{DolphinInstall, DolphinProfile, RetroArchProfile};
use hearth_core::systems::Systems;
use hearth_platform::gamescope::{self, Compositor};
use hearth_platform::input::dev_socket::DevSocketBackend;
use hearth_platform::input::gilrs::GilrsBackend;
use hearth_platform::input::inputplumber::InputPlumberBackend;
use hearth_platform::input::{Action, InputBackend, InputControl, InputError, PadActivity};
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
    /// Where controller input comes from: InputPlumber (hides Hearth's menu input from games,
    /// remaps live), gilrs (reads controllers directly), or auto (InputPlumber when it runs
    /// and allows Hearth to use it, else gilrs).
    #[arg(long, value_enum, default_value_t = InputChoice::Auto)]
    input: InputChoice,
    /// Ignore physical controllers: no controller backend at all (InputPlumber is left alone,
    /// gilrs isn't opened), so only the keyboard and --dev-socket drive Hearth. For scripted
    /// runs on a machine whose controllers are in use.
    #[arg(long)]
    no_pad_input: bool,
    /// Never use the network for artwork: a mock provider that is always offline, so games
    /// keep cached art or placeholders and wait in the needs-art queue.
    #[arg(long)]
    mock_network: bool,
    /// A fake SteamGridDB: `fixtures` matches every game and offers several generated images
    /// of each kind. Only with a throwaway HEARTH_HOME (one holding a `.throwaway` file, as
    /// made by scripts/test-home.sh), so fixture art never mixes with real art.
    #[arg(long, value_enum, value_name = "MOCK")]
    mock_sgdb: Option<MockSgdb>,
    /// No sound output (tests, headless runs); the audio engine still runs. Same as
    /// HEARTH_AUDIO=null.
    #[arg(long)]
    no_audio: bool,
}

/// Hearth's bundled assets (default console icons, sounds). TODO: an installed location
/// once Hearth is packaged.
fn assets_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"))
}

/// `HEARTH_HOME` is set and marked as disposable test data.
fn is_throwaway_home() -> bool {
    std::env::var_os(paths::HEARTH_HOME_ENV)
        .is_some_and(|home| PathBuf::from(home).join(".throwaway").is_file())
}

/// Fake SteamGridDB providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum MockSgdb {
    /// Generated candidates for every game.
    Fixtures,
}

/// The artwork provider: SteamGridDB with an API key, a mock with `--mock-network` or
/// `--mock-sgdb`, else none (placeholders, logged once by the art worker).
fn art_provider(
    mock_network: bool,
    mock_sgdb: Option<MockSgdb>,
    config: &Config,
) -> Option<Box<dyn MetadataProvider>> {
    if mock_sgdb == Some(MockSgdb::Fixtures) {
        tracing::info!("--mock-sgdb=fixtures: generated artwork candidates");
        return Some(Box::new(MockMetadataProvider::fixtures()));
    }
    if mock_network {
        tracing::info!("--mock-network: artwork comes from the cache only");
        return Some(Box::new(MockMetadataProvider::offline()));
    }
    let key = config.steamgriddb_key(std::env::var(config::STEAMGRIDDB_KEY_ENV).ok())?;
    Some(Box::new(SteamGridDb::new(key)))
}

/// The audio engine: the default output device, or nothing with `--no-audio` or
/// `HEARTH_AUDIO=null`. `HEARTH_AUDIO_STATS=1` logs input-to-sound times.
fn start_audio(no_audio: bool, config: &Config) -> Audio {
    let null = no_audio || std::env::var("HEARTH_AUDIO").is_ok_and(|v| v == "null");
    let stats = std::env::var("HEARTH_AUDIO_STATS").is_ok_and(|v| v == "1");
    if null {
        tracing::info!("audio: null output");
    }
    let audio = Audio::start(AudioOptions {
        backend: if null { Backend::Null } else { Backend::Device },
        stats,
    });
    let settings = config.audio();
    audio.set_volumes(Volumes {
        ui: settings.ui_volume,
        music: settings.music_volume,
        mute: settings.mute,
    });
    audio
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
    /// With InputPlumber: gilrs watching the virtual pad, for the button test's raw values.
    observer: Option<GilrsBackend>,
}

/// `--no-pad-input`: a backend that never sends anything.
struct NoPads;

impl InputBackend for NoPads {
    fn start(&mut self, _tx: std::sync::mpsc::Sender<Action>) -> Result<(), InputError> {
        Ok(())
    }
}

/// Picks the controller backend. `auto` falls back to gilrs and says why.
fn select_input(choice: InputChoice, runtime_dir: &std::path::Path) -> anyhow::Result<Input> {
    let gilrs = || {
        let backend = GilrsBackend::new();
        Input {
            activity: backend.activity(),
            control: InputControl::detached(),
            backend: Box::new(backend),
            observer: None,
        }
    };
    let inputplumber = || {
        InputPlumberBackend::connect(&runtime_dir.join("inputplumber")).map(|backend| Input {
            activity: backend.activity(),
            control: backend.control(),
            backend: Box::new(backend),
            observer: Some(GilrsBackend::observer()),
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
    bridge::startup::start();
    // Before any thread exists, so every thread inherits the mask and only the signal thread
    // receives these.
    let shutdown_signals = block_shutdown_signals()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args = Args::parse();
    bridge::frames::init();

    let dirs = DataDirs::from_env().context("resolving data directories")?;
    if args.mock_sgdb.is_some() && !is_throwaway_home() {
        anyhow::bail!(
            "--mock-sgdb needs a throwaway HEARTH_HOME (with a .throwaway file; see \
             scripts/test-home.sh), so fixture art never lands in real data"
        );
    }
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
    bridge::startup::install(&app);
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

    // A broken file is left as it is: Hearth runs on defaults and says so on screen. The
    // error never quotes the file (it may hold the API key).
    let (config, config_problem) = match Config::load(&dirs.config_file()) {
        Ok(config) => (config, None),
        Err(err) => {
            tracing::warn!("ignoring the config file: {err}");
            (Config::default(), Some(config_notice(&err)))
        }
    };
    let icons = bridge::art::ConsoleIcons::new(Resolver::new(dirs.packs(), assets_dir(), &config));

    let audio = start_audio(args.no_audio, &config);
    let audio_handle = audio.clone();
    bridge::sounds::install(
        audio.clone(),
        Resolver::new(dirs.packs(), assets_dir(), &config),
    );
    app.global::<Feedback>().on_input(bridge::sounds::on_input);
    if let Some(overlay) = &overlay {
        overlay
            .global::<Feedback>()
            .on_input(bridge::sounds::on_input);
    }

    let input = if args.no_pad_input {
        tracing::info!("--no-pad-input: physical controllers are ignored");
        Input {
            activity: PadActivity::new(),
            control: InputControl::detached(),
            backend: Box::new(NoPads),
            observer: None,
        }
    } else {
        select_input(args.input, dirs.runtime())?
    };
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
        raw_activity: input.observer.as_ref().map(GilrsBackend::activity),
        input: input.control.clone(),
        library_db: dirs.library_db(),
        games_dir,
        art_cache: ArtCache::new(dirs.art_cache()),
        picks: CustomStore::new(dirs.game_data()),
        audio_cache: dirs.cache().join("audio"),
        display_class: std::fs::read_to_string(display_class_file(&dirs))
            .ok()
            .and_then(|name| DisplayClass::from_name(&name))
            .unwrap_or_default(),
        display_class_file: display_class_file(&dirs),
        art_provider: art_provider(args.mock_network, args.mock_sgdb, &config),
        icons,
        config_file: dirs.config_file(),
        config_problem,
        packs_dir: dirs.packs().to_path_buf(),
        audio,
        audio_settings: config.audio(),
        display_settings: config.display(),
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
    if let Some(observer) = input.observer {
        backends.push(Box::new(observer));
    }
    if let Some(path) = args.dev_socket {
        let socket = DevSocketBackend::new(path).with_commands(bridge::session::dev_commands(
            bridge::sounds::dev_commands(audio_handle),
        ));
        backends.push(Box::new(socket));
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

/// The on-screen notice for a config file Hearth couldn't read: title and explanation.
fn config_notice(err: &config::ConfigError) -> (String, String) {
    let place = err
        .line()
        .map_or_else(String::new, |line| format!(" (line {line})"));
    (
        format!("Couldn't read hearth.toml{place}"),
        "Using default settings".to_owned(),
    )
}

/// Where the screen class of the last run is remembered (display-sized art).
fn display_class_file(dirs: &DataDirs) -> std::path::PathBuf {
    dirs.data().join("display-class")
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
    use chrono::Timelike;
    let local = chrono::Local::now();
    let now = local.format("%-I:%M %p").to_string();
    let status = app.global::<Status>();
    status.set_clock(now.as_str().into());
    status.set_greeting(bridge::models::greeting(local.hour()).into());
    bridge::session::tick(&now);
}
