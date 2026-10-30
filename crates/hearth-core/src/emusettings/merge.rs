//! Merging Hearth's keys into an emulator's own config files at launch.
//!
//! Files are merged, never regenerated: only the keys Hearth manages are touched, every other
//! line stays as it was. For each file Hearth remembers what it wrote (`Applied`), which tells
//! a value the user changed in the emulator (or by hand) from one Hearth wrote:
//!
//! - a **locked** key is always set to Hearth's value;
//! - a **setting** (from the schema) gets the value Hearth wants, unless the file no longer
//!   holds what Hearth applied there: then the user changed it, the file keeps it, and it is
//!   reported as adopted, so the caller stores it in the layer it came from;
//! - a key Hearth applied before but no longer manages is removed if it still holds Hearth's
//!   value, otherwise left to the user.
//!
//! "Holds what Hearth applied" compares meanings, not text, because Dolphin rewrites its files
//! when it quits: values are compared as the emulator reads them (`True` = `true`, `2` =
//! `2.0`, see `schema::same_value`), a missing key counts as the emulator's default, and the
//! order of lines doesn't matter (lookups are by section and key).

use std::path::Path;

use super::doc::{ConfigDoc, Style};
use super::schema::{Kind, Setting, same_value};
use super::store::Applied;

/// Which layer a setting's value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// The console base (shared by every profile).
    Console,
    /// The game's own overrides.
    Game,
}

/// One schema setting Hearth wants in a file.
#[derive(Debug, Clone)]
pub struct Wanted<'a> {
    pub setting: &'a Setting,
    /// The section in this file (a game INI's differs from the setting's own).
    pub section: String,
    /// The choice id to write.
    pub choice: String,
    pub layer: Layer,
}

/// What Hearth wants in one config file.
#[derive(Debug, Clone)]
pub struct FilePlan<'a> {
    /// Path relative to the emulator folder (the key in `Applied`).
    pub rel: String,
    pub style: Style,
    /// (section, key, value) always written.
    pub locked: Vec<(String, String, String)>,
    pub settings: Vec<Wanted<'a>>,
    /// (section, key) removed if present (locked keys that mustn't be overridden here).
    pub strip: Vec<(String, String)>,
}

impl<'a> FilePlan<'a> {
    /// An empty plan for `rel`.
    pub fn new(rel: impl Into<String>, style: Style) -> Self {
        Self {
            rel: rel.into(),
            style,
            locked: Vec::new(),
            settings: Vec::new(),
            strip: Vec::new(),
        }
    }

    /// Adds a locked key.
    pub fn lock(&mut self, section: &str, key: &str, value: impl Into<String>) {
        self.locked
            .push((section.to_owned(), key.to_owned(), value.into()));
    }
}

/// A setting the user changed outside Hearth, kept in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adopted {
    pub setting: String,
    pub layer: Layer,
    /// The choice the file holds now.
    pub choice: String,
}

/// What a merge did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Settings changed outside Hearth whose new value Hearth takes over.
    pub adopted: Vec<Adopted>,
    /// Settings changed outside Hearth to a value Hearth has no choice for: left alone.
    pub foreign: Vec<String>,
    /// Locked keys that held another value (`file [section] key`).
    pub restored: Vec<String>,
    /// Files whose text changed.
    pub written: Vec<String>,
}

impl Report {
    /// Logs what happened (only real changes by the user, never a rewrite by the emulator).
    pub fn log(&self, emulator: &str) {
        for a in &self.adopted {
            tracing::info!(
                emulator,
                setting = %a.setting,
                choice = %a.choice,
                "kept a setting changed outside Hearth"
            );
        }
        for id in &self.foreign {
            tracing::info!(emulator, setting = %id, "setting has a value Hearth doesn't offer; left as is");
        }
        for key in &self.restored {
            tracing::info!(emulator, %key, "restored a key Hearth owns");
        }
    }
}

