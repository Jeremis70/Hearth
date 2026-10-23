//! Where the mix goes: the default output device through cpal (ALSA's `default`, which is
//! PipeWire through pipewire-alsa and follows its default sink), or nowhere (the null output,
//! for tests and headless runs, which still runs the mixer in real time so fades and state
//! behave the same).
//!
//! The mixer is shared with the callback behind a mutex that only the callback takes while a
//! stream is open (the control thread takes it only between streams), so it is never contended
//! and the callback never waits.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, ErrorKind, OutputCallbackInfo, SampleFormat, StreamConfig};

use crate::AudioError;
use crate::mixer::{Mixer, MixerCounts};

/// The rate Hearth asks for (PipeWire's default); a device that refuses it gets its own.
pub(crate) const PREFERRED_RATE: u32 = 48_000;
/// Frames per callback Hearth asks for: ~5 ms at 48 kHz. PipeWire may round it up to its
/// quantum; the stats line reports the delay cpal sees.
pub(crate) const PREFERRED_BUFFER: u32 = 256;

/// What the callback and the control thread share about an open stream.
#[derive(Debug, Default)]
pub(crate) struct Health {
    /// The stream reported an error it can't recover from.
    pub failed: AtomicBool,
    /// Callbacks so far (a stalled count means a hung device).
    pub callbacks: AtomicU64,
    pub xruns: AtomicU64,
    /// The last output delay cpal reported, in microseconds.
    pub delay_us: AtomicU64,
    /// Mixer counters after the last callback.
    pub voices: AtomicU64,
    pub underruns: AtomicU64,
}

impl Health {
    fn store(&self, counts: MixerCounts) {
        self.voices.store(counts.voices as u64, Ordering::Relaxed);
        self.underruns.store(counts.underruns, Ordering::Relaxed);
    }
}

/// An open output.
pub(crate) enum Output {
    Cpal {
        // Dropping it closes the device.
        _stream: cpal::Stream,
        name: String,
    },
    Null {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    },
}

/// Facts about an open output, for the state report.
#[derive(Debug, Clone)]
pub(crate) struct OutputInfo {
    pub name: String,
    pub rate: u32,
    pub channels: u16,
    pub buffer: Option<u32>,
}

impl Drop for Output {
    fn drop(&mut self) {
        if let Output::Null { stop, thread } = self {
            stop.store(true, Ordering::Relaxed);
            if let Some(thread) = thread.take() {
                let _ = thread.join();
            }
        }
    }
}

impl Output {
    pub(crate) fn name(&self) -> &str {
        match self {
            Output::Cpal { name, .. } => name,
            Output::Null { .. } => "null",
        }
    }
}

/// Runs the mixer at real-time pace with nowhere to send the sound.
pub(crate) fn open_null(mixer: Arc<Mutex<Mixer>>, health: Arc<Health>) -> (Output, OutputInfo) {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let rate = mixer.lock().map(|m| m.rate()).unwrap_or(PREFERRED_RATE);
    let frames = (rate / 100) as usize;
    let thread = thread::Builder::new()
        .name("hearth-audio-null".into())
        .spawn(move || {
            let mut buf = vec![0.0f32; frames * 2];
            while !flag.load(Ordering::Relaxed) {
                if let Ok(mut mixer) = mixer.lock() {
                    health.store(mixer.render(&mut buf, 2, Duration::ZERO));
                }
                health.callbacks.fetch_add(1, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(10));
            }
        })
        .ok();
    let info = OutputInfo {
        name: "null".into(),
        rate,
        channels: 2,
        buffer: Some(frames as u32),
    };
    (Output::Null { stop, thread }, info)
}

/// The rate the default device will run at, without opening it.
pub(crate) fn device_rate() -> Result<u32, AudioError> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| AudioError::Device("no output device".into()))?;
    let supports_preferred = device
        .supported_output_configs()
        .map(|mut configs| {
            configs.any(|c| {
                c.min_sample_rate() <= PREFERRED_RATE && PREFERRED_RATE <= c.max_sample_rate()
            })
        })
        .unwrap_or(false);
    if supports_preferred {
        return Ok(PREFERRED_RATE);
    }
    device
        .default_output_config()
        .map(|c| c.sample_rate())
        .map_err(|e| AudioError::Device(e.to_string()))
}

