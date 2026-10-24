//! Animated heroes and logos on the UI thread (README §6 "Animated art").
//!
//! - **Home:** only the focused game animates, once Home's focus has settled on it for
//!   [`ANIM_SETTLE`] (like its theme). Any focus move, leaving Home, the overlay, a running
//!   game, Animated artwork off or Reduce motion stops it at once and the still shows again.
//! - **Customize:** the page's background plays the hero in use, as Home does, while the page
//!   is open; the strip plays the focused candidate in its tile once focus rests on it for
//!   [`STRIP_SETTLE`].
//!
//! Frames come from `bridge::anim` (one thread per [`Slot`]). Each slot's single-shot timer
//! flips to the next frame after the shown frame's own delay: a sprite sheet flips by moving
//! its clip (uploaded once), a window by showing the next decoded frame. A frame that isn't
//! decoded yet never holds anything up: the shown one stays, and a track that keeps falling
//! behind ([`LATE_LIMIT`]) gives up and leaves its still.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hearth_core::art::{ArtKey, ArtKind, GameArt};
use hearth_core::model::GameId;
use slint::{ComponentHandle, Image};

use super::customize::StripItem;
use super::{Controller, HomeData, Mode, defer, with};
use crate::bridge::anim::{self, Credits, Delivery, SheetLayout, Slot, TrackInfo, TrackJob};
use crate::bridge::frames;
use crate::{AnimFrame, CustomizeData};

/// How long focus stays on a game before it animates.
pub(super) const ANIM_SETTLE: Duration = Duration::from_millis(500);
/// How long focus stays on a strip tile before it animates.
const STRIP_SETTLE: Duration = Duration::from_millis(300);
/// Over this share of late flips in [`LATE_SPAN`], a track gives up.
const LATE_LIMIT: f32 = 0.25;
const LATE_SPAN: Duration = Duration::from_secs(2);

/// The art Home and the Customize page show: with animation on, a hero or logo that has an
/// animated version shows that version's own still (its still frame, see
/// `hearth_core::art::display`), so the animation starts from the image already on screen;
/// with it off (or Reduce motion), the static image.
pub(super) fn shown_art(art: &GameArt, animates: bool) -> GameArt {
    let mut shown = art.clone();
    if animates {
        if let Some(hero) = &art.hero_animated {
            shown.hero = Some(hero.clone());
        }
        if let Some(logo) = &art.logo_animated {
            shown.logo = Some(logo.clone());
        }
    }
    shown
}

/// A sprite sheet, uploaded once.
struct Sheet {
    image: Image,
    layout: SheetLayout,
    delays: Vec<u32>,
}

/// One playing animation.
struct Track {
    info: Option<TrackInfo>,
    sheet: Option<Sheet>,
    /// Window and stream modes: decoded frames waiting.
    queue: VecDeque<(Image, u32)>,
    /// The frame shown (sheet mode), and its delay.
    index: usize,
    delay_ms: u32,
    shown: bool,
    flips: u64,
    /// Late flips and all flips since `span_start`.
    late: u32,
    ticks: u32,
    late_total: u64,
    span_start: Instant,
    started: Instant,
    /// From the request to the first frame on screen.
    first_frame: Option<Duration>,
    timer: slint::Timer,
    credits: Arc<Credits>,
    /// This track's play; bumping `current` ends its thread.
    generation: u64,
    current: Arc<AtomicU64>,
}

impl Track {
    fn new(generation: u64) -> Self {
        Self {
            info: None,
            sheet: None,
            queue: VecDeque::new(),
            index: 0,
            delay_ms: 100,
            shown: false,
            flips: 0,
            late: 0,
            ticks: 0,
            late_total: 0,
            span_start: Instant::now(),
            started: Instant::now(),
            first_frame: None,
            timer: slint::Timer::default(),
            credits: Arc::new(Credits::default()),
            generation,
            current: Arc::new(AtomicU64::new(generation)),
        }
    }

    /// Decoded bytes held now.
    fn bytes(&self) -> u64 {
        let frame = self.info.as_ref().map_or(0, |i| i.frame_bytes);
        let held = match &self.sheet {
            Some(sheet) => sheet.delays.len(),
            None => self.queue.len() + usize::from(self.shown),
        };
        frame * held as u64
    }

