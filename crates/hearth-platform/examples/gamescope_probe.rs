//! Spike/debug tool for the gamescope integration. Run against gamescope's Xwayland:
//!
//! ```text
//! DISPLAY=:1 cargo run -p hearth-platform --example gamescope_probe -- props
//! DISPLAY=:1 cargo run -p hearth-platform --example gamescope_probe -- windows
//! DISPLAY=:1 cargo run -p hearth-platform --example gamescope_probe -- tag <pgid> <app-id>
//! DISPLAY=:1 cargo run -p hearth-platform --example gamescope_probe -- focus <app-id>...
//! ```

use hearth_platform::gamescope::{Compositor, Gamescope};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(gs) = Gamescope::connect()? else {
        return Err("DISPLAY is not a gamescope Xwayland server".into());
    };
    match args.first().map(String::as_str) {
        Some("props") => {
            for name in gs.root_property_names()? {
                let values = gs.root_cardinals(&name)?;
                println!("{name} = {values:?}");
            }
        }
        Some("windows") => {
            for w in gs.windows()? {
                println!(
                    "0x{:08x} pid={:?} STEAM_GAME={:?} overlay={}",
                    w.window, w.pid, w.app, w.overlay
                );
            }
        }
        Some("tag") => {
            let pgid = args.get(1).ok_or("tag <pgid> <app-id>")?.parse()?;
            let app = args.get(2).ok_or("tag <pgid> <app-id>")?.parse()?;
            println!("tagged {} window(s)", gs.assign_app(pgid, app)?);
        }
        Some("focus") => {
            let apps: Vec<u32> = args[1..]
                .iter()
                .map(|a| a.parse())
                .collect::<Result<_, _>>()?;
            gs.focus(&apps)?;
            std::thread::sleep(std::time::Duration::from_millis(300));
            println!("focused app now {:?}", gs.focused_app()?);
        }
        _ => return Err("usage: props | windows | tag <pgid> <app> | focus <app>...".into()),
    }
    Ok(())
}
