//! Emulator settings in the shell: Settings → Emulators, a game's Game settings page, and their
//! dev socket words. The values live in `hearth_core::emusettings`; they reach the emulator at
//! its next launch (`DolphinProfile::prepare`).

use std::rc::Rc;

use std::collections::BTreeMap;

use hearth_core::emusettings::{EmulatorSettings, Scope, Setting, merge};
use hearth_core::model::{Game, GameId};
use hearth_core::profiles::dolphin::GameRef;
use hearth_core::systems::EmulatorKind;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::{Controller, with};
use crate::bridge::models::set_category_status;
use crate::{AppWindow, EmuRow, HomeData, SettingsCategory, SettingsData};

/// Emulators listed in Settings, by key.
const EMULATORS: [(&str, &str); 2] = [("dolphin", "Dolphin"), ("retroarch", "RetroArch")];

/// Connects Settings → Emulators to the controller.
pub(super) fn wire(app: &AppWindow) {
    let settings = app.global::<SettingsData>();
    // Synchronous: the page is shown right after.
    settings.on_open_emulator(|key| {
        with(|c| c.open_emulator(&key));
    });
    settings.on_emu_step(|key, step| with(|c| c.emu_step(&key, step)).unwrap_or(false));
    settings.on_emu_reset(|| {
        with(Controller::emu_reset);
    });
    let home = app.global::<HomeData>();
    // Synchronous, like request-properties: the page is shown right after.
    home.on_open_game_settings(|id| {
        let id = GameId::new(id.as_str());
        with(|c| c.open_game_settings(&id));
    });
    home.on_game_setting_step(|key, step| {
        with(|c| c.game_setting_step(&key, step)).unwrap_or(false)
    });
    home.on_game_setting_reset(|key| {
        with(|c| c.game_setting_reset(&key));
    });
}

/// What a per-game row shows: the game's own value, else what Dolphin's shipped game INI sets
/// (it beats the console's), else the console's; and whether the game has its own.
fn game_value(
    settings: &EmulatorSettings,
    setting: &Setting,
    game: &BTreeMap<String, String>,
    shipped: &BTreeMap<String, String>,
) -> (String, GameSource) {
    if let Some(v) = game
        .get(&setting.id)
        .filter(|v| setting.choice(v).is_some())
    {
        return (v.clone(), GameSource::Game);
    }
    if let Some(v) = shipped.get(&setting.id) {
        return (v.clone(), GameSource::Dolphin);
    }
    let shared = settings.shared();
    (
        settings.resolved(&shared).console(setting).0,
        GameSource::Console,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GameSource {
    Console,
    /// Dolphin's own game settings (`Sys/GameSettings`).
    Dolphin,
    Game,
}

/// "Balanced", "Custom".
fn status(settings: &EmulatorSettings) -> String {
    let shared = settings.shared();
    settings.resolved(&shared).status().unwrap_or_default()
}

fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// The preset row: the active preset (or Custom) and what it does.
fn preset_row(settings: &EmulatorSettings) -> EmuRow {
    let recommended = settings
        .recommended()
        .map(|p| format!(" Recommended for this GPU: {}.", p.label))
        .unwrap_or_default();
    EmuRow {
        key: "preset".into(),
        label: "Preset".into(),
        value: status(settings).into(),
        description: format!("Sets the rows below for speed or for looks.{recommended}").into(),
        note: Default::default(),
        marked: false,
        action: false,
    }
}

/// A setting's row with its current value (`value` is a choice id).
fn setting_row(setting: &Setting, value: &str) -> EmuRow {
    EmuRow {
        key: setting.id.as_str().into(),
        label: setting.label.as_str().into(),
        value: setting.label_of(value).into(),
        description: setting.description.as_str().into(),
        note: Default::default(),
        marked: false,
        action: false,
    }
}

fn action_row(key: &str, label: &str, description: &str) -> EmuRow {
    EmuRow {
        key: key.into(),
        label: label.into(),
        value: Default::default(),
        description: description.into(),
        note: Default::default(),
        marked: false,
        action: true,
    }
}

/// The choice `step` places from `current` in `setting`, if there is one.
fn stepped<'a>(setting: &'a Setting, current: &str, step: i32) -> Option<&'a str> {
    let at = setting.choices.iter().position(|c| c.id == current)?;
    let to = at.checked_add_signed(step as isize)?;
    setting.choices.get(to).map(|c| c.id.as_str())
}

impl Controller {
    /// The game whose art key is `key` (`gc/GZ2E01`).
    fn game_by_key(&self, key: &str) -> Option<Game> {
        self.entries
            .iter()
            .find(|e| self.picks.key_of(&e.game).as_str() == key)
            .map(|e| e.game.clone())
    }

