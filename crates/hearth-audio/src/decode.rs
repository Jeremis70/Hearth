//! Decoding with symphonia (MPL-2.0): ogg/vorbis, wav, flac and mp3, always to interleaved
//! stereo `f32` at the file's own rate. Mono is doubled; more than two channels keep the first
//! two (front left and right).

use std::fs::File;
use std::path::{Path, PathBuf};

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::AudioError;

/// Packets that fail to decode in a row before the file counts as broken.
const MAX_BAD_PACKETS: usize = 32;

/// An open file, decoded packet by packet.
pub(crate) struct Decoder {
    path: PathBuf,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    rate: u32,
    /// Scratch for one packet in the file's channel layout.
    scratch: Vec<f32>,
}

impl Decoder {
    /// Opens `path` and finds its first audio track.
    pub(crate) fn open(path: &Path) -> Result<Self, AudioError> {
        let err = |message: String| AudioError::Decode {
            path: path.to_path_buf(),
            message,
        };
        let file = File::open(path).map_err(|e| err(e.to_string()))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| err(e.to_string()))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| err("no audio track".into()))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| err("no audio codec parameters".into()))?;
        let rate = params
            .sample_rate
            .filter(|&r| r > 0)
            .ok_or_else(|| err("unknown sample rate".into()))?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default().gapless(true))
            .map_err(|e| err(e.to_string()))?;
        let track_id = track.id;
        Ok(Self {
            path: path.to_path_buf(),
            format,
            decoder,
            track_id,
            rate,
            scratch: Vec::new(),
        })
    }

    /// The file's sample rate.
    pub(crate) fn rate(&self) -> u32 {
        self.rate
    }

    /// Decodes the next packet and appends it to `out` as stereo frames. Returns the number of
    /// frames appended; 0 means the end of the file.
    pub(crate) fn next_into(&mut self, out: &mut Vec<f32>) -> Result<usize, AudioError> {
        let mut bad = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return Ok(0),
                // A broken tail or a chained stream: treat as the end.
                Err(SymphoniaError::ResetRequired) => return Ok(0),
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(0);
                }
                Err(e) => return Err(self.error(e)),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buf) => {
                    let frames = buf.frames();
                    if frames == 0 {
                        continue;
                    }
                    let channels = buf.spec().channels().count().max(1);
                    self.scratch.resize(buf.samples_interleaved(), 0.0);
                    buf.copy_to_slice_interleaved(&mut self.scratch);
                    out.reserve(frames * 2);
                    for frame in self.scratch.chunks_exact(channels) {
                        let left = frame[0];
                        let right = if channels > 1 { frame[1] } else { left };
                        out.push(left);
                        out.push(right);
                    }
                    return Ok(frames);
                }
                Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::IoError(_)) => {
                    bad += 1;
                    if bad >= MAX_BAD_PACKETS {
                        return Err(self.error(SymphoniaError::DecodeError("too many bad packets")));
                    }
                }
                Err(e) => return Err(self.error(e)),
            }
        }
    }

    fn error(&self, e: SymphoniaError) -> AudioError {
        AudioError::Decode {
            path: self.path.clone(),
            message: e.to_string(),
        }
    }
}

/// Decodes a whole file (a UI sound) to stereo at its own rate, at most `max_frames` long.
pub(crate) fn decode_all(path: &Path, max_frames: usize) -> Result<(Vec<f32>, u32), AudioError> {
    let mut decoder = Decoder::open(path)?;
    let mut out = Vec::new();
    while out.len() / 2 < max_frames && decoder.next_into(&mut out)? > 0 {}
    out.truncate(max_frames * 2);
    Ok((out, decoder.rate()))
}
