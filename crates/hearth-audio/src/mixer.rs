//! The mixer that runs inside the output callback. It never allocates, never blocks and never
//! frees memory: commands arrive over a lock-free queue, and every clip or ring it lets go of is
//! sent back to the control thread to be dropped there.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer, RingBuffer};

/// Voices (UI sounds) mixed at once; a new one beyond this replaces the oldest.
const MAX_VOICES: usize = 24;
/// Music streams at once (playing, fading or parked).
const MAX_MUSIC: usize = 6;
/// A cut voice fades out over this long, so cutting never clicks.
pub(crate) const CUT_FADE: Duration = Duration::from_millis(6);
/// A new music track waits until this much is decoded before it starts.
const MUSIC_PRIME_MS: usize = 50;
/// Volume changes glide over this long instead of jumping (no zipper noise).
const GAIN_GLIDE: Duration = Duration::from_millis(30);

/// A decoded UI sound: interleaved stereo at the mixer's rate.
pub(crate) type Clip = Arc<[f32]>;

/// Identifies a play request, for latency stats.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Stamp {
    /// Index into the engine's sound names.
    pub sound: usize,
    /// When the input that caused it happened, if known.
    pub input_at: Option<Instant>,
}

/// What to do once a music stream has faded to silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtSilence {
    /// Keep going (a fade to a non-zero level).
    Nothing,
    /// Stop reading it; it resumes where it was.
    Park,
    /// Give it back to the control thread.
    Remove,
}

pub(crate) enum Command {
    Voice {
        clip: Clip,
        gain: f32,
        /// A new voice of the same group cuts the previous one (with a short fade).
        group: Option<u8>,
        stamp: Stamp,
    },
    Music {
        id: u64,
        ring: Consumer<f32>,
    },
    Fade {
        id: u64,
        target: f32,
        frames: u32,
        then: AtSilence,
    },
    Gains {
        ui: f32,
        music: f32,
    },
}

pub(crate) enum Event {
    /// Memory the callback let go of, to be dropped outside it (never read, only dropped).
    #[allow(dead_code)]
    Clip(Clip),
    #[allow(dead_code)]
    Ring(Consumer<f32>),
    VoiceStarted {
        stamp: Stamp,
        mixed_at: Instant,
        output_delay: Duration,
    },
    /// A music stream reached silence and was parked or removed.
    Silent { id: u64, removed: bool },
    /// A music stream ran dry and its decoder is gone (end of a broken file).
    MusicEnded { id: u64 },
}

struct Voice {
    clip: Clip,
    pos: usize,
    gain: f32,
    group: Option<u8>,
    /// Per-frame gain decrement once cut (0: not cut).
    cut_step: f32,
    cut_gain: f32,
    stamp: Option<Stamp>,
}

struct Music {
    id: u64,
    ring: Consumer<f32>,
    gain: f32,
    target: f32,
    step: f32,
    then: AtSilence,
    parked: bool,
    /// Its decoder has filled enough of the ring to start (no silent first samples).
    primed: bool,
}

/// A smoothly changing gain.
#[derive(Clone, Copy)]
struct Glide {
    value: f32,
    target: f32,
    step: f32,
}

impl Glide {
    fn new(value: f32) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
        }
    }

    fn set(&mut self, target: f32, frames: u32) {
        self.target = target;
        self.step = (target - self.value).abs() / frames.max(1) as f32;
    }

    fn next(&mut self) -> f32 {
        if self.value < self.target {
            self.value = (self.value + self.step).min(self.target);
        } else if self.value > self.target {
            self.value = (self.value - self.step).max(self.target);
        }
        self.value
    }
}

/// Counters the control thread reads for the state report.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct MixerCounts {
    pub voices: usize,
    pub underruns: u64,
}

pub(crate) struct Mixer {
    rate: u32,
    commands: Consumer<Command>,
    events: Producer<Event>,
    voices: Vec<Voice>,
    music: Vec<Music>,
    ui: Glide,
    music_gain: Glide,
    underruns: u64,
    /// Samples a new music ring must hold before it plays.
    prime: usize,
}

/// The control thread's ends of the mixer's queues.
pub(crate) struct MixerLink {
    pub commands: Producer<Command>,
    pub events: Consumer<Event>,
}

impl Mixer {
    pub(crate) fn new(rate: u32) -> (Self, MixerLink) {
        let (command_tx, command_rx) = RingBuffer::new(256);
        let (event_tx, event_rx) = RingBuffer::new(1024);
        let mixer = Self {
            rate,
            commands: command_rx,
            events: event_tx,
            voices: Vec::with_capacity(MAX_VOICES),
            music: Vec::with_capacity(MAX_MUSIC),
            ui: Glide::new(0.0),
            music_gain: Glide::new(0.0),
            underruns: 0,
            prime: (rate as usize / 1000 * MUSIC_PRIME_MS) * 2,
        };
        let link = MixerLink {
            commands: command_tx,
            events: event_rx,
        };
        (mixer, link)
    }

