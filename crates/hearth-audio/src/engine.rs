//! The `hearth-audio` control thread: takes requests from [`Audio`](crate::Audio), keeps the
//! output open, decodes UI sounds and runs the music streams.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::mixer::{AtSilence, Clip, Command, Event, Mixer, MixerLink, Stamp};
use crate::output::{self, Health, Output, OutputInfo, PREFERRED_RATE};
use crate::stream::MusicStream;
use crate::{AudioOptions, AudioState, Backend, Music, Request, SoundFile, TrackState, Volumes};
use crate::{decode, resample, volume_to_gain};

/// UI sounds longer than this are cut (they are kept in memory).
const MAX_SOUND_SECONDS: usize = 20;
/// The first retry after the output fails; doubles up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(5);
/// An open output whose callback hasn't run for this long is considered hung and reopened.
const STALL: Duration = Duration::from_secs(3);
/// Parked tracks kept for resuming (the ambient, and one spare).
const MAX_PARKED: usize = 2;
/// Sounds that cut each other (navigation), so a held D-pad never piles up.
const NAV_GROUP: u8 = 1;

struct Track {
    id: u64,
    music: Music,
    /// Dropping it stops the decode thread.
    _stream: MusicStream,
    wanted: bool,
    parked_at: Option<Instant>,
}

struct Engine {
    options: AudioOptions,
    rate: u32,
    mixer: Arc<Mutex<Mixer>>,
    link: MixerLink,
    health: Arc<Health>,
    output: Option<Output>,
    info: Option<OutputInfo>,
    retry_at: Instant,
    backoff: Duration,
    failures: u32,
    reopens: u64,
    opened_once: bool,
    last_callbacks: (u64, Instant),
    sources: Vec<(String, Option<SoundFile>)>,
    clips: HashMap<String, Clip>,
    /// Names for stats stamps (index = `Stamp::sound`).
    names: Vec<String>,
    missing_logged: HashSet<String>,
    volumes: Volumes,
    tracks: Vec<Track>,
    current: Option<String>,
    next_id: u64,
}

pub(crate) fn spawn(options: AudioOptions, rx: Receiver<Request>) {
    let started = thread::Builder::new()
        .name("hearth-audio".into())
        .spawn(move || Engine::new(options).run(rx));
    if let Err(err) = started {
        tracing::warn!("couldn't start the audio thread, Hearth is silent: {err}");
    }
}

impl Engine {
    fn new(options: AudioOptions) -> Self {
        let rate = match options.backend {
            Backend::Null => PREFERRED_RATE,
            Backend::Device => output::device_rate().unwrap_or(PREFERRED_RATE),
        };
        let (mixer, link) = Mixer::new(rate);
        Self {
            options,
            rate,
            mixer: Arc::new(Mutex::new(mixer)),
            link,
            health: Arc::new(Health::default()),
            output: None,
            info: None,
            retry_at: Instant::now(),
            backoff: FIRST_BACKOFF,
            failures: 0,
            reopens: 0,
            opened_once: false,
            last_callbacks: (0, Instant::now()),
            sources: Vec::new(),
            clips: HashMap::new(),
            names: Vec::new(),
            missing_logged: HashSet::new(),
            volumes: Volumes::default(),
            tracks: Vec::new(),
            current: None,
            next_id: 1,
        }
    }

    fn run(mut self, rx: Receiver<Request>) {
        self.send_gains();
        loop {
            self.keep_output();
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(request) => self.handle(request),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.drain_events();
        }
        // Stop the music threads before the output.
        self.tracks.clear();
        self.output = None;
    }

    fn handle(&mut self, request: Request) {
        match request {
            Request::Sounds(sounds) => {
                self.sources = sounds;
                self.load_clips();
            }
            Request::Play { name, input_at } => self.play(&name, input_at),
            Request::Volumes(volumes) => {
                self.volumes = volumes;
                self.send_gains();
            }
            Request::Music { music, fade } => self.set_music(music, fade),
            Request::State(reply) => {
                let _ = reply.send(self.state());
            }
        }
    }

