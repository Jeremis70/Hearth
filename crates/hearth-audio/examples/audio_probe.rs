//! Plays a file through the real output for a while and prints the engine's state, e.g.
//! `cargo run -p hearth-audio --example audio_probe -- music.mp3 20 30`
//! (file, seconds, music volume 0–100). Optional: `--sound <file>` plays it as a UI sound
//! every second, with latency stats.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use hearth_audio::{Audio, AudioOptions, Backend, Music, SoundFile, Volumes};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let sound = args.iter().position(|a| a == "--sound").map(|i| {
        let path = args.remove(i + 1);
        args.remove(i);
        PathBuf::from(path)
    });
    let file = args.first().map(PathBuf::from);
    let seconds: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let volume: u8 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(40);

    let audio = Audio::start(AudioOptions {
        backend: Backend::Device,
        stats: true,
    });
    audio.set_volumes(Volumes {
        ui: volume,
        music: volume,
        mute: false,
    });
    let source = |path: PathBuf| SoundFile {
        path,
        origin: "probe".into(),
        loop_spec: None,
        auto_loop: None,
        gain_db: 0,
    };
    if let Some(sound) = sound.clone() {
        audio.set_sounds(vec![("move".into(), Some(source(sound)))]);
    }
    if let Some(file) = file {
        audio.set_music(
            Some(Music {
                key: "probe".into(),
                file: source(file),
                resume: false,
            }),
            Duration::from_millis(300),
        );
    }
    for _ in 0..seconds {
        std::thread::sleep(Duration::from_secs(1));
        if sound.is_some() {
            audio.play("move", Some(Instant::now()));
        }
        if let Some(state) = audio.state(Duration::from_secs(1)) {
            println!(
                "device={:?} rate={} buffer={:?} delay={:.1}ms callbacks={} xruns={} underruns={} reopens={} music={}",
                state.device,
                state.rate,
                state.buffer,
                state.output_delay_ms,
                state.callbacks,
                state.xruns,
                state.underruns,
                state.reopens,
                state.music.len()
            );
        }
    }
    audio.set_music(None, Duration::from_millis(300));
    std::thread::sleep(Duration::from_millis(400));
}
