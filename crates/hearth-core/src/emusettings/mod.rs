//! Emulator settings: which config keys Hearth owns, which belong to the user, and how both
//! reach the emulator's own files at launch (design README §9 "Emulator settings").
//!
//! Every key of an emulator's config is either locked (Hearth's: fullscreen, input, hotkeys,
//! paths… re-applied at every launch) or the user's (never overwritten). The settings Hearth
//! offers (`schema`) are the user's too: Hearth writes them from its layers (the console base
//! shared by every profile, then the game's overrides), and keeps a value the user changed in
//! the emulator itself (`merge`).

pub mod backup;
pub mod doc;
pub mod gpu;
pub mod merge;
pub mod schema;
pub mod store;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use doc::{ConfigDoc, Style};
pub use gpu::Gpu;
pub use merge::{Layer, Report};
pub use schema::{Schema, Scope, Setting};
pub use store::{GameOverrides, Resolved, Shared, Source};

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// One emulator's settings: its schema, its console base file and the preset recommended for
/// this box. Cheap to clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorSettings {
    schema: Arc<Schema>,
    shared_path: PathBuf,
    recommended: Option<String>,
}

impl EmulatorSettings {
    /// The settings of `schema`'s emulator, with the console base in `shared_dir`
    /// (`<home>/emulators/<emu>/`) and the preset recommended for `gpu`.
    pub fn new(schema: Schema, shared_dir: &Path, gpu: Option<Gpu>) -> Self {
        let recommended = schema.preset_for_gpu(gpu).map(|p| p.id.clone());
        Self {
            schema: Arc::new(schema),
            shared_path: shared_dir.join(store::SHARED_FILE),
            recommended,
        }
    }

    /// The emulator id (`dolphin`).
    pub fn emulator(&self) -> &str {
        &self.schema.emulator
    }

    /// The schema.
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The preset recommended for this box's GPU.
    pub fn recommended(&self) -> Option<&schema::Preset> {
        self.recommended
            .as_deref()
            .and_then(|id| self.schema.preset(id))
    }

    /// The console base file.
    pub fn shared_path(&self) -> &Path {
        &self.shared_path
    }

    /// The console base as stored now.
    pub fn shared(&self) -> Shared {
        Shared::load(&self.shared_path)
    }

    /// Stores the console base.
    pub fn save_shared(&self, shared: &Shared) -> io::Result<()> {
        shared.save(&self.shared_path)
    }

    /// Resolution over `shared`.
    pub fn resolved<'a>(&'a self, shared: &'a Shared) -> Resolved<'a> {
        Resolved {
            schema: &self.schema,
            shared,
            recommended: self.recommended(),
        }
    }

    /// A game's overrides for this emulator (`game_dir` is `<data>/games/<art key>`).
    pub fn game(&self, game_dir: &Path) -> std::collections::BTreeMap<String, String> {
        GameOverrides::load(game_dir).of(self.emulator())
    }

    /// Changes a console value (Settings). Unknown settings or choices are errors.
    pub fn set_console(&self, id: &str, choice: &str) -> io::Result<()> {
        let setting = self.checked(id, choice)?;
        let mut shared = self.shared();
        let snapshot = shared.clone();
        store::set_console(&self.resolved(&snapshot), &mut shared, setting, choice);
        self.save_shared(&shared)
    }

    /// Picks a preset (`None`: back to the one recommended for the GPU, rows of presets
    /// reset too).
    pub fn set_preset(&self, preset: Option<&str>) -> io::Result<()> {
        let mut shared = self.shared();
        match preset {
            Some(id) if self.schema.preset(id).is_some() => {
                store::set_preset(&self.schema, &mut shared, id);
            }
            Some(id) => return Err(invalid(format!("no preset {id}"))),
            None => {
                let recommended = self.recommended.clone().unwrap_or_default();
                store::set_preset(&self.schema, &mut shared, &recommended);
                shared.preset = None;
            }
        }
        self.save_shared(&shared)
    }

    /// Back to Hearth's defaults: the recommended preset, no changed row.
    pub fn reset_console(&self) -> io::Result<()> {
        self.save_shared(&Shared::default())
    }

    /// Sets (`Some`) or clears (`None`) a game's override. Only settings with per-game scope.
    pub fn set_game(&self, game_dir: &Path, id: &str, choice: Option<&str>) -> io::Result<()> {
        let setting = match choice {
            Some(choice) => self.checked(id, choice)?,
            None => self
                .schema
                .setting(id)
                .ok_or_else(|| invalid(format!("no setting {id}")))?,
        };
        if setting.scope != Scope::PerGame {
            return Err(invalid(format!("{id} can't be set per game")));
        }
        let mut game = GameOverrides::load(game_dir);
        game.set(self.emulator(), id, choice);
        game.save(game_dir)
    }

