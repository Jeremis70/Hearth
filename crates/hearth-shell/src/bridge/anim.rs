//! Decoding animated heroes and logos for Home, off the UI thread (README §6 "Animated art").
//!
//! One thread per track (the focused game's hero, its logo; the Customize page's preview)
//! turns the source into display-sized frames and hands them to the UI thread one by one
//! ([`Delivery`]); `session/anim.rs` flips them with each frame's own delay.
//!
//! - The first play writes the `.anim` file (`hearth_core::art::animation`), so later plays
//!   only decode QOI frames.
//! - **Full:** when all frames fit the track's cap ([`cap`]), all are decoded once and kept.
//! - **Window:** otherwise [`WINDOW`] frames at most wait decoded ahead of the one shown; the
//!   thread decodes the next as each is shown (from the `.anim` file, or from the source when
//!   the file would be over [`FILE_CAP`]).
//! - **Still:** when even the window doesn't fit, or decoding fails: the still stays.
//!
//! The caps add up to under [`TOTAL_CAP`], so the tracks that can play at once (hero and logo,
//! or the Customize preview) never go over it together.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use hearth_core::art::animation::{self, AnimReader, FrameStream, SizedAnim};
use hearth_core::art::{AnimFormat, ArtCache, ArtKey, ArtKind, DisplayClass};
use slint::{Rgba8Pixel, SharedPixelBuffer};

const MB: u64 = 1_000_000;
/// Decoded frames one hero may keep (a 1920 × 620 hero is 4.8 MB a frame: 20 frames).
pub const HERO_CAP: u64 = 96 * MB;
/// Decoded frames one logo may keep (640 × 260 at 1080p is 0.67 MB a frame: ~70 frames).
pub const LOGO_CAP: u64 = 48 * MB;
/// All animations together.
pub const TOTAL_CAP: u64 = 160 * MB;
/// The largest `.anim` file written; bigger animations stream from their source.
pub const FILE_CAP: u64 = 64 * MB;
/// The `.anim` files together; the least recently played games' go first.
pub const DISK_CAP: u64 = 2_000 * MB;
/// Frames decoded ahead in window mode.
pub const WINDOW: usize = 8;

const _: () = assert!(HERO_CAP + LOGO_CAP <= TOTAL_CAP);

/// The most decoded frames a track may keep.
pub fn cap(kind: ArtKind) -> u64 {
    if kind == ArtKind::Logo {
        LOGO_CAP
    } else {
        HERO_CAP
    }
}

/// How a track plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every frame decoded once and kept.
    Full,
    /// A few frames decoded ahead, from the `.anim` file.
    Window,
    /// A few frames decoded ahead, straight from the source (too large for the file cache).
    Stream,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Window => "window",
            Self::Stream => "stream",
        }
    }
}

/// What a track found before its first frame.
#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub source: PathBuf,
    pub format: Option<AnimFormat>,
    pub mode: Mode,
    /// Frame count (unknown while streaming).
    pub frames: Option<usize>,
    pub frame_bytes: u64,
    pub size: (u32, u32),
    /// Making or opening the `.anim` file (the first play decodes the whole source).
    pub prepare: Duration,
    /// The `.anim` file's size (0 when streaming).
    pub file_bytes: u64,
}

/// From a track's thread to the UI thread.
pub enum Delivery {
    /// About to send frames.
    Start {
        generation: u64,
        kind: ArtKind,
        info: TrackInfo,
    },
    /// The next frame.
    Frame {
        generation: u64,
        kind: ArtKind,
        pixels: SharedPixelBuffer<Rgba8Pixel>,
        delay_ms: u32,
    },
    /// Full mode: every frame was sent.
    Complete { generation: u64, kind: ArtKind },
    /// It can't animate: the still stays.
    Still {
        generation: u64,
        kind: ArtKind,
        reason: String,
    },
}

/// Frames sent in window mode but not shown yet; a track's thread waits while there are
/// [`WINDOW`].
#[derive(Debug, Default)]
pub struct Credits {
    queued: Mutex<usize>,
    freed: Condvar,
}

impl Credits {
    /// One frame was shown (or dropped).
    pub fn release(&self) {
        if let Ok(mut queued) = self.queued.lock() {
            *queued = queued.saturating_sub(1);
            self.freed.notify_all();
        }
    }

    /// All frames dropped (the track stopped).
    pub fn reset(&self) {
        if let Ok(mut queued) = self.queued.lock() {
            *queued = 0;
            self.freed.notify_all();
        }
    }