    fn command(&mut self, command: Command) {
        if self.link.commands.push(command).is_err() {
            tracing::warn!("audio command queue full; dropped a command");
        }
    }

    fn send_gains(&mut self) {
        let (ui, music) = if self.volumes.mute {
            (0.0, 0.0)
        } else {
            (
                volume_to_gain(self.volumes.ui),
                volume_to_gain(self.volumes.music),
            )
        };
        self.command(Command::Gains { ui, music });
    }

    // --- UI sounds -----------------------------------------------------------------------

    fn load_clips(&mut self) {
        self.clips.clear();
        self.missing_logged.clear();
        let started = Instant::now();
        for (name, file) in &self.sources {
            if !self.names.contains(name) {
                self.names.push(name.clone());
            }
            let Some(file) = file else { continue };
            match load_clip(file, self.rate) {
                Ok(clip) => {
                    self.clips.insert(name.clone(), clip);
                }
                Err(err) => tracing::warn!(sound = %name, "can't load a UI sound: {err}"),
            }
        }
        tracing::info!(
            loaded = self.clips.len(),
            ms = started.elapsed().as_millis() as u64,
            "UI sounds decoded"
        );
    }

    fn play(&mut self, name: &str, input_at: Option<Instant>) {
        let Some(clip) = self.clips.get(name).cloned() else {
            if self.missing_logged.insert(name.to_owned()) {
                tracing::info!(sound = name, "no sound file for this name; staying silent");
            }
            return;
        };
        let sound = self.names.iter().position(|n| n == name).unwrap_or(0);
        let group = matches!(name, "move" | "edge_bump").then_some(NAV_GROUP);
        self.command(Command::Voice {
            clip,
            gain: 1.0,
            group,
            stamp: Stamp { sound, input_at },
        });
    }

    // --- music ---------------------------------------------------------------------------

    fn set_music(&mut self, music: Option<Music>, fade: Duration) {
        let frames = self.frames(fade);
        let key = music.as_ref().map(|m| m.key.clone());
        let mut fades = Vec::new();
        let mut found = false;
        for track in &mut self.tracks {
            let same = music
                .as_ref()
                .is_some_and(|m| m.key == track.music.key && m.file == track.music.file);
            if same {
                found = true;
                track.wanted = true;
                track.parked_at = None;
                fades.push((track.id, 1.0, AtSilence::Nothing));
            } else if track.wanted {
                track.wanted = false;
                // A track replaced under its own key (a pack change) never comes back.
                let replaced = music.as_ref().is_some_and(|m| m.key == track.music.key);
                let then = if track.music.resume && !replaced {
                    AtSilence::Park
                } else {
                    AtSilence::Remove
                };
                fades.push((track.id, 0.0, then));
            }
        }
        for (id, target, then) in fades {
            self.command(Command::Fade {
                id,
                target,
                frames,
                then,
            });
        }
        if let (Some(music), false) = (music, found) {
            self.start_track(music, frames);
        }
        self.current = key;
    }

    fn start_track(&mut self, music: Music, frames: u32) {
        let id = self.next_id;
        self.next_id += 1;
        let (stream, ring) =
            MusicStream::start(music.file.path.clone(), music.file.loop_spec, self.rate);
        tracing::info!(key = %music.key, path = %music.file.path.display(), origin = %music.file.origin, "music");
        self.command(Command::Music { id, ring });
        self.command(Command::Fade {
            id,
            target: 1.0,
            frames,
            then: AtSilence::Nothing,
        });
        self.tracks.push(Track {
            id,
            music,
            _stream: stream,
            wanted: true,
            parked_at: None,
        });
    }

    fn frames(&self, duration: Duration) -> u32 {
        (duration.as_secs_f64() * f64::from(self.rate))
            .round()
            .max(1.0) as u32
    }

