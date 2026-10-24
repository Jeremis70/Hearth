//! Animated art: telling an animated image from a still one by its content, decoding its
//! frames, the display-sized frame cache (`.anim` files) and the still shown when animation
//! is off.
//!
//! Formats: animated WebP, APNG and GIF. The file extension is never trusted
//! (SteamGridDB serves animated WebP named `.png`, and downloads get renamed), so [`probe`]
//! reads the header.
//!
//! An `.anim` file holds one animation scaled for a display class, so the second play doesn't
//! decode the WebP/GIF again: `sized/<class>/{local-}<kind>.anim` next to the QOI copies
//! (`display.rs`). Layout, little endian:
//!
//! ```text
//! "HRTHANIM" width:u32 height:u32          header (16 bytes)
//! <frame 0 QOI> <frame 1 QOI> …             full frames, already composited
//! (delay_ms:u32 offset:u64 len:u32) × n     index
//! index_offset:u64 n:u32 still:u32 "HRTHEND\0"   trailer (24 bytes)
//! ```
//!
//! `still` is the frame shown when animation is off: the first for heroes, the most opaque for
//! logos (animated logos often fade in from nothing).

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use image::{AnimationDecoder, DynamicImage, ImageError, ImageFormat, RgbaImage};

use super::cache::ArtCache;
use super::display::{DisplayClass, fit};
use super::identity::ArtKey;
use super::provider::ArtKind;

const MAGIC: &[u8; 8] = b"HRTHANIM";
const END: &[u8; 8] = b"HRTHEND\0";
const HEADER_LEN: u64 = 16;
const TRAILER_LEN: u64 = 24;
const INDEX_ENTRY_LEN: usize = 16;
/// Frames at most in one animation (a longer one is cut there).
pub const MAX_FRAMES: usize = 1000;
/// Delays under this are treated as [`DEFAULT_DELAY_MS`], as browsers do: many GIFs say 0 or
/// 10 ms and mean "as fast as you like".
const MIN_DELAY_MS: u32 = 20;
const DEFAULT_DELAY_MS: u32 = 100;
/// How many frames a logo's still is chosen from.
const STILL_SCAN: usize = 300;

/// Animated image formats Hearth plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimFormat {
    /// Animated WebP.
    WebP,
    /// Animated PNG.
    Apng,
    /// GIF with more than one image.
    Gif,
}

impl AnimFormat {
    /// "webp", "apng", "gif".
    pub fn name(self) -> &'static str {
        match self {
            Self::WebP => "webp",
            Self::Apng => "apng",
            Self::Gif => "gif",
        }
    }

    /// The usual file extension for this format.
    pub fn extension(self) -> &'static str {
        match self {
            Self::WebP => "webp",
            Self::Apng => "png",
            Self::Gif => "gif",
        }
    }
}

/// The animated format of `bytes`, or `None` for a still image (or anything else).
pub fn probe_bytes(bytes: &[u8]) -> Option<AnimFormat> {
    if bytes.len() >= 21 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        // VP8X's flags byte: bit 1 is "animation".
        return (&bytes[12..16] == b"VP8X" && bytes[20] & 0x02 != 0).then_some(AnimFormat::WebP);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return apng_frames(bytes)
            .is_some_and(|n| n > 1)
            .then_some(AnimFormat::Apng);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return (gif_images(bytes) > 1).then_some(AnimFormat::Gif);
    }
    None
}

/// The animated format of the file at `path`, or `None` for a still image or an unreadable
/// file. Reads the first 64 KiB, or the whole file for a GIF (its frame count needs a walk).
pub fn probe(path: &Path) -> Option<AnimFormat> {
    let mut file = File::open(path).ok()?;
    let mut head = Vec::with_capacity(64 * 1024);
    (&mut file).take(64 * 1024).read_to_end(&mut head).ok()?;
    if head.starts_with(b"GIF8") {
        file.read_to_end(&mut head).ok()?;
    }
    probe_bytes(&head)
}

/// The frame count from an APNG's `acTL` chunk, if it comes before the image data.
fn apng_frames(bytes: &[u8]) -> Option<u32> {
    let mut at = 8;
    while at + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        let kind = &bytes[at + 4..at + 8];
        if kind == b"acTL" {
            let data = bytes.get(at + 8..at + 12)?;
            return Some(u32::from_be_bytes(data.try_into().ok()?));
        }
        if kind == b"IDAT" {
            return None;
        }
        at = at.checked_add(12 + len)?;
    }
    None
}