    /// Waits for room; false once `alive` says the track is over.
    fn take(&self, alive: &dyn Fn() -> bool) -> bool {
        let Ok(mut queued) = self.queued.lock() else {
            return false;
        };
        while *queued >= WINDOW {
            if !alive() {
                return false;
            }
            queued = match self.freed.wait_timeout(queued, Duration::from_millis(100)) {
                Ok((guard, _)) => guard,
                Err(_) => return false,
            };
        }
        *queued += 1;
        alive()
    }
}

/// One track to play.
pub struct TrackJob {
    pub kind: ArtKind,
    pub key: ArtKey,
    pub source: PathBuf,
    pub class: DisplayClass,
    pub cache: ArtCache,
    /// Last played time by art key, for evicting `.anim` files.
    pub last_played: Arc<HashMap<String, i64>>,
    /// The play this track belongs to; it ends when this changes.
    pub generation: u64,
    pub current: Arc<AtomicU64>,
    pub credits: Arc<Credits>,
}

/// Starts a track's thread. `deliver` hands each [`Delivery`] to the UI thread.
pub fn spawn(job: TrackJob, deliver: Arc<dyn Fn(Delivery) + Send + Sync>) {
    let name = format!("hearth-anim-{}", job.kind.name());
    let spawned = thread::Builder::new().name(name).spawn(move || {
        let (generation, kind) = (job.generation, job.kind);
        if let Err(reason) = run(&job, &*deliver) {
            tracing::info!(kind = kind.name(), source = %job.source.display(), "still: {reason}");
            deliver(Delivery::Still {
                generation,
                kind,
                reason,
            });
        }
    });
    if let Err(err) = spawned {
        tracing::warn!("can't start an animation thread: {err}");
    }
}

fn run(job: &TrackJob, deliver: &dyn Fn(Delivery)) -> Result<(), String> {
    let alive = || job.current.load(Ordering::Relaxed) == job.generation;
    let (generation, kind) = (job.generation, job.kind);
    let started = Instant::now();
    let sized = job
        .cache
        .sized_anim(&job.key, kind, &job.source, job.class, FILE_CAP)
        .map_err(|err| format!("can't decode: {err}"))?;
    let format = animation::probe(&job.source);
    match sized {
        SizedAnim::Still => Err("not animated".into()),
        SizedAnim::File(path) => {
            let disk = job.cache.trim_anim(DISK_CAP, &job.last_played, Some(&path));
            let mut reader = AnimReader::open(&path).map_err(|err| err.to_string())?;
            let file_bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
            let mode = if reader.decoded_bytes() <= cap(kind) {
                Mode::Full
            } else if reader.frame_bytes() * WINDOW as u64 <= cap(kind) {
                Mode::Window
            } else {
                return Err(format!(
                    "{} MB a frame is too large",
                    reader.frame_bytes() / MB
                ));
            };
            tracing::debug!(kind = kind.name(), disk_mb = disk / MB, "animation cache");
            deliver(Delivery::Start {
                generation,
                kind,
                info: TrackInfo {
                    source: job.source.clone(),
                    format,
                    mode,
                    frames: Some(reader.len()),
                    frame_bytes: reader.frame_bytes(),
                    size: reader.size(),
                    prepare: started.elapsed(),
                    file_bytes,
                },
            });
            if mode == Mode::Full {
                for i in 0..reader.len() {
                    if !alive() {
                        return Ok(());
                    }
                    let frame = reader.frame(i).map_err(|err| err.to_string())?;
                    deliver(frame_delivery(job, frame, reader.delay_ms(i)));
                }
                deliver(Delivery::Complete { generation, kind });
                return Ok(());
            }
            let mut i = 0;
            while job.credits.take(&alive) {
                let frame = reader.frame(i).map_err(|err| err.to_string())?;
                deliver(frame_delivery(job, frame, reader.delay_ms(i)));
                i = (i + 1) % reader.len();
            }
            Ok(())
        }
        SizedAnim::TooLarge => {
            let mut stream = FrameStream::open(&job.source, kind, job.class)
                .map_err(|err| format!("can't decode: {err}"))?;
            let (first, delay) = stream.next_frame().map_err(|err| err.to_string())?;
            let frame_bytes = u64::from(first.width()) * u64::from(first.height()) * 4;
            if frame_bytes * WINDOW as u64 > cap(kind) {
                return Err(format!("{} MB a frame is too large", frame_bytes / MB));
            }
            deliver(Delivery::Start {
                generation,
                kind,
                info: TrackInfo {
                    source: job.source.clone(),
                    format,
                    mode: Mode::Stream,
                    frames: None,
                    frame_bytes,
                    size: first.dimensions(),
                    prepare: started.elapsed(),
                    file_bytes: 0,
                },
            });
            let mut next = Some((first, delay));
            while job.credits.take(&alive) {
                let (frame, delay) = match next.take() {
                    Some(frame) => frame,
                    None => stream.next_frame().map_err(|err| err.to_string())?,
                };
                deliver(frame_delivery(job, frame, delay));
            }
            Ok(())
        }
    }
}

