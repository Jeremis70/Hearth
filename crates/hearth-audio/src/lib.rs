//! Hearth's audio engine: UI sounds, the ambient loop and game themes.
//!
//! [`Audio`] is a cheap handle; every call only sends a message to the `hearth-audio` thread
//! and returns at once, so the UI thread never waits on audio. That thread owns the output
//! device (reopening it when it goes away), the decoded UI sounds and the music streams.
//!
//! Threads:
//! - `hearth-audio`: commands, device, decoding UI sounds, starting/stopping music.
//! - `hearth-audio-stream`: one per music track, decoding and resampling into a ring.
//! - the output callback (cpal's thread, or `hearth-audio-null`): the mixer.
//!
//! Crates: cpal (output), symphonia (decoding, MPL-2.0), rubato (resampling), rtrb (lock-free
//! queues into the callback).

pub mod analyze;
mod decode;
pub mod director;
mod engine;
mod gain;
mod mixer;
mod output;
mod resample;
pub(crate) mod stream;
#[cfg(test)]
mod testwav;

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use hearth_core::packs::LoopSpec;
use serde::Serialize;

pub use analyze::{Analysis, AutoLoop};
pub use gain::volume_to_gain;
#[doc(hidden)]
pub use stream::loop_seam;

/// Errors inside the engine (logged, never returned to the UI).
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// A file can't be read or decoded.
    #[error("{path}: {message}")]
    Decode {
        /// File.
        path: PathBuf,
        /// What went wrong.
        message: String,
    },
    /// Resampling failed.
    #[error("resampling: {0}")]
    Resample(String),
    /// The output device can't be opened or failed.
    #[error("output device: {0}")]
    Device(String),
}

/// A sound file found for a logical name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SoundFile {
    /// The file.
    pub path: PathBuf,
    /// Where it came from ("pack:ps5-sounds", "default", "rom"), for the state report.
    pub origin: String,
    /// Loop points (music only).
    #[serde(skip)]
    pub loop_spec: Option<LoopSpec>,
    /// Loop it by analysing it instead ([`analyze`]; themes). Wins over `loop_spec`.
    #[serde(skip)]
    pub auto_loop: Option<AutoLoop>,
    /// Gain correction in dB (a pack's `[gain]`), on top of the volumes.
    pub gain_db: i32,
}

impl SoundFile {
    /// The gain correction as an amplitude factor.
    pub fn gain(&self) -> f32 {
        10f32.powf(self.gain_db as f32 / 20.0)
    }
}

/// Hearth's own volumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Volumes {
    /// UI sounds, 0–100 (perceptual).
    pub ui: u8,
    /// Music, 0–100 (perceptual).
    pub music: u8,
    /// Everything silent.
    pub mute: bool,
}

impl Default for Volumes {
    fn default() -> Self {
        Self {
            ui: 80,
            music: 70,
            mute: false,
        }
    }
}

/// A music track to play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Music {
    /// What it is ("ambient", "theme:<game>"); the same key keeps the same stream.
    pub key: String,
    /// The file.
    pub file: SoundFile,
    /// When it is faded out for another track, keep its place and continue from there when it
    /// comes back (the ambient); otherwise it restarts from the top (themes).
    pub resume: bool,
}

/// Where the sound goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The default output device.
    Device,
    /// Nowhere (tests, headless runs); the mixer still runs in real time.
    Null,
}

/// Engine options.
#[derive(Debug, Clone, Copy)]
pub struct AudioOptions {
    /// Output.
    pub backend: Backend,
    /// Log the time from input to the first mixed sample of every UI sound.
    pub stats: bool,
}

/// One music track in the state report.
#[derive(Debug, Clone, Serialize)]
pub struct TrackState {
    /// Its key.
    pub key: String,
    /// The file.
    pub path: PathBuf,
    /// Where it came from.
    pub origin: String,
    /// Fading in or playing (not on its way out or parked).
    pub wanted: bool,
}

/// What the engine is doing, for the dev socket.
#[derive(Debug, Clone, Serialize)]
pub struct AudioState {
    /// "device" or "null".
    pub backend: &'static str,
    /// The output's name, if one is open.
    pub device: Option<String>,
    /// The mixer's rate.
    pub rate: u32,
    /// Frames per callback asked for (`None`: the device's default).
    pub buffer: Option<u32>,
    /// The output delay cpal reported last, in ms.
    pub output_delay_ms: f32,
    /// Output callbacks so far.
    pub callbacks: u64,
    /// Output underruns reported by the device.
    pub xruns: u64,
    /// Music samples the mixer wanted but the decoder hadn't made yet.
    pub underruns: u64,
    /// Times the output was reopened.
    pub reopens: u64,
    /// Volumes.
    pub volumes: Volumes,
    /// UI sounds playing now.
    pub voices: u64,
    /// Music tracks (playing, fading or parked).
    pub music: Vec<TrackState>,
    /// The key of the music asked for, if any.
    pub current: Option<String>,
    /// Logical name → file, for every UI sound loaded.
    pub sounds: Vec<(String, Option<SoundFile>)>,
}

enum Request {
    Sounds(Vec<(String, Option<SoundFile>)>),
    Play {
        name: String,
        input_at: Option<Instant>,
    },
    Volumes(Volumes),
    Music {
        music: Option<Music>,
        fade: Duration,
    },
    State(Sender<AudioState>),
}

/// Handle to the audio thread. Clones share it; the thread ends when the last one is dropped.
#[derive(Debug, Clone)]
pub struct Audio {
    tx: Sender<Request>,
}

impl Audio {
    /// Starts the engine. Never fails: without a device Hearth is silent and keeps retrying.
    pub fn start(options: AudioOptions) -> Self {
        let (tx, rx) = mpsc::channel();
        engine::spawn(options, rx);
        Self { tx }
    }

    /// Replaces the UI sounds (decoded on the audio thread). `None`: no file for that name,
    /// it stays silent.
    pub fn set_sounds(&self, sounds: Vec<(String, Option<SoundFile>)>) {
        let _ = self.tx.send(Request::Sounds(sounds));
    }

    /// Plays a UI sound. `input_at`: when the input that caused it happened (for stats).
    pub fn play(&self, name: &str, input_at: Option<Instant>) {
        let _ = self.tx.send(Request::Play {
            name: name.to_owned(),
            input_at,
        });
    }

    /// Sets the volumes (applied with a short glide).
    pub fn set_volumes(&self, volumes: Volumes) {
        let _ = self.tx.send(Request::Volumes(volumes));
    }

    /// Crossfades to `music` (or fades to silence) over `fade`.
    pub fn set_music(&self, music: Option<Music>, fade: Duration) {
        let _ = self.tx.send(Request::Music { music, fade });
    }

    /// The engine's state; waits up to `timeout`. Not for the UI thread.
    pub fn state(&self, timeout: Duration) -> Option<AudioState> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Request::State(tx)).ok()?;
        rx.recv_timeout(timeout).ok()
    }
}