    /// Too many parked tracks: the oldest go.
    fn trim_parked(&mut self) {
        let mut parked: Vec<(Instant, u64)> = self
            .tracks
            .iter()
            .filter_map(|t| t.parked_at.map(|at| (at, t.id)))
            .collect();
        if parked.len() <= MAX_PARKED {
            return;
        }
        parked.sort();
        for &(_, id) in &parked[..parked.len() - MAX_PARKED] {
            self.command(Command::Fade {
                id,
                target: 0.0,
                frames: 1,
                then: AtSilence::Remove,
            });
        }
    }

    fn drain_events(&mut self) {
        let mut parked_any = false;
        while let Ok(event) = self.link.events.pop() {
            match event {
                // Dropped here, outside the callback.
                Event::Clip(_) | Event::Ring(_) => {}
                Event::VoiceStarted {
                    stamp,
                    mixed_at,
                    output_delay,
                } => {
                    if self.options.stats {
                        self.log_stats(stamp, mixed_at, output_delay);
                    }
                }
                Event::Silent { id, removed } => {
                    if removed {
                        self.tracks.retain(|t| t.id != id);
                    } else if let Some(track) = self.tracks.iter_mut().find(|t| t.id == id)
                        && !track.wanted
                    {
                        track.parked_at = Some(Instant::now());
                        parked_any = true;
                    }
                }
                Event::MusicEnded { id } => {
                    self.tracks.retain(|t| t.id != id);
                }
            }
        }
        if parked_any {
            self.trim_parked();
        }
    }

    fn log_stats(&self, stamp: Stamp, mixed_at: Instant, output_delay: Duration) {
        let name = self.names.get(stamp.sound).map_or("?", String::as_str);
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        match stamp.input_at {
            Some(input_at) => {
                let to_mix = mixed_at.saturating_duration_since(input_at);
                tracing::info!(
                    target: "hearth_audio::stats",
                    sound = name,
                    input_to_mix_ms = format!("{:.1}", ms(to_mix)),
                    output_delay_ms = format!("{:.1}", ms(output_delay)),
                    total_ms = format!("{:.1}", ms(to_mix + output_delay)),
                    "audio stats"
                );
            }
            None => tracing::info!(
                target: "hearth_audio::stats",
                sound = name,
                output_delay_ms = format!("{:.1}", ms(output_delay)),
                "audio stats (no input time)"
            ),
        }
    }

    // --- output --------------------------------------------------------------------------

    /// Opens the output when it's closed and due, closes it when it failed or hung.
    fn keep_output(&mut self) {
        let now = Instant::now();
        if self.output.is_some() {
            let callbacks = self.health.callbacks.load(Ordering::Relaxed);
            if callbacks != self.last_callbacks.0 {
                self.last_callbacks = (callbacks, now);
            }
            let stalled = now.duration_since(self.last_callbacks.1) > STALL;
            if stalled {
                tracing::warn!("audio output stopped calling back; reopening it");
            }
            if stalled || self.health.failed.load(Ordering::Relaxed) {
                self.output = None;
                self.info = None;
                self.retry_at = now + FIRST_BACKOFF;
                self.backoff = FIRST_BACKOFF;
            }
            return;
        }
        if now < self.retry_at {
            return;
        }
        self.health.failed.store(false, Ordering::Relaxed);
        let opened = match self.options.backend {
            Backend::Null => Ok(output::open_null(self.mixer.clone(), self.health.clone())),
            Backend::Device => self.open_device(),
        };
        match opened {
            Ok((output, info)) => {
                tracing::info!(
                    device = %info.name,
                    rate = info.rate,
                    channels = info.channels,
                    buffer = ?info.buffer,
                    "audio output open"
                );
                if self.opened_once {
                    self.reopens += 1;
                }
                self.opened_once = true;
                self.failures = 0;
                self.backoff = FIRST_BACKOFF;
                self.last_callbacks = (self.health.callbacks.load(Ordering::Relaxed), now);
                self.output = Some(output);
                self.info = Some(info);
            }
            Err(err) => {
                if self.failures == 0 {
                    tracing::warn!("no audio output, retrying: {err}");
                } else {
                    tracing::debug!("still no audio output: {err}");
                }
                self.failures += 1;
                self.retry_at = now + self.backoff;
                self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
            }
        }
    }

