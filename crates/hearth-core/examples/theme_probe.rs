//! Shows where Hearth looks for a game's extras (read-only): scans a games root like Hearth
//! does and prints, for each game matching the filters, its own folder and the files tried.
//! `cargo run -p hearth-core --example theme_probe -- ~/Games [title filter]`

use std::path::PathBuf;

use hearth_core::art::ArtKind;
use hearth_core::extras::{Extra, folder_candidates};
use hearth_core::systems::Systems;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().unwrap_or_else(|| "./.dev-data/games".into()));
    let filter = args.next().unwrap_or_default().to_lowercase();
    let systems = Systems::builtin().expect("built-in systems");
    for game in hearth_core::scan::scan_roms(&root, &systems) {
        if !game.title.to_lowercase().contains(&filter) {
            continue;
        }
        println!("{} ({})", game.title, game.path.display());
        let Some(folder) = &game.folder else {
            println!("  no game folder (flat or in a grouping folder): no extras");
            continue;
        };
        println!("  game folder: {}", folder.display());
        let extras = [Extra::Theme]
            .into_iter()
            .chain(ArtKind::ALL.map(Extra::Art));
        for extra in extras {
            for path in folder_candidates(&game, extra) {
                let mark = if path.is_file() { "FOUND" } else { "-" };
                println!("  {mark:5} {}", path.display());
            }
        }
    }
}
