//! Streamed music (ambient loops, game themes): one decode thread per track fills a ring the
//! mixer reads. Tracks loop forever, gaplessly:
//!
//! - The first pass plays the file from its very start. On the way, the first `crossfade`
//!   frames after the loop start (the "head") are kept in memory.
//! - The last `crossfade` frames before the loop end (the "tail") are held back instead of
//!   being sent. At the loop end the tail is mixed into the head with an equal-power curve, so
//!   the end fades into the start with no gap and no click.
//! - The file is then reopened and decoded silently up to the end of the head, and playback
//!   continues from there. Reopening instead of seeking keeps this sample-exact for every
//!   format (mp3 seeking isn't).
//!
//! Without loop points the whole file loops with no crossfade (decoders trim the encoder's
//! padding, so a file made to loop does).
//!
//! Resampling happens after the loop is assembled, so the seam is resampled continuously too.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use hearth_core::packs::LoopSpec;
use rtrb::{Producer, RingBuffer};

use crate::AudioError;
use crate::decode::Decoder;
use crate::resample::StreamResampler;

/// Buffered music, in seconds: enough to ride over a slow disk or a busy CPU, small enough that
/// a parked track resumes close to where it stopped.
const RING_SECONDS: f32 = 0.75;

/// Loop points in frames at the file's rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LoopFrames {
    start: u64,
    end: Option<u64>,
    crossfade: u64,
}

impl LoopFrames {
    fn new(spec: Option<LoopSpec>, rate: u32) -> Self {
        let spec = spec.unwrap_or_default();
        let frames = |ms: u64| ms * u64::from(rate) / 1000;
        let start = frames(spec.start_ms);
        let end = spec.end_ms.map(frames).filter(|&end| end > start);
        let mut crossfade = frames(u64::from(spec.crossfade_ms));
        if let Some(end) = end {
            // The head and the tail must not overlap.
            crossfade = crossfade.min((end - start) / 2);
        }
        Self {
            start,
            end,
            crossfade,
        }
    }
}

/// Produces a file's frames forever, looping as described in the module docs.
struct Looper {
    path: PathBuf,
    decoder: Decoder,
    points: LoopFrames,
    /// Frames decoded from the start of the file in the current pass.
    pos: u64,
    /// Frames before this are decoded and dropped (after a loop).
    skip_until: u64,
    /// The head (stereo samples), filled during the first pass.
    head: Vec<f32>,
    head_done: bool,
    /// Held back samples: the last `crossfade` frames, the tail-to-be.
    held: VecDeque<f32>,
    chunk: Vec<f32>,
    /// Frames sent in the current pass (a pass that sends nothing would spin).
    sent_this_pass: u64,
    /// Times it looped.
    loops: u32,
}

impl Looper {
    fn open(path: &Path, spec: Option<LoopSpec>) -> Result<Self, AudioError> {
        let decoder = Decoder::open(path)?;
        let points = LoopFrames::new(spec, decoder.rate());
        Ok(Self {
            path: path.to_path_buf(),
            decoder,
            points,
            pos: 0,
            skip_until: 0,
            head: Vec::with_capacity(points.crossfade as usize * 2),
            head_done: points.crossfade == 0,
            held: VecDeque::with_capacity(points.crossfade as usize * 2 + 8192),
            chunk: Vec::with_capacity(8192),
            sent_this_pass: 0,
            loops: 0,
        })
    }

    fn rate(&self) -> u32 {
        self.decoder.rate()
    }

    /// Appends the next frames to `out` (at least one packet's worth, more at a loop point).
    fn next(&mut self, out: &mut Vec<f32>) -> Result<(), AudioError> {
        self.chunk.clear();
        let n = self.decoder.next_into(&mut self.chunk)? as u64;
        let mut at_end = n == 0;
        let mut first = self.pos;
        let mut last = self.pos + n;
        if let Some(end) = self.points.end
            && last >= end
        {
            last = end.max(first);
            at_end = true;
        }
        // Drop what comes before `skip_until` (after a loop).
        let skip = self.skip_until.saturating_sub(first).min(last - first);
        let from = (skip * 2) as usize;
        let to = ((last - self.pos) * 2) as usize;
        first += skip;
        if !self.head_done {
            let head_start = self.points.start;
            let head_end = head_start + self.points.crossfade;
            let a = first.max(head_start);
            let b = last.min(head_end);
            if a < b {
                let off = from + ((a - first) * 2) as usize;
                self.head
                    .extend_from_slice(&self.chunk[off..off + ((b - a) * 2) as usize]);
            }
            if last >= head_end {
                self.head_done = true;
            }
        }
        self.held.extend(&self.chunk[from..to]);
        let keep = (self.points.crossfade * 2) as usize;
        while self.held.len() > keep {
            let take = self.held.len() - keep;
            let before = out.len();
            out.extend(self.held.drain(..take));
            self.sent_this_pass += ((out.len() - before) / 2) as u64;
        }
        self.pos += n;
        if at_end {
            self.wrap(out)?;
        }
        Ok(())
    }

