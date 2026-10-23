//! Sample-rate conversion to the output rate with rubato's synchronous FFT resampler (MIT): the
//! files are mostly 44.1 kHz, PipeWire runs at 48 kHz.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

use crate::AudioError;

/// Input frames per resampler call.
const CHUNK: usize = 1024;

fn resample_error(e: impl std::fmt::Display) -> AudioError {
    AudioError::Resample(e.to_string())
}

/// Resamples a whole stereo clip from `rate_in` to `rate_out`.
pub(crate) fn resample_clip(
    samples: Vec<f32>,
    rate_in: u32,
    rate_out: u32,
) -> Result<Vec<f32>, AudioError> {
    if rate_in == rate_out || samples.is_empty() {
        return Ok(samples);
    }
    let frames = samples.len() / 2;
    let mut resampler = Fft::<f32>::new(
        rate_in as usize,
        rate_out as usize,
        CHUNK,
        2,
        FixedSync::Input,
    )
    .map_err(resample_error)?;
    let input = InterleavedSlice::new(&samples, 2, frames).map_err(resample_error)?;
    let out = resampler
        .process_all(&input, frames, None)
        .map_err(resample_error)?;
    Ok(out.take_data())
}

/// Resamples a continuous stereo stream, chunk by chunk.
pub(crate) struct StreamResampler {
    inner: Option<Fft<f32>>,
    /// Input waiting for a full chunk.
    pending: Vec<f32>,
    out: Vec<f32>,
    /// Output frames still to drop (the resampler's start-up delay).
    skip: usize,
}

impl StreamResampler {
    /// A resampler from `rate_in` to `rate_out` (a pass-through when they are equal).
    pub(crate) fn new(rate_in: u32, rate_out: u32) -> Result<Self, AudioError> {
        let inner = if rate_in == rate_out {
            None
        } else {
            Some(
                Fft::<f32>::new(
                    rate_in as usize,
                    rate_out as usize,
                    CHUNK,
                    2,
                    FixedSync::Input,
                )
                .map_err(resample_error)?,
            )
        };
        let skip = inner.as_ref().map_or(0, |r| r.output_delay());
        let out_len = inner.as_ref().map_or(0, |r| r.output_frames_max() * 2);
        Ok(Self {
            inner,
            pending: Vec::with_capacity(CHUNK * 4),
            out: vec![0.0; out_len],
            skip,
        })
    }

    /// Feeds stereo `input` and appends whatever output is ready to `output`.
    pub(crate) fn push(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<(), AudioError> {
        let Some(resampler) = &mut self.inner else {
            output.extend_from_slice(input);
            return Ok(());
        };
        self.pending.extend_from_slice(input);
        let mut used = 0;
        loop {
            let need = resampler.input_frames_next();
            if (self.pending.len() - used) / 2 < need {
                break;
            }
            let chunk = &self.pending[used..used + need * 2];
            let input = InterleavedSlice::new(chunk, 2, need).map_err(resample_error)?;
            let max = self.out.len() / 2;
            let mut out =
                InterleavedSlice::new_mut(&mut self.out, 2, max).map_err(resample_error)?;
            let (consumed, produced) = resampler
                .process_into_buffer(&input, &mut out, None)
                .map_err(resample_error)?;
            used += consumed * 2;
            let drop = self.skip.min(produced);
            self.skip -= drop;
            output.extend_from_slice(&self.out[drop * 2..produced * 2]);
        }
        self.pending.drain(..used);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn clips_and_streams_come_out_at_the_new_rate() {
        let clip = resample_clip(sine(44_100, 44_100), 44_100, 48_000).unwrap();
        let frames = clip.len() / 2;
        assert!((47_900..=48_100).contains(&frames), "{frames}");

        let mut stream = StreamResampler::new(44_100, 48_000).unwrap();
        let input = sine(44_100, 44_100 * 2);
        let mut out = Vec::new();
        for chunk in input.chunks(2 * 700) {
            stream.push(chunk, &mut out).unwrap();
        }
        let frames = out.len() / 2;
        // Everything but the last partial chunk and the start-up delay.
        assert!((93_000..=96_000).contains(&frames), "{frames}");
        let peak = out.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((0.45..0.56).contains(&peak), "{peak}");
    }
}
