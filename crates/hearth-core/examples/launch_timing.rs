//! Launches a ROM exactly the way Hearth does (RetroArch profile, ProcessRuntime, the gamescope
//! X11 rule) with `--max-frames`, and reports the frame rate. For diagnosing slow games.
//!
//! ```text
//! DISPLAY=:1 cargo run -p hearth-core --example launch_timing -- <profile-root> <core> <rom> [frames]
//! ```

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hearth_core::model::{Game, GameId, SystemId};
use hearth_core::profiles::RetroArchProfile;
use hearth_core::profiles::input::Translation;
use hearth_core::runtime::{LaunchCtx, ProcessRuntime, Runtime};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, core, rom, rest @ ..] = args.as_slice() else {
        return Err("usage: <profile-root> <core> <rom> [frames]".into());
    };
    let frames: u32 = rest.first().map_or(Ok(600), |f| f.parse())?;
    let profile = RetroArchProfile::new(root);
    // Input mapping doesn't matter for timing: keep whatever remap the last launch wrote.
    profile.prepare(&Translation::default())?;
    let mut command = profile.command(Path::new(core), Path::new(rom));
    command
        .args
        .insert(0, format!("--max-frames={frames}").into());
    if std::env::var_os("NO_FORCE_X11").is_none() {
        command.force_x11();
    }
    let (tx, rx) = mpsc::channel();
    let game = Game {
        id: GameId::new("timing"),
        title: "timing".into(),
        system: SystemId::new("n64"),
        path: rom.into(),
    };
    let started = Instant::now();
    let session = ProcessRuntime::new().launch(
        &game,
        &LaunchCtx {
            command,
            events: tx,
        },
    )?;
    println!("pgid {}", session.pgid());
    rx.recv_timeout(Duration::from_secs(600))?;
    let secs = started.elapsed().as_secs_f64();
    println!(
        "{frames} frames in {secs:.1}s -> {:.1} fps (includes startup)",
        f64::from(frames) / secs
    );
    Ok(())
}