/// How many images a GIF holds, counting up to 2 (enough to know it moves).
fn gif_images(bytes: &[u8]) -> usize {
    // Header (6) + logical screen descriptor (7), then the global color table if flagged.
    let Some(&flags) = bytes.get(10) else {
        return 0;
    };
    let mut at = 13;
    if flags & 0x80 != 0 {
        at += 3 << ((flags & 0x07) + 1);
    }
    let skip_sub_blocks = |mut at: usize| -> Option<usize> {
        loop {
            let len = *bytes.get(at)? as usize;
            at += 1 + len;
            if len == 0 {
                return Some(at);
            }
        }
    };
    let mut images = 0;
    while let Some(&block) = bytes.get(at) {
        match block {
            0x21 => match skip_sub_blocks(at + 2) {
                Some(next) => at = next,
                None => break,
            },
            0x2C => {
                images += 1;
                if images > 1 {
                    break;
                }
                let Some(&local) = bytes.get(at + 9) else {
                    break;
                };
                at += 10;
                if local & 0x80 != 0 {
                    at += 3 << ((local & 0x07) + 1);
                }
                // LZW minimum code size, then the image data.
                match skip_sub_blocks(at + 1) {
                    Some(next) => at = next,
                    None => break,
                }
            }
            _ => break,
        }
    }
    images
}

/// The frames of an animated file, composited to full frames, in order.
pub fn frames(path: &Path) -> Result<image::Frames<'static>, ImageError> {
    let format = probe(path).ok_or_else(|| {
        ImageError::Unsupported(image::error::UnsupportedError::from_format_and_kind(
            image::error::ImageFormatHint::PathExtension(path.to_path_buf()),
            image::error::UnsupportedErrorKind::GenericFeature("not animated".into()),
        ))
    })?;
    let reader = BufReader::new(File::open(path)?);
    Ok(match format {
        AnimFormat::WebP => image::codecs::webp::WebPDecoder::new(reader)?.into_frames(),
        AnimFormat::Apng => image::codecs::png::PngDecoder::new(reader)?
            .apng()?
            .into_frames(),
        AnimFormat::Gif => image::codecs::gif::GifDecoder::new(reader)?.into_frames(),
    })
}

/// A frame's delay in milliseconds, with tiny delays replaced (see [`MIN_DELAY_MS`]).
pub fn delay_ms(delay: image::Delay) -> u32 {
    let (numer, denom) = delay.numer_denom_ms();
    let ms = numer / denom.max(1);
    if ms < MIN_DELAY_MS {
        DEFAULT_DELAY_MS
    } else {
        ms
    }
}

/// How many pixels of `frame` are at least half opaque.
fn opaque_pixels(frame: &RgbaImage) -> usize {
    frame.pixels().filter(|p| p[3] >= 128).count()
}

/// The still of an image for `kind`: the image itself when it isn't animated; else the first
/// frame for heroes and the most opaque of the first frames for everything else.
pub fn still(path: &Path, kind: ArtKind) -> Result<DynamicImage, ImageError> {
    if probe(path).is_none() {
        return image::ImageReader::open(path)?
            .with_guessed_format()?
            .decode();
    }
    let mut frames = frames(path)?;
    let first = frames.next().ok_or_else(|| {
        ImageError::IoError(io::Error::new(io::ErrorKind::InvalidData, "no frames"))
    })??;
    let mut best = first.into_buffer();
    if kind != ArtKind::Hero {
        let mut best_count = opaque_pixels(&best);
        for frame in frames.take(STILL_SCAN - 1) {
            let frame = frame?.into_buffer();
            let count = opaque_pixels(&frame);
            if count > best_count {
                best_count = count;
                best = frame;
            }
        }
    }
    Ok(DynamicImage::ImageRgba8(best))
}

/// What [`ArtCache::sized_anim`] found or made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SizedAnim {
    /// The `.anim` file.
    File(PathBuf),
    /// The animation needs more than the cap on disk: stream it from the source or show the
    /// still. Remembered (a `.toolarge` marker) until the source changes.
    TooLarge,
    /// The source isn't animated (or has one frame).
    Still,
}

