//! A game's extras (its theme music, its own art) by fixed names, from a chain of sources.
//!
//! One rule: a game in its own folder under the system folder (a folder whose contents are
//! that one game; see [`Game::folder`]) may hold
//!
//! ```text
//! <games>/<system>/<any folder>/
//!   <the ROM>
//!   theme.ogg | theme.mp3 | theme.flac
//!   grid.png | hero.png | logo.png | icon.png   (or .jpg, .webp)
//! ```
//!
//! A folder holding several games is only a grouping folder, and games laid out flat in the
//! system folder have no extras; nothing is matched by the ROM's name.
//!
//! Sources are tried in order, first match wins. Today: [`GameFolder`]. The Round C picker
//! (a choice stored in Hearth's own per-game data, keyed by game id) goes in front of it in
//! [`Extras::standard`]; after the chain come the pack or Hearth's defaults (the ambient loop)
//! and the art cache, which the callers handle.

use std::path::PathBuf;

use crate::art::{ArtKind, GameArt};
use crate::model::Game;

const THEME_EXTENSIONS: [&str; 3] = ["ogg", "mp3", "flac"];
const IMAGE_EXTENSIONS: [&str; 3] = ["png", "jpg", "webp"];

/// What can be looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extra {
    /// Theme music.
    Theme,
    /// One kind of art.
    Art(ArtKind),
}

impl Extra {
    /// The fixed file name, without extension.
    pub fn name(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Art(ArtKind::Grid) => "grid",
            Self::Art(ArtKind::Hero) => "hero",
            Self::Art(ArtKind::Logo) => "logo",
            Self::Art(ArtKind::Icon) => "icon",
        }
    }

    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Theme => &THEME_EXTENSIONS,
            Self::Art(_) => &IMAGE_EXTENSIONS,
        }
    }
}

/// An extra found for a game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The file.
    pub path: PathBuf,
    /// Which source found it ("folder" for the game's own folder).
    pub source: &'static str,
}

/// Somewhere extras come from.
pub trait ExtraSource: Send + Sync {
    /// The file this source has for `game`'s `extra`, if any.
    fn find(&self, game: &Game, extra: Extra) -> Option<PathBuf>;
    /// Its name, for logs and the audio state ("folder").
    fn name(&self) -> &'static str;
}

/// The files the game's own folder could hold for `extra`, in the order they're tried. Empty
/// when the game has no folder of its own.
pub fn folder_candidates(game: &Game, extra: Extra) -> Vec<PathBuf> {
    let Some(folder) = &game.folder else {
        return Vec::new();
    };
    extra
        .extensions()
        .iter()
        .map(|ext| folder.join(format!("{}.{ext}", extra.name())))
        .collect()
}

/// `theme.*`, `hero.*`… in the game's own folder.
#[derive(Debug, Default, Clone, Copy)]
pub struct GameFolder;

impl ExtraSource for GameFolder {
    fn find(&self, game: &Game, extra: Extra) -> Option<PathBuf> {
        let candidates = folder_candidates(game, extra);
        if candidates.is_empty() {
            tracing::debug!(game = %game.id, extra = extra.name(), "no game folder: no extras");
        }
        candidates.into_iter().find(|path| {
            let found = path.is_file();
            tracing::debug!(path = %path.display(), found, "extra lookup");
            found
        })
    }

    fn name(&self) -> &'static str {
        "folder"
    }
}

/// The chain of sources.
pub struct Extras {
    sources: Vec<Box<dyn ExtraSource>>,
}

impl std::fmt::Debug for Extras {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<_> = self.sources.iter().map(|s| s.name()).collect();
        f.debug_struct("Extras").field("sources", &names).finish()
    }
}

impl Extras {
    /// Hearth's chain. Round C: the stored choice goes first.
    pub fn standard() -> Self {
        Self::new(vec![Box::new(GameFolder)])
    }

    /// A chain over `sources`, in order.
    pub fn new(sources: Vec<Box<dyn ExtraSource>>) -> Self {
        Self { sources }
    }