    /// Clears every override of a game for this emulator.
    pub fn reset_game(&self, game_dir: &Path) -> io::Result<()> {
        let mut game = GameOverrides::load(game_dir);
        game.0.remove(self.emulator());
        game.save(game_dir)
    }

    fn checked(&self, id: &str, choice: &str) -> io::Result<&Setting> {
        let setting = self
            .schema
            .setting(id)
            .ok_or_else(|| invalid(format!("no setting {id}")))?;
        if setting.choice(choice).is_none() {
            return Err(invalid(format!("{choice} isn't a choice of {id}")));
        }
        Ok(setting)
    }

    /// The settings as JSON for the dev socket: preset, status, and per row its value, label
    /// and source; with `game`, the game's values (and what Dolphin's own game INIs set,
    /// `shipped`).
    pub fn state_json(
        &self,
        game: Option<&std::collections::BTreeMap<String, String>>,
        shipped: &std::collections::BTreeMap<String, String>,
    ) -> serde_json::Value {
        let shared = self.shared();
        let r = self.resolved(&shared);
        let rows: Vec<serde_json::Value> = self
            .schema
            .settings
            .iter()
            .map(|s| {
                let (value, source) = match game {
                    Some(g) => r.for_game(s, g),
                    None => r.console(s),
                };
                serde_json::json!({
                    "id": s.id,
                    "value": value,
                    "label": s.label_of(&value),
                    "source": source,
                    "per_game": s.scope == Scope::PerGame,
                    "shipped": shipped.get(&s.id),
                })
            })
            .collect();
        serde_json::json!({
            "emulator": self.emulator(),
            "preset": r.preset().map(|p| p.id.as_str()),
            "preset_chosen": r.preset_is_chosen(),
            "recommended": self.recommended,
            "status": r.status(),
            "rows": rows,
        })
    }

    /// Stores what a merge found changed outside Hearth in the layer it came from.
    pub fn adopt(&self, report: &Report, game_dir: Option<&Path>) -> io::Result<()> {
        if report.adopted.is_empty() {
            return Ok(());
        }
        let mut shared = self.shared();
        let mut game = game_dir.map(GameOverrides::load);
        for a in &report.adopted {
            let Some(setting) = self.schema.setting(&a.setting) else {
                continue;
            };
            match (a.layer, game.as_mut()) {
                (Layer::Game, Some(game)) => game.set(self.emulator(), &a.setting, Some(&a.choice)),
                _ => {
                    let snapshot = shared.clone();
                    let resolved = self.resolved(&snapshot);
                    store::set_console(&resolved, &mut shared, setting, &a.choice);
                }
            }
        }
        self.save_shared(&shared)?;
        if let (Some(game), Some(dir)) = (game, game_dir) {
            game.save(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    #[test]
    fn changes_from_settings_and_the_game_page() {
        let dir = tempdir();
        let gpu = Some(Gpu {
            vendor: 0x8086,
            device: 0x9a49,
        });
        let settings = EmulatorSettings::new(Schema::dolphin(), &dir.path().join("shared"), gpu);
        let status = || {
            let shared = settings.shared();
            settings.resolved(&shared).status()
        };
        assert_eq!(status().as_deref(), Some("Balanced"));
        settings.set_preset(Some("quality")).unwrap();
        assert_eq!(status().as_deref(), Some("Quality"));
        settings.set_console("internal_resolution", "6").unwrap();
        assert_eq!(status().as_deref(), Some("Custom"));
        assert!(settings.set_console("internal_resolution", "5").is_err());
        assert!(settings.set_console("nope", "1").is_err());
        settings.set_preset(None).unwrap();
        assert_eq!(status().as_deref(), Some("Balanced"));
        assert_eq!(settings.shared(), Shared::default());
        settings.set_console("vsync", "True").unwrap();
        settings.reset_console().unwrap();
        assert!(!settings.shared_path().exists());

        let game = dir.path().join("games/gc/GZ2E01");
        settings
            .set_game(&game, "internal_resolution", Some("4"))
            .unwrap();
        assert!(settings.set_game(&game, "backend", Some("Vulkan")).is_err());
        let json = settings.state_json(Some(&settings.game(&game)), &Default::default());
        let row = &json["rows"][1];
        assert_eq!(row["id"], "internal_resolution");
        assert_eq!(row["value"], "4");
        assert_eq!(row["source"], "game");
        assert_eq!(row["label"], "4× (1440p)");
        settings
            .set_game(&game, "internal_resolution", None)
            .unwrap();
        assert!(settings.game(&game).is_empty());
        settings.set_game(&game, "vsync", Some("True")).unwrap();
        settings.reset_game(&game).unwrap();
        assert!(!game.join(store::GAME_FILE).exists());
    }
}
