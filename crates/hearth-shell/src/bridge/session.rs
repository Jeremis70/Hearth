//! Game sessions: launching, the in-game overlay, going Home, resuming and closing. Ties the
//! runtime, gamescope and the UI together. Lives on the Slint thread.
//!
//! Inside gamescope:
//! - Guide short press while playing: freeze the game, show the overlay over it.
//! - Guide long press while playing: freeze the game, show Hearth's Home.
//! - Resume (overlay, Home tile, Guide again in the overlay): unfreeze, show the game. Menu
//!   buttons act on release, so the press that resumed the game is already over.
//!
//! Outside gamescope (or in gamescope without Steam mode, where focus can't be switched) the
//! game keeps the screen, there is no overlay, and a long press closes the game.
//!
//! Whatever fails (window never appears, focus switch refused), Hearth comes back in front, the
//! game keeps running (never left frozen) and the Guide button keeps working.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hearth_core::art::{
    ArtCache, ArtKind, ArtService, DisplayClass, MetadataProvider, remove_orphaned_art,
};
use hearth_core::config::{self, PackCategory};
use hearth_core::controls::edit::{Editor, Scope};
use hearth_core::controls::{ControllerKind, GameKey, HearthAction, PadInput, Target, defaults};
use hearth_core::custom::CustomStore;
use hearth_core::library::{Library, LibraryEntry};
use hearth_core::model::{Game, GameId, SystemId};
use hearth_core::packs;
use hearth_core::profiles::{DolphinProfile, RetroArchProfile};
use hearth_core::runtime::{
    LaunchCtx, ProcessRuntime, Runtime, RuntimeEvent, Session as RuntimeSession, SessionId,
};
use hearth_core::scan::{root_is_available, scan_roms};
use hearth_core::session::{ActiveSession, SessionState};
use hearth_core::systems::{EmulatorKind, SuspendPolicy, Systems};
use hearth_platform::gamescope::{AppId, Compositor, SHELL_APP_ID};
use hearth_platform::input::{
    Action, InputControl, InputMode, InputStatus, PadActivity, PadInfo, Power,
};
use slint::{
    ComponentHandle, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use std::rc::Rc;

use super::art::{self as art_ui, ArtLoader, ArtMap, ConsoleIcons};

mod customize;

use super::controls_page::{
    self, ApplyOutcome, EmulatorConfigApplier, InputPlumberApplier, MappingApplier, Page, TestPage,
};
use super::models;
use super::sounds;
use crate::controls;
use crate::{
    AppWindow, AudioRow, ControllerType, ControlsData, GameProperties, HomeData, HomeTile,
    OverlayPanel, OverlayWindow, PadNotice, PadRow, Session, SettingsCategory, SettingsData,
    Status, Theme, TileKind,
};
pub use customize::dev_commands;
use hearth_audio::director::{self, GameScene, MusicDirector, Scene};
use hearth_audio::{Audio, SoundFile, Volumes};
use hearth_core::config::{AudioSettings, DisplaySettings};
use hearth_core::extras::{Extra, Extras};

/// When the controls page's drawing is rasterized ahead of use (the window has its size).
const DIAGRAM_WARM_UP: Duration = Duration::from_secs(6);

/// How long Home's focus must stay on a game before its theme plays (or stops).
const THEME_SETTLE: Duration = Duration::from_millis(500);

/// How long a problem notice (e.g. hearth.toml unreadable) stays on screen.
const PROBLEM_TIME: Duration = Duration::from_secs(20);

/// How long to look for a game's window before giving up.
const WINDOW_SEARCH: Duration = Duration::from_secs(15);
const WINDOW_POLL: Duration = Duration::from_millis(250);
/// Windows found later (e.g. re-created for fullscreen) are tagged at this interval.
const RETAG_INTERVAL: Duration = Duration::from_secs(1);
/// How often to check (and re-request) that gamescope shows the game.
const FOCUS_POLL: Duration = Duration::from_millis(250);
/// Give up switching to the game after this long.
const FOCUS_RETRY: Duration = Duration::from_secs(3);
const TOAST_TIME: Duration = Duration::from_secs(5);
/// How long a notice stays over a running game.
const NOTICE_TIME: Duration = Duration::from_secs(3);
/// InputPlumber went away (the game lost its virtual pad).
const SERVICE_LOST: &str = "Controller service restarting…";
/// It stayed away.
const SERVICE_STILL_LOST: &str = "The controller service stopped: the game can't read the \
controller until it's back. Hearth's menus still work";
/// It came back after the game was left frozen.
const SERVICE_BACK: &str = "Controller service is back. Resume when you're ready";
/// Back this fast: a restart; the game resumes on its own.
const QUICK_RESTART: Duration = Duration::from_secs(2);
/// The controller driving the game went away.
const PAD_LOST: &str = "Reconnect your controller";
/// It is back, the game still frozen.
const PAD_BACK: &str = "Controller connected. Resume when you're ready";
/// How often controllers are checked for coming and going.
const PAD_POLL: Duration = Duration::from_millis(250);
/// How long a controller notice stays.
const PAD_NOTICE_TIME: Duration = Duration::from_secs(4);
/// Controllers coming and going this long after startup or a controller service change are
/// that, not the player's doing: no notices.
const PAD_SETTLE: Duration = Duration::from_secs(5);
/// Battery levels that warn, once each per charge.
const BATTERY_WARNINGS: [u8; 2] = [20, 10];
/// What the Capture button does for now.
const CAPTURE_MESSAGE: &str = "Screenshots and clips are coming later";
/// First app id given to games (Hearth is 769).
const FIRST_GAME_APP_ID: AppId = 10_000;

thread_local! {
    static CONTROLLER: RefCell<Option<Controller>> = const { RefCell::new(None) };
}

/// Runs `f` on the controller. Returns `None` if there is none or it is busy (re-entrant call;
/// use [`defer`] from callbacks).
fn with<R>(f: impl FnOnce(&mut Controller) -> R) -> Option<R> {
    CONTROLLER.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => {
            tracing::error!("session controller re-entered; use defer()");
            None
        }
    })
}

/// Runs `f` on the controller from the event loop, after the current callback returns.
/// Safe to call from Slint callbacks and from any thread.
pub fn defer(f: impl FnOnce(&mut Controller) + Send + 'static) {
    if let Err(err) = slint::invoke_from_event_loop(move || {
        with(f);
    }) {
        tracing::warn!("event loop gone: {err}");
    }
}

/// What the UI is doing with the controller's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Hearth's main window is in front (a game may be running in the background).
    Shell,
    /// A game was started and its window hasn't been shown yet; Hearth stays in front.
    Launching,
    /// The game is in front; only the Guide button is ours.
    Game,
    /// The overlay is shown over the frozen game.
    Overlay,
}

/// Where a controller action goes.
pub enum Route {
    /// Inject into the main window.
    Main(AppWindow),
    /// Inject into the overlay window.
    Overlay(OverlayWindow),
    /// Handled or dropped.
    None,
}

/// Owns the current session and everything around it.
pub struct Controller {
    app: slint::Weak<AppWindow>,
    overlay: Option<OverlayWindow>,
    overlay_window: Option<u32>,
    compositor: Arc<dyn Compositor>,
    runtime: Arc<ProcessRuntime>,
    systems: Systems,
    profile: RetroArchProfile,
    dolphin: DolphinProfile,
    controls_dir: PathBuf,
    activity: PadActivity,
    raw_activity: Option<PadActivity>,
    input: InputControl,
    library_db: PathBuf,
    entries: Vec<LibraryEntry>,
    /// Decoded art per game, filled in by the art loader.
    art: ArtMap,
    /// Images the loader has delivered (startup timing).
    art_delivered: std::collections::HashSet<(GameId, ArtKind)>,
    art_loader: ArtLoader,
    /// Where the screen class is remembered, and a new class seen at the last tick.
    display_class_file: PathBuf,
    class_candidate: Option<DisplayClass>,
    icons: ConsoleIcons,
    /// Hearth's settings file (pack choices).
    config_file: PathBuf,
    /// Installed asset packs.
    packs_dir: PathBuf,
    active: Option<ActiveSession>,
    closing: bool,
    /// The game the switch dialog offers to start (waiting for the user).
    switch_to: Option<GameId>,
    /// The game to start once the running one has ended (the user confirmed the switch).
    launch_after_close: Option<GameId>,
    mode: Mode,
    events: Sender<RuntimeEvent>,
    next_app_id: AppId,
    toast_timer: slint::Timer,
    /// Hides the problem notice (NoticeCard).
    problem_timer: slint::Timer,
    notice_timer: slint::Timer,
    /// The running game's Capture buttons (Hearth pad position names).
    game_capture: Vec<&'static str>,
    focus_timer: slint::Timer,
    /// Open controls pages: [main window, overlay].
    pages: [Option<Page>; 2],
    applier: Box<dyn MappingApplier>,
    message_timer: slint::Timer,
    /// The button test is open (it takes all input; a long Guide press closes it).
    testing: bool,
    /// When InputPlumber went away, and whether Hearth froze the game for it.
    service_lost: Option<(Instant, bool)>,
    /// Controllers as last seen, to notice them coming and going.
    known_pads: Vec<PadInfo>,
    /// Whether the UI shows the pads (`Some`), and as "still looking" (`Some(true)`).
    pads_shown: Option<bool>,
    /// No controller notices before this (startup, controller service changes).
    pads_settle: Instant,
    /// The controller driving the running game (player 1 when it started).
    game_pad: Option<String>,
    /// Battery warnings given since each controller's last charge (by pad id).
    battery_warned: std::collections::HashMap<String, Vec<u8>>,
    pad_timer: slint::Timer,
    pad_notice_timer: slint::Timer,
    test_timer: slint::Timer,
    audio: Audio,
    music: MusicDirector,
    audio_settings: AudioSettings,
    /// Animated art and Reduce motion (`[display]`).
    display_settings: DisplaySettings,
    /// The ambient loop of the selected sound pack (else Hearth's).
    ambient: Option<SoundFile>,
    /// A game's own extras (its theme).
    extras: Extras,
    /// The theme of the game focused on Home, once focus settled there.
    focused_theme: Option<SoundFile>,
    /// Waits for focus to settle before the theme changes.
    theme_timer: slint::Timer,
    /// What the user picked for each game (read here, written by the art worker).
    picks: CustomStore,
    /// Cached art (sources and candidates for the Customize page).
    art_cache: ArtCache,
    /// Where theme analyses are cached.
    audio_cache: PathBuf,
    /// The art worker, for the Customize page's requests.
    art_service: Option<ArtService>,
    /// Each game's art files (the art worker's last report).
    art_paths: std::collections::HashMap<GameId, hearth_core::art::GameArt>,
    /// The Customize page, while open.
    customize: Option<customize::CustomizeState>,
    /// A theme previewed on the Customize page (wins over Home's music).
    preview: Option<SoundFile>,
    /// Waits for focus to settle on an audio file before previewing it.
    preview_timer: slint::Timer,
}