    fn stop(&self) {
        self.current.fetch_add(1, Ordering::Relaxed);
        self.credits.reset();
    }
}

/// The player's state (a field of the controller).
#[derive(Default)]
pub(super) struct AnimPlayer {
    generations: u64,
    tracks: HashMap<Slot, Track>,
    /// Tracks that gave up this play, and why.
    stopped: Vec<(Slot, String)>,
    /// The game Home animates, and the one its Games row has focused.
    home: Option<GameId>,
    focus: Option<GameId>,
    settle: slint::Timer,
    /// The Customize page: the game its background animates, the strip tile focused.
    page: Option<GameId>,
    strip_index: Option<usize>,
    strip_settle: slint::Timer,
}

impl Controller {
    // --- Home --------------------------------------------------------------------------

    /// Home's focus moved (`None`: not on a game of the Games row, or not on Home).
    pub(super) fn anim_focus_changed(&mut self, id: Option<GameId>) {
        self.anim.settle.stop();
        if self.anim.home.is_some() && self.anim.home != id {
            self.anim_stop("focus moved");
        }
        self.anim.focus = id;
        if self.anim.focus.is_some() {
            self.anim
                .settle
                .start(slint::TimerMode::SingleShot, ANIM_SETTLE, || {
                    with(Controller::anim_sync);
                });
        }
    }

    /// The game Home should animate now, if any.
    fn anim_wanted(&self) -> Option<GameId> {
        if !self.display_settings.animates() || self.mode != Mode::Shell || self.customize.is_some()
        {
            return None;
        }
        let game = self.anim.focus.clone()?;
        let art = self.art_paths.get(&game)?;
        (art.hero_animated.is_some() || art.logo_animated.is_some()).then_some(game)
    }

    /// Starts or stops to match what Hearth shows (after focus settles, a mode change, a
    /// setting change, the Customize page opening or closing).
    pub(super) fn anim_sync(&mut self) {
        let wanted = self.anim_wanted();
        if wanted != self.anim.home {
            self.anim_stop("not wanted");
            if let Some(game) = wanted {
                self.anim_start_home(game);
            }
        }
        self.anim_sync_page();
    }

    /// Plays again from scratch (new art for the game, another screen class).
    pub(super) fn anim_restart(&mut self) {
        self.anim_stop("restart");
        self.anim_stop_page();
        self.anim_sync();
    }

    /// New art paths for `game`: restart if they change what plays.
    pub(super) fn anim_art_changed(&mut self, game: &GameId, old: Option<&GameArt>) {
        let shown = [self.anim.focus.as_ref(), self.anim.page.as_ref()];
        if !shown.contains(&Some(game)) {
            return;
        }
        let animated =
            |art: Option<&GameArt>| art.map(|a| (a.hero_animated.clone(), a.logo_animated.clone()));
        if animated(old) != animated(self.art_paths.get(game)) {
            self.anim_restart();
        }
    }

    fn anim_start_home(&mut self, game: GameId) {
        let (Some(art), Some(key)) = (self.art_paths.get(&game), self.art_keys.get(&game)) else {
            return;
        };
        let (key, hero, logo) = (
            key.clone(),
            art.hero_animated.clone(),
            art.logo_animated.clone(),
        );
        self.anim.stopped.clear();
        if let Some(hero) = hero {
            self.anim_track(Slot::HomeHero, ArtKind::Hero, &key, hero, None);
        }
        if let Some(logo) = logo {
            self.anim_track(Slot::HomeLogo, ArtKind::Logo, &key, logo, None);
        }
        tracing::debug!(game = %game, "animation starts");
        if let Some(app) = self.app.upgrade() {
            app.global::<HomeData>().set_anim_id(game.as_str().into());
        }
        self.anim.home = Some(game);
    }

    /// Stops what Home plays; the stills show again.
    pub(super) fn anim_stop(&mut self, why: &str) {
        self.anim.settle.stop();
        self.anim_stop_slot(Slot::HomeHero);
        self.anim_stop_slot(Slot::HomeLogo);
        if let Some(game) = self.anim.home.take() {
            tracing::debug!(game = %game, why, "animation stops");
        }
        if let Some(app) = self.app.upgrade() {
            app.global::<HomeData>().set_anim_id("".into());
        }
    }