fn frame_delivery(job: &TrackJob, frame: image::RgbaImage, delay_ms: u32) -> Delivery {
    Delivery::Frame {
        generation: job.generation,
        kind: job.kind,
        pixels: SharedPixelBuffer::clone_from_slice(frame.as_raw(), frame.width(), frame.height()),
        delay_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hearth_core::model::SystemId;

    fn tempdir() -> tempfile::TempDir {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
        std::fs::create_dir_all(&base).unwrap();
        tempfile::tempdir_in(base).unwrap()
    }

    struct Played {
        deliveries: Vec<Delivery>,
    }

    impl Played {
        fn frames(&self) -> usize {
            self.deliveries
                .iter()
                .filter(|d| matches!(d, Delivery::Frame { .. }))
                .count()
        }

        fn mode(&self) -> Option<Mode> {
            self.deliveries.iter().find_map(|d| match d {
                Delivery::Start { info, .. } => Some(info.mode),
                _ => None,
            })
        }
    }

    /// Plays `bytes` as `kind` until `frames` frames arrived (or the track ends).
    fn play(kind: ArtKind, bytes: &[u8], frames: usize, release: bool) -> Played {
        let dir = tempdir();
        let cache = ArtCache::new(dir.path().join("art"));
        let key = ArtKey::new(&SystemId::new("n64"), Some("CZLE"), "");
        let source = cache.store(&key, kind, "gif", bytes).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let current = Arc::new(AtomicU64::new(1));
        let credits = Arc::new(Credits::default());
        let deliver: Arc<dyn Fn(Delivery) + Send + Sync> = Arc::new(move |d| {
            let _ = tx.send(d);
        });
        spawn(
            TrackJob {
                kind,
                key,
                source,
                class: DisplayClass::default(),
                cache,
                last_played: Arc::default(),
                generation: 1,
                current: current.clone(),
                credits: credits.clone(),
            },
            deliver,
        );
        let mut deliveries = Vec::new();
        while let Ok(d) = rx.recv_timeout(Duration::from_secs(10)) {
            let done = matches!(d, Delivery::Complete { .. } | Delivery::Still { .. });
            if matches!(d, Delivery::Frame { .. }) && release {
                credits.release();
            }
            deliveries.push(d);
            let frames_now = deliveries
                .iter()
                .filter(|d| matches!(d, Delivery::Frame { .. }))
                .count();
            if done || frames_now >= frames {
                break;
            }
        }
        // Not taken by a window that waits for room: nothing more arrives.
        if !release {
            assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        }
        current.store(2, Ordering::Relaxed);
        Played { deliveries }
    }

    #[test]
    fn small_animations_are_decoded_once_in_full() {
        let gif = animation::samples::gif(320, 120, 5, 60);
        let played = play(ArtKind::Logo, &gif, usize::MAX, false);
        assert_eq!(played.mode(), Some(Mode::Full));
        assert_eq!(played.frames(), 5);
        assert!(matches!(
            played.deliveries.last(),
            Some(Delivery::Complete { .. })
        ));
    }

    #[test]
    fn large_animations_play_through_a_window_that_waits_for_room() {
        // 13 frames of 1920 × 1080 (8.3 MB each, 108 MB) are over the hero's cap.
        let gif = animation::samples::apng(1920, 1080, 13, 60);
        let played = play(ArtKind::Hero, &gif, usize::MAX, false);
        assert_eq!(played.mode(), Some(Mode::Window));
        assert_eq!(played.frames(), WINDOW, "stops once the window is full");
        // Shown frames make room: it keeps going, past the end (looping).
        let played = play(ArtKind::Hero, &gif, 20, true);
        assert_eq!(played.frames(), 20);
    }

    #[test]
    fn stills_say_why() {
        let mut png = Vec::new();
        image::RgbaImage::new(8, 8)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let played = play(ArtKind::Hero, &png, usize::MAX, false);
        assert!(matches!(
            played.deliveries.as_slice(),
            [Delivery::Still { reason, .. }] if reason == "not animated"
        ));
    }
}