/// Which window a controls page is in.
const MAIN: usize = 0;
const OVERLAY: usize = 1;
/// How long a controls page's feedback line stays.
const MESSAGE_TIME: Duration = Duration::from_secs(4);
/// Button test refresh interval.
const TEST_POLL: Duration = Duration::from_millis(33);

/// Everything the controller needs at startup.
pub struct Setup {
    /// Main window.
    pub app: slint::Weak<AppWindow>,
    /// Overlay window (only inside gamescope).
    pub overlay: Option<OverlayWindow>,
    /// Compositor (no-op outside gamescope).
    pub compositor: Arc<dyn Compositor>,
    /// Known systems.
    pub systems: Systems,
    /// RetroArch profile.
    pub profile: RetroArchProfile,
    /// Dolphin profile.
    pub dolphin: DolphinProfile,
    /// The user's controls files (`<config>/controls`).
    pub controls_dir: PathBuf,
    /// Files read by other processes (the game's InputPlumber profile).
    pub runtime_dir: PathBuf,
    /// Live controller state: connected pads, disarming held buttons.
    pub activity: PadActivity,
    /// With InputPlumber: the virtual pad as gilrs sees it (raw stick and trigger values for
    /// the button test).
    pub raw_activity: Option<PadActivity>,
    /// Who gets the controller (InputPlumber), and the game's remapping.
    pub input: InputControl,
    /// Library database file.
    pub library_db: PathBuf,
    /// Root scanned for ROMs (read-only).
    pub games_dir: PathBuf,
    /// What the user picked for each game.
    pub picks: CustomStore,
    /// Cached art.
    pub art_cache: ArtCache,
    /// Cached theme analyses (`<cache>/audio`).
    pub audio_cache: PathBuf,
    /// Screen class remembered from the last run, and its file.
    pub display_class: DisplayClass,
    pub display_class_file: PathBuf,
    /// Where art comes from; `None` without an API key.
    pub art_provider: Option<Box<dyn MetadataProvider>>,
    /// Console icons (pack, then Hearth's glyphs).
    pub icons: ConsoleIcons,
    /// Hearth's settings file.
    pub config_file: PathBuf,
    /// Title and text of a notice about that file (it couldn't be read).
    pub config_problem: Option<(String, String)>,
    /// The audio engine (UI sounds are in `sounds`; music is the session's).
    pub audio: Audio,
    /// Volumes and music options from `hearth.toml`.
    pub audio_settings: AudioSettings,
    /// Animated art and Reduce motion from `hearth.toml`.
    pub display_settings: DisplaySettings,
    /// Installed asset packs.
    pub packs_dir: PathBuf,
}

/// Creates the controller, wires the UI callbacks and starts the library scan.
pub fn install(setup: Setup) {
    let (events, rx) = mpsc::channel::<RuntimeEvent>();
    let spawned = thread::Builder::new()
        .name("hearth-session-events".into())
        .spawn(move || {
            for event in rx {
                defer(move |c| c.on_runtime_event(event));
            }
        });
    if let Err(err) = spawned {
        tracing::error!("cannot start session event thread: {err}");
    }

    if let Some(app) = setup.app.upgrade() {
        let session = app.global::<Session>();
        session.on_launch(|id| {
            let id = GameId::new(id.as_str());
            defer(move |c| c.launch(&id));
        });
        session.on_resume(|| defer(Controller::resume_game));
        session.on_close_game(|| defer(Controller::close_game));
        session.on_confirm_switch(|| defer(Controller::confirm_switch));
        session.on_home_button(|long| defer(move |c| c.home_button(long)));
        session.on_open_controls(|| defer(|c| c.open_game_controls(MAIN)));
        let home = app.global::<HomeData>();
        home.on_focus_changed(|index| {
            let index = usize::try_from(index).unwrap_or(0);
            defer(move |c| c.art_loader.set_focus(index));
        });
        home.on_music_focus_changed(|id| {
            let id = (!id.is_empty()).then(|| GameId::new(id.as_str()));
            defer(move |c| c.music_focus_changed(id));
        });
        home.on_toggle_favorite(|id| {
            let id = GameId::new(id.as_str());
            defer(move |c| c.toggle_favorite(&id));
        });
        home.on_open_game_controls(|id| {
            let id = GameId::new(id.as_str());
            defer(move |c| c.open_controls_for(&id));
        });
        // Synchronous: the panel shows the values right away. Never re-entrant (Slint calls it
        // while handling a key, after `route` returned).
        home.on_request_properties(|id| {
            with(|c| c.fill_properties(&GameId::new(id.as_str())));
        });
        home.on_customize(|id, section| {
            let (id, section) = (GameId::new(id.as_str()), section.to_string());
            defer(move |c| c.open_customize(&id, &section));
        });
        customize::wire(&app);
        let settings = app.global::<SettingsData>();
        settings.on_controller_item(|key| {
            let key = key.to_string();
            defer(move |c| c.settings_controller_item(&key));
        });
        settings.on_audio_change(|key, step| {
            let key = key.to_string();
            defer(move |c| c.audio_change(&key, step));
        });
        // Synchronous, like request-properties: the list is shown right after.
        settings.on_open_packs(|key| {
            with(|c| c.fill_pack_choices(&key));
        });
        settings.on_choose_pack(|key, id| {
            let (key, id) = (key.to_string(), id.to_string());
            defer(move |c| c.choose_pack(&key, &id));
        });
        wire_controls(&app.global::<ControlsData>(), MAIN);
    }
    if let Some(overlay) = &setup.overlay {
        overlay.on_resume(|| defer(Controller::resume_game));
        overlay.on_go_home(|| defer(Controller::go_home));
        overlay.on_close_game(|| defer(Controller::close_game));
        overlay.on_home_button(|long| defer(move |c| c.home_button(long)));
        overlay.on_dock(|item| {
            if item == "Controls" {
                defer(|c| c.open_game_controls(OVERLAY));
            } else {
                tracing::info!(%item, "overlay dock item not implemented yet");
            }
        });
        wire_controls(&overlay.global::<ControlsData>(), OVERLAY);
    }

    if !setup.profile.has_autoconfig() {
        tracing::warn!(
            dir = %setup.profile.autoconfig_dir().display(),
            "no RetroArch joypad profiles installed: controllers won't work in games \
             (install retroarch-autoconfig-udev-git)"
        );
    }

    let art_loader = art_ui::ArtLoader::start(&setup.art_cache, setup.display_class);
    let art_service = art_ui::start(
        setup.art_cache.clone(),
        setup.picks.clone(),
        setup.library_db.clone(),
        setup.art_provider,
        setup.display_settings.animated_art,
        art_loader.clone(),
    );
    load_library(LibraryLoad {
        db: setup.library_db.clone(),
        games_dir: setup.games_dir.clone(),
        systems: setup.systems.clone(),
        art_cache: setup.art_cache.clone(),
        art: art_service.clone(),
    });

    let applier: Box<dyn MappingApplier> = if setup.input.is_live() {
        Box::new(InputPlumberApplier {
            path: setup.runtime_dir.join("inputplumber").join("game.yaml"),
            input: setup.input.clone(),
        })
    } else {
        Box::new(EmulatorConfigApplier)
    };
    setup
        .input
        .on_status(|status| defer(move |c| c.on_input_status(status)));
    let controller = Controller {
        app: setup.app,
        overlay: setup.overlay,
        overlay_window: None,
        compositor: setup.compositor,
        runtime: Arc::new(ProcessRuntime::new()),
        systems: setup.systems,
        profile: setup.profile,
        dolphin: setup.dolphin,
        controls_dir: setup.controls_dir,
        activity: setup.activity,
        raw_activity: setup.raw_activity,
        input: setup.input,
        library_db: setup.library_db,
        entries: Vec::new(),
        art: ArtMap::new(),
        art_delivered: std::collections::HashSet::new(),
        art_loader,
        display_class_file: setup.display_class_file,
        class_candidate: None,
        icons: setup.icons,
        config_file: setup.config_file,
        packs_dir: setup.packs_dir,
        active: None,
        closing: false,
        switch_to: None,
        launch_after_close: None,
        mode: Mode::Shell,
        events,
        next_app_id: FIRST_GAME_APP_ID,
        toast_timer: slint::Timer::default(),
        problem_timer: slint::Timer::default(),
        notice_timer: slint::Timer::default(),
        game_capture: Vec::new(),
        focus_timer: slint::Timer::default(),
        pages: [None, None],
        applier,
        message_timer: slint::Timer::default(),
        testing: false,
        test_timer: slint::Timer::default(),
        service_lost: None,
        known_pads: Vec::new(),
        pads_shown: None,
        pads_settle: Instant::now() + PAD_SETTLE,
        game_pad: None,
        battery_warned: std::collections::HashMap::new(),
        pad_timer: slint::Timer::default(),
        pad_notice_timer: slint::Timer::default(),
        audio: setup.audio,
        music: MusicDirector::default(),
        audio_settings: setup.audio_settings,
        display_settings: setup.display_settings,
        ambient: sounds::music_file("ambient_loop"),
        extras: Extras::standard(setup.picks.clone()),
        focused_theme: None,
        theme_timer: slint::Timer::default(),
        picks: setup.picks,
        art_cache: setup.art_cache,
        audio_cache: setup.audio_cache,
        art_service,
        art_paths: std::collections::HashMap::new(),
        customize: None,
        preview: None,
        preview_timer: slint::Timer::default(),
    };
    CONTROLLER.with(|cell| *cell.borrow_mut() = Some(controller));
    with(Controller::fill_display_items);
    with(Controller::fill_audio_rows);
    with(Controller::apply_motion);
    with(|c| c.sync_music(director::FADE_IN));
    if let Some((title, text)) = setup.config_problem {
        with(|c| c.show_problem(&title, &text));
    }
    with(|c| {
        c.pad_timer.start(slint::TimerMode::Repeated, PAD_POLL, || {
            with(Controller::poll_pads);
        });
    });
    // Prepare the controls pages on workers once the window has its size and the controllers
    // are known (InputPlumber takes a few seconds), so the first page opens at once.
    slint::Timer::single_shot(DIAGRAM_WARM_UP, || {
        with(Controller::warm_up_controls);
    });
}

/// Connects a window's controls page callbacks to the controller.
fn wire_controls(data: &ControlsData<'_>, which: usize) {
    // Synchronous: the page needs the current choice's index right away. Never re-entrant
    // (Slint calls it while handling a key, after `route` returned).
    data.on_request_choices(move |input| with(|c| c.controls_choices(which, &input)).unwrap_or(0));
    data.on_choose(move |input, id| {
        let (input, id) = (input.to_string(), id.to_string());
        defer(move |c| c.controls_choose(which, &input, &id));
    });
    data.on_change_option(move |id, step| {
        let id = id.to_string();
        defer(move |c| c.controls_change_option(which, &id, step));
    });
    data.on_closed(move || defer(move |c| c.controls_closed(which)));
}

