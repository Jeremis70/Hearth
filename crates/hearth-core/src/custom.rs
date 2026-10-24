//! A game's customizations: the art, theme and settings the user picked for it, stored in
//! Hearth's own per-game data and found first by [`crate::extras`].
//!
//! ```text
//! <data>/games/<art key>/          e.g. <data>/games/gc/GZ2E01/
//!   custom.toml                    what was picked, and from where
//!   grid.png hero.jpg logo.webp icon.png theme.ogg     copies of the picked files
//! ```
//!
//! Keyed by art key (like the art cache), so two dumps of one game share their picks, and
//! a game keeps them when its file moves. A game without a header serial is keyed by its
//! cleaned title, so renaming its file loses them (as it loses the match).
//!
//! Chosen files are copied in: the original can go. Picks are the user's data and are never
//! deleted by the library's clean-up of vanished games.
//!
//! ```toml
//! [hero]
//! source = "sgdb"      # or "file"
//! asset = 98765        # the SteamGridDB asset, for "sgdb"
//! from = "x.png"       # the file it was copied from, for "file" (information only)
//! position = 0.3       # 0 = left edge … 1 = right edge of the hero; also for automatic heroes
//!
//! [theme]
//! source = "file"      # or "none": play no theme, even one in the game folder
//! start = 12.0         # seconds added to where the first play starts
//! ```

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::art::{ArtKey, ArtKind, identify};
use crate::extras::Extra;
use crate::model::{Game, GameId};

const FILE: &str = "custom.toml";
/// Where the hero is centered by default.
pub const CENTER: f32 = 0.5;

/// Where a pick came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// A SteamGridDB image (its `asset` id is kept).
    Sgdb,
    /// A file the user chose.
    File,
    /// Nothing: hides what later sources would find (a theme in the game folder).
    None,
}

/// One section of `custom.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Where the pick came from; `None` when nothing is picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    /// The SteamGridDB asset id, for [`Source::Sgdb`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<u64>,
    /// The file it was copied from, for [`Source::File`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Hero only: horizontal position of the crop, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<f32>,
    /// Theme only: seconds added to the first play's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f32>,
}

impl Entry {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn clear_pick(&mut self) {
        self.source = None;
        self.asset = None;
        self.from = None;
    }
}

/// A game's `custom.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Custom {
    #[serde(default, skip_serializing_if = "Entry::is_empty")]
    pub grid: Entry,
    #[serde(default, skip_serializing_if = "Entry::is_empty")]
    pub hero: Entry,
    #[serde(default, skip_serializing_if = "Entry::is_empty")]
    pub logo: Entry,
    #[serde(default, skip_serializing_if = "Entry::is_empty")]
    pub icon: Entry,
    #[serde(default, skip_serializing_if = "Entry::is_empty")]
    pub theme: Entry,
}

impl Custom {
    /// The section for `extra`.
    pub fn entry(&self, extra: Extra) -> &Entry {
        match extra {
            Extra::Theme => &self.theme,
            Extra::Art(ArtKind::Grid) => &self.grid,
            Extra::Art(ArtKind::Hero) => &self.hero,
            Extra::Art(ArtKind::Logo) => &self.logo,
            Extra::Art(ArtKind::Icon) => &self.icon,
        }
    }

    fn entry_mut(&mut self, extra: Extra) -> &mut Entry {
        match extra {
            Extra::Theme => &mut self.theme,
            Extra::Art(ArtKind::Grid) => &mut self.grid,
            Extra::Art(ArtKind::Hero) => &mut self.hero,
            Extra::Art(ArtKind::Logo) => &mut self.logo,
            Extra::Art(ArtKind::Icon) => &mut self.icon,
        }
    }

    /// The hero's crop position, 0 (left) to 1 (right).
    pub fn hero_position(&self) -> f32 {
        self.hero.position.unwrap_or(CENTER).clamp(0.0, 1.0)
    }

    /// Seconds added to the theme's first-play start.
    pub fn theme_start(&self) -> f32 {
        self.theme.start.unwrap_or(0.0).max(0.0)
    }
}

/// What the store says about one extra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// This file.
    Found(PathBuf),
    /// Deliberately nothing: later sources aren't asked.
    Blocked,
    /// Nothing picked: ask the next source.
    Pass,
}

/// What [`CustomStore::remove_theme`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeRemoved {
    /// The picked file was deleted; a theme in the game folder plays again.
    Picked,
    /// No theme plays any more (the game folder's is ignored, never deleted).
    Hidden,
}

