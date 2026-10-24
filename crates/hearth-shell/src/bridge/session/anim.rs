//! Home's animated hero and logo, on the UI thread (README §6 "Animated art").
//!
//! Only the focused game animates, once Home's focus has settled on it for [`ANIM_SETTLE`]
//! (like its theme). Any focus move, leaving Home, the overlay, a running game, the Customize
//! page, Animated artwork off or Reduce motion stops it at once and the still shows again.
//! Frames come from `bridge::anim` (one thread per track); each track's single-shot timer
//! flips to the next frame after the shown frame's own delay. A frame that isn't decoded yet
//! never holds anything up: the shown one stays, and a track that keeps falling behind
//! ([`LATE_LIMIT`]) gives up and leaves its still.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hearth_core::art::ArtKind;
use hearth_core::model::GameId;
use slint::{ComponentHandle, Image};

use super::{Controller, HomeData, Mode, defer, with};
use crate::bridge::anim::{self, Credits, Delivery, TrackInfo, TrackJob};
use crate::bridge::frames;

/// How long focus stays on a game before it animates.
pub(super) const ANIM_SETTLE: Duration = Duration::from_millis(500);
/// Over this share of late flips in [`LATE_SPAN`], a track gives up.
const LATE_LIMIT: f32 = 0.25;
const LATE_SPAN: Duration = Duration::from_secs(2);
/// The kinds that animate on Home.
const KINDS: [ArtKind; 2] = [ArtKind::Hero, ArtKind::Logo];

/// The art Home and the Customize page show: with animation on, a hero or logo that has an
/// animated version shows that version's own still (its first frame, a logo's most opaque),
/// so the animation starts from the image already on screen; with it off (or Reduce motion),
/// the static image.
pub(super) fn shown_art(
    art: &hearth_core::art::GameArt,
    animates: bool,
) -> hearth_core::art::GameArt {
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

/// One playing hero or logo.
struct Track {
    info: Option<TrackInfo>,
    /// Full mode: every frame so far; window and stream modes: the frames waiting.
    frames: Vec<(Image, u32)>,
    queue: VecDeque<(Image, u32)>,
    /// Full mode: every frame arrived.
    complete: bool,
    /// The frame shown (full mode), and its delay.
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
}

impl Track {
    fn new() -> Self {
        Self {
            info: None,
            frames: Vec::new(),
            queue: VecDeque::new(),
            complete: false,
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
        }
    }

    fn windowed(&self) -> bool {
        self.info
            .as_ref()
            .is_some_and(|i| i.mode != anim::Mode::Full)
    }

    /// Decoded bytes held now.
    fn bytes(&self) -> u64 {
        let frame = self.info.as_ref().map_or(0, |i| i.frame_bytes);
        let held = if self.windowed() {
            self.queue.len() + usize::from(self.shown)
        } else {
            self.frames.len()
        };
        frame * held as u64
    }
}

/// Why a track isn't playing.
#[derive(Debug, Clone)]
struct Stopped {
    kind: ArtKind,
    reason: String,
}

/// The player's state (a field of the controller).
pub(super) struct AnimPlayer {
    /// Bumped by every start and stop; tracks of older plays end.
    generation: Arc<AtomicU64>,
    /// The game playing, and its tracks.
    playing: Option<GameId>,
    tracks: HashMap<ArtKind, Track>,
    stopped: Vec<Stopped>,
    /// The game Home's Games row has focused, if any.
    focus: Option<GameId>,
    settle: slint::Timer,
}

impl Default for AnimPlayer {
    fn default() -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(0)),
            playing: None,
            tracks: HashMap::new(),
            stopped: Vec::new(),
            focus: None,
            settle: slint::Timer::default(),
        }
    }
}