/// Closes the running game, if any, before Hearth exits (blocks a few seconds at most).
pub fn shutdown() {
    with(Controller::shutdown);
}

/// Updates time-based text; call every second.
pub fn tick(clock: &str) {
    with(|c| c.tick(clock));
}

/// Decides where a controller action goes; Guide presses are handled right here.
pub fn route(action: Action) -> Route {
    with(|c| c.route(action)).unwrap_or(Route::None)
}

/// What the library thread needs.
struct LibraryLoad {
    db: PathBuf,
    games_dir: PathBuf,
    systems: Systems,
    art_cache: ArtCache,
    art: Option<ArtService>,
}

/// Scans the games root and loads the library on a worker thread, then hands the games to the
/// art worker. A root that can't be read (missing, unmounted) leaves the library as it was:
/// no game, history or art is dropped for it.
fn load_library(load: LibraryLoad) {
    let spawned = thread::Builder::new()
        .name("hearth-library".into())
        .spawn(move || {
            let result = Library::open(&load.db).and_then(|mut library| {
                if root_is_available(&load.games_dir) {
                    let games = scan_roms(&load.games_dir, &load.systems);
                    tracing::info!(count = games.len(), root = %load.games_dir.display(), "scanned games");
                    let removed = library.sync(&games)?;
                    remove_orphaned_art(&library, &load.art_cache, &removed, &load.games_dir);
                } else {
                    tracing::warn!(
                        root = %load.games_dir.display(),
                        "games folder unavailable; showing the library as last scanned"
                    );
                }
                library.recent()
            });
            match result {
                Ok(entries) => {
                    super::startup::note("library loaded");
                    if let Some(art) = &load.art {
                        art.library_loaded(entries.iter().map(|e| e.game.clone()).collect());
                    }
                    defer(move |c| c.set_entries(entries));
                }
                Err(err) => {
                    tracing::warn!("loading the library failed: {err}");
                    defer(move |c| c.toast(&format!("Couldn't load the library: {err}")));
                }
            }
        });
    if let Err(err) = spawned {
        tracing::error!("cannot start library thread: {err}");
    }
}

/// The battery warning due at `percent`, given the levels already `warned` about since the
/// last charge (which it adds to). The lowest level reached wins, so a pad found at 5 % warns
/// once, not twice.
fn battery_warning(percent: u8, warned: &mut Vec<u8>) -> Option<u8> {
    let level = BATTERY_WARNINGS
        .into_iter()
        .filter(|level| percent <= *level)
        .min()
        .filter(|level| !warned.contains(level))?;
    warned.extend(BATTERY_WARNINGS.iter().filter(|l| **l >= level));
    Some(level)
}