    // --- Customize ----------------------------------------------------------------------

    /// The page's background: the game's animated hero while the page is open.
    fn anim_sync_page(&mut self) {
        let wanted = self
            .customize_game()
            .filter(|_| self.display_settings.animates() && self.mode == Mode::Shell)
            .filter(|game| {
                self.art_paths
                    .get(game)
                    .is_some_and(|a| a.hero_animated.is_some())
            });
        if wanted == self.anim.page {
            return;
        }
        self.anim_stop_page();
        let Some(game) = wanted else {
            return;
        };
        let (Some(art), Some(key)) = (self.art_paths.get(&game), self.art_keys.get(&game)) else {
            return;
        };
        let (key, hero) = (key.clone(), art.hero_animated.clone());
        if let Some(hero) = hero {
            self.anim_track(Slot::Page, ArtKind::Hero, &key, hero, None);
        }
        self.anim.page = Some(game);
    }

    fn anim_stop_page(&mut self) {
        self.anim_stop_slot(Slot::Page);
        self.anim.page = None;
    }

    /// Focus rests on strip tile `index` (-1: not in the strip). An animated candidate plays in
    /// its tile after [`STRIP_SETTLE`].
    pub(super) fn customize_strip_focused(&mut self, index: i32) {
        self.anim.strip_settle.stop();
        self.anim_stop_slot(Slot::Strip);
        let index = usize::try_from(index).ok();
        self.anim.strip_index = index;
        if let Some(app) = self.app.upgrade() {
            app.global::<CustomizeData>().set_strip_anim_index(-1);
        }
        if index.is_none() || !self.display_settings.animates() {
            return;
        }
        self.anim
            .strip_settle
            .start(slint::TimerMode::SingleShot, STRIP_SETTLE, || {
                with(Controller::anim_start_strip);
            });
    }

    fn anim_start_strip(&mut self) {
        let Some(index) = self.anim.strip_index else {
            return;
        };
        let Some((key, kind, source)) = self.strip_candidate_source(index) else {
            return;
        };
        self.anim_track(
            Slot::Strip,
            kind,
            &key,
            source,
            Some(super::customize::THUMB_MAX),
        );
        if let Some(app) = self.app.upgrade() {
            app.global::<CustomizeData>()
                .set_strip_anim_index(i32::try_from(index).unwrap_or(-1));
        }
    }

    /// The animated file behind strip tile `index`, if it animates.
    fn strip_candidate_source(&self, index: usize) -> Option<(ArtKey, ArtKind, PathBuf)> {
        let (key, kind, item) = self.customize_strip_item(index)?;
        let path = match item {
            StripItem::InUse => {
                let game = self.customize_game()?;
                self.art_paths.get(&game)?.animated(kind)?.to_path_buf()
            }
            StripItem::Candidate {
                thumb,
                animated: true,
            } => thumb?,
            StripItem::Candidate { .. } => return None,
        };
        Some((key, kind, path))
    }

    // --- tracks -------------------------------------------------------------------------

    fn anim_track(
        &mut self,
        slot: Slot,
        kind: ArtKind,
        key: &ArtKey,
        source: PathBuf,
        bounds: Option<(u32, u32)>,
    ) {
        self.anim_stop_slot(slot);
        self.anim.generations += 1;
        let track = Track::new(self.anim.generations);
        let last_played: HashMap<String, i64> = self
            .entries
            .iter()
            .filter_map(|e| {
                let key = self.art_keys.get(&e.game.id)?;
                Some((key.as_str().to_owned(), e.last_played?))
            })
            .collect();
        let deliver: Arc<dyn Fn(Delivery) + Send + Sync> =
            Arc::new(|d| defer(move |c| c.anim_delivery(d)));
        anim::spawn(
            TrackJob {
                slot,
                kind,
                key: key.clone(),
                source,
                class: self.art_loader.class(),
                cache: self.art_cache.clone(),
                bounds,
                last_played: Arc::new(last_played),
                generation: track.generation,
                current: track.current.clone(),
                credits: track.credits.clone(),
            },
            deliver,
        );
        self.anim.tracks.insert(slot, track);
    }