    fn open_device(&mut self) -> Result<(Output, OutputInfo), crate::AudioError> {
        let rate = output::device_rate()?;
        if rate != self.rate {
            self.rebuild(rate);
        }
        output::open_cpal(self.mixer.clone(), self.health.clone(), self.rate)
    }

    /// A device at another rate: a new mixer, the sounds decoded again and the music restarted
    /// (only while no output is open, so nothing else holds the mixer).
    fn rebuild(&mut self, rate: u32) {
        tracing::info!(from = self.rate, to = rate, "output rate changed");
        self.rate = rate;
        let (mixer, link) = Mixer::new(rate);
        self.mixer = Arc::new(Mutex::new(mixer));
        self.link = link;
        self.send_gains();
        self.load_clips();
        let wanted = self
            .tracks
            .iter()
            .find(|t| t.wanted)
            .map(|t| t.music.clone());
        self.tracks.clear();
        if let Some(music) = wanted {
            self.start_track(music, self.frames(Duration::from_millis(300)));
        }
    }

    fn state(&self) -> AudioState {
        let ms = self.health.delay_us.load(Ordering::Relaxed) as f32 / 1000.0;
        AudioState {
            backend: match self.options.backend {
                Backend::Device => "device",
                Backend::Null => "null",
            },
            device: self.output.as_ref().map(|o| o.name().to_owned()),
            rate: self.rate,
            buffer: self.info.as_ref().and_then(|i| i.buffer),
            output_delay_ms: ms,
            callbacks: self.health.callbacks.load(Ordering::Relaxed),
            xruns: self.health.xruns.load(Ordering::Relaxed),
            underruns: self.health.underruns.load(Ordering::Relaxed),
            reopens: self.reopens,
            volumes: self.volumes,
            voices: self.health.voices.load(Ordering::Relaxed),
            music: self
                .tracks
                .iter()
                .map(|t| TrackState {
                    key: t.music.key.clone(),
                    path: t.music.file.path.clone(),
                    origin: t.music.file.origin.clone(),
                    wanted: t.wanted,
                })
                .collect(),
            current: self.current.clone(),
            sounds: self.sources.clone(),
        }
    }
}

