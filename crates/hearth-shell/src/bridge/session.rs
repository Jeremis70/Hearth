//! Game sessions: launching, the in-game overlay, going Home, resuming and closing. Ties the
//! runtime, gamescope and the UI together. Lives on the Slint thread.
//!
//! Inside gamescope:
//! - Guide short press while playing: freeze the game, show the overlay over it.
//! - Guide long press while playing: freeze the game, show Hearth's Home.
//! - Resume (overlay, Home tile, Guide again in the overlay): unfreeze, show the game.
//!
//! Outside gamescope (or in gamescope without Steam mode, where focus can't be switched) the
//! game keeps the screen, there is no overlay, and a long press closes the game.
//!
//! Whatever fails (window never appears, focus switch refused), Hearth stays in front and the
//! Guide button keeps working.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hearth_core::library::{Library, LibraryEntry};
use hearth_core::model::GameId;
use hearth_core::profiles::RetroArchProfile;
use hearth_core::runtime::{
    LaunchCtx, ProcessRuntime, Runtime, RuntimeEvent, Session as RuntimeSession, SessionId,
};
use hearth_core::scan::scan_roms;
use hearth_core::session::{ActiveSession, SessionState};
use hearth_core::systems::{EmulatorKind, SuspendPolicy, Systems};
use hearth_platform::gamescope::{AppId, Compositor, SHELL_APP_ID};
use hearth_platform::input::Action;
use slint::ComponentHandle;

use super::models;
use crate::{AppWindow, OverlayWindow, Session, Status};

/// How long to look for a game's window before giving up.
const WINDOW_SEARCH: Duration = Duration::from_secs(15);
const WINDOW_POLL: Duration = Duration::from_millis(250);
/// Windows found later (e.g. re-created for fullscreen) are tagged at this interval.
const RETAG_INTERVAL: Duration = Duration::from_secs(1);
/// Delay before checking that gamescope really shows the game.
const FOCUS_CHECK: Duration = Duration::from_millis(700);
const TOAST_TIME: Duration = Duration::from_secs(5);
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
    library_db: PathBuf,
    entries: Vec<LibraryEntry>,
    active: Option<ActiveSession>,
    closing: bool,
    mode: Mode,
    events: Sender<RuntimeEvent>,
    next_app_id: AppId,
    toast_timer: slint::Timer,
}

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
    }
    if let Some(overlay) = &setup.overlay {
        overlay.on_resume(|| defer(Controller::resume_game));
        overlay.on_go_home(|| defer(Controller::go_home));
        overlay.on_close_game(|| defer(Controller::close_game));
        overlay.on_home_button(|long| defer(move |c| c.home_button(long)));
        overlay.on_dock(|item| tracing::info!(%item, "overlay dock item not implemented yet"));
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

    let controller = Controller {
        app: setup.app,
        overlay: setup.overlay,
        overlay_window: None,
        compositor: setup.compositor,
        runtime: Arc::new(ProcessRuntime::new()),
        systems: setup.systems,
        profile: setup.profile,
        library_db: setup.library_db,
        entries: Vec::new(),
        active: None,
        closing: false,
        mode: Mode::Shell,
        events,
        next_app_id: FIRST_GAME_APP_ID,
        toast_timer: slint::Timer::default(),
    };
    CONTROLLER.with(|cell| *cell.borrow_mut() = Some(controller));
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
        self.sync_session_ui();
    }

    fn route(&mut self, action: Action) -> Route {
        match action {
            Action::HomeShort => {
                self.home_button(false);
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
                // The game owns the controller; gilrs can't hide input from it, so drop ours.
                Mode::Game | Mode::Launching => Route::None,
            },
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
        let mut command = match (emulator.kind, emulator.core.as_ref()) {
            (EmulatorKind::Retroarch, Some(core)) if core.is_file() => {
                if let Err(err) = self.profile.prepare() {
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
        };
        if !self.profile.has_autoconfig() {
            self.toast("No controller profiles for RetroArch: the pad won't work in the game");
        } else if !self.compositor.controls_focus() {
            self.toast("No overlay outside gamescope: hold the Guide button to close the game");
        }
        if self.compositor.is_active() {
            // Launcher rule: inside gamescope, games must never reach the host's Wayland.
            command.force_x11();
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
            self.mode = Mode::Launching;
            self.toast(&format!("Starting {title}…"));
            self.search_window(session, app_id);
        } else {
            // Outside gamescope (or without Steam mode) the game keeps the screen.
            self.mode = Mode::Game;
            if let Some(active) = &mut self.active {
                active.state = SessionState::Playing;
            }
        }
        self.refresh();
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
                                windows = n,
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
                            "no window with _NET_WM_PID in the game's process group or its \
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

    /// Puts the game in front. On any failure Hearth stays in front.
    fn show_game(&mut self) {
        let Some(app_id) = self.active.as_ref().map(|a| a.app_id) else {
            return;
        };
        self.hide_overlay();
        self.thaw();
        if !self.compositor.controls_focus() {
            self.mode = Mode::Game;
            return;
        }
        if let Err(err) = self.compositor.focus(&[app_id, SHELL_APP_ID]) {
            self.game_not_shown(&format!("focus switch failed: {err}"));
            return;
        }
        self.mode = Mode::Game;
        if let Some(active) = &mut self.active {
            active.state = SessionState::Playing;
        }
        self.refresh();
        let id = self.active.as_ref().map(|a| a.session.id());
        slint::Timer::single_shot(FOCUS_CHECK, move || {
            with(|c| {
                if id.is_some_and(|id| c.is_current(id)) {
                    c.check_focus(app_id);
                }
            });
        });
    }

    /// Gamescope may accept the property but keep Hearth in front (e.g. not in Steam mode).
    fn check_focus(&mut self, app_id: AppId) {
        if self.mode != Mode::Game {
            return;
        }
        match self.compositor.focused_app() {
            Ok(Some(focused)) if focused == app_id => {}
            Ok(focused) => {
                self.game_not_shown(&format!("gamescope kept app {focused:?} in front"));
            }
            Err(err) => self.game_not_shown(&format!("cannot read gamescope focus: {err}")),
        }
    }

    fn game_not_shown(&mut self, reason: &str) {
        tracing::warn!("couldn't show the game: {reason}");
        let _ = self.compositor.focus(&[SHELL_APP_ID]);
        self.freeze();
        self.mode = Mode::Shell;
        if let Some(active) = &mut self.active {
            active.state = SessionState::InBackground;
        }
        let title = self.title();
        self.refresh();
        self.show_home();
        self.toast(&format!(
            "Couldn't show {title}. Resume or close it from Home"
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
        self.freeze();
        self.sync_session_ui();
        overlay.invoke_reset();
        overlay.set_shown(true);
        if let Err(err) = self.compositor.set_overlay(window, true) {
            tracing::warn!("showing the overlay failed: {err}");
            self.go_home();
            return;
        }
        self.mode = Mode::Overlay;
        if let Some(active) = &mut self.active {
            active.state = SessionState::InBackground;
        }
    }

    fn hide_overlay(&mut self) {
        if let Some(overlay) = &self.overlay {
            overlay.set_shown(false);
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
        self.hide_overlay();
        self.freeze();
        if let Err(err) = self.compositor.focus(&[SHELL_APP_ID]) {
            tracing::warn!("focusing Hearth failed: {err}");
        }
        self.mode = Mode::Shell;
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
        self.mode = Mode::Shell;
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

        self.hide_overlay();
        let _ = self.compositor.focus(&[SHELL_APP_ID]);
        self.mode = Mode::Shell;
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