/// One frame's place in an `.anim` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    delay_ms: u32,
    offset: u64,
    len: u32,
}

/// Reads frames from an `.anim` file.
#[derive(Debug)]
pub struct AnimReader {
    file: File,
    width: u32,
    height: u32,
    index: Vec<Entry>,
    still: usize,
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("bad .anim file: {what}"),
    )
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or_default())
}

impl AnimReader {
    /// Opens an `.anim` file and reads its index.
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let size = file.metadata()?.len();
        if size < HEADER_LEN + TRAILER_LEN {
            return Err(invalid("too short"));
        }
        let mut header = [0; HEADER_LEN as usize];
        file.read_exact(&mut header)?;
        if &header[0..8] != MAGIC {
            return Err(invalid("magic"));
        }
        let (width, height) = (u32_at(&header, 8), u32_at(&header, 12));
        let mut trailer = [0; TRAILER_LEN as usize];
        file.seek(SeekFrom::Start(size - TRAILER_LEN))?;
        file.read_exact(&mut trailer)?;
        if &trailer[16..24] != END {
            return Err(invalid("end"));
        }
        let index_at = u64_at(&trailer, 0);
        let count = u32_at(&trailer, 8) as usize;
        let still = u32_at(&trailer, 12) as usize;
        let index_len = count * INDEX_ENTRY_LEN;
        if count == 0 || count > MAX_FRAMES || index_at + index_len as u64 + TRAILER_LEN != size {
            return Err(invalid("index"));
        }
        let mut raw = vec![0; index_len];
        file.seek(SeekFrom::Start(index_at))?;
        file.read_exact(&mut raw)?;
        let index: Vec<Entry> = raw
            .as_chunks::<INDEX_ENTRY_LEN>()
            .0
            .iter()
            .map(|e| Entry {
                delay_ms: u32_at(e, 0),
                offset: u64_at(e, 4),
                len: u32_at(e, 12),
            })
            .collect();
        if index
            .iter()
            .any(|e| e.offset < HEADER_LEN || e.offset + u64::from(e.len) > index_at)
        {
            return Err(invalid("frame bounds"));
        }
        Ok(Self {
            file,
            width,
            height,
            index,
            still: still.min(count - 1),
        })
    }

    /// Frame width and height.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Number of frames.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Always false: an `.anim` file holds at least one frame.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Bytes all frames take decoded (RGBA).
    pub fn decoded_bytes(&self) -> u64 {
        self.frame_bytes() * self.index.len() as u64
    }

    /// Bytes one frame takes decoded (RGBA).
    pub fn frame_bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 4
    }

    /// The delay after frame `i`, in milliseconds.
    pub fn delay_ms(&self, i: usize) -> u32 {
        self.index.get(i).map_or(DEFAULT_DELAY_MS, |e| e.delay_ms)
    }

    /// The frame shown when animation is off.
    pub fn still_index(&self) -> usize {
        self.still
    }

    /// Decodes frame `i`.
    pub fn frame(&mut self, i: usize) -> Result<RgbaImage, ImageError> {
        let entry = *self
            .index
            .get(i)
            .ok_or_else(|| ImageError::IoError(invalid("frame index")))?;
        let mut bytes = vec![0; entry.len as usize];
        self.file.seek(SeekFrom::Start(entry.offset))?;
        self.file.read_exact(&mut bytes)?;
        let frame = image::load_from_memory_with_format(&bytes, ImageFormat::Qoi)?.into_rgba8();
        if frame.dimensions() != (self.width, self.height) {
            return Err(ImageError::IoError(invalid("frame size")));
        }
        Ok(frame)
    }
}