    /// The game's own art, kind by kind (the art cache fills the rest).
    pub fn art(&self, game: &Game) -> GameArt {
        let mut art = GameArt::default();
        for kind in ArtKind::ALL {
            if let Some(found) = self.find(game, Extra::Art(kind)) {
                art.set(kind, found.path);
            }
        }
        art
    }

    /// `game`'s `extra` from the first source that has it.
    pub fn find(&self, game: &Game, extra: Extra) -> Option<Found> {
        self.sources.iter().find_map(|source| {
            source.find(game, extra).map(|path| Found {
                path,
                source: source.name(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GameId, SystemId};
    use crate::testdir::tempdir;

    struct Chosen;
    impl ExtraSource for Chosen {
        fn find(&self, game: &Game, extra: Extra) -> Option<PathBuf> {
            (game.id.as_str() == "picked" && extra == Extra::Theme)
                .then(|| PathBuf::from("/data/picked/theme.ogg"))
        }
        fn name(&self) -> &'static str {
            "chosen"
        }
    }

    fn game(id: &str, rom: PathBuf, folder: Option<PathBuf>) -> Game {
        Game {
            id: GameId::new(id),
            title: "Ocarina of Time".into(),
            system: SystemId::new("n64"),
            path: rom,
            folder,
        }
    }

    #[test]
    fn fixed_names_in_the_game_folder_only() {
        let dir = tempdir();
        let folder = dir.path().join("n64/Ocarina of Time");
        std::fs::create_dir_all(&folder).unwrap();
        let rom = folder.join("Legend of Zelda, The - Ocarina of Time (U) (V1.2) [!].z64");
        std::fs::write(&rom, b"x").unwrap();
        // The old name-based files are not extras any more.
        std::fs::write(
            folder.join("Legend of Zelda, The - Ocarina of Time (U) (V1.2) [!]-theme.ogg"),
            b"x",
        )
        .unwrap();
        std::fs::write(
            folder.join("Legend of Zelda, The - Ocarina of Time (U) (V1.2) [!]-hero.png"),
            b"x",
        )
        .unwrap();
        let extras = Extras::standard();
        let oot = game("oot", rom.clone(), Some(folder.clone()));
        assert_eq!(extras.find(&oot, Extra::Theme), None);
        assert_eq!(extras.find(&oot, Extra::Art(ArtKind::Hero)), None);

        std::fs::write(folder.join("theme.mp3"), b"x").unwrap();
        std::fs::write(folder.join("hero.webp"), b"x").unwrap();
        let theme = extras.find(&oot, Extra::Theme).unwrap();
        assert_eq!(
            (theme.path, theme.source),
            (folder.join("theme.mp3"), "folder")
        );
        assert_eq!(
            extras.find(&oot, Extra::Art(ArtKind::Hero)).unwrap().path,
            folder.join("hero.webp")
        );
        std::fs::write(folder.join("theme.ogg"), b"x").unwrap();
        assert_eq!(
            extras.find(&oot, Extra::Theme).unwrap().path,
            folder.join("theme.ogg")
        );

        // Flat (or in a grouping folder): no folder, no extras.
        let flat = game("flat", rom, None);
        assert_eq!(extras.find(&flat, Extra::Theme), None);
        assert!(folder_candidates(&flat, Extra::Theme).is_empty());
    }

    #[test]
    fn earlier_sources_win() {
        let dir = tempdir();
        let folder = dir.path().to_path_buf();
        std::fs::write(folder.join("theme.ogg"), b"x").unwrap();
        let extras = Extras::new(vec![Box::new(Chosen), Box::new(GameFolder)]);
        let picked = game("picked", folder.join("a.z64"), Some(folder.clone()));
        assert_eq!(extras.find(&picked, Extra::Theme).unwrap().source, "chosen");
        let other = game("other", folder.join("b.z64"), Some(folder));
        assert_eq!(extras.find(&other, Extra::Theme).unwrap().source, "folder");
    }
}
