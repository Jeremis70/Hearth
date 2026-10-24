//! Finds ROMs under a games root: `<root>/<system folder>/**/<file with a system extension>`.
//! Read-only: nothing under the root is created or modified.
//!
//! A folder below the system folder whose whole contents are one game is that game's own
//! folder ([`Game::folder`]): its extras live there (`crate::extras`). A folder holding
//! several games is only a grouping folder. (Every system today has one file per game; when
//! `.m3u` playlists or folder-based games arrive, they count as one game here.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::model::{Game, GameId};
use crate::systems::Systems;
use crate::title::clean_title;

/// Whether `root` can be scanned at all (exists and is a readable folder). A missing or
/// unmounted root must not be mistaken for an empty one: that would drop every game.
pub fn root_is_available(root: &Path) -> bool {
    std::fs::read_dir(root).is_ok()
}

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
            let first = games.len();
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
                    folder: None,
                });
            }
            assign_folders(&mut games[first..], &dir);
        }
    }
    games.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.id.cmp(&b.id)));
    games
}

/// Sets [`Game::folder`] for the games under `system_dir` that are alone in their folder.
fn assign_folders(games: &mut [Game], system_dir: &Path) {
    // Games under each folder below the system folder, nested folders included.
    let mut counts: HashMap<PathBuf, usize> = HashMap::new();
    for game in games.iter() {
        for ancestor in game.path.ancestors().skip(1) {
            if ancestor == system_dir || !ancestor.starts_with(system_dir) {
                break;
            }
            *counts.entry(ancestor.to_path_buf()).or_default() += 1;
        }
    }
    for game in games.iter_mut() {
        game.folder = game
            .path
            .parent()
            .filter(|parent| counts.get(*parent) == Some(&1))
            .map(Path::to_path_buf);
    }
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
    fn a_folder_holding_one_game_is_its_folder() {
        let dir = tempdir();
        let root = dir.path();
        touch(
            root,
            "gc/Twilight Princess/Legend of Zelda, The - Twilight Princess (USA).nkit.iso",
        );
        touch(root, "gc/Twilight Princess/theme.mp3");
        touch(root, "gc/Zelda/Wind Waker (USA).iso");
        touch(root, "gc/Zelda/Minish/Four Swords (USA).iso");
        touch(root, "gc/Mario/Sunshine/Super Mario Sunshine (USA).iso");
        touch(root, "gc/Pikmin (USA).iso");

        let games = scan_roms(root, &Systems::builtin().unwrap());
        let folder = |title: &str| {
            let game = games.iter().find(|g| g.title.contains(title)).unwrap();
            game.folder
                .as_ref()
                .map(|f| f.strip_prefix(root).unwrap().display().to_string())
        };
        assert_eq!(folder("Twilight").as_deref(), Some("gc/Twilight Princess"));
        // Zelda holds two games: a grouping folder. Minish holds one.
        assert_eq!(folder("Wind Waker"), None);
        assert_eq!(folder("Four Swords").as_deref(), Some("gc/Zelda/Minish"));
        // Mario holds only Sunshine's folder: Sunshine's own folder is the nearer one.
        assert_eq!(folder("Sunshine").as_deref(), Some("gc/Mario/Sunshine"));
        // Flat in the system folder: no folder.
        assert_eq!(folder("Pikmin"), None);
    }

    #[test]
    fn missing_root_has_no_games() {
        let dir = tempdir();
        let games = scan_roms(&dir.path().join("nope"), &Systems::builtin().unwrap());
        assert!(games.is_empty());
        assert!(!root_is_available(&dir.path().join("nope")));
        assert!(root_is_available(dir.path()));
    }
}
