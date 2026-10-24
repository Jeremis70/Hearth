//! Shows where Hearth looks for a game's theme (read-only):
//! `cargo run -p hearth-core --example theme_probe -- <rom>...`

use std::path::Path;

fn main() {
    for rom in std::env::args().skip(1) {
        let rom = Path::new(&rom);
        println!("{}", rom.display());
        for path in hearth_core::themes::local_theme_candidates(rom) {
            let mark = if path.is_file() { "FOUND" } else { "-" };
            println!("  {mark:5} {}", path.display());
        }
    }
}