fn load_clip(file: &SoundFile, rate: u32) -> Result<Clip, crate::AudioError> {
    // Generous: the longest file could be 192 kHz.
    let (samples, file_rate) = decode::decode_all(&file.path, MAX_SOUND_SECONDS * 192_000)?;
    let max = MAX_SOUND_SECONDS * file_rate as usize;
    let mut samples = samples;
    samples.truncate(max * 2);
    let samples = resample::resample_clip(samples, file_rate, rate)?;
    Ok(samples.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testwav::write_wav;
    use crate::{Audio, Music};

    #[test]
    fn a_failed_or_hung_output_is_reopened_and_the_music_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ambient.wav");
        let tone: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        write_wav(&path, 48_000, &tone);
        let mut engine = Engine::new(AudioOptions {
            backend: Backend::Null,
            stats: false,
        });
        engine.keep_output();
        assert!(engine.output.is_some());
        engine.set_music(
            Some(Music {
                key: "ambient".into(),
                file: SoundFile {
                    path,
                    origin: "test".into(),
                    loop_spec: None,
                },
                resume: true,
            }),
            Duration::from_millis(10),
        );

        // The stream reports a fatal error (the HDMI sink went away): closed, then reopened
        // after the back-off, keeping the mixer and its music.
        engine.health.failed.store(true, Ordering::Relaxed);
        engine.keep_output();
        assert!(engine.output.is_none());
        thread::sleep(FIRST_BACKOFF + Duration::from_millis(50));
        engine.keep_output();
        assert!(engine.output.is_some());
        assert_eq!(engine.reopens, 1);
        assert_eq!(engine.state().music.len(), 1);

        // A callback that stops coming (a hung device) counts as failed too.
        // (An output that renders but reports to another counter, so ours stands still.)
        engine.output =
            Some(output::open_null(engine.mixer.clone(), Arc::new(Health::default())).0);
        engine.last_callbacks = (
            engine.health.callbacks.load(Ordering::Relaxed),
            Instant::now() - STALL - Duration::from_millis(1),
        );
        engine.keep_output();
        assert!(engine.output.is_none());
        thread::sleep(FIRST_BACKOFF + Duration::from_millis(50));
        engine.keep_output();
        assert_eq!(engine.reopens, 2);
        let callbacks = engine.health.callbacks.load(Ordering::Relaxed);
        thread::sleep(Duration::from_millis(100));
        assert!(engine.health.callbacks.load(Ordering::Relaxed) > callbacks);
    }

    fn wait_for(audio: &Audio, what: impl Fn(&AudioState) -> bool) -> AudioState {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let state = audio.state(Duration::from_secs(1)).unwrap();
            if what(&state) || Instant::now() > deadline {
                return state;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn null_engine_plays_sounds_and_crossfades_music_with_resume() {
        let dir = tempfile::tempdir().unwrap();
        let tone: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let click = dir.path().join("move.wav");
        write_wav(&click, 48_000, &tone[..480]);
        let ambient = dir.path().join("ambient.wav");
        write_wav(&ambient, 44_100, &tone);
        let theme = dir.path().join("theme.wav");
        write_wav(&theme, 48_000, &tone);

        let audio = Audio::start(AudioOptions {
            backend: Backend::Null,
            stats: true,
        });
        let file = |path: &std::path::Path| SoundFile {
            path: path.to_path_buf(),
            origin: "test".into(),
            loop_spec: None,
        };
        audio.set_sounds(vec![
            ("move".into(), Some(file(&click))),
            ("back".into(), None),
        ]);
        audio.play("move", Some(Instant::now()));
        audio.play("back", None);
        let state = wait_for(&audio, |s| s.callbacks > 5);
        assert_eq!(state.backend, "null");
        assert_eq!(state.device.as_deref(), Some("null"));
        assert_eq!(state.sounds.len(), 2);

        let ambient_music = Music {
            key: "ambient".into(),
            file: file(&ambient),
            resume: true,
        };
        audio.set_music(Some(ambient_music.clone()), Duration::from_millis(50));
        let state = wait_for(&audio, |s| s.music.len() == 1);
        assert_eq!(state.current.as_deref(), Some("ambient"));

        let theme_music = Music {
            key: "theme:oot".into(),
            file: file(&theme),
            resume: false,
        };
        audio.set_music(Some(theme_music), Duration::from_millis(50));
        // The ambient is parked (kept), the theme plays.
        let state = wait_for(&audio, |s| s.music.len() == 2);
        assert_eq!(state.current.as_deref(), Some("theme:oot"));
        assert!(state.music.iter().any(|t| t.key == "ambient" && !t.wanted));

        // Back to the ambient: the theme goes away, the same ambient stream resumes.
        audio.set_music(Some(ambient_music), Duration::from_millis(50));
        let state = wait_for(&audio, |s| s.music.len() == 1);
        assert_eq!(state.music[0].key, "ambient");
        assert!(state.music[0].wanted);

        audio.set_music(None, Duration::from_millis(50));
        let state = wait_for(&audio, |s| s.music.iter().all(|t| !t.wanted));
        assert_eq!(state.current, None);
        assert_eq!(state.music.len(), 1, "the ambient stays parked");
    }
}
