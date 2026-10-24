//! Making any theme loop well: where it really starts, and a loop region that leaves out its
//! fade-in and fade-out.
//!
//! - **First play** starts at the first sample above −50 dBFS ([`SILENCE_DB`]), so a theme
//!   with leading silence starts at once, and an intro is never cut. The user's "Start at"
//!   offset is added to it, and to nothing else.
//! - **The loop** runs between the first and the last 50 ms window whose level is within
//!   10 dB ([`LOOP_RANGE_DB`]) of the track's median level (over its non-silent windows),
//!   then each end moves inward past the rest of a fade: while the level (smoothed over
//!   500 ms) keeps rising toward the middle and is still more than 1 dB under the median. A
//!   fade-in or fade-out thus stays outside it, every pass stays at full level, and the end
//!   crossfades into the start over [`THEME_CROSSFADE_MS`] with no dip.
//!
//! Analysing means decoding the whole file (up to a second for a long mp3), so it is done
//! once per file and cached ([`AutoLoop::cache_dir`]); while it runs the first play has
//! already started from a quick scan for the first sound.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hearth_core::packs::LoopSpec;
use serde::{Deserialize, Serialize};

use crate::AudioError;
use crate::decode::Decoder;

/// Below this a sample counts as silence.
pub const SILENCE_DB: f32 = -50.0;
/// Windows this close to the median level are inside the loop.
pub const LOOP_RANGE_DB: f32 = 10.0;
/// How long a theme's end fades into its start.
pub const THEME_CROSSFADE_MS: u32 = 2000;
/// Level windows.
const WINDOW_MS: u64 = 50;
/// Windows averaged to follow a fade.
const SMOOTH_WINDOWS: usize = 10;
/// A fade is over this close to the median.
const FADE_DONE_DB: f32 = 1.0;
/// The quick scan for the first sound gives up here (the analysis will know).
const ONSET_SCAN_LIMIT_MS: u64 = 30_000;
/// Bumped when the analysis changes, so cached results are made again.
const VERSION: u32 = 1;

/// Loop a file automatically ([`analyze`]) instead of with fixed loop points.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutoLoop {
    /// Added to where the first play starts ("Start at").
    pub offset_ms: u64,
    /// Where analyses are cached (`<cache>/audio`); `None`: not cached.
    pub cache_dir: Option<PathBuf>,
}

/// What [`analyze`] found, in ms from the start of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Analysis {
    /// The first sample above [`SILENCE_DB`].
    pub onset_ms: u64,
    /// The loop region: first and last window near the median level.
    pub loop_start_ms: u64,
    /// See `loop_start_ms`.
    pub loop_end_ms: u64,
    /// The whole file.
    pub duration_ms: u64,
}

impl Analysis {
    /// Loop points for a theme, its first play starting `offset_ms` after the first sound.
    pub fn theme_loop(&self, offset_ms: u64) -> LoopSpec {
        // Leave at least two crossfades of loop after the first play's start.
        let latest = self
            .loop_end_ms
            .saturating_sub(2 * u64::from(THEME_CROSSFADE_MS))
            .max(self.loop_start_ms);
        LoopSpec {
            start_ms: self.loop_start_ms,
            end_ms: Some(self.loop_end_ms),
            crossfade_ms: THEME_CROSSFADE_MS,
            intro_ms: (self.onset_ms + offset_ms).min(latest.max(self.onset_ms)),
        }
    }
}

fn db(mean_square: f64) -> f32 {
    (10.0 * mean_square.max(1e-12).log10()) as f32
}

