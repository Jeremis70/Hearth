//! A game's own theme music. Sources are tried in order; today there is one, a file next to
//! the ROM (`<rom stem>-theme.ogg`, `.mp3` or `.flac`, like the local art overrides). Themes
//! extracted from game dumps (PSP `SND0.AT3`, Wii U `bootSound`, …) will be a second source
//! behind it, so a file the user put there always wins.

use std::path::{Path, PathBuf};

use crate::model::Game;

/// Extensions tried, in order.
const THEME_EXTENSIONS: [&str; 3] = ["ogg", "mp3", "flac"];

/// A theme found for a game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// The file.
    pub path: PathBuf,
    /// Which source found it ("rom" for the file next to the ROM).
    pub source: &'static str,
}

/// Somewhere themes come from.
pub trait ThemeSource: Send + Sync {
    /// The game's theme, if this source has one.
    fn theme(&self, game: &Game) -> Option<Theme>;
}

/// `<rom stem>-theme.<ext>` next to the ROM.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalThemeFile;

impl ThemeSource for LocalThemeFile {
    fn theme(&self, game: &Game) -> Option<Theme> {
        local_theme(&game.path).map(|path| Theme {
            path,
            source: "rom",
        })
    }
}

/// The local theme file of `rom`, if there is one.
pub fn local_theme(rom: &Path) -> Option<PathBuf> {
    local_theme_candidates(rom).into_iter().find(|path| {
        let found = path.is_file();
        tracing::debug!(path = %path.display(), found, "theme lookup");
        found
    })
}

/// The files [`local_theme`] tries, in order.
pub fn local_theme_candidates(rom: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (rom.parent(), rom.file_stem()) else {
        return Vec::new();
    };
    let stem = stem.to_string_lossy();
    THEME_EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{stem}-theme.{ext}")))
        .collect()
}

/// Every source, first match wins.
pub struct ThemeLookup {
    sources: Vec<Box<dyn ThemeSource>>,
}

impl Default for ThemeLookup {
    fn default() -> Self {
        Self {
            sources: vec![Box::new(LocalThemeFile)],
        }
    }
}

impl std::fmt::Debug for ThemeLookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThemeLookup")
            .field("sources", &self.sources.len())
            .finish()
    }
}

impl ThemeLookup {
    /// A lookup over `sources`, in order.
    pub fn new(sources: Vec<Box<dyn ThemeSource>>) -> Self {
        Self { sources }
    }

    /// The game's theme from the first source that has one.
    pub fn find(&self, game: &Game) -> Option<Theme> {
        self.sources.iter().find_map(|s| s.theme(game))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GameId, SystemId};
    use crate::testdir::tempdir;

    struct Extracted;
    impl ThemeSource for Extracted {
        fn theme(&self, game: &Game) -> Option<Theme> {
            Some(Theme {
                path: game.path.with_extension("at3"),
                source: "extracted",
            })
        }
    }

    #[test]
    fn the_file_next_to_the_rom_wins_then_later_sources() {
        let dir = tempdir();
        let rom = dir.path().join("Ocarina of Time (USA).z64");
        std::fs::write(&rom, b"x").unwrap();
        let game = Game {
            id: GameId::new("n64/oot"),
            title: "Ocarina of Time".into(),
            system: SystemId::new("n64"),
            path: rom.clone(),
        };
        let lookup = ThemeLookup::new(vec![Box::new(LocalThemeFile), Box::new(Extracted)]);
        assert_eq!(lookup.find(&game).unwrap().source, "extracted");
        assert_eq!(ThemeLookup::default().find(&game), None);

        let mp3 = dir.path().join("Ocarina of Time (USA)-theme.mp3");
        std::fs::write(&mp3, b"x").unwrap();
        assert_eq!(lookup.find(&game).unwrap().path, mp3);
        let ogg = dir.path().join("Ocarina of Time (USA)-theme.ogg");
        std::fs::write(&ogg, b"x").unwrap();
        let found = lookup.find(&game).unwrap();
        assert_eq!((found.path, found.source), (ogg, "rom"));
    }
}
