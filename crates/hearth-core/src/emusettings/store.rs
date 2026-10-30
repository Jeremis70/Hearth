//! Where Hearth keeps emulator settings, and which value wins:
//!
//! - the console base, shared by every profile: `<home>/emulators/<emu>/shared.toml` (the
//!   preset and the rows changed in Settings);
//! - per-game overrides, shared by every profile: `<data>/games/<art key>/emulator.toml`, beside
//!   the game's Customize picks (deleted only with the game);
//! - what Hearth last wrote into a profile's emulator files: `hearth-applied.toml` in that
//!   profile's emulator folder (see `merge`).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::schema::{Preset, Schema, Scope, Setting};
use crate::users::write_atomic;

/// The console base file in `<home>/emulators/<emu>/`.
pub const SHARED_FILE: &str = "shared.toml";
/// The per-game file in `<data>/games/<art key>/`.
pub const GAME_FILE: &str = "emulator.toml";
/// The applied record in a profile's emulator folder.
pub const APPLIED_FILE: &str = "hearth-applied.toml";

fn load_toml<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), "unreadable emulator settings: {err}");
            T::default()
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => T::default(),
        Err(err) => {
            tracing::warn!(path = %path.display(), "can't read emulator settings: {err}");
            T::default()
        }
    }
}

fn save_toml<T: Serialize + Default + PartialEq>(path: &Path, value: &T) -> io::Result<()> {
    if *value == T::default() {
        return match std::fs::remove_file(path) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string(value).map_err(io::Error::other)?;
    write_atomic(path, text.as_bytes())
}

/// The console base of one emulator.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shared {
    /// The preset picked by the user; `None`: the one recommended for the GPU.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Setting id → choice id, for rows changed from the preset (or default).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, String>,
}

impl Shared {
    /// Reads `path` (missing or unreadable: nothing set).
    pub fn load(path: &Path) -> Self {
        load_toml(path)
    }

    /// Writes `path`, or removes it when nothing is set.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        save_toml(path, self)
    }
}

/// Per-game overrides: emulator id → setting id → choice id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GameOverrides(pub BTreeMap<String, BTreeMap<String, String>>);

impl GameOverrides {
    /// Reads `<dir>/emulator.toml`.
    pub fn load(dir: &Path) -> Self {
        load_toml(&dir.join(GAME_FILE))
    }

    /// Writes `<dir>/emulator.toml`, or removes it when empty.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let mut clean = self.clone();
        clean.0.retain(|_, v| !v.is_empty());
        save_toml(&dir.join(GAME_FILE), &clean)
    }

    /// One emulator's overrides.
    pub fn of(&self, emulator: &str) -> BTreeMap<String, String> {
        self.0.get(emulator).cloned().unwrap_or_default()
    }

    /// Sets (`Some`) or clears (`None`) one override.
    pub fn set(&mut self, emulator: &str, setting: &str, choice: Option<&str>) {
        let map = self.0.entry(emulator.to_owned()).or_default();
        match choice {
            Some(choice) => {
                map.insert(setting.to_owned(), choice.to_owned());
            }
            None => {
                map.remove(setting);
            }
        }
    }
}

/// Where a setting's effective value comes from, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The emulator's own default.
    Default,
    /// The active preset.
    Preset,
    /// Changed in Settings (console-wide).
    Console,
    /// Overridden for this game.
    Game,
}

/// One emulator's settings as Hearth resolves them.
#[derive(Debug, Clone)]
pub struct Resolved<'a> {
    pub schema: &'a Schema,
    pub shared: &'a Shared,
    /// The preset recommended for this box's GPU.
    pub recommended: Option<&'a Preset>,
}

