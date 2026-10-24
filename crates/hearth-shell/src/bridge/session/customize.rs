//! A game's Customize page (design README §4 "Customize"): its artwork, which game it was
//! matched to, and its theme music, from the couch.
//!
//! The page shows what the art worker lists (`hearth_core::art::service`), and every change
//! goes through the worker too (so `custom.toml` writes never race); the worker answers with
//! new art (Home and the page update) and the game's picks. Reads (what's picked now) come
//! straight from the store. Thumbnails are decoded on a worker thread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use hearth_core::art::service::{Availability, Candidate as Listed, CandidateSet};
use hearth_core::art::{ArtEvent, ArtKey, ArtKind};
use hearth_core::custom::{CENTER, Lookup, Source};
use hearth_core::extras::{Extra, folder_candidates};
use hearth_core::files::{self, FileKind, ListedFile, Place};
use hearth_core::model::{Game, GameId};
use slint::{ComponentHandle, Image, ModelRc, VecModel};

use super::{Controller, defer, director, with};
use crate::bridge::art::decode_fit;
use crate::{AppWindow, Candidate, CustomRow, CustomSection, CustomizeData, FileEntry};

/// How far one Left/Right moves the hero (a share of the part cropped away).
const POSITION_STEP: f32 = 0.05;
/// How far one Left/Right moves a theme's start.
const START_STEP: f32 = 1.0;
/// Focus stays on an audio file this long before it plays.
const PREVIEW_SETTLE: Duration = Duration::from_millis(500);
/// Thumbnails are decoded this size (twice the largest tile, for sharpness).
const THUMB_MAX: (u32, u32) = (720, 360);

/// The page's sections, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Section {
    Art(ArtKind),
    Theme,
    Match,
}

const SECTIONS: [Section; 6] = [
    Section::Art(ArtKind::Grid),
    Section::Art(ArtKind::Hero),
    Section::Art(ArtKind::Logo),
    Section::Art(ArtKind::Icon),
    Section::Theme,
    Section::Match,
];

impl Section {
    fn key(self) -> &'static str {
        match self {
            Self::Art(kind) => kind.name(),
            Self::Theme => "theme",
            Self::Match => "match",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        SECTIONS.into_iter().find(|s| s.key() == key)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Art(ArtKind::Grid) => "Cover",
            Self::Art(ArtKind::Hero) => "Hero",
            Self::Art(ArtKind::Logo) => "Logo",
            Self::Art(ArtKind::Icon) => "Icon",
            Self::Theme => "Theme music",
            Self::Match => "Wrong game?",
        }
    }

    fn set(self) -> Option<CandidateSet> {
        match self {
            Self::Art(kind) => Some(CandidateSet::Art(kind)),
            Self::Theme => None,
            Self::Match => Some(CandidateSet::Games),
        }
    }

    fn extra(self) -> Option<Extra> {
        match self {
            Self::Art(kind) => Some(Extra::Art(kind)),
            Self::Theme => Some(Extra::Theme),
            Self::Match => None,
        }
    }
}