/// Writes the frames of `source`, scaled into `bounds`, as an `.anim` file at `target` (through
/// a `.part` file). Leaves no file when it would take more than `max_bytes` on disk, or when
/// the source has a single frame.
fn write_anim(
    source: &Path,
    kind: ArtKind,
    bounds: (u32, u32),
    target: &Path,
    max_bytes: u64,
) -> Result<WriteOutcome, ImageError> {
    let tmp = target.with_extension("anim.part");
    let result = write_anim_to(source, kind, bounds, &tmp, max_bytes);
    match &result {
        Ok(WriteOutcome::Written) => fs::rename(&tmp, target)?,
        _ => {
            let _ = fs::remove_file(&tmp);
        }
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteOutcome {
    Written,
    TooLarge,
    OneFrame,
}

fn write_anim_to(
    source: &Path,
    kind: ArtKind,
    bounds: (u32, u32),
    tmp: &Path,
    max_bytes: u64,
) -> Result<WriteOutcome, ImageError> {
    let mut out = BufWriter::new(File::create(tmp)?);
    let mut size = None;
    let mut index = Vec::new();
    let mut written = HEADER_LEN;
    let (mut still, mut still_count) = (0, 0);
    let mut qoi = Vec::new();
    for frame in frames(source)?.take(MAX_FRAMES) {
        let frame = frame?;
        let delay = delay_ms(frame.delay());
        let buffer = frame.into_buffer();
        let (w, h) = match size {
            Some(size) => size,
            None => {
                let (w, h) = fit(buffer.width(), buffer.height(), bounds);
                out.write_all(MAGIC)?;
                out.write_all(&w.to_le_bytes())?;
                out.write_all(&h.to_le_bytes())?;
                *size.insert((w, h))
            }
        };
        let buffer = if buffer.dimensions() == (w, h) {
            buffer
        } else {
            image::imageops::resize(&buffer, w, h, image::imageops::FilterType::Triangle)
        };
        if kind != ArtKind::Hero && index.len() < STILL_SCAN {
            let count = opaque_pixels(&buffer);
            if count > still_count {
                (still, still_count) = (index.len(), count);
            }
        }
        qoi.clear();
        DynamicImage::ImageRgba8(buffer)
            .write_to(&mut io::Cursor::new(&mut qoi), ImageFormat::Qoi)?;
        let len = u32::try_from(qoi.len()).map_err(|_| io::Error::other("frame too big"))?;
        let projected = written + u64::from(len) + (index.len() as u64 + 1) * 16 + TRAILER_LEN;
        if projected > max_bytes {
            return Ok(WriteOutcome::TooLarge);
        }
        out.write_all(&qoi)?;
        index.push(Entry {
            delay_ms: delay,
            offset: written,
            len,
        });
        written += u64::from(len);
    }
    if index.len() < 2 {
        return Ok(WriteOutcome::OneFrame);
    }
    for e in &index {
        out.write_all(&e.delay_ms.to_le_bytes())?;
        out.write_all(&e.offset.to_le_bytes())?;
        out.write_all(&e.len.to_le_bytes())?;
    }
    out.write_all(&written.to_le_bytes())?;
    out.write_all(&u32::try_from(index.len()).unwrap_or(0).to_le_bytes())?;
    out.write_all(&u32::try_from(still).unwrap_or(0).to_le_bytes())?;
    out.write_all(END)?;
    out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(WriteOutcome::Written)
}

impl ArtCache {
    /// Where the `.anim` file of `source` (the `kind` image of `key`) goes for `class`.
    pub fn anim_path(
        &self,
        key: &ArtKey,
        kind: ArtKind,
        source: &Path,
        class: DisplayClass,
    ) -> PathBuf {
        self.sized_path(key, kind, source, class)
            .with_extension("anim")
    }

    /// The `.anim` file of `source` for `class`, made first if it is missing or older than
    /// `source` (slow: call off the UI thread). Files over `max_bytes` aren't written.
    pub fn sized_anim(
        &self,
        key: &ArtKey,
        kind: ArtKind,
        source: &Path,
        class: DisplayClass,
        max_bytes: u64,
    ) -> Result<SizedAnim, ImageError> {
        let target = self.anim_path(key, kind, source, class);
        let too_large = target.with_extension("toolarge");
        let still = target.with_extension("still");
        if !super::cache::is_stale(&target, source) {
            return Ok(SizedAnim::File(target));
        }
        if !super::cache::is_stale(&too_large, source) {
            return Ok(SizedAnim::TooLarge);
        }
        if !super::cache::is_stale(&still, source) || probe(source).is_none() {
            return Ok(SizedAnim::Still);
        }
        let Some(class_dir) = target.parent() else {
            return Ok(SizedAnim::Still);
        };
        fs::create_dir_all(class_dir)?;
        self.remove_other_classes(key, class)?;
        let outcome = write_anim(source, kind, class.anim_bounds(kind), &target, max_bytes)?;
        Ok(match outcome {
            WriteOutcome::Written => SizedAnim::File(target),
            WriteOutcome::TooLarge => {
                fs::write(&too_large, b"")?;
                SizedAnim::TooLarge
            }
            WriteOutcome::OneFrame => {
                fs::write(&still, b"")?;
                SizedAnim::Still
            }
        })
    }
}

/// Plays frames straight from the source, scaled on the fly, looping forever: for animations
/// too large for the `.anim` cache. Sequential only.
pub struct FrameStream {
    source: PathBuf,
    bounds: (u32, u32),
    frames: image::Frames<'static>,
    played: usize,
}

impl std::fmt::Debug for FrameStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameStream")
            .field("source", &self.source)
            .field("played", &self.played)
            .finish_non_exhaustive()
    }
}

