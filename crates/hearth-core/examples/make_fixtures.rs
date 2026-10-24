//! Fills a folder with fixture "downloads" for the Customize page's file list: images of
//! each shape (animated ones too: a GIF, an APNG, and an animated WebP named `.png` like
//! SteamGridDB's) and themes with leading silence and fades.
//! `cargo run -p hearth-core --example make_fixtures -- [./.dev-data/downloads]`
//! Point Hearth at it with `HEARTH_DOWNLOADS` (scripts/test-home.sh does).

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};

use hearth_core::art::animation;

const RATE: u32 = 44_100;

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "./.dev-data/downloads".into()),
    );
    std::fs::create_dir_all(&dir).expect("creating the folder");

    // Heroes are ~3.1:1 like SteamGridDB's: red, green and blue thirds show the crop.
    image(
        &dir,
        "Hero wide bands.png",
        1920,
        620,
        |x, _, w, _| match x * 3 / w {
            0 => [200, 60, 50, 255],
            1 => [60, 170, 80, 255],
            _ => [50, 90, 200, 255],
        },
    );
    image(&dir, "Cover square.png", 600, 600, |x, y, w, h| {
        [(x * 255 / w) as u8, 120, (y * 255 / h) as u8, 255]
    });
    // A logo: a bright bar on transparency.
    image(&dir, "Logo transparent.png", 900, 300, |x, y, w, h| {
        let inside = x > w / 10 && x < w * 9 / 10 && y > h / 3 && y < h * 2 / 3;
        if inside {
            [250, 230, 200, 255]
        } else {
            [0, 0, 0, 0]
        }
    });
    image(&dir, "Icon.png", 256, 256, |x, y, _, _| {
        [(x as u8) / 2 + 100, 80, (y as u8) / 2 + 60, 255]
    });
    image(&dir, "Photo.jpg", 1280, 720, |x, y, w, h| {
        [(x * 200 / w) as u8, (y * 200 / h) as u8, 150, 255]
    });

    // Animated: told apart by content, whatever the name says.
    let bars = |w: u32, h: u32, n: u32| -> Vec<image::RgbaImage> {
        (0..n)
            .map(|i| {
                image::RgbaImage::from_fn(w, h, |x, y| {
                    let lit = ((x + w - w * i / n) * 7 / w).is_multiple_of(2)
                        && y > h / 3
                        && y < h * 2 / 3;
                    if lit {
                        image::Rgba([250, 230, 200, 255])
                    } else {
                        image::Rgba([0, 0, 0, 0])
                    }
                })
            })
            .collect()
    };
    let write = |name: &str, bytes: Vec<u8>| {
        std::fs::write(dir.join(name), bytes).expect("writing an animation");
    };
    write(
        "Logo animated.gif",
        animation::samples::gif_from(bars(900, 300, 12), 80),
    );
    write(
        "Hero animated.png",
        animation::samples::apng_from(
            &bars(1920, 620, 12)
                .into_iter()
                .map(|mut f| {
                    for p in f.pixels_mut() {
                        if p[3] == 0 {
                            *p = image::Rgba([40, 60, 90, 255]);
                        }
                    }
                    f
                })
                .collect::<Vec<_>>(),
            80,
        ),
    );
    write(
        "Logo-fakepng.png",
        include_bytes!("../tests/fixtures/animated.webp").to_vec(),
    );

    // 3 s of silence, then music: should start at once and loop without the silence.
    wav(&dir, "Theme leading silence.wav", 23.0, |t| {
        if t < 3.0 { 0.0 } else { 1.0 }
    });
    // A fade-in, 20 s, a 3 s fade-out, 1 s of silence: the loop must not dip.
    wav(&dir, "Theme with fades.wav", 25.0, |t| match t {
        t if t < 1.0 => t,
        t if t < 21.0 => 1.0,
        t if t < 24.0 => (24.0 - t) / 3.0,
        _ => 0.0,
    });
    wav(&dir, "Short jingle.wav", 4.0, |_| 1.0);
    println!("fixtures written to {}", dir.display());
}

fn image(dir: &Path, name: &str, w: u32, h: u32, pixel: impl Fn(u32, u32, u32, u32) -> [u8; 4]) {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(pixel(x, y, w, h)));
    let path = dir.join(name);
    if name.ends_with(".jpg") {
        image::DynamicImage::ImageRgba8(img)
            .to_rgb8()
            .save(&path)
            .expect("writing an image");
    } else {
        img.save(&path).expect("writing an image");
    }
}

/// A slow arpeggio (so the level is steady and loops are audible) under `envelope(t)`.
fn wav(dir: &Path, name: &str, secs: f32, envelope: impl Fn(f32) -> f32) {
    let notes = [220.0, 277.18, 329.63, 440.0];
    let samples: Vec<i16> = (0..(secs * RATE as f32) as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let note = notes[(t * 2.0) as usize % notes.len()];
            let s = (t * note * TAU).sin() * 0.5 + (t * 110.0 * TAU).sin() * 0.3;
            (s * 0.4 * envelope(t) * f32::from(i16::MAX)) as i16
        })
        .collect();
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(dir.join(name), out).expect("writing a wav");
}