    /// `key`'s Dolphin reference (only Dolphin games with a disc id have per-game settings).
    fn dolphin_game(&self, key: &str) -> Result<(Game, GameRef), String> {
        let game = self
            .game_by_key(key)
            .ok_or_else(|| format!("error: no game with art key {key}"))?;
        let kind = self
            .systems
            .get(&game.system)
            .and_then(|s| s.emulator())
            .map(|e| e.kind);
        if kind != Some(EmulatorKind::Dolphin) {
            return Err(format!("error: {} doesn't run in Dolphin", game.title));
        }
        let game_ref = GameRef::for_game(&game, &self.picks)
            .ok_or_else(|| format!("error: {} has no disc id", game.title))?;
        Ok((game, game_ref))
    }

    /// Dev socket words for emulator settings (`scripts/send-action.sh`):
    /// - `emu_state:dolphin[:<art key>]`: JSON (preset, status, rows with value and source);
    /// - `emu_set:dolphin:<setting>=<choice>`, `emu_set:dolphin:preset=<id|auto>`,
    ///   `emu_set:dolphin:reset`;
    /// - `emu_game_set:<art key>:<setting>=<choice>`, `emu_game_reset:<art key>[:<setting>]`;
    /// - `emu_effective:<art key>`: JSON of what the next launch writes (files, keys, args).
    pub(super) fn emu_command(&mut self, name: &str, arg: &str) -> String {
        let settings = self.dolphin.settings().clone();
        let result: Result<String, String> = (|| match name {
            "emu_state" => {
                let (emu, key) = arg.split_once(':').unwrap_or((arg, ""));
                if emu != "dolphin" {
                    return Ok(serde_json::json!({ "emulator": emu, "rows": [] }).to_string());
                }
                if key.is_empty() {
                    return Ok(settings.state_json(None, &Default::default()).to_string());
                }
                let (_, game) = self.dolphin_game(key)?;
                let shipped = self.dolphin.shipped_game_values(&game.id);
                Ok(settings
                    .state_json(Some(&settings.game(&game.dir)), &shipped)
                    .to_string())
            }
            "emu_set" => {
                let rest = arg
                    .strip_prefix("dolphin:")
                    .ok_or("error: emu_set:dolphin:<setting>=<choice>")?;
                let done = match rest.split_once('=') {
                    _ if rest == "reset" => settings.reset_console(),
                    Some(("preset", "auto")) => settings.set_preset(None),
                    Some(("preset", id)) => settings.set_preset(Some(id)),
                    Some((id, choice)) => settings.set_console(id, choice),
                    None => return Err("error: emu_set:dolphin:<setting>=<choice>".into()),
                };
                done.map_err(|e| format!("error: {e}"))?;
                self.refresh_emulators();
                Ok("ok".into())
            }
            "emu_game_set" => {
                let (key, assign) = arg
                    .rsplit_once(':')
                    .ok_or("error: emu_game_set:<art key>:<setting>=<choice>")?;
                let (id, choice) = assign
                    .split_once('=')
                    .ok_or("error: emu_game_set:<art key>:<setting>=<choice>")?;
                let (_, game) = self.dolphin_game(key)?;
                settings
                    .set_game(&game.dir, id, Some(choice))
                    .map_err(|e| format!("error: {e}"))?;
                self.refresh_emulators();
                Ok("ok".into())
            }
            "emu_game_reset" => {
                // emu_game_reset:gc/GZ2E01 or emu_game_reset:gc/GZ2E01:<setting>
                let (key, id) = match arg.rsplit_once(':') {
                    Some((key, id)) if self.game_by_key(key).is_some() => (key, Some(id)),
                    _ => (arg, None),
                };
                let (_, game) = self.dolphin_game(key)?;
                match id {
                    Some(id) => settings.set_game(&game.dir, id, None),
                    None => settings.reset_game(&game.dir),
                }
                .map_err(|e| format!("error: {e}"))?;
                self.refresh_emulators();
                Ok("ok".into())
            }
            "emu_effective" => {
                let (game, game_ref) = self.dolphin_game(arg)?;
                let plans = self.dolphin.plans(Some(&game_ref));
                let args: Vec<String> = self
                    .dolphin
                    .command(&game.path)
                    .map(|c| {
                        c.args
                            .iter()
                            .map(|a| a.to_string_lossy().into_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(serde_json::json!({
                    "game": game.title,
                    "game_id": game_ref.id,
                    "user_dir": self.dolphin.root(),
                    "files": merge::describe(&plans),
                    "args": args,
                })
                .to_string())
            }
            _ => Err(format!("error: unknown word {name}")),
        })();
        result.unwrap_or_else(|e| e)
    }

    /// Updates whatever shows emulator settings (Settings → Emulators, the Game settings page)
    /// after a change.
    pub(super) fn refresh_emulators(&mut self) {
        self.fill_emulators();
        if let Some(key) = self.emu_open.clone() {
            self.fill_emulator_page(&key);
        }
        if let Some(id) = self.game_settings_open.clone() {
            self.fill_game_settings(&id);
        }
    }

    /// The Dolphin reference of a game by id.
    fn game_ref(&self, id: &GameId) -> Option<(Game, GameRef)> {
        let game = self.entries.iter().find(|e| &e.game.id == id)?.game.clone();
        let game_ref = GameRef::for_game(&game, &self.picks)?;
        Some((game, game_ref))
    }

    fn open_game_settings(&mut self, id: &GameId) {
        self.game_settings_open = Some(id.clone());
        self.fill_game_settings(id);
    }

    /// A game's Game settings page: its per-game rows, then "Reset to console defaults".
    fn fill_game_settings(&mut self, id: &GameId) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let home = app.global::<HomeData>();
        let Some((game, game_ref)) = self.game_ref(id) else {
            let title = self
                .entries
                .iter()
                .find(|e| &e.game.id == id)
                .map_or_else(String::new, |e| e.game.title.clone());
            home.set_game_settings_title(format!("{title} · no disc id, no game settings").into());
            home.set_game_settings_rows(model(Vec::new()));
            return;
        };
        let settings = self.dolphin.settings();
        let overrides = settings.game(&game_ref.dir);
        let shipped = self.dolphin.shipped_game_values(&game_ref.id);
        let mut rows: Vec<EmuRow> = settings
            .schema()
            .settings
            .iter()
            .filter(|s| s.scope == Scope::PerGame)
            .map(|s| {
                let (value, source) = game_value(settings, s, &overrides, &shipped);
                let mut row = setting_row(s, &value);
                row.marked = source == GameSource::Game;
                row.note = match source {
                    GameSource::Game => "Custom for this game".into(),
                    GameSource::Dolphin => "Set by Dolphin for this game".into(),
                    GameSource::Console => Default::default(),
                };
                row
            })
            .collect();
        rows.push(action_row(
            "reset",
            "Reset to console defaults",
            "Removes this game's own settings: it uses the console's again.",
        ));
        home.set_game_settings_title(format!("{} · Game settings", game.title).into());
        let rows_model = home.get_game_settings_rows();
        if rows_model.row_count() == rows.len() {
            for (i, row) in rows.into_iter().enumerate() {
                if rows_model.row_data(i).as_ref() != Some(&row) {
                    rows_model.set_row_data(i, row);
                }
            }
        } else {
            home.set_game_settings_rows(model(rows));
        }
        home.set_game_settings_note(self.applies_note(EmulatorKind::Dolphin).into());
    }

    /// Left/Right on a row of the Game settings page: the game gets its own value.
    fn game_setting_step(&mut self, key: &str, step: i32) -> bool {
        let Some((_, game_ref)) = self
            .game_settings_open
            .clone()
            .and_then(|id| self.game_ref(&id))
        else {
            return false;
        };
        let settings = self.dolphin.settings().clone();
        let Some(setting) = settings.schema().setting(key) else {
            return false;
        };
        let overrides = settings.game(&game_ref.dir);
        let shipped = self.dolphin.shipped_game_values(&game_ref.id);
        let (current, _) = game_value(&settings, setting, &overrides, &shipped);
        let Some(choice) = stepped(setting, &current, step) else {
            return false;
        };
        if let Err(err) = settings.set_game(&game_ref.dir, key, Some(choice)) {
            tracing::warn!("can't save the game's setting: {err}");
            self.toast("Couldn't save the setting");
            return false;
        }
        self.refresh_emulators();
        true
    }

    /// Removes one override of the open game (`key`), or all of them (`""`).
    fn game_setting_reset(&mut self, key: &str) {
        let Some((_, game_ref)) = self
            .game_settings_open
            .clone()
            .and_then(|id| self.game_ref(&id))
        else {
            return;
        };
        let settings = self.dolphin.settings();
        let done = if key.is_empty() {
            settings.reset_game(&game_ref.dir)
        } else {
            settings.set_game(&game_ref.dir, key, None)
        };
        if let Err(err) = done {
            tracing::warn!("can't reset the game's settings: {err}");
            self.toast("Couldn't reset the settings");
        }
        self.refresh_emulators();
    }

    /// The running (or frozen) game, if it runs in `kind`.
    pub(super) fn running_in(&self, kind: EmulatorKind) -> Option<&Game> {
        let active = self.active.as_ref()?;
        let emulator = self.systems.get(&active.game.system)?.emulator()?;
        (emulator.kind == kind).then_some(&active.game)
    }

    /// "Applies next launch", or "Restart <game> to apply" while a game of `kind` runs.
    pub(super) fn applies_note(&self, kind: EmulatorKind) -> String {
        self.running_in(kind).map_or_else(
            || "Applies next launch".to_owned(),
            |game| format!("Restart {} to apply", game.title),
        )
    }

    /// Settings → Emulators: the list and the category's status.
    pub(super) fn fill_emulators(&mut self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let dolphin = status(self.dolphin.settings());
        let rows: Vec<SettingsCategory> = EMULATORS
            .iter()
            .map(|(key, label)| SettingsCategory {
                key: (*key).into(),
                label: (*label).into(),
                status: if *key == "dolphin" {
                    dolphin.as_str().into()
                } else {
                    Default::default()
                },
            })
            .collect();
        app.global::<SettingsData>().set_emulators(model(rows));
        set_category_status(&app, "emulators", &format!("Dolphin · {dolphin}"));
    }

    fn open_emulator(&mut self, key: &str) {
        self.emu_open = Some(key.to_owned());
        self.fill_emulator_page(key);
    }

    /// An emulator's page: Dolphin's preset, rows and Reset; RetroArch's message.
    fn fill_emulator_page(&mut self, key: &str) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let data = app.global::<SettingsData>();
        let label = EMULATORS
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(key, |(_, l)| l);
        data.set_emu_title(label.into());
        if key != "dolphin" {
            data.set_emu_rows(model(Vec::new()));
            data.set_emu_message("More settings coming soon.".into());
            return;
        }
        let settings = self.dolphin.settings();
        let shared = settings.shared();
        let resolved = settings.resolved(&shared);
        let mut rows = vec![preset_row(settings)];
        rows.extend(
            settings
                .schema()
                .settings
                .iter()
                .map(|s| setting_row(s, &resolved.console(s).0)),
        );
        rows.push(action_row(
            "reset",
            "Reset to defaults",
            "Every setting goes back to the preset recommended for this box.",
        ));
        let rows_model = data.get_emu_rows();
        if rows_model.row_count() == rows.len() {
            // Same rows: update in place, so focus and scrolling stay put.
            for (i, row) in rows.into_iter().enumerate() {
                if rows_model.row_data(i).as_ref() != Some(&row) {
                    rows_model.set_row_data(i, row);
                }
            }
        } else {
            data.set_emu_rows(model(rows));
        }
        data.set_emu_note(self.applies_note(EmulatorKind::Dolphin).into());
    }

    /// Left/Right on a row of the open emulator page; false at the first or last choice.
    fn emu_step(&mut self, key: &str, step: i32) -> bool {
        if self.emu_open.as_deref() != Some("dolphin") {
            return false;
        }
        let settings = self.dolphin.settings().clone();
        let result = if key == "preset" {
            let shared = settings.shared();
            let current = settings
                .resolved(&shared)
                .preset()
                .map(|p| p.id.clone())
                .unwrap_or_default();
            let presets = &settings.schema().presets;
            let at = presets.iter().position(|p| p.id == current).unwrap_or(0);
            let custom = settings.resolved(&shared).is_custom();
            // From Custom, a step goes to the next preset, or (at the end of the list)
            // back to the one the rows came from.
            let next = at
                .checked_add_signed(step as isize)
                .filter(|to| *to < presets.len());
            let to = if custom { next.or(Some(at)) } else { next };
            match to {
                Some(to) => settings.set_preset(Some(&presets[to].id)),
                None => return false,
            }
        } else {
            let Some(setting) = settings.schema().setting(key) else {
                return false;
            };
            let shared = settings.shared();
            let current = settings.resolved(&shared).console(setting).0;
            match stepped(setting, &current, step) {
                Some(choice) => settings.set_console(key, choice),
                None => return false,
            }
        };
        if let Err(err) = result {
            tracing::warn!("can't save the emulator settings: {err}");
            self.toast("Couldn't save the setting");
            return false;
        }
        self.refresh_emulators();
        true
    }

    /// Reset to defaults (confirmed on the page).
    fn emu_reset(&mut self) {
        if let Err(err) = self.dolphin.settings().reset_console() {
            tracing::warn!("can't reset the emulator settings: {err}");
            self.toast("Couldn't reset the settings");
        }
        self.refresh_emulators();
    }
}