impl FrameStream {
    /// Opens `source`, frames scaled into `class`'s bounds for `kind`.
    pub fn open(source: &Path, kind: ArtKind, class: DisplayClass) -> Result<Self, ImageError> {
        Ok(Self {
            source: source.to_path_buf(),
            bounds: class.anim_bounds(kind),
            frames: frames(source)?,
            played: 0,
        })
    }

    /// The next frame and its delay; after the last, the first again.
    pub fn next_frame(&mut self) -> Result<(RgbaImage, u32), ImageError> {
        let frame = match self.frames.next() {
            Some(frame) => frame?,
            None if self.played == 0 => {
                return Err(ImageError::IoError(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "no frames",
                )));
            }
            None => {
                self.frames = frames(&self.source)?;
                self.played = 0;
                return self.next_frame();
            }
        };
        self.played += 1;
        let delay = delay_ms(frame.delay());
        let buffer = frame.into_buffer();
        let (w, h) = fit(buffer.width(), buffer.height(), self.bounds);
        let buffer = if buffer.dimensions() == (w, h) {
            buffer
        } else {
            image::imageops::resize(&buffer, w, h, image::imageops::FilterType::Triangle)
        };
        Ok((buffer, delay))
    }
}

/// Test and fixture helpers: small animations in each format.
pub mod samples {
    use std::io::Cursor;

    use image::{Delay, Frame, Rgba, RgbaImage};

    /// `n` frames of `w` × `h`: an opaque band growing from the left over transparency (each
    /// frame a little more opaque than the one before, the last fully), its color changing.
    pub fn frames(w: u32, h: u32, n: u32) -> Vec<RgbaImage> {
        (0..n)
            .map(|i| {
                let edge = w * (i + 1) / n.max(1);
                let shade = u8::try_from(40 + 200 * i / n.max(1)).unwrap_or(255);
                RgbaImage::from_fn(w, h, |x, _| {
                    if x < edge {
                        Rgba([shade, 255 - shade, 60, 255])
                    } else {
                        Rgba([0, 0, 0, 0])
                    }
                })
            })
            .collect()
    }

    /// A GIF of [`frames`], `delay_ms` per frame, looping.
    pub fn gif(w: u32, h: u32, n: u32, delay_ms: u32) -> Vec<u8> {
        gif_from(frames(w, h, n), delay_ms)
    }

    /// An APNG of [`frames`], `delay_ms` per frame, looping.
    pub fn apng(w: u32, h: u32, n: u32, delay_ms: u32) -> Vec<u8> {
        apng_from(&frames(w, h, n), delay_ms)
    }