/// The open page.
pub(super) struct CustomizeState {
    game: Game,
    key: ArtKey,
    section: Section,
    /// Focus is in the right pane (the theme previews meanwhile).
    in_pane: bool,
    /// Candidate lists as last listed, and why they may be stale.
    lists: HashMap<CandidateSet, (Vec<Listed>, Availability)>,
    /// Lists asked for (once per page, to stay within SteamGridDB's limits).
    requested: HashSet<CandidateSet>,
    /// Decoded thumbnails (candidates, the art in use, files).
    thumbs: HashMap<PathBuf, Image>,
    decoding: HashSet<PathBuf>,
    /// The file list, while open.
    files: Option<(FileKind, Vec<ListedFile>)>,
    durations: HashMap<PathBuf, Duration>,
    /// The audio file previewed (focused long enough).
    file_preview: Option<PathBuf>,
    /// The hero's position before positioning started (to cancel), while positioning.
    position_before: Option<f32>,
    /// A change asked of the art worker and not answered yet (what the page shows meanwhile,
    /// and the strip tile being applied).
    busy: Option<(&'static str, Option<usize>)>,
}

impl CustomizeState {
    /// True while `game`'s hero is being positioned.
    pub(super) fn positioning(&self, game: &GameId) -> bool {
        &self.game.id == game && self.position_before.is_some()
    }
}

/// Connects the page's callbacks.
pub(super) fn wire(app: &AppWindow) {
    let data = app.global::<CustomizeData>();
    data.on_section_changed(|key| {
        let key = key.to_string();
        defer(move |c| c.customize_section(&key));
    });
    data.on_pane_focused(|in_pane| defer(move |c| c.customize_pane(in_pane)));
    data.on_candidate(|index| {
        let index = usize::try_from(index).unwrap_or(0);
        defer(move |c| c.customize_candidate(index));
    });
    data.on_row(|key| {
        let key = key.to_string();
        defer(move |c| c.customize_row(&key));
    });
    data.on_step(|key, delta| {
        let key = key.to_string();
        defer(move |c| c.customize_step(&key, delta));
    });
    data.on_position_done(|save| defer(move |c| c.customize_position_done(save)));
    data.on_file_focused(|index| {
        let index = usize::try_from(index).unwrap_or(0);
        defer(move |c| c.customize_file_focused(index));
    });
    data.on_file_chosen(|index| {
        let index = usize::try_from(index).unwrap_or(0);
        defer(move |c| c.customize_file_chosen(index));
    });
    data.on_files_closed(|| defer(Controller::customize_files_closed));
    data.on_closed(|| defer(Controller::customize_closed));
}

/// "12 s", "1:05".
fn format_start(seconds: f32) -> String {
    let s = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn format_duration(d: Duration) -> String {
    format_start(d.as_secs_f32())
}

fn format_age(modified: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
        .as_secs();
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        86_400..172_800 => "yesterday".into(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

fn position_label(position: f32) -> String {
    if (position - CENTER).abs() < 0.01 {
        "Center".into()
    } else {
        format!("{:.0} %", position * 100.0)
    }
}

impl Controller {
    fn customize_app(&self) -> Option<AppWindow> {
        self.customize.as_ref()?;
        self.app.upgrade()
    }

    /// Opens the page for `id` on `section` ("art", "theme" or "match").
    pub(super) fn open_customize(&mut self, id: &GameId, section: &str) {
        let Some(game) = self
            .entries
            .iter()
            .find(|e| &e.game.id == id)
            .map(|e| e.game.clone())
        else {
            return;
        };
        let section = match section {
            "theme" => Section::Theme,
            "match" => Section::Match,
            _ => Section::Art(ArtKind::Grid),
        };
        let key = self.picks.key_of(&game);
        tracing::info!(game = %game.id, %key, section = section.key(), "customize");
        self.customize = Some(CustomizeState {
            game,
            key,
            section,
            in_pane: false,
            lists: HashMap::new(),
            requested: HashSet::new(),
            thumbs: HashMap::new(),
            decoding: HashSet::new(),
            files: None,
            durations: HashMap::new(),
            file_preview: None,
            position_before: None,
            busy: None,
        });
        self.request_candidates();
        self.decode_current();
        self.fill_customize();
        self.keep_theme();
        let index = SECTIONS.iter().position(|s| *s == section).unwrap_or(0);
        if let Some(app) = self.app.upgrade() {
            app.invoke_open_customize(i32::try_from(index).unwrap_or(0));
        }
    }

    pub(super) fn customize_closed(&mut self) {
        if let Some(state) = self.customize.take()
            && let Some(before) = state.position_before
        {
            self.set_hero_position_live(&state.game.id, before);
        }
        self.preview_timer.stop();
        if self.preview.take().is_some() {
            self.sync_music(director::FADE_LAUNCH);
        }
    }

    fn customize_section(&mut self, key: &str) {
        let Some(section) = Section::from_key(key) else {
            return;
        };
        let Some(state) = self.customize.as_mut() else {
            return;
        };
        if state.section == section {
            return;
        }
        state.section = section;
        state.in_pane = false;
        self.request_candidates();
        self.decode_current();
        self.fill_customize();
        self.update_preview();
    }

    fn customize_pane(&mut self, in_pane: bool) {
        if let Some(state) = self.customize.as_mut() {
            state.in_pane = in_pane;
        }
        self.update_preview();
    }

    /// Asks the worker for the focused section's list, once per page.
    fn request_candidates(&mut self) {
        let Some(state) = self.customize.as_mut() else {
            return;
        };
        let Some(set) = state.section.set() else {
            return;
        };
        // Again when a visit was cut short before every thumbnail arrived (moving on stops
        // them): the list comes from SteamGridDB again, the thumbs already cached are kept.
        let incomplete = state.lists.get(&set).is_some_and(|(items, a)| {
            *a == Availability::Live && items.iter().any(|i| i.thumb.is_none())
        });
        if (state.requested.insert(set) || incomplete)
            && let Some(service) = &self.art_service
        {
            service.candidates(state.key.clone(), set);
        }
    }

    fn customize_candidate(&mut self, index: usize) {
        // The "In use" tile first (art not among the candidates): nothing to apply.
        let in_use = self.customize.as_ref().is_some_and(|s| self.in_use_tile(s));
        let Some(index) = (if in_use {
            index.checked_sub(1)
        } else {
            Some(index)
        }) else {
            return;
        };
        let Some(state) = self.customize.as_mut() else {
            return;
        };
        let Some(set) = state.section.set() else {
            return;
        };
        let Some(item) = state
            .lists
            .get(&set)
            .and_then(|(l, _)| l.get(index))
            .cloned()
        else {
            return;
        };
        let (key, game) = (state.key.clone(), state.game.id.clone());
        let Some(service) = self.art_service.clone() else {
            return;
        };
        match set {
            CandidateSet::Art(kind) => {
                if item.current {
                    return;
                }
                tracing::info!(%key, kind = kind.name(), asset = item.id, "artwork picked");
                state.busy = Some(("Downloading it…", Some(index + usize::from(in_use))));
                service.apply(key, kind, item.id);
            }
            CandidateSet::Games => {
                if item.current {
                    return;
                }
                tracing::info!(%key, id = item.id, name = %item.name, "matched by the user");
                // The old match's lists are no good any more.
                state.lists.clear();
                state.requested.clear();
                state.requested.insert(CandidateSet::Games);
                state.busy = Some(("Matching…", None));
                service.rematch(key, item.id, item.name.clone());
                self.toast(&format!("Matched to {}", item.name));
                // The new game's name shows once the worker saved it.
                if let Some(entry) = self.art.get_mut(&game) {
                    entry.match_name = Some(item.name);
                }
                self.request_candidates();
            }
        }
        self.fill_customize();
    }

    fn customize_row(&mut self, row: &str) {
        let Some(state) = self.customize.as_mut() else {
            return;
        };
        let key = state.key.clone();
        let section = state.section;
        match row {
            "file" => self.open_files(),
            "reset" => {
                if let (Some(service), Some(extra)) = (&self.art_service, section.extra()) {
                    state.busy = Some(("Resetting…", None));
                    service.reset(key, extra);
                    self.fill_customize();
                }
            }
            "remove" => {
                if let Some(service) = &self.art_service {
                    state.busy = Some(("Removing…", None));
                    service.remove_theme(key);
                    self.fill_customize();
                }
            }
            "position" => {
                let current = self
                    .art
                    .get(&state.game.id)
                    .and_then(|a| a.hero_position)
                    .unwrap_or(CENTER);
                state.position_before = Some(current);
            }
            _ => {}
        }
    }

    fn customize_step(&mut self, row: &str, delta: i32) {
        let Some(state) = self.customize.as_ref() else {
            return;
        };
        let (game, key) = (state.game.id.clone(), state.key.clone());
        match row {
            "position" => {
                let current = self
                    .art
                    .get(&game)
                    .and_then(|a| a.hero_position)
                    .unwrap_or(CENTER);
                let next = (current + POSITION_STEP * delta as f32).clamp(0.0, 1.0);
                self.set_hero_position_live(&game, next);
            }
            "start" => {
                let current = self.picks.load(&key).theme_start();
                let next = (current + START_STEP * delta as f32).max(0.0);
                if (next - current).abs() > f32::EPSILON
                    && let Some(service) = &self.art_service
                {
                    service.set_theme_start(key, next);
                }
                // The preview restarts at once from the new start (the store follows).
                self.update_preview_with_start(Some(next));
                self.fill_customize_rows(Some(next));
            }
            _ => {}
        }
    }

    /// Moves the hero on Home and behind the page (not saved yet).
    fn set_hero_position_live(&mut self, game: &GameId, position: f32) {
        self.art.entry(game.clone()).or_default().hero_position = Some(position);
        self.refresh_tiles();
        if let Some(app) = self.customize_app() {
            app.global::<CustomizeData>().set_hero_x(position);
        }
        self.fill_customize_rows(None);
    }

    fn customize_position_done(&mut self, save: bool) {
        let Some(state) = self.customize.as_mut() else {
            return;
        };
        let Some(before) = state.position_before.take() else {
            return;
        };
        let (game, key) = (state.game.id.clone(), state.key.clone());
        if save {
            let position = self
                .art
                .get(&game)
                .and_then(|a| a.hero_position)
                .unwrap_or(CENTER);
            tracing::info!(%key, position, "hero position");
            if let Some(service) = &self.art_service {
                service.set_hero_position(key, position);
            }
        } else {
            self.set_hero_position_live(&game, before);
        }
    }

    // --- the file list ---------------------------------------------------------------

    fn open_files(&mut self) {
        let Some(state) = self.customize.as_mut() else {
            return;
        };
        let kind = if state.section == Section::Theme {
            FileKind::Audio
        } else {
            FileKind::Image
        };
        let mut places = Vec::new();
        if let Some(dir) = hearth_core::paths::downloads_dir() {
            places.push(Place {
                label: "Downloads".into(),
                dir,
            });
        }
        if let Some(dir) = &state.game.folder {
            places.push(Place {
                label: "Game folder".into(),
                dir: dir.clone(),
            });
        }
        let listed = files::list(&places, kind);
        tracing::debug!(count = listed.len(), "file list");
        let paths: Vec<PathBuf> = listed.iter().map(|f| f.path.clone()).collect();
        state.files = Some((kind, listed));
        match kind {
            FileKind::Image => self.decode_thumbs(paths),
            FileKind::Audio => measure_durations(paths),
        }
        self.fill_files();
        if kind == FileKind::Audio {
            self.customize_file_focused(0);
        }
    }

    fn customize_file_focused(&mut self, index: usize) {
        let Some(state) = self.customize.as_mut() else {
            return;
        };
        let Some((FileKind::Audio, files)) = &state.files else {
            return;
        };
        let Some(path) = files.get(index).map(|f| f.path.clone()) else {
            return;
        };
        self.preview_timer
            .start(slint::TimerMode::SingleShot, PREVIEW_SETTLE, move || {
                with(|c| {
                    if let Some(state) = c.customize.as_mut() {
                        state.file_preview = Some(path.clone());
                    }
                    c.update_preview();
                });
            });
    }

    fn customize_file_chosen(&mut self, index: usize) {
        let Some(state) = self.customize.as_mut() else {
            return;
        };
        let file = state
            .files
            .take()
            .and_then(|(_, files)| files.into_iter().nth(index));
        state.file_preview = None;
        let (key, extra) = (state.key.clone(), state.section.extra());
        self.preview_timer.stop();
        if let (Some(file), Some(extra), Some(service)) = (file, extra, &self.art_service) {
            tracing::info!(%key, extra = extra.name(), file = %file.path.display(), "file picked");
            state.busy = Some(("Copying it…", None));
            service.pick_file(key, extra, file.path);
        }
        self.fill_customize();
        self.update_preview();
    }

    fn customize_files_closed(&mut self) {
        if let Some(state) = self.customize.as_mut() {
            state.files = None;
            state.file_preview = None;
        }
        self.preview_timer.stop();
        self.update_preview();
    }

    // --- music -----------------------------------------------------------------------

    fn update_preview(&mut self) {
        self.update_preview_with_start(None);
    }

    /// The preview: the focused audio file in the file list, else the theme while focus is
    /// in the Theme section's pane (from `start`, if just changed), else nothing.
    fn update_preview_with_start(&mut self, start: Option<f32>) {
        let preview = self.customize.as_ref().and_then(|state| {
            if state.files.is_some() {
                return state
                    .file_preview
                    .clone()
                    .map(|path| self.auto_looped(path, "preview", 0.0));
            }
            if state.section != Section::Theme || !state.in_pane {
                return None;
            }
            let theme = self.extras.find(&state.game, Extra::Theme)?;
            let start = start.unwrap_or_else(|| self.picks.load(&state.key).theme_start());
            Some(self.auto_looped(theme.path, theme.source, start))
        });
        if preview != self.preview {
            self.preview = preview;
            self.sync_music(director::FADE_LAUNCH);
        }
    }

    // --- events from the art worker ------------------------------------------------------

    /// The worker's answers for the page (candidates, thumbs, new picks, failures).
    pub(crate) fn on_customize_event(&mut self, event: ArtEvent) {
        match event {
            ArtEvent::Candidates {
                key,
                set,
                items,
                availability,
            } => {
                let Some(state) = self.customize.as_mut().filter(|s| s.key == key) else {
                    return;
                };
                let paths: Vec<PathBuf> = items.iter().filter_map(|i| i.thumb.clone()).collect();
                let current = items.iter().position(|i| i.current);
                state.lists.insert(set, (items, availability));
                let shown = state.section.set() == Some(set);
                self.decode_thumbs(paths);
                self.fill_customize();
                // Where the strip starts: the one in use (after an "In use" tile, or that tile).
                let in_use = self.customize.as_ref().is_some_and(|s| self.in_use_tile(s));
                let current = match current {
                    Some(i) => Some(i + usize::from(in_use)),
                    None => in_use.then_some(0),
                };
                if shown && let (Some(app), Some(current)) = (self.app.upgrade(), current) {
                    app.invoke_customize_current(i32::try_from(current).unwrap_or(0));
                }
            }
            ArtEvent::Thumb { key, set, id, path } => {
                let Some(state) = self.customize.as_mut().filter(|s| s.key == key) else {
                    return;
                };
                if let Some(item) = state
                    .lists
                    .get_mut(&set)
                    .and_then(|(l, _)| l.iter_mut().find(|i| i.id == id))
                {
                    item.thumb = Some(path.clone());
                }
                self.decode_thumbs(vec![path]);
            }
            ArtEvent::Custom { key, .. } => {
                let Some(state) = self.customize.as_mut().filter(|s| s.key == key) else {
                    return;
                };
                // Done (the worker sends the lists again, marked).
                state.busy = None;
                self.fill_customize();
                self.keep_theme();
                // A new theme (or none): the preview follows.
                self.preview = None;
                self.update_preview();
                if self.preview.is_none() {
                    self.sync_music(director::FADE_LAUNCH);
                }
            }
            ArtEvent::Failed { key, message } => {
                if let Some(state) = self.customize.as_mut().filter(|s| s.key == key) {
                    state.busy = None;
                    self.fill_customize();
                    self.toast(&message);
                }
            }
            ArtEvent::Art { .. } => {}
        }
    }

    /// New art for a game: if the page shows it, its preview and backdrop follow.
    pub(super) fn customize_art_changed(&mut self, game: &GameId) {
        if self.customize.as_ref().is_some_and(|s| &s.game.id == game) {
            self.decode_current();
            self.fill_customize();
        }
    }

    // --- thumbnails ------------------------------------------------------------------

    /// The art in use for the focused section, as a preview.
    fn decode_current(&mut self) {
        if let Some(path) = self.current_art_path() {
            self.decode_thumbs(vec![path]);
        }
    }

    fn decode_thumbs(&mut self, paths: Vec<PathBuf>) {
        let Some(state) = self.customize.as_mut() else {
            return;
        };
        let todo: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| !state.thumbs.contains_key(p) && state.decoding.insert(p.clone()))
            .collect();
        if todo.is_empty() {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("hearth-thumbs".into())
            .spawn(move || {
                for path in todo {
                    let pixels = decode_fit(&path, THUMB_MAX.0, THUMB_MAX.1);
                    defer(move |c| c.thumb_ready(path, pixels));
                }
            });
        if let Err(err) = spawned {
            tracing::warn!("can't start a thumbnail thread: {err}");
        }
    }

    fn thumb_ready(
        &mut self,
        path: PathBuf,
        pixels: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    ) {
        let Some(state) = self.customize.as_mut() else {
            return;
        };
        state.decoding.remove(&path);
        let image = pixels.map(Image::from_rgba8).unwrap_or_default();
        state.thumbs.insert(path, image);
        self.fill_customize();
    }

    // --- filling the page ------------------------------------------------------------

    fn current_art_path(&self) -> Option<PathBuf> {
        let state = self.customize.as_ref()?;
        let Section::Art(kind) = state.section else {
            return None;
        };
        self.art_paths
            .get(&state.game.id)?
            .get(kind)
            .map(Path::to_path_buf)
    }

    /// True when the art in use isn't marked among the focused section's candidates (a file,
    /// the game folder's, or automatic art fetched before its source was noted): the strip
    /// then starts with an "In use" tile.
    fn in_use_tile(&self, state: &CustomizeState) -> bool {
        let Section::Art(kind) = state.section else {
            return false;
        };
        let Some((items, _)) = state.lists.get(&CandidateSet::Art(kind)) else {
            return false;
        };
        let in_use = self
            .art_paths
            .get(&state.game.id)
            .is_some_and(|art| art.get(kind).is_some());
        in_use && !items.iter().any(|i| i.current)
    }

    /// The game's theme keeps playing on its page (as on Home), whatever Home's focus says.
    pub(super) fn keep_theme(&mut self) {
        let Some(state) = self.customize.as_ref() else {
            return;
        };
        let theme = self.theme_of(&state.game);
        if theme != self.focused_theme {
            self.focused_theme = theme;
            self.sync_music(director::FADE_LAUNCH);
        }
    }

    /// (short status, sentence) for where `section`'s file comes from now.
    fn source_of(&self, state: &CustomizeState, section: Section) -> (String, String) {
        let custom = self.picks.load(&state.key);
        let in_folder = |extra: Extra| {
            folder_candidates(&state.game, extra)
                .into_iter()
                .find(|p| p.is_file())
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        };
        match section {
            Section::Art(kind) => {
                let entry = custom.entry(Extra::Art(kind));
                match entry.source {
                    Some(Source::Sgdb) => ("Picked".into(), "Now: picked from SteamGridDB".into()),
                    Some(Source::File) => (
                        "Your file".into(),
                        format!(
                            "Now: your file ({})",
                            file_name(entry.from.as_deref().unwrap_or(""))
                        ),
                    ),
                    _ => match in_folder(Extra::Art(kind)) {
                        Some(name) => (
                            "Game folder".into(),
                            format!("Now: {name} from the game's folder"),
                        ),
                        None if self.art_cache.cached(&state.key, kind).is_some() => (
                            "Automatic".into(),
                            "Now: automatic, from SteamGridDB".into(),
                        ),
                        None => ("None".into(), "Now: none (placeholder)".into()),
                    },
                }
            }
            Section::Theme => match self.picks.find(&state.key, Extra::Theme) {
                Lookup::Found(_) => (
                    "Picked".into(),
                    format!(
                        "Now: your file ({})",
                        file_name(custom.theme.from.as_deref().unwrap_or(""))
                    ),
                ),
                Lookup::Blocked => (
                    "None".into(),
                    match in_folder(Extra::Theme) {
                        Some(name) => format!("Now: none ({name} in the game's folder is ignored)"),
                        None => "Now: none".into(),
                    },
                ),
                Lookup::Pass => match in_folder(Extra::Theme) {
                    Some(name) => (
                        "Game folder".into(),
                        format!("Now: {name} from the game's folder"),
                    ),
                    None => ("None".into(), "Now: none".into()),
                },
            },
            Section::Match => {
                let name = self
                    .art
                    .get(&state.game.id)
                    .and_then(|a| a.match_name.clone());
                match name {
                    Some(name) => (name.clone(), format!("Now matched to: {name}")),
                    None => ("No match".into(), "Not matched to any game".into()),
                }
            }
        }
    }

    fn fill_customize(&mut self) {
        let Some(app) = self.customize_app() else {
            return;
        };
        let Some(state) = self.customize.as_ref() else {
            return;
        };
        let data = app.global::<CustomizeData>();
        let tile = self.art.get(&state.game.id).cloned().unwrap_or_default();
        data.set_title(state.game.title.as_str().into());
        data.set_hero(tile.hero.clone());
        data.set_hero_x(tile.hero_position.unwrap_or(CENTER));
        data.set_logo(tile.logo.clone());
        let sections: Vec<CustomSection> = SECTIONS
            .iter()
            .map(|&s| CustomSection {
                key: s.key().into(),
                label: s.label().into(),
                status: self.source_of(state, s).0.into(),
            })
            .collect();
        data.set_sections(ModelRc::from(Rc::new(VecModel::from(sections))));
        let section = state.section;
        data.set_heading(section.label().into());
        match state.busy {
            Some((what, index)) => {
                data.set_source(what.into());
                data.set_busy(true);
                data.set_busy_index(index.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1));
            }
            None => {
                data.set_source(self.source_of(state, section).1.into());
                data.set_busy(false);
                data.set_busy_index(-1);
            }
        }
        let current_thumb = self
            .current_art_path()
            .and_then(|p| state.thumbs.get(&p).cloned())
            .unwrap_or_default();
        data.set_current(current_thumb.clone());
        data.set_warning(
            if section == Section::Theme && !self.audio_settings.game_themes {
                "Game themes are off in Settings · Display & sound (previews still play)".into()
            } else {
                "".into()
            },
        );
        let (items, note) = match section.set() {
            None => (Vec::new(), String::new()),
            Some(set) => match state.lists.get(&set) {
                None => (Vec::new(), "Loading…".into()),
                Some((items, availability)) => {
                    let shown: Vec<Candidate> = items
                        .iter()
                        .map(|i| Candidate {
                            name: i.name.as_str().into(),
                            detail: if i.detail.is_empty() {
                                "".into()
                            } else {
                                format!("Found by searching “{}”", i.detail).into()
                            },
                            thumb: i
                                .thumb
                                .as_ref()
                                .and_then(|p| state.thumbs.get(p).cloned())
                                .unwrap_or_default(),
                            current: i.current,
                        })
                        .collect();
                    let note = note_for(set, *availability, shown.is_empty());
                    // The art in use isn't one of these (or can't be told apart): it comes
                    // first, marked, rather than marking an image that may not be it.
                    let mut shown = shown;
                    if self.in_use_tile(state) {
                        shown.insert(
                            0,
                            Candidate {
                                name: "In use".into(),
                                detail: "".into(),
                                thumb: current_thumb.clone(),
                                current: true,
                            },
                        );
                    }
                    (shown, note)
                }
            },
        };
        data.set_candidates(ModelRc::from(Rc::new(VecModel::from(items))));
        data.set_note(note.into());
        self.fill_customize_rows(None);
        self.fill_files();
    }

    /// The action rows (with the theme start just set, if any).
    fn fill_customize_rows(&self, start: Option<f32>) {
        let (Some(app), Some(state)) = (self.customize_app(), self.customize.as_ref()) else {
            return;
        };
        let row = |key: &str, label: &str, value: String, slider: bool| CustomRow {
            key: key.into(),
            label: label.into(),
            value: value.into(),
            slider,
        };
        let rows = match state.section {
            Section::Art(kind) => {
                let mut rows = Vec::new();
                if kind == ArtKind::Hero {
                    let position = self
                        .art
                        .get(&state.game.id)
                        .and_then(|a| a.hero_position)
                        .unwrap_or(CENTER);
                    rows.push(row("position", "Position", position_label(position), false));
                }
                rows.push(row("file", "Use a file…", String::new(), false));
                rows.push(row("reset", "Reset to automatic", String::new(), false));
                rows
            }
            Section::Theme => {
                let start = start.unwrap_or_else(|| self.picks.load(&state.key).theme_start());
                vec![
                    row("file", "Choose a file…", String::new(), false),
                    row("start", "Start at", format_start(start), true),
                    row("remove", "Remove theme", String::new(), false),
                ]
            }
            Section::Match => Vec::new(),
        };
        app.global::<CustomizeData>()
            .set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    }

    fn fill_files(&self) {
        let (Some(app), Some(state)) = (self.customize_app(), self.customize.as_ref()) else {
            return;
        };
        let data = app.global::<CustomizeData>();
        let Some((kind, files)) = &state.files else {
            return;
        };
        let audio = *kind == FileKind::Audio;
        data.set_files_heading(
            if audio {
                "Choose a theme"
            } else {
                "Choose an image"
            }
            .into(),
        );
        let folders = if state.game.folder.is_some() {
            "Downloads or the game's folder"
        } else {
            "Downloads"
        };
        data.set_files_note(
            format!(
                "No {} in {folders}.",
                if audio { "music files" } else { "images" }
            )
            .into(),
        );
        let entries: Vec<FileEntry> = files
            .iter()
            .map(|f| {
                let mut detail = format!("{} · {}", f.place, format_age(f.modified));
                if let Some(d) = state.durations.get(&f.path) {
                    detail.push_str(&format!(" · {}", format_duration(*d)));
                }
                FileEntry {
                    name: f.name.as_str().into(),
                    detail: detail.into(),
                    thumb: state.thumbs.get(&f.path).cloned().unwrap_or_default(),
                    audio,
                }
            })
            .collect();
        data.set_files(ModelRc::from(Rc::new(VecModel::from(entries))));
    }

    fn duration_ready(&mut self, path: PathBuf, duration: Duration) {
        if let Some(state) = self.customize.as_mut() {
            state.durations.insert(path, duration);
            self.fill_files();
        }
    }
}

/// What to say about a list.
fn note_for(set: CandidateSet, availability: Availability, empty: bool) -> String {
    let what = match set {
        CandidateSet::Art(_) => "images",
        CandidateSet::Games => "games",
    };
    match (availability, empty) {
        (Availability::Live, false) => String::new(),
        (Availability::Live, true) => format!("SteamGridDB has no {what} for this game."),
        (Availability::Offline { .. }, false) => "Offline · saved results".into(),
        (Availability::Offline { retry_in }, true) => format!(
            "Can't reach SteamGridDB (trying again in {} min). Use a file meanwhile.",
            retry_in.as_secs().div_ceil(60).max(1)
        ),
        (Availability::NoProvider, false) => "No SteamGridDB key · saved results".into(),
        (Availability::NoProvider, true) => {
            "No SteamGridDB key: add one to hearth.toml, or use a file.".into()
        }
        (Availability::Refused { status, retry_in }, empty) => {
            let why = match status {
                401 | 403 => format!(
                    "SteamGridDB refused the API key (HTTP {status}): check \
                     HEARTH_STEAMGRIDDB_KEY or [art] steamgriddb_key in hearth.toml."
                ),
                429 => "SteamGridDB asks to slow down (HTTP 429).".into(),
                _ => format!("SteamGridDB answered with an error (HTTP {status})."),
            };
            let saved = if empty { "" } else { " Saved results below." };
            format!(
                "{why}{saved} Trying again in {} min.",
                retry_in.as_secs().div_ceil(60).max(1)
            )
        }
        (Availability::Unexpected, false) => "SteamGridDB's answer was odd · saved results".into(),
        (Availability::Unexpected, true) => {
            "SteamGridDB's answer couldn't be read. Use a file meanwhile.".into()
        }
        (Availability::NotMatched, _) => {
            "Not matched to a SteamGridDB game: try “Wrong game?”, or use a file.".into()
        }
    }
}

/// Reads audio files' durations on a worker thread, reporting each.
fn measure_durations(paths: Vec<PathBuf>) {
    let spawned = std::thread::Builder::new()
        .name("hearth-durations".into())
        .spawn(move || {
            for path in paths {
                if let Ok(duration) = hearth_audio::analyze::duration(&path) {
                    defer(move |c| c.duration_ready(path, duration));
                }
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("can't start a duration thread: {err}");
    }
}

/// Dev socket words for the Customize page (`scripts/send-action.sh`); their reply is "ok", an
/// error, or (`custom_state`) JSON.
const DEV_WORDS: [&str; 11] = [
    "customize:",
    "pick:",
    "pick_file:",
    "position:",
    "rematch:",
    "reset:",
    "theme_file:",
    "theme_start:",
    "theme_remove",
    "custom_state",
    "anim_state",
];

/// Wraps the dev socket's command handler with the Customize page's words, run on the UI
/// thread (the reply waits up to 3 s for it).
pub fn dev_commands(
    other: hearth_platform::input::dev_socket::CommandHandler,
) -> hearth_platform::input::dev_socket::CommandHandler {
    std::sync::Arc::new(move |word: &str| {
        if !DEV_WORDS.iter().any(|w| word.starts_with(w)) {
            return other(word);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let word = word.to_owned();
        defer(move |c| {
            let _ = tx.send(c.dev_command(&word));
        });
        Some(
            rx.recv_timeout(Duration::from_secs(3))
                .unwrap_or_else(|_| "error: the UI didn't answer".into()),
        )
    })
}

impl Controller {
    fn dev_command(&mut self, word: &str) -> String {
        let (name, arg) = word.split_once(':').unwrap_or((word, ""));
        let number = || {
            arg.parse::<usize>()
                .map_err(|_| format!("error: {arg} isn't a number"))
        };
        let result: Result<(), String> = match name {
            "customize" => {
                // customize:<art key>:<section>, the key holding a '/' (gc/GZ2E01).
                let (key, section) = arg.rsplit_once(':').unwrap_or((arg, "art"));
                let game = self
                    .entries
                    .iter()
                    .find(|e| self.picks.key_of(&e.game).as_str() == key)
                    .map(|e| e.game.id.clone());
                match game {
                    Some(id) => {
                        self.open_customize(&id, section);
                        Ok(())
                    }
                    None => Err(format!("error: no game with art key {key}")),
                }
            }
            "custom_state" => return self.custom_state(),
            "anim_state" => return self.anim_state().to_string(),
            _ if self.customize.is_none() => Err("error: the Customize page isn't open".into()),
            "pick" => number().map(|n| self.customize_candidate(n)),
            "rematch" => number().map(|n| {
                self.dev_section(Section::Match);
                self.customize_candidate(n);
            }),
            "pick_file" | "theme_file" => number().map(|n| {
                if name == "theme_file" {
                    self.dev_section(Section::Theme);
                }
                self.open_files();
                self.customize_file_chosen(n);
            }),
            "position" => arg
                .parse::<f32>()
                .map_err(|_| format!("error: {arg} isn't a position (0 to 1)"))
                .map(|position| {
                    let Some(state) = self.customize.as_ref() else {
                        return;
                    };
                    let (game, key) = (state.game.id.clone(), state.key.clone());
                    self.set_hero_position_live(&game, position.clamp(0.0, 1.0));
                    if let Some(service) = &self.art_service {
                        service.set_hero_position(key, position);
                    }
                }),
            "reset" => match ArtKind::from_name(arg) {
                Some(kind) => {
                    self.dev_section(Section::Art(kind));
                    self.customize_row("reset");
                    Ok(())
                }
                None => Err(format!("error: {arg} isn't grid, hero, logo or icon")),
            },
            "theme_start" => arg
                .parse::<f32>()
                .map_err(|_| format!("error: {arg} isn't a number of seconds"))
                .map(|seconds| {
                    self.dev_section(Section::Theme);
                    if let (Some(state), Some(service)) = (&self.customize, &self.art_service) {
                        service.set_theme_start(state.key.clone(), seconds.max(0.0));
                    }
                    self.update_preview_with_start(Some(seconds.max(0.0)));
                    self.fill_customize_rows(Some(seconds.max(0.0)));
                }),
            "theme_remove" => {
                self.dev_section(Section::Theme);
                self.customize_row("remove");
                Ok(())
            }
            _ => Err(format!("error: unknown command {word}")),
        };
        match result {
            Ok(()) => "ok".into(),
            Err(err) => err,
        }
    }

    /// Shows `section` (as if focused there, the pane entered) and asks for its list.
    fn dev_section(&mut self, section: Section) {
        let index = SECTIONS.iter().position(|s| *s == section).unwrap_or(0);
        if let Some(state) = self.customize.as_mut() {
            state.section = section;
            state.in_pane = true;
        }
        if let Some(app) = self.app.upgrade() {
            app.invoke_open_customize(i32::try_from(index).unwrap_or(0));
        }
        if let Some(state) = self.customize.as_mut() {
            state.in_pane = true;
        }
        self.request_candidates();
        self.fill_customize();
        self.update_preview();
    }

    /// The page and the game's picks, as JSON.
    fn custom_state(&self) -> String {
        let Some(state) = &self.customize else {
            return serde_json::json!({ "open": false }).to_string();
        };
        let lists: serde_json::Map<String, serde_json::Value> = state
            .lists
            .iter()
            .map(|(set, (items, availability))| {
                let name = match set {
                    CandidateSet::Art(kind) => kind.name().to_owned(),
                    CandidateSet::Games => "games".into(),
                };
                let items: Vec<_> = items
                    .iter()
                    .map(|i| {
                        serde_json::json!({
                            "id": i.id, "name": i.name, "detail": i.detail,
                            "current": i.current, "thumb": i.thumb.is_some(),
                        })
                    })
                    .collect();
                (
                    name,
                    serde_json::json!({ "availability": format!("{availability:?}"), "items": items }),
                )
            })
            .collect();
        let sources: serde_json::Map<String, serde_json::Value> = SECTIONS
            .iter()
            .map(|&s| (s.key().to_owned(), self.source_of(state, s).1.into()))
            .collect();
        let picks = std::fs::read_to_string(self.picks.dir(&state.key).join("custom.toml"))
            .unwrap_or_default();
        let art = self.art_paths.get(&state.game.id);
        serde_json::json!({
            "open": true,
            "game": state.game.id.as_str(),
            "key": state.key.as_str(),
            "section": state.section.key(),
            "sources": sources,
            "custom_toml": picks,
            "hero_position": self.art.get(&state.game.id).and_then(|a| a.hero_position),
            "art": art.map(|a| serde_json::json!({
                "grid": a.grid, "hero": a.hero, "logo": a.logo, "icon": a.icon,
            })),
            "lists": lists,
            "files": state.files.as_ref().map(|(_, f)| f.iter().map(|f| f.name.clone()).collect::<Vec<_>>()),
            "preview": self.preview.as_ref().map(|p| p.path.display().to_string()),
        })
        .to_string()
    }
}