    fn anim_stop_slot(&mut self, slot: Slot) {
        if let Some(track) = self.anim.tracks.remove(&slot) {
            track.stop();
        }
        // The last frame stays drawn while it fades out (`base`).
        self.anim_show(slot, None, AnimFrame::default(), false);
    }

    fn anim_delivery(&mut self, delivery: Delivery) {
        let (slot, generation) = match &delivery {
            Delivery::Start {
                slot, generation, ..
            }
            | Delivery::Sheet {
                slot, generation, ..
            }
            | Delivery::Frame {
                slot, generation, ..
            }
            | Delivery::Still {
                slot, generation, ..
            } => (*slot, *generation),
        };
        let Some(track) = self
            .anim
            .tracks
            .get_mut(&slot)
            .filter(|t| t.generation == generation)
        else {
            return;
        };
        match delivery {
            Delivery::Start { info, .. } => {
                tracing::info!(
                    slot = slot.name(),
                    mode = info.mode.name(),
                    frames = ?info.frames,
                    size = ?info.size,
                    prepare_ms = info.prepare.as_millis(),
                    "animation ready"
                );
                track.info = Some(info);
            }
            Delivery::Sheet {
                pixels,
                layout,
                delays,
                ..
            } => {
                track.sheet = Some(Sheet {
                    image: Image::from_rgba8(pixels),
                    layout,
                    delays,
                });
                self.anim_tick(slot);
            }
            Delivery::Frame {
                pixels, delay_ms, ..
            } => {
                track.queue.push_back((Image::from_rgba8(pixels), delay_ms));
                if !track.shown {
                    self.anim_tick(slot);
                }
            }
            Delivery::Still { reason, .. } => self.anim_drop_track(slot, reason),
        }
    }

    /// Shows the next frame of `slot` and sets the timer for the one after.
    fn anim_tick(&mut self, slot: Slot) {
        let Some(track) = self.anim.tracks.get_mut(&slot) else {
            return;
        };
        // (image to set, or None to keep the sheet; its clip; its delay)
        let next: Option<(Option<Image>, AnimFrame, u32)> = match &track.sheet {
            Some(sheet) => {
                if track.shown {
                    track.index = (track.index + 1) % sheet.delays.len();
                }
                let (x, y) = sheet.layout.origin(track.index);
                let (w, h) = sheet.layout.frame;
                let int = |v: u32| i32::try_from(v).unwrap_or(0);
                let frame = AnimFrame {
                    x: int(x),
                    y: int(y),
                    w: int(w),
                    h: int(h),
                };
                let image = (!track.shown).then(|| sheet.image.clone());
                Some((image, frame, sheet.delays[track.index]))
            }
            None => track.queue.pop_front().map(|(image, delay)| {
                if track.shown {
                    // The frame it replaces is gone: room for one more.
                    track.credits.release();
                }
                (Some(image), AnimFrame::default(), delay)
            }),
        };
        track.ticks += 1;
        let shown_now = next.is_some();
        let retry_in = match &next {
            Some((_, _, delay)) => {
                track.delay_ms = *delay;
                track.flips += 1;
                if !track.shown {
                    track.shown = true;
                    track.first_frame = Some(track.started.elapsed());
                }
                Duration::from_millis(u64::from(*delay))
            }
            None => {
                // Late: not decoded yet. The shown frame stays.
                track.late += 1;
                track.late_total += 1;
                Duration::from_millis(u64::from(track.delay_ms / 2).max(10))
            }
        };
        let give_up = track.span_start.elapsed() >= LATE_SPAN && {
            #[allow(clippy::cast_precision_loss)]
            let share = track.late as f32 / track.ticks.max(1) as f32;
            track.late = 0;
            track.ticks = 0;
            track.span_start = Instant::now();
            (share > LATE_LIMIT).then_some(share).is_some()
        };
        if track.shown && !give_up {
            track
                .timer
                .start(slint::TimerMode::SingleShot, retry_in, move || {
                    with(|c| c.anim_tick(slot));
                });
        }
        if let Some((image, frame, _)) = next {
            frames::count("anim-flips", 1);
            self.anim_show(slot, image, frame, true);
        }
        if give_up {
            self.anim_drop_track(slot, "falling behind (over 25 % of frames late)".into());
        } else if !shown_now {
            tracing::trace!(slot = slot.name(), "frame late");
        }
    }

