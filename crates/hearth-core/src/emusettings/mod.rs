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