impl<'a> Resolved<'a> {
    /// The active preset (the user's, else the recommended one).
    pub fn preset(&self) -> Option<&'a Preset> {
        self.shared
            .preset
            .as_deref()
            .and_then(|id| self.schema.preset(id))
            .or(self.recommended)
    }

    /// Whether the preset was picked by the user (not automatic).
    pub fn preset_is_chosen(&self) -> bool {
        self.shared
            .preset
            .as_deref()
            .is_some_and(|id| self.schema.preset(id).is_some())
    }

    /// Whether a row of the active preset was changed ("Custom").
    pub fn is_custom(&self) -> bool {
        let Some(preset) = self.preset() else {
            return false;
        };
        preset
            .values
            .iter()
            .any(|(id, v)| self.shared.values.get(id).is_some_and(|mine| mine != v))
    }

    /// "Balanced", "Custom", or `None` without presets.
    pub fn status(&self) -> Option<String> {
        if self.is_custom() {
            return Some("Custom".into());
        }
        self.preset().map(|p| p.label.clone())
    }

    /// A setting's console value and where it comes from.
    pub fn console(&self, setting: &Setting) -> (String, Source) {
        if let Some(v) = self
            .shared
            .values
            .get(&setting.id)
            .filter(|v| setting.choice(v).is_some())
        {
            return (v.clone(), Source::Console);
        }
        if let Some(v) = self
            .preset()
            .and_then(|p| p.values.get(&setting.id))
            .filter(|v| setting.choice(v).is_some())
        {
            return (v.clone(), Source::Preset);
        }
        (setting.default.clone(), Source::Default)
    }

    /// A setting's value for a game with `game` overrides.
    pub fn for_game(&self, setting: &Setting, game: &BTreeMap<String, String>) -> (String, Source) {
        if setting.scope == Scope::PerGame
            && let Some(v) = game
                .get(&setting.id)
                .filter(|v| setting.choice(v).is_some())
        {
            return (v.clone(), Source::Game);
        }
        self.console(setting)
    }

    /// The console value a row would have without its own change (preset or default).
    pub fn base(&self, setting: &Setting) -> String {
        self.preset()
            .and_then(|p| p.values.get(&setting.id))
            .filter(|v| setting.choice(v).is_some())
            .cloned()
            .unwrap_or_else(|| setting.default.clone())
    }
}

/// Changes the console value of `setting`: stored only when it differs from what the preset
/// (or the default) gives.
pub fn set_console(resolved: &Resolved<'_>, shared: &mut Shared, setting: &Setting, choice: &str) {
    if resolved.base(setting) == choice {
        shared.values.remove(&setting.id);
    } else {
        shared.values.insert(setting.id.clone(), choice.to_owned());
    }
}

/// Picks a preset: every row a preset sets goes back to the new preset's value (or the
/// default); rows no preset sets keep their change.
pub fn set_preset(schema: &Schema, shared: &mut Shared, preset: &str) {
    if let Some(p) = schema.preset(preset) {
        shared.preset = Some(p.id.clone());
        let covered: Vec<&String> = schema
            .presets
            .iter()
            .flat_map(|p| p.values.keys())
            .collect();
        shared.values.retain(|id, _| !covered.contains(&id));
    }
}

/// What Hearth wrote into a profile's emulator files at the last launch: file (relative to the
/// emulator folder) → section → key → value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    /// Version of this record.
    #[serde(default)]
    pub version: u32,
    /// The one-time backup before Hearth first merged into these files was made.
    #[serde(default)]
    pub backed_up: bool,
    #[serde(default)]
    pub files: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
    /// Settings left to the user because the file holds a value Hearth doesn't offer: file →
    /// setting id → the choice Hearth wanted then. Hearth takes them back when its own value
    /// changes (the user changed it in Hearth).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub yielded: BTreeMap<String, BTreeMap<String, String>>,
}

impl Applied {
    /// The record in `emulator_dir`.
    pub fn path(emulator_dir: &Path) -> PathBuf {
        emulator_dir.join(APPLIED_FILE)
    }

    /// Reads it (missing: nothing applied yet).
    pub fn load(emulator_dir: &Path) -> Self {
        load_toml(&Self::path(emulator_dir))
    }

    /// Writes it.
    pub fn save(&self, emulator_dir: &Path) -> io::Result<()> {
        let mut out = self.clone();
        out.version = 1;
        out.files.retain(|_, sections| {
            sections.retain(|_, keys| !keys.is_empty());
            !sections.is_empty()
        });
        save_toml(&Self::path(emulator_dir), &out)
    }

    /// What Hearth wrote for `key` in `file`'s `section`.
    pub fn get(&self, file: &str, section: &str, key: &str) -> Option<&str> {
        self.files
            .get(file)?
            .get(section)?
            .get(key)
            .map(String::as_str)
    }