    /// At the loop end: crossfades the held tail into the head and starts the next pass.
    fn wrap(&mut self, out: &mut Vec<f32>) -> Result<(), AudioError> {
        let tail: Vec<f32> = self.held.drain(..).collect();
        let tail_frames = tail.len() / 2;
        let head_frames = self.head.len() / 2;
        let mix = tail_frames.min(head_frames);
        // The part of the tail before the crossfade (only when the head came out short).
        out.extend_from_slice(&tail[..(tail_frames - mix) * 2]);
        let tail = &tail[(tail_frames - mix) * 2..];
        for i in 0..mix {
            let t = (i as f32 + 0.5) / mix as f32 * std::f32::consts::FRAC_PI_2;
            let (fade_out, fade_in) = (t.cos(), t.sin());
            for c in 0..2 {
                out.push(tail[i * 2 + c] * fade_out + self.head[i * 2 + c] * fade_in);
            }
        }
        out.extend_from_slice(&self.head[mix * 2..]);
        let sent = self.sent_this_pass + (tail_frames + head_frames - mix) as u64;
        if sent == 0 {
            return Err(AudioError::Decode {
                path: self.path.clone(),
                message: "nothing to loop".into(),
            });
        }
        self.sent_this_pass = 0;
        self.loops += 1;
        self.decoder = Decoder::open(&self.path)?;
        self.pos = 0;
        self.skip_until = self.points.start + head_frames as u64;
        Ok(())
    }
}

/// The `around` seconds before and after a track's first loop seam, as the mixer would get
/// them (stereo, at the file's rate), and where the crossfade starts in it. For checking loop
/// points (`examples/loop_seam.rs`).
#[doc(hidden)]
pub fn loop_seam(
    path: &Path,
    spec: Option<LoopSpec>,
    around: f32,
) -> Result<(Vec<f32>, u32, usize), AudioError> {
    let mut looper = Looper::open(path, spec)?;
    let rate = looper.rate();
    let around = (around * rate as f32) as usize;
    let mut out = Vec::new();
    while looper.loops == 0 {
        let before = out.len();
        looper.next(&mut out)?;
        if looper.loops > 0 {
            // Where the wrap started writing: the crossfade.
            let seam = before / 2;
            while out.len() / 2 < seam + around {
                looper.next(&mut out)?;
            }
            let start = seam.saturating_sub(around);
            return Ok((
                out[start * 2..(seam + around) * 2].to_vec(),
                rate,
                seam - start,
            ));
        }
        // Only the last `around` seconds before the seam matter.
        if out.len() > around * 4 {
            out.drain(..out.len() - around * 2);
        }
    }
    Ok((out, rate, 0))
}

/// A playing music track's decode thread. Dropping it stops the thread.
pub(crate) struct MusicStream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MusicStream {
    /// Starts decoding `path` for a mixer at `rate`; returns the stream and the ring to hand to
    /// the mixer. Errors (a missing or broken file) are logged by the thread, which then ends;
    /// the mixer notices the abandoned ring.
    pub(crate) fn start(
        path: PathBuf,
        spec: Option<LoopSpec>,
        rate: u32,
    ) -> (Self, rtrb::Consumer<f32>) {
        let capacity = ((rate as f32 * RING_SECONDS) as usize).max(4096) * 2;
        let (producer, consumer) = RingBuffer::new(capacity);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::Builder::new()
            .name("hearth-audio-stream".into())
            .spawn(move || {
                if let Err(err) = run(&path, spec, rate, producer, &flag) {
                    tracing::warn!(path = %path.display(), "music stopped: {err}");
                }
            })
            .map_err(|err| tracing::warn!("couldn't start a music thread: {err}"))
            .ok();
        (Self { stop, thread }, consumer)
    }
}