/// Merges `plans` into the files under `dir`, updating `applied`. Nothing is written for a
/// file whose text wouldn't change.
pub fn merge(dir: &Path, plans: &[FilePlan<'_>], applied: &mut Applied) -> std::io::Result<Report> {
    let mut report = Report::default();
    for plan in plans {
        let path = dir.join(&plan.rel);
        let before = ConfigDoc::load(&path, plan.style)?;
        let mut doc = before.clone();
        let old = applied.files.remove(&plan.rel).unwrap_or_default();
        let mut record = Applied::default();
        let rec = record.files.entry(plan.rel.clone()).or_default();
        let was = |s: &str, k: &str| -> Option<&str> {
            old.iter()
                .find(|(sec, _)| sec.eq_ignore_ascii_case(s))
                .and_then(|(_, keys)| {
                    keys.iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case(k))
                        .map(|(_, v)| v.as_str())
                })
        };

        for (section, key, value) in &plan.locked {
            match doc.get(section, key) {
                Some(have) if same_value(None, have, value) => {}
                have => {
                    if have.is_some() {
                        report
                            .restored
                            .push(format!("{} [{section}] {key}", plan.rel));
                    }
                    doc.set(section, key, value);
                }
            }
            rec.entry(section.clone())
                .or_default()
                .insert(key.clone(), value.clone());
        }

        for want in &plan.settings {
            let s = want.setting;
            let section = want.section.as_str();
            let keys = s.all_keys();
            let applied_before = keys.iter().any(|k| was(section, k).is_some());
            let in_file = s.read(|k| doc.get(section, k).map(str::to_owned));
            let hearth_wrote = s.read(|k| was(section, k).map(str::to_owned));
            let target = if applied_before && in_file != hearth_wrote {
                match in_file {
                    Some(choice) => {
                        report.adopted.push(Adopted {
                            setting: s.id.clone(),
                            layer: want.layer,
                            choice: choice.to_owned(),
                        });
                        choice.to_owned()
                    }
                    None => {
                        // Not a value Hearth offers: the user's; Hearth stops managing it
                        // until it is changed in Hearth again.
                        report.foreign.push(s.id.clone());
                        continue;
                    }
                }
            } else {
                want.choice.clone()
            };
            for (key, value) in s.keys_for(&target) {
                // A missing key already means the emulator's default.
                let keep = match doc.get(section, key) {
                    Some(have) => same_value(Some(kind_of(s, key)), have, value),
                    None => s
                        .default_value(key)
                        .is_some_and(|d| same_value(Some(kind_of(s, key)), d, value)),
                };
                if !keep {
                    doc.set(section, key, value);
                }
                rec.entry(section.to_owned())
                    .or_default()
                    .insert(key.to_owned(), value.to_owned());
            }
        }

        // Keys Hearth wrote before and doesn't manage any more.
        for (section, keys) in &old {
            for (key, value) in keys {
                let managed = rec.iter().any(|(s, ks)| {
                    s.eq_ignore_ascii_case(section)
                        && ks.keys().any(|k| k.eq_ignore_ascii_case(key))
                });
                if managed {
                    continue;
                }
                if doc
                    .get(section, key)
                    .is_none_or(|have| same_value(None, have, value))
                {
                    doc.remove(section, key);
                }
            }
        }

        for (section, key) in &plan.strip {
            if doc.remove(section, key) {
                report
                    .restored
                    .push(format!("{} [{section}] {key}", plan.rel));
            }
        }

        if doc != before {
            doc.save(&path)?;
            report.written.push(plan.rel.clone());
        }
        applied.files.extend(record.files);
    }
    Ok(report)
}