/// Decodes `path` and finds where it starts and the loop region (see the module docs).
pub fn analyze(path: &Path) -> Result<Analysis, AudioError> {
    let mut decoder = Decoder::open(path)?;
    let rate = u64::from(decoder.rate());
    let window = (rate * WINDOW_MS / 1000).max(1);
    let silence = 10f32.powf(SILENCE_DB / 20.0);
    let mut levels: Vec<f32> = Vec::new();
    let (mut sum, mut in_window) = (0f64, 0u64);
    let mut onset: Option<u64> = None;
    let mut last_sound = 0u64;
    let mut pos = 0u64;
    let mut chunk = Vec::with_capacity(8192);
    loop {
        chunk.clear();
        if decoder.next_into(&mut chunk)? == 0 {
            break;
        }
        for frame in chunk.as_chunks::<2>().0 {
            let (l, r) = (frame[0], frame[1]);
            if l.abs().max(r.abs()) > silence {
                onset.get_or_insert(pos);
                last_sound = pos + 1;
            }
            sum += f64::from(l * l + r * r) / 2.0;
            in_window += 1;
            if in_window == window {
                levels.push(db(sum / window as f64));
                (sum, in_window) = (0.0, 0);
            }
            pos += 1;
        }
    }
    if in_window > 0 {
        levels.push(db(sum / in_window as f64));
    }
    let ms = |frames: u64| frames * 1000 / rate;
    let duration_ms = ms(pos);
    let Some(onset) = onset else {
        return Ok(Analysis {
            onset_ms: 0,
            loop_start_ms: 0,
            loop_end_ms: duration_ms,
            duration_ms,
        });
    };
    let mut sounding: Vec<f32> = levels.iter().copied().filter(|&l| l > SILENCE_DB).collect();
    sounding.sort_by(f32::total_cmp);
    let median = sounding
        .get(sounding.len() / 2)
        .copied()
        .unwrap_or(SILENCE_DB);
    let near = |l: &f32| *l >= median - LOOP_RANGE_DB;
    let mut first = levels.iter().position(near).unwrap_or(0);
    let mut last = levels
        .iter()
        .rposition(near)
        .map_or(levels.len(), |i| i + 1);
    // Past the rest of a fade-in and a fade-out.
    let smooth = smoothed(&levels);
    let fading = |i: usize| smooth[i] < median - FADE_DONE_DB;
    while first + 1 < last && fading(first) && smooth[first + 1] > smooth[first] {
        first += 1;
    }
    while last > first + 2 && fading(last - 1) && smooth[last - 2] > smooth[last - 1] {
        last -= 1;
    }
    let (first, last) = (first as u64, last as u64);
    let mut loop_start = (first * window).max(onset);
    let mut loop_end = (last * window).min(last_sound.max(onset + 1));
    // Too short a region to loop on (a blip in silence): loop everything that sounds.
    if loop_end <= loop_start + window * 4 {
        (loop_start, loop_end) = (onset, last_sound.max(onset + 1));
    }
    Ok(Analysis {
        onset_ms: ms(onset),
        loop_start_ms: ms(loop_start),
        loop_end_ms: ms(loop_end),
        duration_ms,
    })
}

/// Each level averaged (in dB) with its neighbours over [`SMOOTH_WINDOWS`].
fn smoothed(levels: &[f32]) -> Vec<f32> {
    let half = SMOOTH_WINDOWS / 2;
    (0..levels.len())
        .map(|i| {
            let window = &levels[i.saturating_sub(half)..(i + half).min(levels.len())];
            window.iter().sum::<f32>() / window.len() as f32
        })
        .collect()
}

/// Where the first sound is, scanning at most 30 s (0 if none by then).
pub(crate) fn onset_ms(path: &Path) -> Result<u64, AudioError> {
    let mut decoder = Decoder::open(path)?;
    let rate = u64::from(decoder.rate());
    let silence = 10f32.powf(SILENCE_DB / 20.0);
    let limit = rate * ONSET_SCAN_LIMIT_MS / 1000;
    let mut pos = 0u64;
    let mut chunk = Vec::with_capacity(8192);
    while pos < limit {
        chunk.clear();
        if decoder.next_into(&mut chunk)? == 0 {
            break;
        }
        for frame in chunk.as_chunks::<2>().0 {
            if frame[0].abs().max(frame[1].abs()) > silence {
                return Ok(pos * 1000 / rate);
            }
            pos += 1;
        }
    }
    Ok(0)
}

/// How long `path` plays (from its header, else by reading its packets; nothing is decoded).
pub fn duration(path: &Path) -> Result<Duration, AudioError> {
    crate::decode::duration(path)
}

#[derive(Serialize, Deserialize)]
struct Cached {
    version: u32,
    analysis: Analysis,
}

/// The cache file for `path`'s analysis: named by its path, size and modification time, so
/// an edited file is analysed again.
pub(crate) fn cache_file(dir: &Path, path: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let id = format!(
        "{}|{}|{}.{}",
        path.display(),
        meta.len(),
        modified.as_secs(),
        modified.subsec_nanos()
    );
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    Some(dir.join(format!("{hash:016x}.json")))
}