/// Opens the default output device at `rate`, rendering from `mixer`.
pub(crate) fn open_cpal(
    mixer: Arc<Mutex<Mixer>>,
    health: Arc<Health>,
    rate: u32,
) -> Result<(Output, OutputInfo), AudioError> {
    let device_err = |e: &dyn std::fmt::Display| AudioError::Device(e.to_string());
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| AudioError::Device("no output device".into()))?;
    let name = device
        .id()
        .map(|id| id.to_string())
        .unwrap_or_else(|_| "default".into());
    let default = device.default_output_config().map_err(|e| device_err(&e))?;
    let format = match default.sample_format() {
        SampleFormat::I16 => SampleFormat::I16,
        _ => SampleFormat::F32,
    };
    let channels = default.channels().max(1);
    let mut last_err = None;
    for buffer in [BufferSize::Fixed(PREFERRED_BUFFER), BufferSize::Default] {
        let config = StreamConfig {
            channels,
            sample_rate: rate,
            buffer_size: buffer,
        };
        match build(&device, config, format, mixer.clone(), health.clone()) {
            Ok(stream) => {
                stream.play().map_err(|e| device_err(&e))?;
                let info = OutputInfo {
                    name: name.clone(),
                    rate,
                    channels,
                    buffer: match buffer {
                        BufferSize::Fixed(n) => Some(n),
                        BufferSize::Default => None,
                    },
                };
                return Ok((
                    Output::Cpal {
                        _stream: stream,
                        name,
                    },
                    info,
                ));
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| AudioError::Device("no usable configuration".into())))
}

fn build(
    device: &cpal::Device,
    config: StreamConfig,
    format: SampleFormat,
    mixer: Arc<Mutex<Mixer>>,
    health: Arc<Health>,
) -> Result<cpal::Stream, AudioError> {
    let channels = usize::from(config.channels);
    let on_error = {
        let health = health.clone();
        move |err: cpal::Error| match err.kind() {
            ErrorKind::Xrun => {
                health.xruns.fetch_add(1, Ordering::Relaxed);
            }
            ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {
                tracing::info!("audio output: {err}");
            }
            _ => {
                tracing::warn!("audio output failed: {err}");
                health.failed.store(true, Ordering::Relaxed);
            }
        }
    };
    let stream = match format {
        SampleFormat::I16 => {
            let mut scratch: Vec<f32> = Vec::with_capacity(8192 * channels);
            device.build_output_stream(
                config,
                move |data: &mut [i16], info: &OutputCallbackInfo| {
                    // Grows only if the device hands a bigger buffer than any before.
                    scratch.resize(data.len(), 0.0);
                    render(&mixer, &health, &mut scratch, channels, info);
                    for (out, v) in data.iter_mut().zip(&scratch) {
                        *out = (v * f32::from(i16::MAX)) as i16;
                    }
                },
                on_error,
                None,
            )
        }
        _ => device.build_output_stream(
            config,
            move |data: &mut [f32], info: &OutputCallbackInfo| {
                render(&mixer, &health, data, channels, info);
            },
            on_error,
            None,
        ),
    };
    stream.map_err(|e| AudioError::Device(e.to_string()))
}

fn render(
    mixer: &Mutex<Mixer>,
    health: &Health,
    data: &mut [f32],
    channels: usize,
    info: &OutputCallbackInfo,
) {
    let ts = info.timestamp();
    let delay = ts
        .playback
        .checked_duration_since(ts.callback)
        .unwrap_or_default();
    health
        .delay_us
        .store(delay.as_micros() as u64, Ordering::Relaxed);
    health.callbacks.fetch_add(1, Ordering::Relaxed);
    match mixer.try_lock() {
        Ok(mut mixer) => health.store(mixer.render(data, channels, delay)),
        Err(_) => data.fill(0.0),
    }
}