impl Controller {
    /// Home's focus moved (`None`: not on a game of the Games row, or not on Home).
    pub(super) fn anim_focus_changed(&mut self, id: Option<GameId>) {
        self.anim.settle.stop();
        if self.anim.playing.is_some() && self.anim.playing != id {
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

    /// The game that should animate now, if any.
    fn anim_wanted(&self) -> Option<GameId> {
        if !self.display_settings.animates() || self.mode != Mode::Shell || self.customize.is_some()
        {
            return None;
        }
        let game = self.anim.focus.clone()?;
        let art = self.art_paths.get(&game)?;
        KINDS
            .iter()
            .any(|kind| art.animated(*kind).is_some())
            .then_some(game)
    }

    /// Starts or stops to match what Home shows (after focus settles, a mode change, a
    /// setting change).
    pub(super) fn anim_sync(&mut self) {
        let wanted = self.anim_wanted();
        if wanted == self.anim.playing {
            return;
        }
        self.anim_stop("not wanted");
        if let Some(game) = wanted {
            self.anim_start(game);
        }
    }

    /// Plays again from scratch (new art for the game, another screen class).
    pub(super) fn anim_restart(&mut self) {
        if self.anim.playing.is_some() {
            self.anim_stop("restart");
        }
        self.anim_sync();
    }

    /// New art paths for `game`: restart if they change what plays.
    pub(super) fn anim_art_changed(
        &mut self,
        game: &GameId,
        old: Option<&hearth_core::art::GameArt>,
    ) {
        if self.anim.focus.as_ref() != Some(game) {
            return;
        }
        let new = self.art_paths.get(game);
        let animated = |art: Option<&hearth_core::art::GameArt>| {
            KINDS.map(|k| {
                art.and_then(|a| a.animated(k))
                    .map(std::path::Path::to_path_buf)
            })
        };
        if animated(old) != animated(new) {
            self.anim_restart();
        }
    }

    fn anim_start(&mut self, game: GameId) {
        let (Some(art), Some(key)) = (self.art_paths.get(&game), self.art_keys.get(&game)) else {
            return;
        };
        let generation = self.anim.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let last_played: HashMap<String, i64> = self
            .entries
            .iter()
            .filter_map(|e| {
                let key = self.art_keys.get(&e.game.id)?;
                Some((key.as_str().to_owned(), e.last_played?))
            })
            .collect();
        let last_played = Arc::new(last_played);
        let deliver: Arc<dyn Fn(Delivery) + Send + Sync> =
            Arc::new(|d| defer(move |c| c.anim_delivery(d)));
        self.anim.stopped.clear();
        for kind in KINDS {
            let Some(source) = art.animated(kind) else {
                continue;
            };
            let track = Track::new();
            anim::spawn(
                TrackJob {
                    kind,
                    key: key.clone(),
                    source: source.to_path_buf(),
                    class: self.art_loader.class(),
                    cache: self.art_cache.clone(),
                    last_played: last_played.clone(),
                    generation,
                    current: self.anim.generation.clone(),
                    credits: track.credits.clone(),
                },
                deliver.clone(),
            );
            self.anim.tracks.insert(kind, track);
        }
        tracing::debug!(game = %game, tracks = self.anim.tracks.len(), "animation starts");
        if let Some(app) = self.app.upgrade() {
            app.global::<HomeData>().set_anim_id(game.as_str().into());
        }
        self.anim.playing = Some(game);
    }

    /// Stops whatever plays; the stills show again.
    pub(super) fn anim_stop(&mut self, why: &str) {
        self.anim.settle.stop();
        self.anim.generation.fetch_add(1, Ordering::Relaxed);
        for track in self.anim.tracks.values() {
            track.credits.reset();
        }
        if let Some(game) = self.anim.playing.take() {
            tracing::debug!(game = %game, why, "animation stops");
        }
        self.anim.tracks.clear();
        if let Some(app) = self.app.upgrade() {
            let home = app.global::<HomeData>();
            // The last frames stay drawn while they fade out (`base`).
            home.set_anim_hero_on(false);
            home.set_anim_logo_on(false);
            home.set_anim_id("".into());
        }
    }

    fn anim_delivery(&mut self, delivery: Delivery) {
        let current = self.anim.generation.load(Ordering::Relaxed);
        match delivery {
            Delivery::Start {
                generation,
                kind,
                info,
            } if generation == current => {
                if let Some(track) = self.anim.tracks.get_mut(&kind) {
                    tracing::info!(
                        kind = kind.name(),
                        mode = info.mode.name(),
                        frames = ?info.frames,
                        size = ?info.size,
                        prepare_ms = info.prepare.as_millis(),
                        "animation ready"
                    );
                    track.info = Some(info);
                }
            }
            Delivery::Frame {
                generation,
                kind,
                pixels,
                delay_ms,
            } if generation == current => {
                let Some(track) = self.anim.tracks.get_mut(&kind) else {
                    return;
                };
                let frame = (Image::from_rgba8(pixels), delay_ms);
                if track.windowed() {
                    track.queue.push_back(frame);
                } else {
                    track.frames.push(frame);
                }
                if !track.shown {
                    self.anim_tick(kind);
                }
            }
            Delivery::Complete { generation, kind } if generation == current => {
                if let Some(track) = self.anim.tracks.get_mut(&kind) {
                    track.complete = true;
                }
            }
            Delivery::Still {
                generation,
                kind,
                reason,
            } if generation == current => {
                self.anim_drop_track(kind, reason);
            }
            // An older play's: dropped (its window frame frees nothing: the count was reset).
            _ => {}
        }
    }

    /// Shows the next frame of `kind` and sets the timer for the one after.
    fn anim_tick(&mut self, kind: ArtKind) {
        let Some(track) = self.anim.tracks.get_mut(&kind) else {
            return;
        };
        let next = if track.windowed() {
            track.queue.pop_front().inspect(|_| {
                if track.shown {
                    // The frame it replaces is gone: room for one more.
                    track.credits.release();
                }
            })
        } else if !track.shown {
            track.frames.first().cloned()
        } else {
            let i = track.index + 1;
            match track.frames.get(i) {
                Some(frame) => {
                    track.index = i;
                    Some(frame.clone())
                }
                None if track.complete => {
                    track.index = 0;
                    track.frames.first().cloned()
                }
                None => None,
            }
        };
        // Late: the frame isn't decoded yet. Only counts once playing for real (the first
        // full play is still filling in).
        let counts = track.windowed() || track.complete;
        track.ticks += u32::from(counts);
        let retry_in;
        match next {
            Some((image, delay)) => {
                track.delay_ms = delay;
                track.flips += 1;
                frames::count("anim-flips", 1);
                if !track.shown {
                    track.shown = true;
                    track.first_frame = Some(track.started.elapsed());
                }
                retry_in = Duration::from_millis(u64::from(delay));
                if let Some(app) = self.app.upgrade() {
                    let home = app.global::<HomeData>();
                    match kind {
                        ArtKind::Hero => {
                            home.set_anim_hero(image);
                            home.set_anim_hero_on(true);
                        }
                        _ => {
                            home.set_anim_logo(image);
                            home.set_anim_logo_on(true);
                        }
                    }
                }
            }
            None => {
                track.late += u32::from(counts);
                track.late_total += u64::from(counts);
                retry_in = Duration::from_millis(u64::from(track.delay_ms / 2).max(10));
            }
        }
        if track.span_start.elapsed() >= LATE_SPAN {
            #[allow(clippy::cast_precision_loss)]
            let share = track.late as f32 / track.ticks.max(1) as f32;
            if share > LATE_LIMIT {
                let reason = format!("falling behind ({:.0} % of frames late)", share * 100.0);
                self.anim_drop_track(kind, reason);
                return;
            }
            track.late = 0;
            track.ticks = 0;
            track.span_start = Instant::now();
        }
        if track.shown {
            track
                .timer
                .start(slint::TimerMode::SingleShot, retry_in, move || {
                    with(|c| c.anim_tick(kind));
                });
        }
    }

    /// One track stops for good this play; its still shows.
    fn anim_drop_track(&mut self, kind: ArtKind, reason: String) {
        if let Some(track) = self.anim.tracks.remove(&kind) {
            track.credits.reset();
            tracing::info!(kind = kind.name(), "animation shows its still: {reason}");
        }
        self.anim.stopped.push(Stopped { kind, reason });
        if let Some(app) = self.app.upgrade() {
            let home = app.global::<HomeData>();
            match kind {
                ArtKind::Hero => home.set_anim_hero_on(false),
                _ => home.set_anim_logo_on(false),
            }
        }
    }

    /// What plays now, as JSON (the dev socket's `anim_state`).
    pub(super) fn anim_state(&self) -> serde_json::Value {
        let tracks: Vec<serde_json::Value> = self
            .anim
            .tracks
            .iter()
            .map(|(kind, t)| {
                let info = t.info.as_ref();
                serde_json::json!({
                    "kind": kind.name(),
                    "source": info.map(|i| i.source.display().to_string()),
                    "format": info.and_then(|i| i.format).map(|f| f.name()),
                    "mode": info.map(|i| i.mode.name()),
                    "frames": info.and_then(|i| i.frames),
                    "size": info.map(|i| [i.size.0, i.size.1]),
                    "frame_bytes": info.map(|i| i.frame_bytes),
                    "file_bytes": info.map(|i| i.file_bytes),
                    "prepare_ms": info.map(|i| i.prepare.as_millis()),
                    "first_frame_ms": t.first_frame.map(|d| d.as_millis()),
                    "decoded": if t.windowed() { t.queue.len() } else { t.frames.len() },
                    "complete": t.complete,
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
            .map(|s| serde_json::json!({ "kind": s.kind.name(), "reason": s.reason }))
            .collect();
        let total: u64 = self.anim.tracks.values().map(Track::bytes).sum();
        serde_json::json!({
            "enabled": self.display_settings.animated_art,
            "reduce_motion": self.display_settings.reduce_motion,
            "focus": self.anim.focus.as_ref().map(GameId::as_str),
            "playing": self.anim.playing.as_ref().map(GameId::as_str),
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