    /// Every key Hearth wrote in `file`, as (section, key, value).
    pub fn keys_of(&self, file: &str) -> Vec<(String, String, String)> {
        self.files
            .get(file)
            .into_iter()
            .flat_map(|sections| {
                sections.iter().flat_map(|(s, keys)| {
                    keys.iter()
                        .map(move |(k, v)| (s.clone(), k.clone(), v.clone()))
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    #[test]
    fn presets_custom_and_overrides() {
        let schema = Schema::dolphin();
        let balanced = schema.preset("balanced");
        let mut shared = Shared::default();
        let ir = schema.setting("internal_resolution").unwrap();
        let vsync = schema.setting("vsync").unwrap();
        {
            let r = Resolved {
                schema: &schema,
                shared: &shared,
                recommended: balanced,
            };
            assert_eq!(r.status().as_deref(), Some("Balanced"));
            assert!(!r.preset_is_chosen());
            assert_eq!(r.console(ir), ("2".into(), Source::Preset));
            assert_eq!(r.console(vsync), ("False".into(), Source::Default));
        }
        set_preset(&schema, &mut shared, "quality");
        let r = Resolved {
            schema: &schema,
            shared: &shared.clone(),
            recommended: balanced,
        };
        assert_eq!(r.console(ir).0, "3");
        // A row equal to the preset stores nothing; another value makes it Custom.
        set_console(&r, &mut shared, ir, "3");
        assert!(shared.values.is_empty());
        set_console(&r, &mut shared, ir, "4");
        set_console(&r, &mut shared, vsync, "True");
        let r = Resolved {
            schema: &schema,
            shared: &shared.clone(),
            recommended: balanced,
        };
        assert_eq!(r.status().as_deref(), Some("Custom"));
        // V-Sync isn't a preset row: it survives picking a preset, which clears the rest.
        set_preset(&schema, &mut shared, "performance");
        assert_eq!(shared.values.keys().collect::<Vec<_>>(), ["vsync"]);
        let r = Resolved {
            schema: &schema,
            shared: &shared,
            recommended: balanced,
        };
        assert_eq!(r.status().as_deref(), Some("Performance"));
        let mut game = GameOverrides::default();
        game.set("dolphin", "internal_resolution", Some("6"));
        game.set("dolphin", "backend", Some("Vulkan")); // console-only: ignored
        let g = game.of("dolphin");
        assert_eq!(r.for_game(ir, &g), ("6".into(), Source::Game));
        let backend = schema.setting("backend").unwrap();
        assert_eq!(r.for_game(backend, &g).1, Source::Default);
    }

    #[test]
    fn files_round_trip_and_vanish_when_empty() {
        let dir = tempdir();
        let shared_path = dir.path().join("emulators/dolphin/shared.toml");
        let mut shared = Shared::load(&shared_path);
        assert_eq!(shared, Shared::default());
        shared.preset = Some("quality".into());
        shared.save(&shared_path).unwrap();
        assert_eq!(Shared::load(&shared_path), shared);
        Shared::default().save(&shared_path).unwrap();
        assert!(!shared_path.exists());

        let game_dir = dir.path().join("games/gc/GZ2E01");
        let mut game = GameOverrides::load(&game_dir);
        game.set("dolphin", "vsync", Some("True"));
        game.save(&game_dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(game_dir.join(GAME_FILE)).unwrap(),
            "[dolphin]\nvsync = \"True\"\n"
        );
        game.set("dolphin", "vsync", None);
        game.save(&game_dir).unwrap();
        assert!(!game_dir.join(GAME_FILE).exists());

        let emu = dir.path().join("profiles/default/emulators/dolphin");
        let mut applied = Applied::load(&emu);
        applied
            .files
            .entry("Config/GFX.ini".into())
            .or_default()
            .entry("Settings".into())
            .or_default()
            .insert("InternalResolution".into(), "2".into());
        applied.save(&emu).unwrap();
        let back = Applied::load(&emu);
        assert_eq!(back.version, 1);
        assert_eq!(
            back.get("Config/GFX.ini", "Settings", "InternalResolution"),
            Some("2")
        );
        assert_eq!(back.keys_of("Config/GFX.ini").len(), 1);
    }
}