/// A cached analysis of `path`, if there is one.
pub fn cached(dir: &Path, path: &Path) -> Option<Analysis> {
    let file = cache_file(dir, path)?;
    let cached: Cached = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    (cached.version == VERSION).then_some(cached.analysis)
}

/// Analyses `path` and caches the result in `dir`.
pub fn analyze_cached(dir: Option<&Path>, path: &Path) -> Result<Analysis, AudioError> {
    if let Some(found) = dir.and_then(|dir| cached(dir, path)) {
        return Ok(found);
    }
    let analysis = analyze(path)?;
    tracing::debug!(path = %path.display(), ?analysis, "theme analysed");
    if let Some(file) = dir.and_then(|dir| cache_file(dir, path)) {
        let saved = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                let json = serde_json::to_vec(&Cached {
                    version: VERSION,
                    analysis,
                })
                .map_err(std::io::Error::other)?;
                std::fs::write(&file, json)
            });
        if let Err(err) = saved {
            tracing::warn!(path = %file.display(), "can't cache a theme analysis: {err}");
        }
    }
    Ok(analysis)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testwav::write_wav;

    pub(crate) const RATE: u32 = 48_000;

    /// A tone of `secs` seconds whose amplitude follows `envelope(t)`.
    pub(crate) fn tone(secs: f32, envelope: impl Fn(f32) -> f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                // Two tones, so the level is steady over a window.
                let s = (t * 220.0 * std::f32::consts::TAU).sin() * 0.6
                    + (t * 331.0 * std::f32::consts::TAU).sin() * 0.3;
                s * 0.5 * envelope(t)
            })
            .collect()
    }

    /// 3 s of silence, a 1 s fade-in, 20 s of music, a 3 s fade-out, 1 s of silence.
    pub(crate) fn faded_theme() -> Vec<f32> {
        tone(28.0, |t| match t {
            t if t < 3.0 => 0.0,
            t if t < 4.0 => t - 3.0,
            t if t < 24.0 => 1.0,
            t if t < 27.0 => (27.0 - t) / 3.0,
            _ => 0.0,
        })
    }

    #[test]
    fn leading_silence_and_fades_stay_out_of_the_loop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.wav");
        write_wav(&path, RATE, &faded_theme());
        let a = analyze(&path).unwrap();
        assert!((3000..3020).contains(&a.onset_ms), "onset {}", a.onset_ms);
        assert_eq!(a.duration_ms, 28_000);
        // Out of the fades: −1 dB is 0.89 of the amplitude, 0.89 s into the fade-in and
        // 0.33 s into the fade-out (give or take the smoothing).
        assert!(
            (3700..4150).contains(&a.loop_start_ms),
            "start {}",
            a.loop_start_ms
        );
        assert!(
            (23_900..24_500).contains(&a.loop_end_ms),
            "end {}",
            a.loop_end_ms
        );
        assert_eq!(onset_ms(&path).unwrap(), a.onset_ms);

        let spec = a.theme_loop(2000);
        assert_eq!(spec.intro_ms, a.onset_ms + 2000);
        assert_eq!(a.theme_loop(60_000).intro_ms, a.loop_end_ms - 4000);
    }

    #[test]
    fn a_steady_file_loops_whole_and_silence_gives_up_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let steady = dir.path().join("steady.wav");
        write_wav(&steady, RATE, &tone(4.0, |_| 1.0));
        let a = analyze(&steady).unwrap();
        assert_eq!((a.onset_ms, a.loop_start_ms), (0, 0));
        assert!(a.loop_end_ms >= 3990);

        let quiet = dir.path().join("quiet.wav");
        write_wav(&quiet, RATE, &vec![0.0; RATE as usize]);
        let a = analyze(&quiet).unwrap();
        assert_eq!((a.onset_ms, a.loop_start_ms, a.loop_end_ms), (0, 0, 1000));
    }

    #[test]
    fn analyses_are_cached_per_file_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.wav");
        write_wav(&path, RATE, &tone(2.0, |_| 1.0));
        let cache = dir.path().join("cache");
        assert_eq!(cached(&cache, &path), None);
        let a = analyze_cached(Some(&cache), &path).unwrap();
        assert_eq!(cached(&cache, &path), Some(a));
        assert_eq!(duration(&path).unwrap(), Duration::from_secs(2));
    }
}