/// Hearth's per-game data. Cheap to clone; clones share the game → key map.
#[derive(Debug, Clone)]
pub struct CustomStore {
    root: PathBuf,
    keys: Arc<RwLock<HashMap<GameId, ArtKey>>>,
}

impl CustomStore {
    /// A store under `root` (`<data>/games`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            keys: Arc::default(),
        }
    }

    /// The folder for `key`.
    pub fn dir(&self, key: &ArtKey) -> PathBuf {
        self.root.join(key.as_str())
    }

    /// Notes `game`'s art key (the art worker knows it after identifying the game).
    pub fn remember(&self, game: &GameId, key: &ArtKey) {
        if let Ok(mut keys) = self.keys.write() {
            keys.insert(game.clone(), key.clone());
        }
    }

    /// `game`'s art key: remembered, else identified from its file (and remembered).
    pub fn key_of(&self, game: &Game) -> ArtKey {
        if let Some(key) = self.keys.read().ok().and_then(|k| k.get(&game.id).cloned()) {
            return key;
        }
        let key = identify(&game.system, &game.path).key;
        self.remember(&game.id, &key);
        key
    }

    /// `key`'s customizations (empty when none, or when the file can't be read).
    pub fn load(&self, key: &ArtKey) -> Custom {
        let path = self.dir(key).join(FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|err| {
                tracing::warn!(path = %path.display(), "unreadable customizations: {err}");
                Custom::default()
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Custom::default(),
            Err(err) => {
                tracing::warn!(path = %path.display(), "can't read customizations: {err}");
                Custom::default()
            }
        }
    }

    fn save(&self, key: &ArtKey, custom: &Custom) -> io::Result<()> {
        let dir = self.dir(key);
        let path = dir.join(FILE);
        if *custom == Custom::default() {
            return match std::fs::remove_file(&path) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
                _ => Ok(()),
            };
        }
        std::fs::create_dir_all(&dir)?;
        let text = toml::to_string(custom).map_err(io::Error::other)?;
        write_atomic(&path, text.as_bytes())
    }

    fn update(&self, key: &ArtKey, change: impl FnOnce(&mut Custom)) -> io::Result<Custom> {
        let mut custom = self.load(key);
        change(&mut custom);
        self.save(key, &custom)?;
        Ok(custom)
    }

    /// The stored file for `extra`, whatever its extension.
    pub fn file(&self, key: &ArtKey, extra: Extra) -> Option<PathBuf> {
        let dir = self.dir(key);
        extra
            .extensions()
            .iter()
            .map(|ext| dir.join(format!("{}.{ext}", extra.name())))
            .find(|path| path.is_file())
    }

    /// What the store has for `key`'s `extra`.
    pub fn find(&self, key: &ArtKey, extra: Extra) -> Lookup {
        let custom = self.load(key);
        match custom.entry(extra).source {
            None => Lookup::Pass,
            Some(Source::None) => Lookup::Blocked,
            Some(_) => match self.file(key, extra) {
                Some(path) => Lookup::Found(path),
                None => {
                    tracing::warn!(key = %key, extra = extra.name(), "picked file is missing");
                    Lookup::Pass
                }
            },
        }
    }

    /// Copies `file` in as `key`'s `extra`. The original is only read.
    pub fn pick_file(&self, key: &ArtKey, extra: Extra, file: &Path) -> io::Result<PathBuf> {
        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|e| extra.extensions().contains(&e.as_str()))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "not a supported file type")
            })?;
        let bytes = std::fs::read(file)?;
        let from = file.display().to_string();
        self.store(key, extra, &ext, &bytes, |entry| {
            entry.source = Some(Source::File);
            entry.from = Some(from);
        })
    }

    /// Stores a downloaded SteamGridDB image as `key`'s `kind`.
    pub fn pick_asset(
        &self,
        key: &ArtKey,
        kind: ArtKind,
        asset: u64,
        ext: &str,
        bytes: &[u8],
    ) -> io::Result<PathBuf> {
        self.store(key, Extra::Art(kind), ext, bytes, |entry| {
            entry.source = Some(Source::Sgdb);
            entry.asset = Some(asset);
        })
    }

    fn store(
        &self,
        key: &ArtKey,
        extra: Extra,
        ext: &str,
        bytes: &[u8],
        mark: impl FnOnce(&mut Entry),
    ) -> io::Result<PathBuf> {
        let dir = self.dir(key);
        std::fs::create_dir_all(&dir)?;
        let target = dir.join(format!("{}.{ext}", extra.name()));
        write_atomic(&target, bytes)?;
        self.remove_files(key, extra, Some(&target))?;
        self.update(key, |custom| {
            let entry = custom.entry_mut(extra);
            entry.clear_pick();
            mark(entry);
        })?;
        Ok(target)
    }

    fn remove_files(&self, key: &ArtKey, extra: Extra, keep: Option<&Path>) -> io::Result<()> {
        let dir = self.dir(key);
        for ext in extra.extensions() {
            let path = dir.join(format!("{}.{ext}", extra.name()));
            if Some(path.as_path()) != keep {
                match std::fs::remove_file(&path) {
                    Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Back to automatic: the picked file goes, settings (hero position) stay.
    pub fn reset(&self, key: &ArtKey, extra: Extra) -> io::Result<()> {
        self.remove_files(key, extra, None)?;
        self.update(key, |custom| custom.entry_mut(extra).clear_pick())?;
        Ok(())
    }

    /// Removes the theme: a picked file is deleted (a game-folder theme plays again);
    /// otherwise no theme plays (the game folder's file is ignored, never deleted).
    pub fn remove_theme(&self, key: &ArtKey) -> io::Result<ThemeRemoved> {
        if self.load(key).theme.source == Some(Source::File) {
            self.reset(key, Extra::Theme)?;
            return Ok(ThemeRemoved::Picked);
        }
        self.update(key, |custom| {
            custom.theme.clear_pick();
            custom.theme.source = Some(Source::None);
        })?;
        Ok(ThemeRemoved::Hidden)
    }

    /// Sets the hero's crop position (0 to 1).
    pub fn set_hero_position(&self, key: &ArtKey, position: f32) -> io::Result<()> {
        let position = position.clamp(0.0, 1.0);
        self.update(key, |custom| {
            custom.hero.position = ((position - CENTER).abs() > f32::EPSILON).then_some(position);
        })?;
        Ok(())
    }

    /// Sets the seconds added to the theme's first-play start.
    pub fn set_theme_start(&self, key: &ArtKey, seconds: f32) -> io::Result<()> {
        let seconds = seconds.max(0.0);
        self.update(key, |custom| {
            custom.theme.start = (seconds > 0.0).then_some(seconds);
        })?;
        Ok(())
    }

    /// The game was matched again: SteamGridDB picks (made from the old match) go, the
    /// hero is centered again; picks from files stay.
    pub fn forget_match(&self, key: &ArtKey) -> io::Result<()> {
        let custom = self.load(key);
        for kind in ArtKind::ALL {
            if custom.entry(Extra::Art(kind)).source == Some(Source::Sgdb) {
                self.reset(key, Extra::Art(kind))?;
            }
        }
        self.set_hero_position(key, CENTER)
    }
}

/// Writes through a `.part` file, so a crash never leaves half a file under the real name.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    fn key() -> ArtKey {
        ArtKey::from_stored("gc/GZ2E01")
    }

    #[test]
    fn nothing_picked_passes() {
        let dir = tempdir();
        let store = CustomStore::new(dir.path());
        assert_eq!(store.find(&key(), Extra::Theme), Lookup::Pass);
        assert_eq!(store.load(&key()), Custom::default());
        assert_eq!(store.load(&key()).hero_position(), CENTER);
    }

    #[test]
    fn a_picked_file_is_copied_in_and_found() {
        let dir = tempdir();
        let downloads = dir.path().join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let original = downloads.join("My Logo.PNG");
        std::fs::write(&original, b"png").unwrap();
        let store = CustomStore::new(dir.path().join("games"));

        let copy = store
            .pick_file(&key(), Extra::Art(ArtKind::Logo), &original)
            .unwrap();
        assert_eq!(copy, dir.path().join("games/gc/GZ2E01/logo.png"));
        std::fs::remove_file(&original).unwrap();
        assert_eq!(
            store.find(&key(), Extra::Art(ArtKind::Logo)),
            Lookup::Found(copy)
        );
        let custom = store.load(&key());
        assert_eq!(custom.logo.source, Some(Source::File));
        assert!(custom.logo.from.unwrap().ends_with("My Logo.PNG"));

        // A new pick of another type replaces the old file.
        store
            .pick_asset(&key(), ArtKind::Logo, 42, "webp", b"webp")
            .unwrap();
        let dir = store.dir(&key());
        assert!(!dir.join("logo.png").exists());
        assert!(dir.join("logo.webp").is_file());
        let logo = store.load(&key()).logo;
        assert_eq!(
            (logo.source, logo.asset, logo.from),
            (Some(Source::Sgdb), Some(42), None)
        );
    }

    #[test]
    fn unsupported_files_are_refused() {
        let dir = tempdir();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, b"x").unwrap();
        let store = CustomStore::new(dir.path().join("games"));
        assert!(store.pick_file(&key(), Extra::Theme, &file).is_err());
        assert!(!store.dir(&key()).exists());
    }

    #[test]
    fn reset_keeps_the_hero_position() {
        let dir = tempdir();
        let store = CustomStore::new(dir.path());
        store
            .pick_asset(&key(), ArtKind::Hero, 7, "png", b"x")
            .unwrap();
        store.set_hero_position(&key(), 0.2).unwrap();
        store.reset(&key(), Extra::Art(ArtKind::Hero)).unwrap();
        assert_eq!(store.find(&key(), Extra::Art(ArtKind::Hero)), Lookup::Pass);
        assert_eq!(store.load(&key()).hero_position(), 0.2);
        assert!(!store.dir(&key()).join("hero.png").exists());

        // Back to center and nothing left: the file goes too.
        store.set_hero_position(&key(), CENTER).unwrap();
        assert!(!store.dir(&key()).join(FILE).exists());
    }

    #[test]
    fn removing_a_theme_falls_back_then_hides() {
        let dir = tempdir();
        let song = dir.path().join("song.ogg");
        std::fs::write(&song, b"x").unwrap();
        let store = CustomStore::new(dir.path().join("games"));
        store.pick_file(&key(), Extra::Theme, &song).unwrap();
        store.set_theme_start(&key(), 3.0).unwrap();

        assert_eq!(store.remove_theme(&key()).unwrap(), ThemeRemoved::Picked);
        assert_eq!(store.find(&key(), Extra::Theme), Lookup::Pass);
        assert_eq!(store.remove_theme(&key()).unwrap(), ThemeRemoved::Hidden);
        assert_eq!(store.find(&key(), Extra::Theme), Lookup::Blocked);
        assert_eq!(store.load(&key()).theme_start(), 3.0);

        // Picking a file again un-hides.
        store.pick_file(&key(), Extra::Theme, &song).unwrap();
        assert!(matches!(store.find(&key(), Extra::Theme), Lookup::Found(_)));
    }

    #[test]
    fn a_new_match_drops_only_steamgriddb_picks() {
        let dir = tempdir();
        let file = dir.path().join("grid.png");
        std::fs::write(&file, b"x").unwrap();
        let store = CustomStore::new(dir.path().join("games"));
        store
            .pick_file(&key(), Extra::Art(ArtKind::Grid), &file)
            .unwrap();
        store
            .pick_asset(&key(), ArtKind::Hero, 1, "png", b"x")
            .unwrap();
        store.set_hero_position(&key(), 0.9).unwrap();

        store.forget_match(&key()).unwrap();
        assert!(matches!(
            store.find(&key(), Extra::Art(ArtKind::Grid)),
            Lookup::Found(_)
        ));
        assert_eq!(store.find(&key(), Extra::Art(ArtKind::Hero)), Lookup::Pass);
        assert_eq!(store.load(&key()).hero_position(), CENTER);
    }

    #[test]
    fn a_broken_file_reads_as_nothing_picked() {
        let dir = tempdir();
        let store = CustomStore::new(dir.path());
        std::fs::create_dir_all(store.dir(&key())).unwrap();
        std::fs::write(store.dir(&key()).join(FILE), "hero = [").unwrap();
        assert_eq!(store.load(&key()), Custom::default());
    }

    #[test]
    fn keys_are_remembered_per_game() {
        let dir = tempdir();
        let store = CustomStore::new(dir.path());
        let game = Game {
            id: GameId::new("roms:n64/a.z64"),
            title: "A".into(),
            system: crate::model::SystemId::new("n64"),
            path: dir.path().join("missing.z64"),
            folder: None,
        };
        let guessed = store.key_of(&game);
        assert!(guessed.as_str().starts_with("n64/name-"));
        store.remember(&game.id, &key());
        assert_eq!(store.key_of(&game), key());
    }
}
