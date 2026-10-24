//! Renders the seconds around a track's loop seam to a WAV and prints levels, to check loop
//! points without listening to the whole track:
//! `cargo run --release -p hearth-audio --example loop_seam -- <file> <start> <end> <crossfade_ms> <out.wav>`
//! (start/end in seconds; `-` for none), or with the points a theme gets automatically:
//! `… loop_seam -- <file> auto <out.wav>`.

use std::path::Path;

use hearth_core::packs::LoopSpec;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (file, spec, out) = match &args[..] {
        [file, auto, out] if auto == "auto" => {
            let analysis = hearth_audio::analyze::analyze(Path::new(file)).expect("analysing");
            println!("analysis (ms): {analysis:?}");
            (file, analysis.theme_loop(0), out)
        }
        [file, start, end, crossfade, out] => {
            let secs = |s: &str| s.parse::<f64>().ok().map(|v| (v * 1000.0) as u64);
            let spec = LoopSpec {
                start_ms: secs(start).unwrap_or(0),
                end_ms: secs(end),
                crossfade_ms: crossfade.parse().unwrap_or(0),
                intro_ms: 0,
            };
            (file, spec, out)
        }
        _ => {
            eprintln!(
                "usage: loop_seam <file> <start> <end> <crossfade_ms> <out.wav> | <file> auto <out.wav>"
            );
            std::process::exit(2);
        }
    };
    let (samples, rate, seam) =
        hearth_audio::loop_seam(Path::new(file), Some(spec), 5.0).expect("decoding");
    let frames = samples.len() / 2;
    // 100 ms RMS around the seam, and the largest sample step.
    let window = rate as usize / 10;
    let db = |a: usize, b: usize| {
        let s: f64 = samples[a * 2..b * 2]
            .iter()
            .map(|v| f64::from(*v).powi(2))
            .sum();
        10.0 * (s / ((b - a) * 2) as f64).max(1e-12).log10()
    };
    let levels: Vec<String> = (0..frames / window)
        .map(|i| format!("{:.0}", db(i * window, (i + 1) * window)))
        .collect();
    println!(
        "seam (crossfade start) at {:.2} s of {:.2} s",
        seam as f64 / f64::from(rate),
        frames as f64 / f64::from(rate)
    );
    println!("100 ms levels (dB): {}", levels.join(" "));
    let steps: Vec<f32> = samples
        .chunks(2)
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| (w[1][0] - w[0][0]).abs())
        .collect();
    let max = steps.iter().fold(0f32, |m, &v| m.max(v));
    let mut sorted = steps.clone();
    sorted.sort_by(f32::total_cmp);
    println!(
        "largest step {max:.4}, p99.9 {:.4}",
        sorted[sorted.len() * 999 / 1000]
    );
    // 16-bit stereo WAV.
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for v in &samples {
        wav.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(out, wav).expect("writing the WAV");
}