/// `ControllerType.family` in `ui/components/glyphs.slint`.
fn glyph_family(kind: ControllerKind) -> i32 {
    match kind {
        ControllerKind::DualSense | ControllerKind::DualShock4 => 1,
        ControllerKind::Xbox => 2,
        ControllerKind::SwitchPro => 3,
        ControllerKind::Generic => 0,
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

impl Controller {
    /// Called once the overlay window's X11 id is known.
    pub fn set_overlay_window(&mut self, window: u32) {
        self.overlay_window = Some(window);
        if let Err(err) = self.compositor.set_overlay(window, false) {
            tracing::warn!("preparing the overlay window failed: {err}");
        }
        tracing::info!(window, "overlay window ready");
    }

    /// Updates time-based text (session time, overlay clock). Called every second.
    pub fn tick(&mut self, clock: &str) {
        if let Some(overlay) = &self.overlay {
            overlay.set_clock(clock.into());
        }
        self.sync_controller_type();
        self.sync_capture();
        self.check_display_class();
        self.refresh_settings();
        self.sync_session_ui();
    }

    fn route(&mut self, action: Action) -> Route {
        if self.testing {
            if action == Action::HomeLong {
                self.stop_button_test();
            }
            return Route::None;
        }
        match action {
            Action::HomeShort => {
                self.home_button(false);
                Route::None
            }
            Action::Capture => {
                self.capture();
                Route::None
            }
            Action::HomeLong => {
                self.home_button(true);
                Route::None
            }
            _ => match self.mode {
                Mode::Shell => self.app.upgrade().map_or(Route::None, Route::Main),
                Mode::Overlay => self
                    .overlay
                    .as_ref()
                    .map_or(Route::None, |o| Route::Overlay(o.clone_strong())),
                // The game owns the controller (with gilrs, the game sees these presses too).
                Mode::Game | Mode::Launching => Route::None,
            },
        }
    }

    /// Switches what has the controller, and with it which buttons are Capture: the game's
    /// mapping while a game is in front, the controller type's default in Hearth's menus.
    ///
    /// With InputPlumber the controller follows: the game only gets input while it is in front.
    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.input.set_mode(match mode {
            Mode::Game => InputMode::Game,
            Mode::Shell | Mode::Launching | Mode::Overlay => InputMode::Shell,
        });
        self.sync_capture();
        // Every mode change follows the freezing or thawing, so the music sees the real state.
        // The fades run in parallel: a launch or Resume never waits for them.
        self.sync_music(match mode {
            Mode::Game => director::FADE_RESUME,
            Mode::Shell | Mode::Launching | Mode::Overlay => director::FADE_LAUNCH,
        });
    }

    // --- music -------------------------------------------------------------------------

    /// The game's theme, looped automatically from where it starts (plus its "Start at").
    fn theme_of(&self, game: &Game) -> Option<SoundFile> {
        let theme = self.extras.find(game, Extra::Theme)?;
        let start = self.picks.load(&self.picks.key_of(game)).theme_start();
        Some(self.auto_looped(theme.path, theme.source, start))
    }

    /// A music file that loops by itself (`hearth_audio::analyze`), its first play starting
    /// `start` seconds after its first sound.
    fn auto_looped(&self, path: PathBuf, origin: &str, start: f32) -> SoundFile {
        SoundFile {
            path,
            origin: origin.to_owned(),
            loop_spec: None,
            auto_loop: Some(hearth_audio::AutoLoop {
                offset_ms: (start.max(0.0) * 1000.0).round() as u64,
                cache_dir: Some(self.audio_cache.clone()),
            }),
            gain_db: 0,
        }
    }

    /// Home's focused game changed (`None`: no game, or Home isn't showing). The theme follows
    /// once focus has stayed put for [`THEME_SETTLE`], so scrolling past games doesn't start
    /// and stop music; leaving Home goes back to the ambient at once.
    fn music_focus_changed(&mut self, id: Option<GameId>) {
        self.theme_timer.stop();
        let Some(id) = id else {
            // The Customize page keeps its game's theme.
            if self.customize.is_some() {
                self.keep_theme();
            } else if self.focused_theme.take().is_some() {
                self.sync_music(director::FADE_LAUNCH);
            }
            return;
        };
        self.theme_timer
            .start(slint::TimerMode::SingleShot, THEME_SETTLE, move || {
                with(|c| {
                    let theme = c
                        .entries
                        .iter()
                        .find(|e| e.game.id == id)
                        .and_then(|e| c.theme_of(&e.game));
                    if theme != c.focused_theme {
                        c.focused_theme = theme;
                        c.sync_music(director::FADE_LAUNCH);
                    }
                });
            });
    }

    /// Tells the music director what Hearth shows now (see `hearth_audio::director`).
    fn sync_music(&mut self, silence_fade: Duration) {
        let game = match &self.active {
            None => GameScene::None,
            Some(active) if active.frozen => GameScene::Frozen,
            Some(_) => GameScene::Running,
        };
        let scene = Scene {
            game,
            overlay: self.mode == Mode::Overlay,
            ambient: self.ambient.as_ref(),
            focused_theme: self.focused_theme.as_ref(),
            preview: self.preview.as_ref(),
            music_in_overlay: self.audio_settings.music_in_overlay,
            game_themes: self.audio_settings.game_themes,
        };
        self.music.update(&self.audio, &scene, silence_fade);
    }

    /// Tells the input thread which buttons are Capture now. Also called every second, as
    /// controllers come and go.
    fn sync_capture(&self) {
        let capture = match self.mode {
            Mode::Game | Mode::Launching => self.game_capture.clone(),
            Mode::Shell | Mode::Overlay => defaults::pad_hearth_binds(self.connected_kind())
                .into_iter()
                .filter(|(_, action)| *action == HearthAction::Capture)
                .map(|(input, _)| input.name())
                .collect(),
        };
        if capture != self.activity.capture_inputs() {
            tracing::info!(mode = ?self.mode, ?capture, "capture buttons");
            self.activity.set_capture_inputs(capture);
        }
    }

    /// The Capture button. A notice for now (screenshots and clips come later; see
    /// docs/design/overlay.md). Over a running game it shows without pausing it.
    fn capture(&mut self) {
        tracing::info!(mode = ?self.mode, "capture button");
        match self.mode {
            Mode::Game if self.compositor.controls_focus() => self.show_notice(CAPTURE_MESSAGE),
            Mode::Overlay => {
                if let Some(overlay) = &self.overlay {
                    overlay.global::<Status>().set_toast(CAPTURE_MESSAGE.into());
                    let weak = overlay.as_weak();
                    self.notice_timer
                        .start(slint::TimerMode::SingleShot, TOAST_TIME, move || {
                            if let Some(overlay) = weak.upgrade() {
                                overlay.global::<Status>().set_toast("".into());
                            }
                        });
                }
            }
            _ => self.toast(CAPTURE_MESSAGE),
        }
    }

    /// InputPlumber went away or came back. A game in front loses its virtual pad, so it is
    /// frozen (following its suspend policy) under the overlay, which explains; a quick
    /// restart resumes it on its own.
    fn on_input_status(&mut self, status: InputStatus) {
        tracing::info!(?status, mode = ?self.mode, "controller service");
        // Controllers vanish and come back with the service; that isn't the player's doing.
        self.pads_settle = Instant::now() + PAD_SETTLE;
        match status {
            InputStatus::Lost => {
                let froze = self.mode == Mode::Game && self.compositor.controls_focus();
                if froze {
                    self.open_overlay();
                }
                self.service_lost = Some((Instant::now(), froze));
                self.overlay_message(SERVICE_LOST);
            }
            InputStatus::StillLost => self.overlay_message(SERVICE_STILL_LOST),
            InputStatus::Back => {
                let Some((since, froze)) = self.service_lost.take() else {
                    return;
                };
                if froze && self.mode == Mode::Overlay && since.elapsed() <= QUICK_RESTART {
                    self.resume_game();
                    return;
                }
                if self.mode == Mode::Overlay
                    && let Some(overlay) = &self.overlay
                    && overlay.get_panel() == OverlayPanel::Menu
                {
                    // Resume focused, ready to go.
                    overlay.invoke_reset();
                }
                let message = if self.active.is_some() {
                    SERVICE_BACK
                } else {
                    "Controller service is back"
                };
                self.overlay_message(message);
            }
        }
    }

    /// A message about the controller: on the overlay while it is open (it stays until the
    /// overlay closes), over the running game, else a toast.
    fn overlay_message(&mut self, message: &str) {
        match (&self.overlay, self.mode) {
            (Some(overlay), Mode::Overlay) => {
                self.notice_timer.stop();
                overlay.global::<Status>().set_toast(message.into());
            }
            (Some(_), Mode::Game) if self.compositor.controls_focus() => self.show_notice(message),
            _ => self.toast(message),
        }
    }

    /// Notices controllers coming and going and low batteries. Called every [`PAD_POLL`].
    fn poll_pads(&mut self) {
        let pads = self.activity.pads();
        let now = Instant::now();
        // Connects and disconnects reach the input backend's list at once, battery levels
        // every few seconds: the UI follows whenever the list changes.
        // Until the backend has had time to find them (InputPlumber takes a few seconds), no
        // pad means "still looking", not "none".
        let checking = pads.is_empty() && now < self.pads_settle;
        if pads != self.known_pads || self.pads_shown != Some(checking) {
            self.pads_shown = Some(checking);
            self.refresh_pads(&pads, checking);
        }
        if self.service_lost.is_some() || now < self.pads_settle {
            self.known_pads = pads;
            return;
        }
        let gone: Vec<PadInfo> = self
            .known_pads
            .iter()
            .filter(|old| !pads.iter().any(|p| p.id == old.id))
            .cloned()
            .collect();
        let came: Vec<PadInfo> = pads
            .iter()
            .filter(|new| !self.known_pads.iter().any(|p| p.id == new.id))
            .cloned()
            .collect();
        self.known_pads.clone_from(&pads);
        for pad in &gone {
            self.pad_gone(pad);
        }
        for pad in &came {
            self.pad_came(pad);
        }
        self.check_batteries(&pads);
    }

    /// Whether `pad` is the one the running game reads (or, if that one is gone, the one that
    /// takes its place).
    fn drives_game(&self, pad: &PadInfo) -> bool {
        self.active.is_some()
            && self
                .game_pad
                .as_ref()
                .is_none_or(|id| *id == pad.id || !self.known_pads.iter().any(|p| &p.id == id))
    }

    /// A controller went away. The game's own controller freezes the game under the overlay
    /// (it can't be played without it); any other gets a notice.
    fn pad_gone(&mut self, pad: &PadInfo) {
        tracing::info!(pad = %pad.id, name = %pad.os_name, mode = ?self.mode, "controller gone");
        let driving = self.active.is_some() && self.game_pad.as_deref() == Some(pad.id.as_str());
        match self.mode {
            Mode::Game if driving && self.compositor.controls_focus() => {
                self.open_overlay();
                self.overlay_message(PAD_LOST);
            }
            Mode::Overlay if driving => self.overlay_message(PAD_LOST),
            _ => self.pad_notice("Controller disconnected", &pad.os_name, None),
        }
    }

    /// A controller came (back). If the game is waiting for it, the game is frozen (or stays
    /// frozen) under the overlay with Resume focused: the emulator picks the pad up while the
    /// game can't move, and the player resumes when ready.
    fn pad_came(&mut self, pad: &PadInfo) {
        tracing::info!(pad = %pad.id, name = %pad.os_name, mode = ?self.mode, "controller came");
        if !self.drives_game(pad) {
            self.pad_notice("Controller connected", &pad.os_name, None);
            return;
        }
        self.game_pad = Some(pad.id.clone());
        match self.mode {
            Mode::Game if self.compositor.controls_focus() => {
                self.open_overlay();
                self.overlay_message(PAD_BACK);
            }
            Mode::Overlay => {
                if let Some(overlay) = &self.overlay
                    && overlay.get_panel() == OverlayPanel::Menu
                {
                    overlay.invoke_reset();
                }
                self.overlay_message(PAD_BACK);
            }
            _ => self.pad_notice("Controller connected", &pad.os_name, None),
        }
    }

    /// Low battery notices: at each level of [`BATTERY_WARNINGS`] once per charge; charging
    /// starts over.
    fn check_batteries(&mut self, pads: &[PadInfo]) {
        for pad in pads {
            let warned = self.battery_warned.entry(pad.id.clone()).or_default();
            let percent = match pad.power {
                Power::Discharging(p) => p,
                Power::Charging(_) | Power::Charged => {
                    warned.clear();
                    continue;
                }
                Power::Unknown | Power::Wired => continue,
            };
            if battery_warning(percent, warned).is_none() {
                continue;
            }
            tracing::info!(pad = %pad.id, percent, "controller battery low");
            self.pad_notice(
                "Controller battery low",
                &format!("{percent} %"),
                Some(percent),
            );
        }
    }

    /// A controller notice for [`PAD_NOTICE_TIME`]: over the running game on the overlay's
    /// notice panel (no scrim, no freeze), else in Hearth's windows.
    fn pad_notice(&mut self, text: &str, detail: &str, battery: Option<u8>) {
        let set = |data: PadNotice<'_>| {
            data.set_text(text.into());
            data.set_detail(detail.into());
            data.set_battery(battery.map_or(-1, i32::from));
            data.set_shown(true);
        };
        if let Some(app) = self.app.upgrade() {
            set(app.global::<PadNotice>());
        }
        if let Some(overlay) = &self.overlay {
            set(overlay.global::<PadNotice>());
            if self.mode == Mode::Game
                && self.compositor.controls_focus()
                && let Some(window) = self.overlay_window
                && overlay.get_panel() == OverlayPanel::None
            {
                overlay.set_panel(OverlayPanel::Notice);
                if let Err(err) = self.compositor.set_overlay(window, true) {
                    tracing::warn!("showing a controller notice failed: {err}");
                }
            }
        }
        self.pad_notice_timer
            .start(slint::TimerMode::SingleShot, PAD_NOTICE_TIME, || {
                defer(Controller::hide_pad_notice);
            });
    }

    fn hide_pad_notice(&mut self) {
        if let Some(app) = self.app.upgrade() {
            app.global::<PadNotice>().set_shown(false);
        }
        let Some(overlay) = &self.overlay else {
            return;
        };
        overlay.global::<PadNotice>().set_shown(false);
        // Only the notice panel goes; a menu opened meanwhile stays, a toast keeps it.
        if overlay.get_panel() == OverlayPanel::Notice
            && overlay.global::<Status>().get_toast().is_empty()
        {
            self.hide_overlay();
        }
    }

    /// A message over the running game: the overlay's notice panel, no scrim, no freeze.
    fn show_notice(&mut self, message: &str) {
        let (Some(overlay), Some(window)) = (&self.overlay, self.overlay_window) else {
            return;
        };
        overlay.global::<Status>().set_toast(message.into());
        overlay.set_panel(OverlayPanel::Notice);
        if let Err(err) = self.compositor.set_overlay(window, true) {
            tracing::warn!("showing a notice failed: {err}");
            return;
        }
        self.notice_timer
            .start(slint::TimerMode::SingleShot, NOTICE_TIME, || {
                defer(Controller::hide_notice);
            });
    }

    fn hide_notice(&mut self) {
        let Some(overlay) = &self.overlay else {
            return;
        };
        overlay.global::<Status>().set_toast("".into());
        // The quick menu may have opened over the notice; it stays.
        if overlay.get_panel() == OverlayPanel::Notice {
            self.hide_overlay();
        }
    }

    fn home_button(&mut self, long: bool) {
        let in_gamescope = self.compositor.controls_focus();
        tracing::debug!(long, mode = ?self.mode, in_gamescope, "guide button");
        match self.mode {
            Mode::Shell => self.show_home(self.active.is_some()),
            Mode::Launching => {
                if long {
                    self.close_game();
                }
            }
            Mode::Game if !in_gamescope => {
                if long {
                    self.close_game();
                } else {
                    tracing::info!(
                        "the overlay needs gamescope in Steam mode; long press closes the game"
                    );
                    // InputPlumber gave Hearth the controller when Guide went down: give it
                    // back to the game.
                    self.set_mode(Mode::Game);
                }
            }
            Mode::Game => {
                if long {
                    self.go_home();
                } else {
                    self.open_overlay();
                }
            }
            Mode::Overlay => {
                if long {
                    self.go_home();
                } else {
                    self.resume_game();
                }
            }
        }
    }

    // --- launching ---------------------------------------------------------------------

    fn launch(&mut self, id: &GameId) {
        if let Some(active) = &self.active {
            if &active.game.id == id {
                self.resume_game();
            } else {
                self.ask_switch(id);
            }
            return;
        }
        let Some(entry) = self.entries.iter().find(|e| &e.game.id == id).cloned() else {
            tracing::warn!(%id, "launch: unknown game");
            return;
        };
        let game = entry.game;
        let Some(system) = self.systems.get(&game.system) else {
            self.toast(&format!("No system for {}", game.title));
            return;
        };
        let suspend = system.suspend;
        let Some(emulator) = system.emulator() else {
            self.toast(&format!("No emulator set up for {}", system.name));
            return;
        };
        let input = self.game_input(system, &game, emulator.kind);
        self.game_pad = self.activity.pads().first().map(|p| p.id.clone());
        if self.input.is_live() && self.applier.apply(&input) != ApplyOutcome::Applied {
            // No remapping rather than the previous game's.
            self.input.set_game_profile(None);
        }
        // Started without InputPlumber (it is gone): this game reads the physical controller,
        // which InputPlumber coming back must not take from it.
        self.input.keep_direct(!self.input.is_live());
        tracing::info!(key = %input.key.as_string(), "controls resolved");
        self.game_capture = input
            .resolved
            .iter()
            .flat_map(|r| &r.mapping.binds)
            .filter(|(_, t)| **t == Target::Hearth(HearthAction::Capture))
            .map(|(input, _)| input.name())
            .collect();
        let controls_problem = input.report();
        let mut command = match (emulator.kind, emulator.core.as_ref()) {
            (EmulatorKind::Retroarch, Some(core)) if core.is_file() => {
                if let Err(err) = self.profile.prepare(&input.translation) {
                    self.toast(&format!("Couldn't write the RetroArch config: {err}"));
                    return;
                }
                self.profile.command(core, &game.path)
            }
            (EmulatorKind::Retroarch, core) => {
                let core = core.map_or_else(String::new, |c| c.display().to_string());
                tracing::warn!(%core, "libretro core missing");
                self.toast(&format!("RetroArch core not found: {core}"));
                return;
            }
            (EmulatorKind::Dolphin, _) => {
                let Some(command) = self.dolphin.command(&game.path) else {
                    tracing::warn!("Dolphin not found (dolphin-emu in PATH or Flatpak)");
                    self.toast("Dolphin isn't installed");
                    return;
                };
                if let Err(err) = self.dolphin.prepare(&input.translation) {
                    self.toast(&format!("Couldn't write the Dolphin config: {err}"));
                    return;
                }
                command
            }
        };
        command.env.extend(
            input
                .translation
                .env
                .iter()
                .map(|(k, v)| (k.into(), v.into())),
        );
        let retroarch = emulator.kind == EmulatorKind::Retroarch;
        if retroarch && input.autoconfig.is_none() {
            self.toast("No controller profiles for RetroArch: the pad won't work in the game");
        } else if !self.compositor.controls_focus() {
            self.toast("No overlay outside gamescope: hold the Guide button to close the game");
        }
        if self.compositor.is_active() {
            // Launcher rule: inside gamescope, games must never reach the host's Wayland.
            command.force_x11();
        }
        // Debugging aid: extra RetroArch arguments, e.g. HEARTH_RETROARCH_ARGS=--max-frames=600
        if let Some(extra) = std::env::var_os("HEARTH_RETROARCH_ARGS").filter(|_| retroarch) {
            let extra = extra.to_string_lossy().into_owned();
            tracing::info!(%extra, "extra RetroArch arguments");
            for (i, arg) in extra.split_whitespace().enumerate() {
                command.args.insert(i, arg.into());
            }
        }
        let ctx = LaunchCtx {
            command,
            events: self.events.clone(),
        };
        let session = match self.runtime.launch(&game, &ctx) {
            Ok(session) => session,
            Err(err) => {
                tracing::warn!("launch failed: {err}");
                self.toast(&format!("Couldn't start {}: {err}", game.title));
                return;
            }
        };
        let app_id = self.next_app_id;
        self.next_app_id += 1;
        let title = game.title.clone();
        self.active = Some(ActiveSession::new(
            game,
            session.clone(),
            app_id,
            suspend,
            Instant::now(),
        ));
        self.closing = false;
        if self.compositor.controls_focus() {
            self.set_mode(Mode::Launching);
            self.toast(&controls_problem.unwrap_or_else(|| format!("Starting {title}…")));
            self.search_window(session, app_id);
        } else {
            // Outside gamescope (or without Steam mode) the game keeps the screen.
            if let Some(problem) = controls_problem {
                self.toast(&problem);
            }
            self.set_mode(Mode::Game);
            if let Some(active) = &mut self.active {
                active.state = SessionState::Playing;
            }
        }
        self.refresh();
    }

    /// Opens "Close <running>?" with "Close and play <id>"; see [`Self::confirm_switch`].
    fn ask_switch(&mut self, id: &GameId) {
        let (Some(app), Some(active)) = (self.app.upgrade(), &self.active) else {
            return;
        };
        let Some(next) = self.entries.iter().find(|e| &e.game.id == id) else {
            tracing::warn!(%id, "switch: unknown game");
            return;
        };
        tracing::info!(running = %active.game.id, next = %id, "asking to switch games");
        let session = app.global::<Session>();
        session.set_switch_from(active.game.title.as_str().into());
        session.set_switch_to(next.game.title.as_str().into());
        session.set_switch_open(true);
        self.switch_to = Some(id.clone());
    }

    /// The user confirmed the switch: the running game closes the usual way (thawed first if
    /// frozen) and the new one starts once its session has ended.
    fn confirm_switch(&mut self) {
        let Some(id) = self.switch_to.take() else {
            return;
        };
        if self.active.is_none() {
            self.launch(&id);
            return;
        }
        tracing::info!(next = %id, "closing the running game to start another");
        self.launch_after_close = Some(id);
        self.close_game();
    }

    /// Resolves and translates `game`'s controls for the controller as it is now.
    fn game_input(
        &self,
        system: &hearth_core::systems::System,
        game: &hearth_core::model::Game,
        kind: EmulatorKind,
    ) -> controls::GameInput {
        let device = controls::device(&self.activity.pads(), &self.controls_dir);
        let pad = if self.input.is_live() {
            controls::PadTarget::Virtual
        } else {
            controls::PadTarget::Physical
        };
        controls::game_input(
            system,
            game,
            kind,
            &self.controls_dir,
            device,
            &self.profile,
            pad,
        )
    }

    /// Applies the running game's controls again (after an edit on a controls page).
    fn apply_running_controls(&self) -> Option<ApplyOutcome> {
        let game = &self.active.as_ref()?.game;
        let system = self.systems.get(&game.system)?;
        let kind = system.emulator()?.kind;
        let input = self.game_input(system, game, kind);
        let outcome = self.applier.apply(&input);
        tracing::info!(?outcome, game = %game.id, "controls applied to the running game");
        Some(outcome)
    }

    /// Looks for the game's windows on a worker thread and tags them with `app_id`.
    fn search_window(&self, session: RuntimeSession, app_id: AppId) {
        let compositor = self.compositor.clone();
        let spawned = thread::Builder::new()
            .name("hearth-window-search".into())
            .spawn(move || {
                let id = session.id();
                let pgid = session.pgid();
                let started = Instant::now();
                let mut found = false;
                while !session.has_exited() {
                    match compositor.assign_app(pgid, app_id) {
                        Ok(n) if n > 0 && !found => {
                            found = true;
                            tracing::info!(
                                pgid,
                                app_id,
                                mapped_windows = n,
                                after_ms = started.elapsed().as_millis(),
                                "found game window"
                            );
                            defer(move |c| c.on_window_found(id));
                        }
                        Ok(_) => {}
                        Err(err) => tracing::warn!("tagging game windows failed: {err}"),
                    }
                    if !found && started.elapsed() >= WINDOW_SEARCH {
                        tracing::warn!(
                            pgid,
                            app_id,
                            secs = WINDOW_SEARCH.as_secs(),
                            "no mapped window with _NET_WM_PID in the game's process group or its \
                             descendants appeared"
                        );
                        defer(move |c| c.on_window_timeout(id));
                        return;
                    }
                    thread::sleep(if found { RETAG_INTERVAL } else { WINDOW_POLL });
                }
            });
        if let Err(err) = spawned {
            tracing::error!("cannot start window search: {err}");
        }
    }

    fn is_current(&self, id: SessionId) -> bool {
        self.active.as_ref().is_some_and(|a| a.session.id() == id)
    }

    fn on_window_found(&mut self, id: SessionId) {
        if self.is_current(id) && self.mode == Mode::Launching {
            self.clear_toast();
            self.show_game();
        }
    }

    fn on_window_timeout(&mut self, id: SessionId) {
        if !self.is_current(id) || self.mode != Mode::Launching {
            return;
        }
        let title = self.title();
        self.toast(&format!("{title} didn't open a window, closing it"));
        self.close_game();
    }

    // --- switching between game, overlay and Home --------------------------------------

    /// Puts the game in front and unfreezes it. If gamescope doesn't show it within
    /// [`FOCUS_RETRY`], Hearth comes back and the game keeps running.
    fn show_game(&mut self) {
        let Some(app_id) = self.active.as_ref().map(|a| a.app_id) else {
            return;
        };
        // A game that opened its window faster than the launch fade: be silent now. (Resume
        // keeps its own fade, overlapping the game a little.)
        if self.mode == Mode::Launching {
            self.music.hurry(&self.audio, director::FADE_FOCUS);
        }
        self.hide_overlay();
        self.thaw();
        self.set_mode(Mode::Game);
        if !self.compositor.controls_focus() {
            return;
        }
        if let Some(active) = &mut self.active {
            active.state = SessionState::Playing;
        }
        self.refresh();
        if let Err(err) = self.compositor.focus(&[app_id, SHELL_APP_ID]) {
            tracing::warn!("focus switch failed, retrying: {err}");
        }
        let Some(id) = self.active.as_ref().map(|a| a.session.id()) else {
            return;
        };
        let started = Instant::now();
        self.focus_timer
            .start(slint::TimerMode::Repeated, FOCUS_POLL, move || {
                with(|c| c.poll_focus(id, app_id, started));
            });
    }

    /// Checks that gamescope shows the game; asks again until [`FOCUS_RETRY`] runs out.
    fn poll_focus(&mut self, id: SessionId, app_id: AppId, started: Instant) {
        if self.mode != Mode::Game || !self.is_current(id) {
            self.focus_timer.stop();
            return;
        }
        let problem = match self.compositor.focused_app() {
            Ok(Some(focused)) if focused == app_id => {
                self.focus_timer.stop();
                return;
            }
            Ok(focused) => format!("gamescope kept app {focused:?} in front"),
            Err(err) => format!("cannot read gamescope focus: {err}"),
        };
        if started.elapsed() >= FOCUS_RETRY {
            self.focus_timer.stop();
            self.game_not_shown(&problem);
            return;
        }
        tracing::debug!(%problem, "game not in front yet, asking again");
        if let Err(err) = self.compositor.focus(&[app_id, SHELL_APP_ID]) {
            tracing::warn!("focus switch failed: {err}");
        }
    }

    /// The game couldn't be brought to the front: Hearth takes it back, the game keeps running.
    fn game_not_shown(&mut self, reason: &str) {
        tracing::warn!("couldn't show the game: {reason}");
        self.activity.disarm();
        let _ = self.compositor.focus(&[SHELL_APP_ID]);
        self.set_mode(Mode::Shell);
        if let Some(active) = &mut self.active {
            active.state = SessionState::InBackground;
        }
        let title = self.title();
        self.refresh();
        self.show_home(true);
        self.toast(&format!(
            "Couldn't switch to {title}; it is still running. Resume it from Home"
        ));
    }

    fn open_overlay(&mut self) {
        let (Some(overlay), Some(window)) = (
            self.overlay.as_ref().map(ComponentHandle::clone_strong),
            self.overlay_window,
        ) else {
            tracing::warn!("overlay window not ready; going Home instead");
            self.go_home();
            return;
        };
        self.activity.disarm();
        self.freeze();
        self.sync_session_ui();
        overlay.invoke_reset();
        overlay.set_panel(OverlayPanel::Menu);
        sounds::play("overlay_open");
        if let Err(err) = self.compositor.set_overlay(window, true) {
            tracing::warn!("showing the overlay failed: {err}");
            self.go_home();
            return;
        }
        self.set_mode(Mode::Overlay);
        if let Some(active) = &mut self.active {
            active.state = SessionState::InBackground;
        }
    }

    fn hide_overlay(&mut self) {
        if let Some(overlay) = &self.overlay {
            // Only the menu (and its pages) sound; a notice comes and goes silently.
            if matches!(
                overlay.get_panel(),
                OverlayPanel::Menu | OverlayPanel::Controls
            ) {
                sounds::play("overlay_close");
            }
            overlay.set_panel(OverlayPanel::None);
            overlay.global::<Status>().set_toast("".into());
        }
        if let Some(window) = self.overlay_window
            && let Err(err) = self.compositor.set_overlay(window, false)
        {
            tracing::warn!("hiding the overlay failed: {err}");
        }
    }

    /// Resume from the overlay or from Home.
    fn resume_game(&mut self) {
        if self.active.is_some() {
            self.show_game();
        }
    }

    /// Freezes the game and shows Home with focus on its tile.
    fn go_home(&mut self) {
        self.activity.disarm();
        self.hide_overlay();
        self.freeze();
        if let Err(err) = self.compositor.focus(&[SHELL_APP_ID]) {
            tracing::warn!("focusing Hearth failed: {err}");
        }
        self.set_mode(Mode::Shell);
        if let Some(active) = &mut self.active {
            active.state = SessionState::InBackground;
        }
        self.refresh();
        self.show_home(true);
    }

    /// Shows Home over any screen, focused on the first game (`focus_game`) or Welcome.
    fn show_home(&self, focus_game: bool) {
        if let Some(app) = self.app.upgrade() {
            app.invoke_go_home(focus_game);
        }
    }

    // --- freezing and closing ----------------------------------------------------------

    fn freeze(&mut self) {
        // Without focus control the game's window stays on top; freezing it would just hang it.
        if !self.compositor.controls_focus() {
            return;
        }
        let Some(active) = &mut self.active else {
            return;
        };
        if active.frozen || active.suspend != SuspendPolicy::Sigstop {
            return;
        }
        match self.runtime.suspend(&active.session) {
            Ok(()) => active.mark_frozen(Instant::now()),
            Err(err) => tracing::warn!("freezing the game failed: {err}"),
        }
    }

    fn thaw(&mut self) {
        let Some(active) = &mut self.active else {
            return;
        };
        if !active.frozen {
            return;
        }
        match self.runtime.resume(&active.session) {
            Ok(()) => active.mark_thawed(Instant::now()),
            Err(err) => tracing::warn!("resuming the game failed: {err}"),
        }
    }

    /// Closes the running game (after the user confirmed). Hearth comes to the front at once;
    /// the exit event finishes the session.
    fn close_game(&mut self) {
        let Some(active) = &self.active else {
            return;
        };
        let session = active.session.clone();
        let title = active.game.title.clone();
        self.closing = true;
        self.hide_overlay();
        let _ = self.compositor.focus(&[SHELL_APP_ID]);
        self.set_mode(Mode::Shell);
        self.show_home(true);
        self.toast(&format!("Closing {title}…"));
        let runtime = self.runtime.clone();
        let spawned = thread::Builder::new()
            .name("hearth-close-game".into())
            .spawn(move || {
                if let Err(err) = runtime.close(&session) {
                    tracing::warn!("closing the game failed: {err}");
                }
            });
        if let Err(err) = spawned {
            tracing::error!("cannot start close thread: {err}");
        }
    }

    fn shutdown(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        tracing::info!(game = %active.game.id, "closing the game before exiting");
        self.hide_overlay();
        if let Err(err) = self.runtime.close(&active.session) {
            tracing::warn!("closing the game failed: {err}");
        }
    }

    fn on_runtime_event(&mut self, event: RuntimeEvent) {
        let RuntimeEvent::Exited {
            session,
            game,
            success,
        } = event;
        let Some(active) = self.active.take_if(|a| a.is(session, &game)) else {
            return;
        };
        let played = active.played(Instant::now());
        let title = active.game.title.clone();
        let closed_by_us = std::mem::take(&mut self.closing);
        tracing::info!(%game, played_secs = played.as_secs(), success, "session ended");
        self.input.keep_direct(false);

        self.activity.disarm();
        self.hide_overlay();
        let _ = self.compositor.focus(&[SHELL_APP_ID]);
        self.set_mode(Mode::Shell);
        self.refresh();
        self.show_home(true);
        let message = if success || closed_by_us {
            format!("{title}: {}", models::format_session_total(played))
        } else {
            format!("{title} stopped unexpectedly")
        };
        self.toast(&message);
        if let Some(next) = self.launch_after_close.take() {
            self.launch(&next);
        }

        // Save playtime and reload the library off the UI thread.
        let db = self.library_db.clone();
        let spawned = thread::Builder::new()
            .name("hearth-record-session".into())
            .spawn(move || {
                let result = Library::open(&db).and_then(|library| {
                    library.record_session(&game, SystemTime::now(), played)?;
                    library.recent()
                });
                match result {
                    Ok(entries) => defer(move |c| c.set_entries(entries)),
                    Err(err) => tracing::warn!("saving playtime failed: {err}"),
                }
            });
        if let Err(err) = spawned {
            tracing::error!("cannot start session recording thread: {err}");
        }
    }

    // --- UI ----------------------------------------------------------------------------

    fn set_entries(&mut self, entries: Vec<LibraryEntry>) {
        self.entries = entries;
        self.refresh();
    }

    /// A decoded image (or none any more) for a game, from the art loader.
    pub fn set_art_image(
        &mut self,
        game: &GameId,
        kind: ArtKind,
        pixels: Option<SharedPixelBuffer<Rgba8Pixel>>,
    ) {
        self.art.entry(game.clone()).or_default().set(kind, pixels);
        self.art_delivered.insert((game.clone(), kind));
        self.refresh_tiles();
        self.customize_art_changed(game);
        self.probe_startup();
    }

    /// The SteamGridDB game a game matched (for Properties), where its hero is cropped and
    /// its art files, from the art worker.
    pub fn set_art_meta(
        &mut self,
        game: &GameId,
        match_name: Option<String>,
        hero_position: f32,
        paths: hearth_core::art::GameArt,
    ) {
        let art = self.art.entry(game.clone()).or_default();
        art.match_name = match_name;
        // While the hero is being positioned, the page's position wins.
        let positioning = self.customize.as_ref().is_some_and(|c| c.positioning(game));
        let moved = !positioning && art.hero_position != Some(hero_position);
        if !positioning {
            art.hero_position = Some(hero_position);
        }
        self.art_paths.insert(game.clone(), paths);
        if moved {
            self.refresh_tiles();
        }
        self.customize_art_changed(game);
        self.probe_startup();
    }

    /// Follows the window into another screen class once it has kept its size for a tick:
    /// display copies are remade for it and remembered for the next start.
    fn check_display_class(&mut self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let size = app.window().size();
        #[allow(clippy::cast_precision_loss)]
        crate::diagram::set_scale(
            (size.width as f32 / crate::callouts::PAGE_W).min(size.height as f32 / 1080.0),
        );
        let class = DisplayClass::for_height(size.height);
        if class == self.art_loader.class() {
            self.class_candidate = None;
            return;
        }
        if self.class_candidate != Some(class) {
            self.class_candidate = Some(class);
            return;
        }
        tracing::info!(class = %class.name(), "screen class changed; remaking display art");
        self.class_candidate = None;
        self.art_loader.set_class(class);
        if let Err(err) = std::fs::write(&self.display_class_file, class.name()) {
            tracing::warn!("can't remember the screen class: {err}");
        }
    }

    /// Startup timing: the first game's hero, and art for every game in view.
    fn probe_startup(&self) {
        /// Game cells in view at startup (Welcome focused, any 16:9 or 16:10 screen).
        const IN_VIEW: usize = 11;
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let tiles = app.global::<HomeData>().get_tiles();
        let games: Vec<HomeTile> = tiles.iter().filter(|t| t.kind == TileKind::Game).collect();
        if games.first().is_some_and(|t| t.hero.size().width > 0) {
            super::startup::shown("first game's hero", &app);
        }
        let in_view = &games[..games.len().min(IN_VIEW)];
        let settled = |t: &HomeTile| {
            let id = GameId::new(t.id.as_str());
            self.art.contains_key(&id)
                && (!self.art_loader.has_source(&id, ArtKind::Grid)
                    || self.art_delivered.contains(&(id, ArtKind::Grid)))
        };
        if !in_view.is_empty() && in_view.iter().all(settled) {
            super::startup::shown("art of the games in view", &app);
        }
    }

    fn title(&self) -> String {
        self.active
            .as_ref()
            .map(|a| a.game.title.clone())
            .unwrap_or_default()
    }

    /// Rebuilds the tile row and the session globals.
    fn refresh(&mut self) {
        self.refresh_tiles();
        self.sync_session_ui();
    }

    /// Rebuilds the tile row. Rows are updated in place when the games are the same, so
    /// Home keeps its focus and animations.
    fn refresh_tiles(&mut self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let running = self.active.as_ref().map(|a| &a.game.id);
        let tiles = models::home_tiles(
            &self.entries,
            &self.systems,
            running,
            unix_now(),
            &self.art,
            &mut self.icons,
        );
        let order: Vec<Option<GameId>> = tiles
            .iter()
            .map(|t| (t.kind == TileKind::Game).then(|| GameId::new(t.id.as_str())))
            .collect();
        self.art_loader.set_order(&order);
        models::set_tiles(&app, tiles);
    }

    fn sync_session_ui(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let globals = app.global::<Session>();
        let (title, badge, played) = match &self.active {
            Some(a) => (
                a.game.title.clone(),
                self.systems
                    .get(&a.game.system)
                    .map_or_else(String::new, |s| s.short.clone()),
                models::format_session(a.played(Instant::now())),
            ),
            None => Default::default(),
        };
        globals.set_active(self.active.is_some());
        globals.set_title(title.as_str().into());
        globals.set_badge(badge.as_str().into());
        globals.set_played(played.as_str().into());
        if let Some(overlay) = &self.overlay {
            overlay.set_game_title(title.into());
            overlay.set_badge(badge.into());
            overlay.set_played(played.into());
        }
    }

    /// The first connected controller's type (generic when none is connected).
    fn connected_kind(&self) -> ControllerKind {
        self.activity
            .pads()
            .first()
            .map_or(ControllerKind::Generic, |pad| {
                ControllerKind::from_ids(pad.vendor_id, pad.product_id)
            })
    }

    /// Player 1's controller type, as launches see it (see `controls::device`).
    fn pad_kind(&self) -> ControllerKind {
        controls::device(&self.activity.pads(), &self.controls_dir).kind
    }

    /// Player 1's extra inputs (touchpad, share, paddles, mute) that the input backend reads:
    /// gilrs can't see the touchpad (a separate device) or the mute button.
    /// The button test's inputs beyond the drawing: Guide, and the extras the backend reads.
    fn test_extras(&self) -> Vec<&'static str> {
        let live = self.input.is_live();
        std::iter::once("guide")
            .chain(
                self.pad_extras()
                    .into_iter()
                    .filter(|i| live || *i != PadInput::Touchpad)
                    .map(PadInput::name),
            )
            .collect()
    }

    /// Prepares the controls pages on worker threads (callout placement, the drawing) so they
    /// open at once.
    fn warm_up_controls(&mut self) {
        self.check_display_class();
        let page: Vec<&'static str> = self.pad_extras().into_iter().map(PadInput::name).collect();
        let test = self.test_extras();
        let spawned = thread::Builder::new()
            .name("hearth-callouts".into())
            .spawn(move || {
                crate::callouts::place_generic(&page);
                crate::callouts::place_generic(&test);
            });
        if let Err(err) = spawned {
            tracing::warn!("cannot start the callout thread: {err}");
        }
        let layout = crate::controller_layout::Layout::generic();
        crate::diagram::warm_up(crate::callouts::drawing(&layout));
    }

    fn pad_extras(&self) -> Vec<PadInput> {
        let live = self.input.is_live();
        controls::device(&self.activity.pads(), &self.controls_dir)
            .extra_inputs()
            .into_iter()
            .filter(|i| live || *i != PadInput::Mute)
            .collect()
    }

    /// Button glyphs follow the first connected controller (both windows have their own
    /// globals).
    fn sync_controller_type(&self) {
        let family = glyph_family(self.connected_kind());
        if let Some(app) = self.app.upgrade() {
            app.global::<ControllerType>().set_family(family);
        }
        if let Some(overlay) = &self.overlay {
            overlay.global::<ControllerType>().set_family(family);
        }
    }

    // --- controls pages ------------------------------------------------------------------

    fn with_page_data(&self, which: usize, f: impl FnOnce(&Page, &ControlsData<'_>)) {
        let Some(page) = &self.pages[which] else {
            return;
        };
        if which == MAIN {
            if let Some(app) = self.app.upgrade() {
                f(page, &app.global::<ControlsData>());
            }
        } else if let Some(overlay) = &self.overlay {
            f(page, &overlay.global::<ControlsData>());
        }
    }

    /// Settings → Controllers: the button test or a console's page.
    fn settings_controller_item(&mut self, key: &str) {
        if let Some(pad) = key.strip_prefix("test:") {
            self.start_button_test(pad);
            return;
        }
        let Some(id) = key.strip_prefix("system:") else {
            return;
        };
        let Some(system) = self.systems.get(&SystemId::new(id)).cloned() else {
            return;
        };
        let started = Instant::now();
        let key = GameKey::for_rom(&system.id, std::path::Path::new(""), "");
        let editor = match Editor::new(
            &system,
            key,
            &self.controls_dir,
            self.pad_kind(),
            Scope::System,
        ) {
            Ok(editor) => editor,
            Err(err) => {
                self.toast(&err.to_string());
                return;
            }
        };
        let affects = self
            .active
            .as_ref()
            .is_some_and(|a| a.game.system == system.id);
        self.pages[MAIN] = Some(Page::new(
            editor,
            None,
            affects,
            self.applier.is_live(),
            self.pad_extras(),
        ));
        let built = started.elapsed();
        self.with_page_data(MAIN, |page, data| {
            data.set_message("".into());
            page.fill(data);
        });
        tracing::debug!(
            built_ms = built.as_millis(),
            total_ms = started.elapsed().as_millis(),
            "controls page filled"
        );
        if let Some(app) = self.app.upgrade() {
            app.invoke_open_controls();
        }
    }

    /// The running game's controls page, in the main window (Home) or the overlay.
    fn open_game_controls(&mut self, which: usize) {
        let Some(active) = &self.active else {
            return;
        };
        let game = active.game.clone();
        self.open_controls_page(&game, true, which);
    }

    /// Home → Options → Controls for this game, running or not.
    fn open_controls_for(&mut self, id: &GameId) {
        let running = self.active.as_ref().is_some_and(|a| &a.game.id == id);
        let Some(game) = self
            .entries
            .iter()
            .find(|e| &e.game.id == id)
            .map(|e| e.game.clone())
        else {
            return;
        };
        self.open_controls_page(&game, running, MAIN);
    }

    /// A game's controls page. `affects_running`: changes apply to the running game.
    fn open_controls_page(&mut self, game: &Game, affects_running: bool, which: usize) {
        let Some(system) = self.systems.get(&game.system).cloned() else {
            return;
        };
        let key = GameKey::for_rom(&system.id, &game.path, &game.title);
        let editor = match Editor::new(
            &system,
            key,
            &self.controls_dir,
            self.pad_kind(),
            Scope::Game,
        ) {
            Ok(editor) => editor,
            Err(err) => {
                self.toast(&err.to_string());
                return;
            }
        };
        self.pages[which] = Some(Page::new(
            editor,
            Some(game.title.clone()),
            affects_running,
            self.applier.is_live(),
            self.pad_extras(),
        ));
        self.with_page_data(which, |page, data| {
            data.set_message("".into());
            page.fill(data);
        });
        if which == MAIN {
            if let Some(app) = self.app.upgrade() {
                app.invoke_open_controls();
            }
        } else if let Some(overlay) = &self.overlay {
            overlay.invoke_open_controls();
        }
    }

    /// Flips a game's favorite mark (shown at once, saved off the UI thread).
    fn toggle_favorite(&mut self, id: &GameId) {
        let Some(entry) = self.entries.iter_mut().find(|e| &e.game.id == id) else {
            return;
        };
        entry.favorite = !entry.favorite;
        let favorite = entry.favorite;
        let (db, id) = (self.library_db.clone(), id.clone());
        let spawned = thread::Builder::new()
            .name("hearth-favorite".into())
            .spawn(move || {
                if let Err(err) = Library::open(&db).and_then(|l| l.set_favorite(&id, favorite)) {
                    tracing::warn!("saving the favorite failed: {err}");
                    defer(move |c| c.toast(&format!("Couldn't save the favorite: {err}")));
                }
            });
        if let Err(err) = spawned {
            tracing::error!("cannot start favorite thread: {err}");
        }
        self.refresh_tiles();
    }

    /// Fills `HomeData.properties` for a game.
    fn fill_properties(&self, id: &GameId) {
        let (Some(app), Some(entry)) = (
            self.app.upgrade(),
            self.entries.iter().find(|e| &e.game.id == id),
        ) else {
            return;
        };
        let game = &entry.game;
        let console = self
            .systems
            .get(&game.system)
            .map_or_else(|| game.system.to_string(), |s| s.name.clone());
        let size = std::fs::metadata(&game.path).map_or_else(
            |_| "File not found".to_owned(),
            |m| models::format_size(m.len()),
        );
        let match_name = self
            .art
            .get(id)
            .and_then(|a| a.match_name.clone())
            .unwrap_or_else(|| "None (placeholder art)".to_owned());
        app.global::<HomeData>().set_properties(GameProperties {
            title: game.title.as_str().into(),
            console: console.into(),
            path: game.path.display().to_string().into(),
            size: size.into(),
            match_name: match_name.into(),
        });
    }

    fn controls_choices(&mut self, which: usize, input: &str) -> i32 {
        let Some(input) = PadInput::from_name(input) else {
            return 0;
        };
        let mut index = 0;
        self.with_page_data(which, |page, data| {
            let (choices, current) = page.choices(input);
            index = i32::try_from(current).unwrap_or(0);
            data.set_choices(ModelRc::from(Rc::new(VecModel::from(choices))));
        });
        index
    }

    fn controls_choose(&mut self, which: usize, input: &str, id: &str) {
        let Some(input) = PadInput::from_name(input) else {
            return;
        };
        let Some(page) = &mut self.pages[which] else {
            return;
        };
        let result = page.choose(input, id);
        self.after_edit(which, result);
    }

    fn controls_change_option(&mut self, which: usize, id: &str, step: i32) {
        let Some(page) = &mut self.pages[which] else {
            return;
        };
        let result = page.change_option(id, step);
        self.after_edit(which, result);
    }

    fn after_edit(&mut self, which: usize, result: Result<String, String>) {
        // Saved at once, so a live remapping is ready the moment the game is resumed.
        let affects = self.pages[which]
            .as_ref()
            .is_some_and(|p| p.affects_running);
        if result.is_ok() && affects && self.applier.is_live() {
            self.apply_running_controls();
        }
        let message = match result {
            Ok(message) => message,
            Err(err) => {
                tracing::warn!("saving controls failed: {err}");
                format!("Couldn't save: {err}")
            }
        };
        self.with_page_data(which, |page, data| {
            page.fill(data);
            data.set_message(message.as_str().into());
        });
        if !message.is_empty() {
            let app = self.app.clone();
            let overlay = self.overlay.as_ref().map(ComponentHandle::as_weak);
            self.message_timer
                .start(slint::TimerMode::SingleShot, MESSAGE_TIME, move || {
                    if let Some(app) = app.upgrade() {
                        app.global::<ControlsData>().set_message("".into());
                    }
                    if let Some(overlay) = overlay.as_ref().and_then(slint::Weak::upgrade) {
                        overlay.global::<ControlsData>().set_message("".into());
                    }
                });
        }
    }

    fn controls_closed(&mut self, which: usize) {
        let Some(page) = self.pages[which].take() else {
            return;
        };
        if !page.changed {
            return;
        }
        let title = self.title();
        let outcome = page
            .affects_running
            .then(|| self.apply_running_controls())
            .flatten();
        match outcome {
            Some(ApplyOutcome::Applied) if page.changed_next_launch => self.toast(&format!(
                "New buttons active in {title}; stick, trigger and vibration options apply next time you start it"
            )),
            Some(ApplyOutcome::Applied) => self.toast(&format!("New controls active in {title}")),
            Some(ApplyOutcome::NextLaunch) if which == MAIN => {
                self.toast(&format!("New controls apply next time you start {title}"));
            }
            _ => {}
        }
        self.refresh_settings();
    }

    /// Settings → Controllers rows: connected pads, the button test, each console.
    /// Settings → Display & sound: the selected pack of each category.
    fn fill_display_items(&mut self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let packs = packs::installed(&self.packs_dir);
        let items: Vec<SettingsCategory> = PackCategory::ALL
            .into_iter()
            .map(|category| {
                let status = match self.icons.selected(category) {
                    None => "Hearth default".to_owned(),
                    Some(id) => packs
                        .iter()
                        .find(|p| p.id == id)
                        .map_or_else(|| format!("{id} (missing)"), |p| p.name.clone()),
                };
                SettingsCategory {
                    key: category.key().into(),
                    label: match category {
                        PackCategory::ConsoleIcons => "Console icons",
                        PackCategory::UiSounds => "UI sounds",
                    }
                    .into(),
                    status: status.into(),
                }
            })
            .collect();
        app.global::<SettingsData>()
            .set_display_items(ModelRc::from(Rc::new(VecModel::from(items))));
    }

    /// Settings → Display & sound: the volumes and music options.
    fn fill_audio_rows(&mut self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let a = self.audio_settings;
        let slider = |key: &str, label: &str, value: u8| AudioRow {
            key: key.into(),
            label: label.into(),
            slider: true,
            value: i32::from(value),
            on: false,
            detail: SharedString::new(),
        };
        let toggle = |key: &str, label: &str, on: bool| AudioRow {
            key: key.into(),
            label: label.into(),
            slider: false,
            value: 0,
            on,
            detail: SharedString::new(),
        };
        let d = self.display_settings;
        let mut animated = toggle("animated_art", "Animated artwork", d.animated_art);
        if d.reduce_motion && d.animated_art {
            animated.detail = "Off · Reduce motion".into();
        }
        let rows = vec![
            slider("ui_volume", "UI sounds volume", a.ui_volume),
            slider("music_volume", "Music volume", a.music_volume),
            toggle("mute", "Mute all", a.mute),
            toggle("music_in_overlay", "Music in overlay", a.music_in_overlay),
            toggle("game_themes", "Game themes", a.game_themes),
            animated,
            toggle("reduce_motion", "Reduce motion", d.reduce_motion),
        ];
        app.global::<SettingsData>()
            .set_audio_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    }

    /// A volume step (`step` ±1 = 10) or a toggle (`step` 0), applied at once and saved.
    fn audio_change(&mut self, key: &str, step: i32) {
        if matches!(key, "animated_art" | "reduce_motion") {
            self.display_change(key);
            return;
        }
        let a = &mut self.audio_settings;
        let volume = |v: u8| (i32::from(v) + step * 10).clamp(0, 100) as u8;
        match key {
            "ui_volume" => a.ui_volume = volume(a.ui_volume),
            "music_volume" => a.music_volume = volume(a.music_volume),
            "mute" => a.mute = !a.mute,
            "music_in_overlay" => a.music_in_overlay = !a.music_in_overlay,
            "game_themes" => a.game_themes = !a.game_themes,
            _ => return,
        }
        let a = *a;
        self.audio.set_volumes(Volumes {
            ui: a.ui_volume,
            music: a.music_volume,
            mute: a.mute,
        });
        // Heard at the new level.
        if key == "ui_volume" {
            sounds::play("move");
        }
        if let Err(err) = config::save_audio(&self.config_file, &a) {
            tracing::warn!("saving the audio settings failed: {err}");
            self.toast(&format!("Couldn't save the setting: {err}"));
        }
        self.fill_audio_rows();
        self.sync_music(director::FADE_LAUNCH);
    }

    /// Toggles Animated artwork or Reduce motion, applied at once and saved.
    fn display_change(&mut self, key: &str) {
        let d = &mut self.display_settings;
        match key {
            "animated_art" => d.animated_art = !d.animated_art,
            "reduce_motion" => d.reduce_motion = !d.reduce_motion,
            _ => return,
        }
        let d = *d;
        if let Err(err) = config::save_display(&self.config_file, &d) {
            tracing::warn!("saving the display settings failed: {err}");
            self.toast(&format!("Couldn't save the setting: {err}"));
        }
        if key == "animated_art"
            && let Some(art) = &self.art_service
        {
            art.set_animated(d.animated_art);
        }
        self.fill_audio_rows();
        self.apply_motion();
    }

    /// Reduce motion: every transition off in both windows (`Theme.motion-scale`).
    fn apply_motion(&mut self) {
        let scale = if self.display_settings.reduce_motion {
            0.0
        } else {
            1.0
        };
        if let Some(app) = self.app.upgrade() {
            app.global::<Theme>().set_motion_scale(scale);
        }
        if let Some(overlay) = &self.overlay {
            overlay.global::<Theme>().set_motion_scale(scale);
        }
    }

    /// The pack list of one category: Hearth default, then the installed packs that have
    /// files for it.
    fn fill_pack_choices(&mut self, key: &str) {
        let (Some(app), Some(category)) = (
            self.app.upgrade(),
            PackCategory::ALL.into_iter().find(|c| c.key() == key),
        ) else {
            return;
        };
        let mut choices = vec![SettingsCategory {
            key: "".into(),
            label: "Hearth default".into(),
            status: "Built in".into(),
        }];
        choices.extend(
            packs::installed(&self.packs_dir)
                .into_iter()
                .filter(|p| p.categories.contains(&category))
                .map(|p| SettingsCategory {
                    key: p.id.as_str().into(),
                    label: p.name.as_str().into(),
                    status: p
                        .author
                        .map(|a| format!("by {a}"))
                        .unwrap_or_default()
                        .into(),
                }),
        );
        let settings = app.global::<SettingsData>();
        settings.set_pack_selected(self.icons.selected(category).unwrap_or_default().into());
        settings.set_pack_choices(ModelRc::from(Rc::new(VecModel::from(choices))));
    }

    /// Selects a pack (`""`: Hearth default), saves it, and applies console icons at once.
    fn choose_pack(&mut self, key: &str, id: &str) {
        let Some(category) = PackCategory::ALL.into_iter().find(|c| c.key() == key) else {
            return;
        };
        let pack = (!id.is_empty()).then_some(id);
        if let Err(err) = config::save_pack(&self.config_file, category, pack) {
            tracing::warn!("saving the pack choice failed: {err}");
            self.toast(&format!("Couldn't save the choice: {err}"));
        }
        tracing::info!(
            category = key,
            pack = pack.unwrap_or("Hearth default"),
            "pack selected"
        );
        self.icons.select(category, pack);
        if category == PackCategory::UiSounds {
            sounds::select_pack(pack);
            self.ambient = sounds::music_file("ambient_loop");
            self.sync_music(director::FADE_IN);
        }
        if let Some(app) = self.app.upgrade() {
            app.global::<SettingsData>().set_pack_selected(id.into());
        }
        self.fill_display_items();
        self.refresh_tiles();
    }

    /// The console rows of Settings → Controllers ("Customized" changes as controls are
    /// edited).
    fn refresh_settings(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let items: Vec<SettingsCategory> = self
            .systems
            .iter()
            .filter(|s| !s.controls.is_empty())
            .map(|system| {
                let customized =
                    hearth_core::controls::layers::system_file(system, &self.controls_dir)
                        .is_file();
                SettingsCategory {
                    key: format!("system:{}", system.id).into(),
                    label: system.name.as_str().into(),
                    status: if customized { "Customized" } else { "Default" }.into(),
                }
            })
            .collect();
        let settings = app.global::<SettingsData>();
        let current = settings.get_controller_items();
        if current.row_count() != items.len() || current.iter().zip(&items).any(|(a, b)| a != *b) {
            settings.set_controller_items(ModelRc::from(Rc::new(VecModel::from(items))));
        }
    }

    /// The connected controllers everywhere they show: Settings → Controllers and its status
    /// line, the Welcome widget, the top bar.
    fn refresh_pads(&self, pads: &[PadInfo], checking: bool) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let rows: Vec<PadRow> = pads.iter().map(super::pads::row).collect();
        let settings = app.global::<SettingsData>();
        let current = settings.get_pads();
        if current.row_count() == rows.len() {
            for (i, row) in rows.into_iter().enumerate() {
                if current.row_data(i).as_ref() != Some(&row) {
                    current.set_row_data(i, row);
                }
            }
        } else {
            settings.set_pads(ModelRc::from(Rc::new(VecModel::from(rows))));
        }
        let summary = if checking {
            super::pads::checking()
        } else {
            super::pads::summary(pads)
        };
        models::set_category_status(&app, "controllers", &summary.count);
        let status = app.global::<Status>();
        status.set_pads_value(summary.count.into());
        status.set_pads_detail(summary.detail.into());
        status.set_controller_battery(summary.battery.into());
    }

    /// The button test, for the pad with id `pad_id` (its name and battery in the header).
    fn start_button_test(&mut self, pad_id: &str) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        self.testing = true;
        let device = controls::device(&self.activity.pads(), &self.controls_dir);
        let live = self.input.is_live();
        let page = TestPage::new(&self.test_extras());
        let pad_id = pad_id.to_owned();
        let header = move |pads: Vec<PadInfo>| {
            let pad = pads.iter().find(|p| p.id == pad_id).or(pads.first());
            let name = pad.map_or_else(|| device.sdl_name.clone(), super::pads::display_name);
            controls_page::controller_line(&name, pad)
        };
        let line = header(self.activity.pads());
        let data = app.global::<ControlsData>();
        page.fill(&data, &line);
        // With InputPlumber, menus get presses only. Raw values come from the virtual pad,
        // which may only get the input while no game could see it.
        let raw = match self.raw_activity.clone().filter(|_| live) {
            Some(raw) if self.active.is_none() => {
                self.input.set_mode(InputMode::Test);
                Some(raw)
            }
            Some(_) => {
                data.set_message("Stick and trigger values show when no game is running".into());
                None
            }
            None => None,
        };
        let activity = self.activity.clone();
        let weak = self.app.clone();
        let update = move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let mut pressed = activity.pressed();
            let analog = match &raw {
                Some(raw) => {
                    for input in raw.pressed() {
                        if !pressed.contains(&input) {
                            pressed.push(input);
                        }
                    }
                    raw.analog()
                }
                None => activity.analog(),
            };
            page.update(&pressed, &analog);
            let data = app.global::<ControlsData>();
            let line = header(activity.pads());
            if data.get_scope_line() != line.as_str() {
                data.set_scope_line(line.into());
            }
        };
        update();
        self.test_timer
            .start(slint::TimerMode::Repeated, TEST_POLL, update);
        app.invoke_open_button_test();
    }

    fn stop_button_test(&mut self) {
        self.testing = false;
        self.test_timer.stop();
        self.activity.disarm();
        // The controller back to Hearth's menus (InputPlumber).
        self.set_mode(self.mode);
        if let Some(app) = self.app.upgrade() {
            app.global::<ControlsData>().set_message("".into());
            app.invoke_close_button_test();
        }
    }

    fn toast(&self, message: &str) {
        tracing::info!(message, "toast");
        let Some(app) = self.app.upgrade() else {
            return;
        };
        app.global::<Status>().set_toast(message.into());
        let weak = self.app.clone();
        self.toast_timer
            .start(slint::TimerMode::SingleShot, TOAST_TIME, move || {
                if let Some(app) = weak.upgrade() {
                    app.global::<Status>().set_toast("".into());
                }
            });
    }

    /// Shows a problem Hearth worked around, for [`PROBLEM_TIME`].
    fn show_problem(&self, title: &str, text: &str) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let status = app.global::<Status>();
        status.set_notice_title(title.into());
        status.set_notice_text(text.into());
        let weak = self.app.clone();
        self.problem_timer
            .start(slint::TimerMode::SingleShot, PROBLEM_TIME, move || {
                if let Some(app) = weak.upgrade() {
                    app.global::<Status>().set_notice_title("".into());
                }
            });
    }

    fn clear_toast(&self) {
        self.toast_timer.stop();
        if let Some(app) = self.app.upgrade() {
            app.global::<Status>().set_toast("".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_warns_once_per_level_per_charge() {
        let mut warned = Vec::new();
        assert_eq!(battery_warning(35, &mut warned), None);
        assert_eq!(battery_warning(20, &mut warned), Some(20));
        assert_eq!(battery_warning(15, &mut warned), None);
        assert_eq!(battery_warning(10, &mut warned), Some(10));
        assert_eq!(battery_warning(5, &mut warned), None);
        // Charging clears it (the caller empties the list).
        warned.clear();
        // Found already at 5 %: one warning, the 10 % one.
        assert_eq!(battery_warning(5, &mut warned), Some(10));
        assert_eq!(battery_warning(15, &mut warned), None);
    }
}