    /// Puts a frame (or only its clip, with `image` None) in `slot`'s properties.
    fn anim_show(&self, slot: Slot, image: Option<Image>, frame: AnimFrame, on: bool) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let (home, page) = (app.global::<HomeData>(), app.global::<CustomizeData>());
        match slot {
            Slot::HomeHero => {
                if let Some(image) = image {
                    home.set_anim_hero(image);
                }
                if on {
                    home.set_anim_hero_frame(frame);
                }
                home.set_anim_hero_on(on);
            }
            Slot::HomeLogo => {
                if let Some(image) = image {
                    home.set_anim_logo(image);
                }
                if on {
                    home.set_anim_logo_frame(frame);
                }
                home.set_anim_logo_on(on);
            }
            Slot::Strip => {
                if let Some(image) = image {
                    page.set_strip_anim(image);
                }
                if on {
                    page.set_strip_anim_frame(frame);
                }
                page.set_strip_anim_on(on);
            }
            Slot::Page => {
                if let Some(image) = image {
                    page.set_hero_anim(image);
                }
                if on {
                    page.set_hero_anim_frame(frame);
                }
                page.set_hero_anim_on(on);
            }
        }
    }

    /// One track stops for good this play; its still shows.
    fn anim_drop_track(&mut self, slot: Slot, reason: String) {
        if let Some(track) = self.anim.tracks.remove(&slot) {
            track.stop();
            tracing::info!(slot = slot.name(), "animation shows its still: {reason}");
        }
        self.anim.stopped.push((slot, reason));
        self.anim_show(slot, None, AnimFrame::default(), false);
    }

    /// What plays now, as JSON (the dev socket's `anim_state`).
    pub(super) fn anim_state(&self) -> serde_json::Value {
        let mut slots: Vec<&Slot> = self.anim.tracks.keys().collect();
        slots.sort_by_key(|s| s.name());
        let tracks: Vec<serde_json::Value> = slots
            .into_iter()
            .map(|slot| {
                let t = &self.anim.tracks[slot];
                let info = t.info.as_ref();
                serde_json::json!({
                    "slot": slot.name(),
                    "source": info.map(|i| i.source.display().to_string()),
                    "format": info.and_then(|i| i.format).map(|f| f.name()),
                    "mode": info.map(|i| i.mode.name()),
                    "frames": info.and_then(|i| i.frames),
                    "size": info.map(|i| [i.size.0, i.size.1]),
                    "frame_bytes": info.map(|i| i.frame_bytes),
                    "file_bytes": info.map(|i| i.file_bytes),
                    "prepare_ms": info.map(|i| i.prepare.as_millis()),
                    "first_frame_ms": t.first_frame.map(|d| d.as_millis()),
                    "decoded": t.sheet.as_ref().map_or(t.queue.len(), |s| s.delays.len()),
                    "bytes": t.bytes(),
                    "frame_index": t.index,
                    "flips": t.flips,
                    "late": t.late_total,
                    "playing": t.shown,
                })
            })
            .collect();
        let stopped: Vec<serde_json::Value> = self
            .anim
            .stopped
            .iter()
            .map(|(slot, reason)| serde_json::json!({ "slot": slot.name(), "reason": reason }))
            .collect();
        let total: u64 = self.anim.tracks.values().map(Track::bytes).sum();
        serde_json::json!({
            "enabled": self.display_settings.animated_art,
            "reduce_motion": self.display_settings.reduce_motion,
            "focus": self.anim.focus.as_ref().map(GameId::as_str),
            "playing": self.anim.home.as_ref().map(GameId::as_str),
            "page": self.anim.page.as_ref().map(GameId::as_str),
            "tracks": tracks,
            "stills": stopped,
            "bytes": total,
            "caps": {
                "hero": anim::HERO_CAP,
                "logo": anim::LOGO_CAP,
                "total": anim::TOTAL_CAP,
                "file": anim::FILE_CAP,
                "disk": anim::DISK_CAP,
            },
            "disk_bytes": self.art_cache.anim_files().iter().map(|f| f.bytes).sum::<u64>(),
        })
    }
}
