//! What Hearth decodes from an animated image, frame by frame (read-only).
//!
//! `cargo run --release -p hearth-core --example anim_probe -- <file>`: the format, then per
//! frame its delay, how much of it is at least half opaque and its mean alpha, so fades and
//! loop seams show.

use std::path::PathBuf;

use hearth_core::art::animation;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(std::env::args().nth(1).ok_or("usage: anim_probe <file>")?);
    println!("format: {:?}", animation::probe(&path));
    let started = std::time::Instant::now();
    for (i, frame) in animation::frames(&path)?.enumerate() {
        let frame = frame?;
        let delay = animation::delay_ms(frame.delay());
        let buffer = frame.into_buffer();
        let pixels = buffer.pixels().len().max(1);
        let opaque = buffer.pixels().filter(|p| p[3] >= 128).count();
        let alpha: u64 = buffer.pixels().map(|p| u64::from(p[3])).sum();
        println!(
            "{i:4}  {delay:4} ms  opaque {:5.1} %  mean alpha {:5.1}",
            opaque as f64 * 100.0 / pixels as f64,
            alpha as f64 / pixels as f64
        );
    }
    println!("decoded in {} ms", started.elapsed().as_millis());
    Ok(())
}