    pub(crate) fn rate(&self) -> u32 {
        self.rate
    }

    pub(crate) fn frames(&self, duration: Duration) -> u32 {
        (duration.as_secs_f64() * f64::from(self.rate)).round() as u32
    }

    fn send(&mut self, event: Event) {
        // The queue is far larger than a callback's worth of events; if it ever fills up, the
        // event is dropped here (and with it, rarely, a free in this thread).
        let _ = self.events.push(event);
    }

    fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Voice {
                    clip,
                    gain,
                    group,
                    stamp,
                } => {
                    let cut_step = 1.0 / self.frames(CUT_FADE).max(1) as f32;
                    if let Some(group) = group {
                        for voice in &mut self.voices {
                            if voice.group == Some(group) && voice.cut_step == 0.0 {
                                voice.cut_step = cut_step;
                            }
                        }
                    }
                    if self.voices.len() == MAX_VOICES {
                        let old = self.voices.remove(0);
                        self.send(Event::Clip(old.clip));
                    }
                    self.voices.push(Voice {
                        clip,
                        pos: 0,
                        gain,
                        group,
                        cut_step: 0.0,
                        cut_gain: 1.0,
                        stamp: Some(stamp),
                    });
                }
                Command::Music { id, ring } => {
                    if self.music.len() == MAX_MUSIC {
                        // The engine never asks for this many; refuse rather than allocate.
                        self.send(Event::Ring(ring));
                        self.send(Event::Silent { id, removed: true });
                        continue;
                    }
                    self.music.push(Music {
                        id,
                        ring,
                        gain: 0.0,
                        target: 0.0,
                        step: 0.0,
                        then: AtSilence::Nothing,
                        parked: true,
                        primed: false,
                    });
                }
                Command::Fade {
                    id,
                    target,
                    frames,
                    then,
                } => {
                    if let Some(music) = self.music.iter_mut().find(|m| m.id == id) {
                        music.target = target;
                        music.step = (target - music.gain).abs() / frames.max(1) as f32;
                        music.then = then;
                        if target > 0.0 {
                            music.parked = false;
                        }
                    }
                }
                Command::Gains { ui, music } => {
                    let frames = self.frames(GAIN_GLIDE);
                    self.ui.set(ui, frames);
                    self.music_gain.set(music, frames);
                }
            }
        }
    }

    /// Fills `out` (interleaved, `channels` per frame) with the mix. `output_delay` is how long
    /// until this buffer is heard, for latency stats.
    pub(crate) fn render(
        &mut self,
        out: &mut [f32],
        channels: usize,
        output_delay: Duration,
    ) -> MixerCounts {
        self.apply_commands();
        let now = Instant::now();
        for voice in &mut self.voices {
            if let Some(stamp) = voice.stamp.take() {
                let _ = self.events.push(Event::VoiceStarted {
                    stamp,
                    mixed_at: now,
                    output_delay,
                });
            }
        }
        for music in &mut self.music {
            if !music.primed {
                music.primed = music.ring.slots() >= self.prime || music.ring.is_abandoned();
            }
        }
        let channels = channels.max(1);
        for frame in out.chunks_exact_mut(channels) {
            let ui = self.ui.next();
            let music_gain = self.music_gain.next();
            let (mut left, mut right) = (0.0f32, 0.0f32);
            for voice in &mut self.voices {
                if voice.pos + 1 >= voice.clip.len() {
                    continue;
                }
                let mut gain = voice.gain * ui;
                if voice.cut_step > 0.0 {
                    voice.cut_gain = (voice.cut_gain - voice.cut_step).max(0.0);
                    gain *= voice.cut_gain;
                }
                left += voice.clip[voice.pos] * gain;
                right += voice.clip[voice.pos + 1] * gain;
                voice.pos += 2;
            }
            for music in &mut self.music {
                if music.parked || !music.primed {
                    continue;
                }
                if music.gain < music.target {
                    music.gain = (music.gain + music.step).min(music.target);
                } else if music.gain > music.target {
                    music.gain = (music.gain - music.step).max(music.target);
                }
                match (music.ring.pop(), music.ring.pop()) {
                    (Ok(l), Ok(r)) => {
                        let gain = music.gain * music_gain;
                        left += l * gain;
                        right += r * gain;
                    }
                    _ => self.underruns += 1,
                }
                if music.gain == 0.0 && music.target == 0.0 && music.then != AtSilence::Nothing {
                    music.parked = true;
                }
            }
            let (left, right) = (left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0));
            frame[0] = left;
            if channels > 1 {
                frame[1] = right;
                for sample in &mut frame[2..] {
                    *sample = 0.0;
                }
            }
        }
        self.retire();
        MixerCounts {
            voices: self.voices.len(),
            underruns: self.underruns,
        }
    }

    /// Hands finished voices and silent or dead music back to the control thread.
    fn retire(&mut self) {
        let mut i = 0;
        while i < self.voices.len() {
            let voice = &self.voices[i];
            let done = voice.pos + 1 >= voice.clip.len()
                || (voice.cut_step > 0.0 && voice.cut_gain == 0.0);
            if done {
                let voice = self.voices.swap_remove(i);
                self.send(Event::Clip(voice.clip));
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i < self.music.len() {
            let music = &mut self.music[i];
            let id = music.id;
            if music.parked && music.then != AtSilence::Nothing {
                let remove = music.then == AtSilence::Remove;
                // Report once.
                music.then = AtSilence::Nothing;
                if remove {
                    let music = self.music.swap_remove(i);
                    self.send(Event::Ring(music.ring));
                    self.send(Event::Silent { id, removed: true });
                    continue;
                }
                self.send(Event::Silent { id, removed: false });
            } else if music.ring.is_abandoned() && music.ring.is_empty() {
                let music = self.music.swap_remove(i);
                self.send(Event::Ring(music.ring));
                self.send(Event::MusicEnded { id });
                continue;
            }
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(frames: usize, value: f32) -> Clip {
        vec![value; frames * 2].into()
    }

    fn drain(link: &mut MixerLink) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(e) = link.events.pop() {
            events.push(e);
        }
        events
    }

    #[test]
    fn a_cut_voice_fades_instead_of_stopping_dead() {
        let (mut mixer, mut link) = Mixer::new(48_000);
        let _ = link.commands.push(Command::Gains {
            ui: 1.0,
            music: 1.0,
        });
        let stamp = Stamp {
            sound: 0,
            input_at: None,
        };
        let mut out = vec![0.0; 2 * 4800];
        // Let the UI gain glide up first.
        mixer.render(&mut out, 2, Duration::ZERO);
        let _ = link.commands.push(Command::Voice {
            clip: clip(48_000, 0.5),
            gain: 1.0,
            group: Some(1),
            stamp,
        });
        mixer.render(&mut out[..2 * 480], 2, Duration::ZERO);
        assert!((out[0] - 0.5).abs() < 1e-6);
        let _ = link.commands.push(Command::Voice {
            clip: clip(48_000, 0.0),
            gain: 1.0,
            group: Some(1),
            stamp,
        });
        mixer.render(&mut out, 2, Duration::ZERO);
        // The old voice ramps down over ~6 ms (288 frames) instead of jumping to 0.
        let steps: Vec<f32> = out.iter().step_by(2).take(300).copied().collect();
        assert!(steps[0] > 0.49 && steps[0] < 0.5, "{}", steps[0]);
        assert!(steps.windows(2).all(|w| w[1] <= w[0] + 1e-6));
        assert!(steps[150] > 0.1 && steps[150] < 0.4, "{}", steps[150]);
        assert_eq!(steps[299], 0.0);
        let events = drain(&mut link);
        assert!(events.iter().any(|e| matches!(e, Event::Clip(_))));
        assert_eq!(mixer.render(&mut out, 2, Duration::ZERO).voices, 1);
    }

    #[test]
    fn music_fades_parks_and_resumes_where_it_was() {
        let (mut mixer, mut link) = Mixer::new(1000);
        let (mut tx, rx) = RingBuffer::new(4000);
        for i in 0..2000 {
            let _ = tx.push(i as f32 / 10_000.0);
        }
        let _ = link.commands.push(Command::Gains {
            ui: 1.0,
            music: 1.0,
        });
        let _ = link.commands.push(Command::Music { id: 7, ring: rx });
        let _ = link.commands.push(Command::Fade {
            id: 7,
            target: 1.0,
            frames: 10,
            then: AtSilence::Nothing,
        });
        let mut out = vec![0.0; 2 * 100];
        mixer.render(&mut out, 2, Duration::ZERO);
        let _ = link.commands.push(Command::Fade {
            id: 7,
            target: 0.0,
            frames: 10,
            then: AtSilence::Park,
        });
        mixer.render(&mut out, 2, Duration::ZERO);
        let events = drain(&mut link);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::Silent {
                id: 7,
                removed: false
            }
        )));
        // Parked: the ring isn't read any more.
        let before = tx.slots();
        mixer.render(&mut out, 2, Duration::ZERO);
        assert_eq!(tx.slots(), before);
        assert!(out.iter().all(|&v| v == 0.0));
        // Resumed: it continues from the next unread sample.
        let _ = link.commands.push(Command::Fade {
            id: 7,
            target: 1.0,
            frames: 1,
            then: AtSilence::Nothing,
        });
        mixer.render(&mut out[..2], 2, Duration::ZERO);
        assert!(out[0] > 0.0);
        let _ = link.commands.push(Command::Fade {
            id: 7,
            target: 0.0,
            frames: 1,
            then: AtSilence::Remove,
        });
        mixer.render(&mut out, 2, Duration::ZERO);
        let events = drain(&mut link);
        assert!(events.iter().any(|e| matches!(e, Event::Ring(_))));
        assert!(events.iter().any(|e| matches!(
            e,
            Event::Silent {
                id: 7,
                removed: true
            }
        )));
    }
}
