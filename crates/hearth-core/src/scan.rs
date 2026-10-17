//! Finds ROMs under a games root: `<root>/<system folder>/**/<file with a system extension>`.
//! Read-only: nothing under the root is created or modified.

use std::path::Path;

use walkdir::WalkDir;

use crate::model::{Game, GameId};
use crate::systems::Systems;
use crate::title::clean_title;

/// Scans `root` for games of every known system, sorted by title.
///
/// A missing root or system folder isn't an error: it just has no games.
pub fn scan_roms(root: &Path, systems: &Systems) -> Vec<Game> {
    let mut games = Vec::new();
    for system in systems.iter() {
        for folder in &system.folders {
            let dir = root.join(folder);
            if !dir.is_dir() {
                continue;
            }
            for entry in WalkDir::new(&dir).follow_links(true).sort_by_file_name() {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(err) => {
                        tracing::warn!("skipping unreadable entry under {}: {err}", dir.display());
                        continue;
                    }
                };
                let path = entry.path();
                if !entry.file_type().is_file() || !system.accepts(path) {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let relative = path.strip_prefix(&dir).unwrap_or(path);
                games.push(Game {
                    id: GameId::new(format!("roms:{}/{}", system.id, relative.display())),
                    title: clean_title(stem),
                    system: system.id.clone(),
                    path: path.to_path_buf(),
                });
            }
        }
    }
    games.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.id.cmp(&b.id)));
    games
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::model::SystemId;
    use crate::testdir::tempdir;

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"rom").unwrap();
    }

    #[test]
    fn finds_n64_roms_in_system_folder_only() {
        let dir = tempdir();
        let root = dir.path();
        touch(
            root,
            "n64/Legend of Zelda, The - Ocarina of Time (U) (V1.2) [!].z64",
        );
        touch(root, "n64/sub/Super Mario 64 (USA).N64");
        touch(root, "n64/readme.txt");
        touch(root, "n64/Banjo-Kazooie (USA).v64");
        touch(root, "snes/Super Metroid (USA).sfc");
        touch(root, "Mario Kart 64 (USA).z64"); // not in a system folder

        let games = scan_roms(root, &Systems::builtin().unwrap());
        let titles: Vec<_> = games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Banjo-Kazooie",
                "Super Mario 64",
                "The Legend of Zelda: Ocarina of Time"
            ]
        );
        assert!(games.iter().all(|g| g.system == SystemId::new("n64")));
        assert_eq!(
            games[1].id.as_str(),
            "roms:n64/sub/Super Mario 64 (USA).N64"
        );
        assert!(games[2].path.is_file());
    }

    #[test]
    fn shared_extensions_take_the_system_from_the_folder() {
        let dir = tempdir();
        let root = dir.path();
        touch(root, "gc/Pikmin (USA).iso");
        touch(root, "gamecube/Metroid Prime (USA).rvz");
        touch(root, "wii/Wii Sports (USA).iso");
        touch(root, "n64/Stray (USA).iso"); // N64 doesn't take .iso
        touch(root, "Loose (USA).iso"); // not in a system folder

        let games = scan_roms(root, &Systems::builtin().unwrap());
        let found: Vec<_> = games
            .iter()
            .map(|g| (g.title.as_str(), g.system.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("Metroid Prime", "gc"),
                ("Pikmin", "gc"),
                ("Wii Sports", "wii")
            ]
        );
    }

    #[test]
    fn missing_root_has_no_games() {
        let dir = tempdir();
        let games = scan_roms(&dir.path().join("nope"), &Systems::builtin().unwrap());
        assert!(games.is_empty());
    }
}