/// The kind used to compare `key` of `setting`: its own key has the setting's kind; the
/// extra keys a choice sets are compared untyped.
fn kind_of(setting: &Setting, key: &str) -> Kind {
    if key == setting.key {
        setting.kind
    } else {
        Kind::Choice
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::emusettings::schema::Schema;
    use crate::profiles::input::assert_golden;
    use crate::testdir::tempdir;

    const GFX: &str = "Config/GFX.ini";
    const MAIN: &str = "Config/Dolphin.ini";
    const GAME: &str = "GameSettings/GZ2E01.ini";

    /// Hearth's plan for the tests: a few locked keys and the Balanced rows.
    fn plans<'a>(schema: &'a Schema, game: &[(&str, &str)]) -> Vec<FilePlan<'a>> {
        let mut main = FilePlan::new(MAIN, Style::Ini);
        main.lock("Display", "Fullscreen", "True");
        main.lock("Interface", "ConfirmStop", "False");
        main.lock("Analytics", "Enabled", "False");
        main.lock("Core", "SIDevice1", "0");
        main.settings.push(Wanted {
            setting: schema.setting("backend").unwrap(),
            section: "Core".into(),
            choice: "OGL".into(),
            layer: Layer::Console,
        });
        let mut gfx = FilePlan::new(GFX, Style::Ini);
        for (id, choice) in [
            ("internal_resolution", "2"),
            ("vsync", "False"),
            ("anti_aliasing", "1"),
            ("anisotropic_filtering", "2"),
            ("shader_compilation", "3"),
            ("arbitrary_mipmaps", "True"),
        ] {
            let setting = schema.setting(id).unwrap();
            gfx.settings.push(Wanted {
                setting,
                section: setting.section.clone(),
                choice: choice.into(),
                layer: Layer::Console,
            });
        }
        let mut per_game = FilePlan::new(GAME, Style::Ini);
        for (id, choice) in game {
            let setting = schema.setting(id).unwrap();
            per_game.settings.push(Wanted {
                setting,
                section: crate::emusettings::schema::game_section(setting.file, &setting.section)
                    .unwrap(),
                choice: (*choice).into(),
                layer: Layer::Game,
            });
        }
        per_game.strip.push(("Display".into(), "Fullscreen".into()));
        vec![main, gfx, per_game]
    }

    fn run(dir: &Path, schema: &Schema, game: &[(&str, &str)]) -> Report {
        let mut applied = Applied::load(dir);
        let report = merge(dir, &plans(schema, game), &mut applied).unwrap();
        applied.save(dir).unwrap();
        report
    }

    fn read(dir: &Path, rel: &str) -> String {
        fs::read_to_string(dir.join(rel)).unwrap_or_default()
    }

    fn golden(name: &str, dir: &Path) {
        for rel in [MAIN, GFX, GAME] {
            let file = rel.rsplit('/').next().unwrap();
            assert_golden(
                &PathBuf::from(format!("emusettings/merge/{name}/{file}")),
                &read(dir, rel),
            );
        }
    }

    #[test]
    fn fresh_files_get_hearths_keys() {
        let dir = tempdir();
        let schema = Schema::dolphin();
        let report = run(dir.path(), &schema, &[]);
        assert!(report.adopted.is_empty() && report.restored.is_empty());
        assert_eq!(report.written, [MAIN, GFX]);
        golden("fresh", dir.path());
        // A second launch changes nothing.
        let again = run(dir.path(), &schema, &[]);
        assert_eq!(again, Report::default());
    }

    #[test]
    fn user_keys_survive_and_locked_keys_are_restored() {
        let dir = tempdir();
        let schema = Schema::dolphin();
        fs::create_dir_all(dir.path().join("Config")).unwrap();
        let mine = "# my notes\n[Display]\nFullscreen = False\n[Interface]\nShowFPS = True\n\
                    [Analytics]\nID = abc\n[SDL_Hints]\nSDL_JOYSTICK_WGI = 0\n";
        fs::write(dir.path().join(MAIN), mine).unwrap();
        let report = run(dir.path(), &schema, &[]);
        assert_eq!(report.restored, ["Config/Dolphin.ini [Display] Fullscreen"]);
        golden("user_keys", dir.path());
        let text = read(dir.path(), MAIN);
        for kept in [
            "# my notes",
            "ShowFPS = True",
            "ID = abc",
            "SDL_JOYSTICK_WGI = 0",
        ] {
            assert!(text.contains(kept), "{kept}");
        }

        // Hand edits after a launch: a user key, a locked key, a setting.
        let edited = read(dir.path(), MAIN)
            .replace("ShowFPS = True", "ShowFPS = False")
            .replace("ConfirmStop = False", "ConfirmStop = True");
        fs::write(dir.path().join(MAIN), &edited).unwrap();
        let gfx = read(dir.path(), GFX).replace("InternalResolution = 2", "InternalResolution = 4");
        fs::write(dir.path().join(GFX), gfx).unwrap();
        let report = run(dir.path(), &schema, &[]);
        assert_eq!(
            report.restored,
            ["Config/Dolphin.ini [Interface] ConfirmStop"]
        );
        assert_eq!(
            report.adopted,
            [Adopted {
                setting: "internal_resolution".into(),
                layer: Layer::Console,
                choice: "4".into()
            }]
        );
        let text = read(dir.path(), MAIN);
        assert!(text.contains("ShowFPS = False"));
        assert!(text.contains("ConfirmStop = False"));
        assert!(read(dir.path(), GFX).contains("InternalResolution = 4"));
        // Adopted: the next launch (with the store updated by the caller) is quiet.
        let mut applied = Applied::load(dir.path());
        let mut p = plans(&schema, &[]);
        p[1].settings[0].choice = "4".into();
        assert_eq!(
            merge(dir.path(), &p, &mut applied).unwrap(),
            Report::default()
        );
    }

    /// What Dolphin 2609 does to its files (measured on 2026-10-30: TP started and quit with
    /// SIGTERM): at startup comments are dropped and an `[Analytics] ID` is added; on exit
    /// sections of its own defaults are appended (`[DSP]`, `[SDL_Hints]`, every log type in
    /// Logger.ini). Spellings, order and default-valued keys were kept, but other versions may
    /// change them, so this also re-spells, reorders and drops keys at their defaults. None of
    /// it is a change by the user.
    fn dolphin_rewrite(text: &str) -> String {
        let mut sections: Vec<(String, Vec<String>)> = Vec::new();
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with('#') || t.starts_with(';') || t.is_empty() {
                continue;
            }
            if t.starts_with('[') {
                sections.push((t.to_owned(), Vec::new()));
            } else if let Some((_, lines)) = sections.last_mut() {
                let line = t
                    .replace("= True", "= true")
                    .replace("= False", "= false")
                    .replace("InternalResolution = 2", "InternalResolution = 2.0");
                // Keys at Dolphin's default value.
                if !["MSAA = 1", "SSAA = false", "VSync = false", "SIDevice1 = 0"]
                    .contains(&line.as_str())
                {
                    lines.push(line);
                }
            }
        }
        sections.reverse();
        if text.contains("[Display]") {
            sections.push(("[DSP]".into(), vec!["DSPThread = True".into()]));
        }
        let mut out = String::new();
        for (header, mut lines) in sections {
            if header == "[Analytics]" {
                lines.push("ID = 7f1bf52a16c4cbefe864770ed5d93f43".into());
            }
            lines.reverse();
            out.push_str(&header);
            out.push('\n');
            for line in lines {
                out.push_str(&line);
                out.push('\n');
            }
        }
        out
    }

    #[test]
    fn a_rewrite_by_dolphin_changes_nothing() {
        let dir = tempdir();
        let schema = Schema::dolphin();
        let game = [("internal_resolution", "3"), ("vsync", "True")];
        run(dir.path(), &schema, &game);
        for rel in [MAIN, GFX, GAME] {
            let text = dolphin_rewrite(&read(dir.path(), rel));
            fs::write(dir.path().join(rel), text).unwrap();
        }
        let report = run(dir.path(), &schema, &game);
        assert!(report.adopted.is_empty(), "{report:?}");
        assert!(report.foreign.is_empty(), "{report:?}");
        assert!(report.restored.is_empty(), "{report:?}");
        // Nothing is written: keys Dolphin left out hold its defaults, which is what Hearth
        // wants there; the locked one (no schema default) comes back.
        golden("dolphin_rewrite", dir.path());
    }

    #[test]
    fn per_game_values_and_stale_keys() {
        let dir = tempdir();
        let schema = Schema::dolphin();
        run(dir.path(), &schema, &[("internal_resolution", "4")]);
        let game = read(dir.path(), GAME);
        assert_eq!(game, "[Video_Settings]\nInternalResolution = 4\n");
        // The user adds a cheat section and a key of their own in Dolphin; the override goes.
        fs::write(
            dir.path().join(GAME),
            format!("{game}Fullscreen = False\n[ActionReplay_Enabled]\n$Infinite Health\n[Display]\nFullscreen = False\n"),
        )
        .unwrap();
        let report = run(dir.path(), &schema, &[]);
        // A locked key in the game INI (it would beat the command line) is removed.
        assert_eq!(
            report.restored,
            ["GameSettings/GZ2E01.ini [Display] Fullscreen"]
        );
        assert_eq!(
            read(dir.path(), GAME),
            "[Video_Settings]\nFullscreen = False\n[ActionReplay_Enabled]\n$Infinite Health\n[Display]\n"
        );
        // A stale key the user changed stays theirs.
        run(dir.path(), &schema, &[("vsync", "True")]);
        let text = read(dir.path(), GAME).replace("VSync = True", "VSync = False");
        fs::write(dir.path().join(GAME), text).unwrap();
        let report = run(dir.path(), &schema, &[]);
        assert!(report.adopted.is_empty());
        assert!(read(dir.path(), GAME).contains("VSync = False"));
    }

    #[test]
    fn values_hearth_doesnt_offer_are_left_alone() {
        let dir = tempdir();
        let schema = Schema::dolphin();
        run(dir.path(), &schema, &[]);
        let gfx = read(dir.path(), GFX).replace("InternalResolution = 2", "InternalResolution = 5");
        fs::write(dir.path().join(GFX), &gfx).unwrap();
        let report = run(dir.path(), &schema, &[]);
        assert_eq!(report.foreign, ["internal_resolution"]);
        assert_eq!(read(dir.path(), GFX), gfx);
    }
}