impl Drop for MusicStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    path: &Path,
    spec: Option<LoopSpec>,
    rate: u32,
    mut producer: Producer<f32>,
    stop: &AtomicBool,
) -> Result<(), AudioError> {
    let mut looper = Looper::open(path, spec)?;
    let mut resampler = StreamResampler::new(looper.rate(), rate)?;
    let mut decoded = Vec::with_capacity(16_384);
    let mut ready: VecDeque<f32> = VecDeque::with_capacity(32_768);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if producer.is_abandoned() {
            return Ok(());
        }
        if ready.is_empty() {
            decoded.clear();
            looper.next(&mut decoded)?;
            let mut out = Vec::with_capacity(decoded.len() * 2);
            resampler.push(&decoded, &mut out)?;
            ready.extend(out);
            continue;
        }
        let room = producer.slots().min(ready.len());
        if room == 0 {
            thread::sleep(Duration::from_millis(15));
            continue;
        }
        if let Ok(chunk) = producer.write_chunk_uninit(room) {
            chunk.fill_from_iter(ready.drain(..room));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testwav::write_wav;

    /// Collects `frames` frames from a looper.
    fn collect(looper: &mut Looper, frames: usize) -> Vec<f32> {
        let mut out = Vec::new();
        while out.len() < frames * 2 {
            looper.next(&mut out).unwrap();
        }
        out.truncate(frames * 2);
        out
    }

    #[test]
    fn a_plain_loop_repeats_the_file_sample_for_sample() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loop.wav");
        let samples: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        write_wav(&path, 1000, &samples);
        let mut looper = Looper::open(&path, None).unwrap();
        let out = collect(&mut looper, 2500);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        for (i, v) in left.iter().enumerate() {
            assert!(
                (v - (i % 1000) as f32 / 1000.0).abs() < 1e-3,
                "frame {i}: {v}"
            );
        }
    }

    #[test]
    fn loop_points_crossfade_the_end_into_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("track.wav");
        // 1 kHz "file": 0.0 before the loop start, 1.0 inside the loop, 0.25 after the loop end.
        let samples: Vec<f32> = (0..3000)
            .map(|i| match i {
                0..500 => 0.0,
                500..2500 => 1.0,
                _ => 0.25,
            })
            .collect();
        write_wav(&path, 1000, &samples);
        let spec = LoopSpec {
            start_ms: 500,
            end_ms: Some(2500),
            crossfade_ms: 100,
        };
        let mut looper = Looper::open(&path, Some(spec)).unwrap();
        let out = collect(&mut looper, 2500 + 1900 + 10);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        // First pass: from the very start, up to the loop end (the tail is mixed, not cut).
        assert!(left[..500].iter().all(|&v| v.abs() < 1e-3));
        assert!(left[500..2400].iter().all(|&v| (v - 1.0).abs() < 1e-3));
        // The crossfade: tail (1.0) into head (1.0) with equal power, so ~1.0–1.41, never 0.
        for &v in &left[2400..2500] {
            assert!((0.99..1.42).contains(&v), "{v}");
        }
        // Then the loop continues after the head, never playing the intro or outro again.
        assert!(
            left[2500..]
                .iter()
                .all(|&v| (v - 1.0).abs() < 1e-3 || v > 1.0)
        );
        assert!(left.iter().all(|&v| (v - 0.25).abs() > 1e-3));
    }

    #[test]
    fn the_bundled_ambient_loops_without_a_step() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/sounds/ambient_loop.ogg");
        let mut looper = Looper::open(&path, None).unwrap();
        let frames = 32 * 48_000;
        let out = collect(&mut looper, frames + 4800);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let steps: Vec<f32> = left.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        let largest_inside = steps[..frames - 1].iter().fold(0f32, |m, &v| m.max(v));
        let seam = steps[frames - 1];
        assert!(seam <= largest_inside, "seam {seam} > {largest_inside}");
        // And it really wrapped: the loop's second pass matches its first.
        assert!((left[frames + 100] - left[100]).abs() < 1e-4);
    }

    #[test]
    fn loop_frames_are_clamped() {
        let spec = LoopSpec {
            start_ms: 1000,
            end_ms: Some(2000),
            crossfade_ms: 5000,
        };
        let points = LoopFrames::new(Some(spec), 1000);
        assert_eq!(points.crossfade, 500);
        let bad = LoopSpec {
            start_ms: 3000,
            end_ms: Some(2000),
            crossfade_ms: 0,
        };
        assert_eq!(LoopFrames::new(Some(bad), 1000).end, None);
    }
}
