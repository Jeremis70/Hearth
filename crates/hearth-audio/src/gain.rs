//! Volume steps to amplitude. Loudness is heard on a log scale, so a linear slider would do
//! nearly everything in its bottom fifth. Here 100 is 0 dB, every step of 10 is 5 dB less
//! (50 dB of range), and 0 is silence.

/// The quietest audible step, in dB below full volume.
const RANGE_DB: f32 = 50.0;

/// Amplitude for a 0–100 volume.
pub fn volume_to_gain(volume: u8) -> f32 {
    let volume = volume.min(100);
    if volume == 0 {
        return 0.0;
    }
    let db = -RANGE_DB * (1.0 - f32::from(volume) / 100.0);
    10f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_follows_decibels() {
        assert_eq!(volume_to_gain(0), 0.0);
        assert!((volume_to_gain(100) - 1.0).abs() < 1e-6);
        // 50 → −25 dB.
        assert!((volume_to_gain(50) - 0.056_234).abs() < 1e-4);
        // Each step of 10 is the same ratio (−5 dB).
        let ratio = volume_to_gain(70) / volume_to_gain(80);
        assert!((ratio - 0.562_34).abs() < 1e-3, "{ratio}");
        assert_eq!(volume_to_gain(200), 1.0);
    }
}
