//! Emulator settings in the shell: Settings → Emulators, a game's Game settings page, and their
//! dev socket words. The values live in `hearth_core::emusettings`; they reach the emulator at
//! its next launch (`DolphinProfile::prepare`).

use hearth_core::emusettings::merge;
use hearth_core::model::Game;
use hearth_core::profiles::dolphin::GameRef;
use hearth_core::systems::EmulatorKind;

use super::Controller;

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
    pub(super) fn refresh_emulators(&mut self) {}
}