    /// A looping GIF of `frames`, `delay_ms` each (empty if encoding fails).
    pub fn gif_from(frames: Vec<RgbaImage>, delay_ms: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(&mut bytes, 30);
            let delay = Delay::from_numer_denom_ms(delay_ms, 1);
            let encoded = encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .and_then(|()| {
                    encoder.encode_frames(
                        frames
                            .into_iter()
                            .map(|f| Frame::from_parts(f, 0, 0, delay)),
                    )
                });
            if encoded.is_err() {
                return Vec::new();
            }
        }
        bytes
    }

    /// A looping APNG of `frames` (all the same size), `delay_ms` each (empty if encoding
    /// fails).
    pub fn apng_from(frames: &[RgbaImage], delay_ms: u32) -> Vec<u8> {
        let Some(first) = frames.first() else {
            return Vec::new();
        };
        let mut bytes = Vec::new();
        let encoded = (|| -> Result<(), png::EncodingError> {
            let mut encoder =
                png::Encoder::new(Cursor::new(&mut bytes), first.width(), first.height());
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            encoder.set_animated(u32::try_from(frames.len()).unwrap_or(1), 0)?;
            encoder.set_frame_delay(u16::try_from(delay_ms).unwrap_or(100), 1000)?;
            let mut writer = encoder.write_header()?;
            for frame in frames {
                writer.write_image_data(frame.as_raw())?;
            }
            writer.finish()
        })();
        if encoded.is_err() {
            return Vec::new();
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SystemId;
    use crate::testdir::tempdir;

    const WEBP: &[u8] = include_bytes!("../../tests/fixtures/animated.webp");

    fn key() -> ArtKey {
        ArtKey::new(&SystemId::new("gc"), Some("GZ2E01"), "")
    }

    fn still_png(w: u32, h: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn formats_are_told_by_content() {
        assert_eq!(probe_bytes(WEBP), Some(AnimFormat::WebP));
        assert_eq!(
            probe_bytes(&samples::apng(8, 4, 3, 50)),
            Some(AnimFormat::Apng)
        );
        assert_eq!(
            probe_bytes(&samples::gif(8, 4, 3, 50)),
            Some(AnimFormat::Gif)
        );
        assert_eq!(probe_bytes(&samples::gif(8, 4, 1, 50)), None, "one image");
        assert_eq!(probe_bytes(&still_png(8, 4)), None);
        assert_eq!(probe_bytes(b"not an image"), None);

        let mut still_webp = Vec::new();
        RgbaImage::from_pixel(8, 4, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut io::Cursor::new(&mut still_webp), ImageFormat::WebP)
            .unwrap();
        assert_eq!(probe_bytes(&still_webp), None);
    }

    #[test]
    fn a_fake_png_is_found_by_its_content() {
        let dir = tempdir();
        let path = dir.path().join("logo-fakepng.png");
        fs::write(&path, WEBP).unwrap();
        assert_eq!(probe(&path), Some(AnimFormat::WebP));
        assert!(frames(&path).unwrap().count() > 1);
    }

    #[test]
    fn every_format_decodes_to_full_frames_with_delays() {
        let dir = tempdir();
        for (name, bytes) in [
            ("a.gif", samples::gif(16, 8, 4, 70)),
            ("a.png", samples::apng(16, 8, 4, 70)),
        ] {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            let frames: Vec<_> = frames(&path).unwrap().map(Result::unwrap).collect();
            assert_eq!(frames.len(), 4, "{name}");
            assert!(frames.iter().all(|f| f.buffer().dimensions() == (16, 8)));
            assert_eq!(delay_ms(frames[1].delay()), 70, "{name}");
        }
        let path = dir.path().join("a.webp");
        fs::write(&path, WEBP).unwrap();
        let frames: Vec<_> = frames(&path).unwrap().map(Result::unwrap).collect();
        assert!(frames.len() > 1);
        assert!(frames.iter().all(|f| f.buffer().dimensions() == (96, 32)));
    }

    #[test]
    fn tiny_delays_play_at_ten_frames_a_second() {
        assert_eq!(delay_ms(image::Delay::from_numer_denom_ms(0, 1)), 100);
        assert_eq!(delay_ms(image::Delay::from_numer_denom_ms(10, 1)), 100);
        assert_eq!(delay_ms(image::Delay::from_numer_denom_ms(20, 1)), 20);
        assert_eq!(delay_ms(image::Delay::from_numer_denom_ms(100, 3)), 33);
    }

    #[test]
    fn stills_are_the_first_frame_for_heroes_and_the_most_opaque_for_logos() {
        let dir = tempdir();
        let path = dir.path().join("a.gif");
        fs::write(&path, samples::gif(16, 8, 5, 50)).unwrap();
        let alpha = |image: DynamicImage| image.to_rgba8().get_pixel(15, 7)[3];
        let expected = samples::frames(16, 8, 5);
        assert_eq!(
            alpha(still(&path, ArtKind::Hero).unwrap()),
            expected[0].get_pixel(15, 7)[3]
        );
        assert_eq!(
            alpha(still(&path, ArtKind::Logo).unwrap()),
            expected[4].get_pixel(15, 7)[3]
        );
        let png = dir.path().join("still.png");
        fs::write(&png, still_png(4, 4)).unwrap();
        assert_eq!(still(&png, ArtKind::Logo).unwrap().width(), 4);
    }

    #[test]
    fn anim_files_hold_scaled_frames_and_are_reused() {
        let dir = tempdir();
        let cache = ArtCache::new(dir.path().join("art"));
        let source = cache
            .store(
                &key(),
                ArtKind::Logo,
                "png",
                &samples::apng(1600, 520, 6, 40),
            )
            .unwrap();
        let class = DisplayClass::default();
        let SizedAnim::File(path) = cache
            .sized_anim(&key(), ArtKind::Logo, &source, class, u64::MAX)
            .unwrap()
        else {
            panic!("expected a file");
        };
        assert!(path.ends_with("gc/GZ2E01/sized/1080p/logo.anim"));
        let mut reader = AnimReader::open(&path).unwrap();
        assert_eq!(reader.len(), 6);
        assert_eq!(reader.size(), (640, 208), "1x the logo box");
        assert_eq!(reader.delay_ms(3), 40);
        assert_eq!(reader.still_index(), 5);
        assert_eq!(reader.decoded_bytes(), 640 * 208 * 4 * 6);
        assert_eq!(reader.frame(2).unwrap().dimensions(), (640, 208));
        assert!(reader.frame(6).is_err());

        let made = fs::metadata(&path).unwrap().modified().unwrap();
        cache
            .sized_anim(&key(), ArtKind::Logo, &source, class, u64::MAX)
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), made);
    }

    #[test]
    fn animations_over_the_disk_cap_are_remembered_as_too_large() {
        let dir = tempdir();
        let cache = ArtCache::new(dir.path().join("art"));
        let source = cache
            .store(&key(), ArtKind::Hero, "gif", &samples::gif(400, 130, 8, 40))
            .unwrap();
        let class = DisplayClass::default();
        let result = cache.sized_anim(&key(), ArtKind::Hero, &source, class, 2_000);
        assert_eq!(result.unwrap(), SizedAnim::TooLarge);
        let anim = cache.anim_path(&key(), ArtKind::Hero, &source, class);
        assert!(!anim.exists());
        assert!(!anim.with_extension("anim.part").exists());
        // Remembered: no second decode.
        let marker = anim.with_extension("toolarge");
        let marked = fs::metadata(&marker).unwrap().modified().unwrap();
        let again = cache.sized_anim(&key(), ArtKind::Hero, &source, class, 2_000);
        assert_eq!(again.unwrap(), SizedAnim::TooLarge);
        assert_eq!(fs::metadata(&marker).unwrap().modified().unwrap(), marked);
    }

    #[test]
    fn stills_and_single_frames_get_no_anim_file() {
        let dir = tempdir();
        let cache = ArtCache::new(dir.path().join("art"));
        let class = DisplayClass::default();
        let png = cache
            .store(&key(), ArtKind::Hero, "png", &still_png(40, 10))
            .unwrap();
        let sized = cache.sized_anim(&key(), ArtKind::Hero, &png, class, u64::MAX);
        assert_eq!(sized.unwrap(), SizedAnim::Still);
    }

    #[test]
    fn broken_anim_files_are_rejected() {
        let dir = tempdir();
        let path = dir.path().join("x.anim");
        fs::write(&path, b"HRTHANIM\0\0\0\0").unwrap();
        assert!(AnimReader::open(&path).is_err());
        let mut bytes = MAGIC.to_vec();
        bytes.extend([0; 8 + 24]);
        fs::write(&path, &bytes).unwrap();
        assert!(AnimReader::open(&path).is_err());
    }

    #[test]
    fn streams_loop_forever() {
        let dir = tempdir();
        let path = dir.path().join("a.gif");
        fs::write(&path, samples::gif(1200, 900, 3, 50)).unwrap();
        let mut stream =
            FrameStream::open(&path, ArtKind::Hero, DisplayClass::for_height(720)).unwrap();
        for _ in 0..7 {
            let (frame, delay) = stream.next_frame().unwrap();
            assert_eq!(frame.dimensions(), (960, 720));
            assert_eq!(delay, 50);
        }
    }
}
