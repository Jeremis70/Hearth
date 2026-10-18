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

use hearth_core::controls::edit::{Editor, Scope};
use hearth_core::controls::{ControllerKind, GameKey, HearthAction, PadInput, Target, defaults};
use hearth_core::library::{Library, LibraryEntry};
use hearth_core::model::{GameId, SystemId};
use hearth_core::profiles::{DolphinProfile, RetroArchProfile};
use hearth_core::runtime::{
    LaunchCtx, ProcessRuntime, Runtime, RuntimeEvent, Session as RuntimeSession, SessionId,
};
use hearth_core::scan::scan_roms;
use hearth_core::session::{ActiveSession, SessionState};
use hearth_core::systems::{EmulatorKind, SuspendPolicy, Systems};
use hearth_platform::gamescope::{AppId, Compositor, SHELL_APP_ID};
use hearth_platform::input::{Action, InputControl, InputMode, PadActivity};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::rc::Rc;

use super::controls_page::{
    self, ApplyOutcome, EmulatorConfigApplier, InputPlumberApplier, MappingApplier, Page, TestPage,
};
use super::models;
use crate::controls;
use crate::{
    AppWindow, ControllerType, ControlsData, OverlayPanel, OverlayWindow, Session,
    SettingsCategory, SettingsData, Status,
};

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
    active: Option<ActiveSession>,
    closing: bool,
    mode: Mode,
    events: Sender<RuntimeEvent>,
    next_app_id: AppId,
    toast_timer: slint::Timer,
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
    test_timer: slint::Timer,
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
        session.on_home_button(|long| defer(move |c| c.home_button(long)));
        session.on_open_controls(|| defer(|c| c.open_game_controls(MAIN)));
        app.global::<SettingsData>().on_controller_item(|key| {
            let key = key.to_string();
            defer(move |c| c.settings_controller_item(&key));
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

    load_library(
        setup.library_db.clone(),
        setup.games_dir.clone(),
        setup.systems.clone(),
    );

    let applier: Box<dyn MappingApplier> = if setup.input.is_live() {
        Box::new(InputPlumberApplier {
            path: setup.runtime_dir.join("inputplumber").join("game.yaml"),
            input: setup.input.clone(),
        })
    } else {
        Box::new(EmulatorConfigApplier)
    };
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
        active: None,
        closing: false,
        mode: Mode::Shell,
        events,
        next_app_id: FIRST_GAME_APP_ID,
        toast_timer: slint::Timer::default(),
        notice_timer: slint::Timer::default(),
        game_capture: Vec::new(),
        focus_timer: slint::Timer::default(),
        pages: [None, None],
        applier,
        message_timer: slint::Timer::default(),
        testing: false,
        test_timer: slint::Timer::default(),
    };
    CONTROLLER.with(|cell| *cell.borrow_mut() = Some(controller));
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

/// Scans the games root and loads the library on a worker thread.
fn load_library(db: PathBuf, games_dir: PathBuf, systems: Systems) {
    let spawned = thread::Builder::new()
        .name("hearth-library".into())
        .spawn(move || {
            let result = Library::open(&db).and_then(|mut library| {
                let games = scan_roms(&games_dir, &systems);
                tracing::info!(count = games.len(), root = %games_dir.display(), "scanned games");
                library.sync(&games)?;
                library.recent()
            });
            match result {
                Ok(entries) => defer(move |c| c.set_entries(entries)),
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

/// What the Settings page calls a controller family.
fn kind_name(kind: ControllerKind) -> &'static str {
    match kind {
        ControllerKind::DualSense => "PlayStation (DualSense)",
        ControllerKind::DualShock4 => "PlayStation (DualShock 4)",
        ControllerKind::Xbox => "Xbox",
        ControllerKind::SwitchPro => "Nintendo Switch Pro",
        ControllerKind::Generic => "Generic",
    }
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
            Mode::Shell => self.show_home(),
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
                let title = active.game.title.clone();
                self.toast(&format!("Close {title} first"));
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
        if self.input.is_live() && self.applier.apply(&input) != ApplyOutcome::Applied {
            // No remapping rather than the previous game's.
            self.input.set_game_profile(None);
        }
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
        let retroarch = emulator.kind == EmulatorKind::Retroarch;
        if retroarch && !self.profile.has_autoconfig() {
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
        self.show_home();
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
            overlay.set_panel(OverlayPanel::None);
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
        self.show_home();
    }

    fn show_home(&self) {
        if let Some(app) = self.app.upgrade() {
            app.invoke_go_home();
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
        self.show_home();
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

        self.activity.disarm();
        self.hide_overlay();
        let _ = self.compositor.focus(&[SHELL_APP_ID]);
        self.set_mode(Mode::Shell);
        self.refresh();
        self.show_home();
        let message = if success || closed_by_us {
            format!("{title}: {}", models::format_session_total(played))
        } else {
            format!("{title} stopped unexpectedly")
        };
        self.toast(&message);

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

    fn title(&self) -> String {
        self.active
            .as_ref()
            .map(|a| a.game.title.clone())
            .unwrap_or_default()
    }

    /// Rebuilds the tile row and the session globals.
    fn refresh(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let running = self.active.as_ref().map(|a| &a.game.id);
        models::set_tiles(
            &app,
            models::home_tiles(&self.entries, &self.systems, running, unix_now()),
        );
        self.sync_session_ui();
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
        if key == "test" {
            self.start_button_test();
            return;
        }
        let Some(id) = key.strip_prefix("system:") else {
            return;
        };
        let Some(system) = self.systems.get(&SystemId::new(id)).cloned() else {
            return;
        };
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
        self.with_page_data(MAIN, |page, data| {
            data.set_message("".into());
            page.fill(data);
        });
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
            true,
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
    fn refresh_settings(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let settings = app.global::<SettingsData>();
        let info: Vec<SettingsCategory> = self
            .activity
            .pads()
            .iter()
            .map(|pad| SettingsCategory {
                key: "".into(),
                label: pad.os_name.as_str().into(),
                status: kind_name(ControllerKind::from_ids(pad.vendor_id, pad.product_id)).into(),
            })
            .collect();
        let mut items = vec![SettingsCategory {
            key: "test".into(),
            label: "Test buttons".into(),
            status: "".into(),
        }];
        for system in self.systems.iter().filter(|s| !s.controls.is_empty()) {
            let customized =
                hearth_core::controls::layers::system_file(system, &self.controls_dir).is_file();
            items.push(SettingsCategory {
                key: format!("system:{}", system.id).into(),
                label: format!("{} controls", system.name).into(),
                status: if customized { "Customized" } else { "Default" }.into(),
            });
        }
        if settings.get_controller_info().row_count() != info.len() || info.is_empty() {
            settings.set_controller_info(ModelRc::from(Rc::new(VecModel::from(info))));
        }
        settings.set_controller_items(ModelRc::from(Rc::new(VecModel::from(items))));
    }

    fn start_button_test(&mut self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        self.testing = true;
        let device = controls::device(&self.activity.pads(), &self.controls_dir);
        let live = self.input.is_live();
        // Guide, and the extras the backend reads.
        let extras: Vec<&'static str> = std::iter::once("guide")
            .chain(
                self.pad_extras()
                    .into_iter()
                    .filter(|i| live || *i != PadInput::Touchpad)
                    .map(PadInput::name),
            )
            .collect();
        let page = TestPage::new(&extras);
        let name = device.sdl_name.clone();
        let line = controls_page::controller_line(&name, self.activity.pads().first());
        let data = app.global::<ControlsData>();
        page.fill(&data, &line);
        // With InputPlumber, menus get presses only. Raw values come from the virtual pad,
        // which may only get the input while no game could see it.
        let raw = match &self.raw_activity {
            Some(raw) if self.active.is_none() => {
                self.input.set_mode(InputMode::Test);
                Some(raw.clone())
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
            let line = controls_page::controller_line(&name, activity.pads().first());
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

    fn clear_toast(&self) {
        self.toast_timer.stop();
        if let Some(app) = self.app.upgrade() {
            app.global::<Status>().set_toast("".into());
        }
    }
}
