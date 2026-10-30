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
    /// A game's Widescreen row (Dolphin): above the game's overrides for the keys it sets. A
    /// value changed outside Hearth while it's on is left to the user (not adopted into a
    /// lower layer, which Widescreen would cover at the next launch) until it's turned off.
    Widescreen,
}

impl Layer {
    fn name(self) -> &'static str {
        match self {
            Self::Console => "console",
            Self::Game => "game",
            Self::Widescreen => "widescreen",
        }
    }
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
    /// (section, key, value) Hearth sets while it needs them (outside the schema, like
    /// Dolphin's `Core.EnableCheats` for a Widescreen code). Gone from the plan, a key goes if
    /// unchanged; changed outside Hearth, it's left to the user while still planned.
    pub keys: Vec<(String, String, String)>,
    /// (section, line) entries Hearth adds while it needs them (`[Gecko_Enabled]`
    /// `$<code>`). Gone from the plan, a line goes only if Hearth added it; removed by the user
    /// while planned, it isn't added back.
    pub lines: Vec<(String, String)>,
    /// Whether a missing key means the emulator's default (its main files), or only "not set
    /// here" (a game INI: lower layers, Dolphin's shipped game fixes included, then apply).
    pub missing_is_default: bool,
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
            keys: Vec::new(),
            lines: Vec::new(),
            missing_is_default: true,
        }
    }

    /// A plan for a game INI, where a missing key isn't the default.
    pub fn game(rel: impl Into<String>, style: Style) -> Self {
        Self {
            missing_is_default: false,
            ..Self::new(rel, style)
        }
    }

    /// Adds a locked key.
    pub fn lock(&mut self, section: &str, key: &str, value: impl Into<String>) {
        self.locked
            .push((section.to_owned(), key.to_owned(), value.into()));
    }
}

/// What `plans` would write, as JSON (the dev socket's `emu_effective`): per file its locked
/// keys, the settings with the keys they set and their layer, and the keys removed.
pub fn describe(plans: &[FilePlan<'_>]) -> serde_json::Value {
    let files: Vec<serde_json::Value> = plans
        .iter()
        .map(|p| {
            let settings: Vec<serde_json::Value> = p
                .settings
                .iter()
                .map(|w| {
                    let keys: serde_json::Map<String, serde_json::Value> = w
                        .setting
                        .keys_for(&w.choice)
                        .into_iter()
                        .map(|(k, v)| (k.to_owned(), v.into()))
                        .collect();
                    serde_json::json!({
                        "id": w.setting.id,
                        "section": w.section,
                        "choice": w.choice,
                        "keys": keys,
                        "layer": w.layer.name(),
                    })
                })
                .collect();
            let locked: Vec<String> = p
                .locked
                .iter()
                .map(|(s, k, v)| format!("[{s}] {k} = {v}"))
                .collect();
            let strip: Vec<String> = p.strip.iter().map(|(s, k)| format!("[{s}] {k}")).collect();
            let keys: Vec<String> = p
                .keys
                .iter()
                .map(|(s, k, v)| format!("[{s}] {k} = {v}"))
                .collect();
            let lines: Vec<String> = p.lines.iter().map(|(s, l)| format!("[{s}] {l}")).collect();
            serde_json::json!({
                "file": p.rel,
                "locked": locked,
                "settings": settings,
                "keys": keys,
                "lines": lines,
                "removed_if_present": strip,
            })
        })
        .collect();
    serde_json::Value::Array(files)
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
    /// Settings changed outside Hearth to a value Hearth has no choice for (or removed from a
    /// game INI): left to the user until they are changed in Hearth.
    pub foreign: Vec<String>,
    /// Locked keys that held another value (`file [section] key`).
    pub restored: Vec<String>,
    /// Values changed outside Hearth over a layer that doesn't take them back (Widescreen's
    /// keys and lines): kept in the file, not stored (`file [section] key`).
    pub kept: Vec<String>,
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
        for key in &self.kept {
            tracing::info!(emulator, %key, "kept a value changed outside Hearth");
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
        let old_yielded = applied.yielded.remove(&plan.rel).unwrap_or_default();
        let old_lines = applied.lines.remove(&plan.rel).unwrap_or_default();
        let mut lines_rec: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        let mut yielded = std::collections::BTreeMap::new();
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

        for (section, key, value) in &plan.keys {
            let id = format!("key:{section}/{key}");
            let changed = || {
                was(section, key).is_some_and(|then| {
                    doc.get(section, key)
                        .is_none_or(|have| !same_value(None, have, then))
                })
            };
            if old_yielded.get(&id) == Some(value) || changed() {
                if !old_yielded.contains_key(&id) {
                    report.kept.push(format!("{} [{section}] {key}", plan.rel));
                }
                yielded.insert(id, value.clone());
                continue;
            }
            if !doc
                .get(section, key)
                .is_some_and(|have| same_value(None, have, value))
            {
                doc.set(section, key, value);
            }
            rec.entry(section.clone())
                .or_default()
                .insert(key.clone(), value.clone());
        }

        let added_before = |section: &str, line: &str| {
            old_lines
                .iter()
                .any(|(s, ls)| s.eq_ignore_ascii_case(section) && ls.iter().any(|l| l == line))
        };
        for (section, line) in &plan.lines {
            let id = format!("line:{section}/{line}");
            if old_yielded.contains_key(&id) {
                yielded.insert(id, "on".into());
                continue;
            }
            let present = doc.has_line(section, line);
            if added_before(section, line) && !present {
                // Removed by the user while Hearth wanted it: theirs.
                report.kept.push(format!("{} [{section}] {line}", plan.rel));
                yielded.insert(id, "on".into());
                continue;
            }
            if present && !added_before(section, line) {
                // The user's own line: not Hearth's to remove later.
                continue;
            }
            doc.add_line(section, line);
            lines_rec
                .entry(section.clone())
                .or_default()
                .push(line.clone());
        }
        // Lines Hearth added and no longer wants.
        for (section, lines) in &old_lines {
            for line in lines {
                let wanted = plan
                    .lines
                    .iter()
                    .any(|(s, l)| s.eq_ignore_ascii_case(section) && l == line);
                if !wanted {
                    doc.remove_line(section, line);
                }
            }
        }

        for want in &plan.settings {
            let s = want.setting;
            let section = want.section.as_str();
            let fallback = plan.missing_is_default;
            let in_file = s.read_with(|k| doc.get(section, k).map(str::to_owned), fallback);
            let target = if want.layer == Layer::Widescreen
                && old_yielded.get(&s.id) == Some(&want.choice)
            {
                // Changed by the user while Widescreen was on: theirs until it's turned off.
                yielded.insert(s.id.clone(), want.choice.clone());
                continue;
            } else if let Some(then) = old_yielded.get(&s.id) {
                // Left to the user last time: still theirs unless Hearth's value changed since,
                // or the file now holds a value Hearth offers.
                match in_file {
                    _ if *then != want.choice => want.choice.clone(),
                    Some(choice) => {
                        if choice != want.choice {
                            report.adopted.push(Adopted {
                                setting: s.id.clone(),
                                layer: want.layer,
                                choice: choice.to_owned(),
                            });
                        }
                        choice.to_owned()
                    }
                    None => {
                        yielded.insert(s.id.clone(), then.clone());
                        continue;
                    }
                }
            } else if s.all_keys().iter().any(|k| was(section, k).is_some())
                && in_file != s.read_with(|k| was(section, k).map(str::to_owned), fallback)
            {
                // Changed outside Hearth since Hearth wrote it.
                match in_file {
                    _ if want.layer == Layer::Widescreen => {
                        report
                            .kept
                            .push(format!("{} [{section}] {}", plan.rel, s.key));
                        yielded.insert(s.id.clone(), want.choice.clone());
                        continue;
                    }
                    Some(choice) => {
                        report.adopted.push(Adopted {
                            setting: s.id.clone(),
                            layer: want.layer,
                            choice: choice.to_owned(),
                        });
                        choice.to_owned()
                    }
                    None => {
                        report.foreign.push(s.id.clone());
                        yielded.insert(s.id.clone(), want.choice.clone());
                        continue;
                    }
                }
            } else {
                want.choice.clone()
            };
            for (key, value) in s.keys_for(&target) {
                let keep = match doc.get(section, key) {
                    Some(have) => same_value(Some(kind_of(s, key)), have, value),
                    // In a main file a missing key already means the emulator's default.
                    None => {
                        fallback
                            && s.default_value(key)
                                .is_some_and(|d| same_value(Some(kind_of(s, key)), d, value))
                    }
                };
                if !keep {
                    doc.set(section, key, value);
                }
                rec.entry(section.to_owned())
                    .or_default()
                    .insert(key.to_owned(), value.to_owned());
            }
        }

        // Keys Hearth wrote before and doesn't manage any more (keys of a setting left to the
        // user stay theirs).
        let yielded_keys: Vec<(&str, &str)> = plan
            .settings
            .iter()
            .filter(|w| yielded.contains_key(&w.setting.id))
            .flat_map(|w| {
                w.setting
                    .all_keys()
                    .into_iter()
                    .map(move |k| (w.section.as_str(), k))
            })
            .chain(
                plan.keys
                    .iter()
                    .filter(|(s, k, _)| yielded.contains_key(&format!("key:{s}/{k}")))
                    .map(|(s, k, _)| (s.as_str(), k.as_str())),
            )
            .collect();
        for (section, keys) in &old {
            for (key, value) in keys {
                let managed = rec.iter().any(|(s, ks)| {
                    s.eq_ignore_ascii_case(section)
                        && ks.keys().any(|k| k.eq_ignore_ascii_case(key))
                }) || yielded_keys
                    .iter()
                    .any(|(s, k)| s.eq_ignore_ascii_case(section) && k.eq_ignore_ascii_case(key));
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
        if !yielded.is_empty() {
            applied.yielded.insert(plan.rel.clone(), yielded);
        }
        if !lines_rec.is_empty() {
            applied.lines.insert(plan.rel.clone(), lines_rec);
        }
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
        let mut per_game = FilePlan::game(GAME, Style::Ini);
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
        // Still the user's at the next launches…
        assert_eq!(run(dir.path(), &schema, &[]), Report::default());
        assert_eq!(read(dir.path(), GFX), gfx);
        // …until it is changed in Hearth.
        let mut applied = Applied::load(dir.path());
        let mut p = plans(&schema, &[]);
        p[1].settings[0].choice = "3".into();
        merge(dir.path(), &p, &mut applied).unwrap();
        assert!(read(dir.path(), GFX).contains("InternalResolution = 3"));
        assert!(applied.yielded.is_empty());
    }

    #[test]
    fn a_game_value_equal_to_the_default_is_still_written() {
        // Missing from a game INI means "not set here": Dolphin's shipped fixes would apply.
        let dir = tempdir();
        let schema = Schema::dolphin();
        run(dir.path(), &schema, &[("arbitrary_mipmaps", "False")]);
        assert_eq!(
            read(dir.path(), GAME),
            "[Video_Enhancements]\nArbitraryMipmapDetection = False\n"
        );
        // The user removes it in Dolphin: theirs now, not written back.
        fs::write(dir.path().join(GAME), "[Video_Enhancements]\n").unwrap();
        let report = run(dir.path(), &schema, &[("arbitrary_mipmaps", "False")]);
        assert_eq!(report.foreign, ["arbitrary_mipmaps"]);
        assert_eq!(read(dir.path(), GAME), "[Video_Enhancements]\n");
    }

    /// TP's game INI plan with per-game Stretch, with or without the Widescreen layer.
    fn widescreen_plan(schema: &Schema, on: bool) -> Vec<FilePlan<'_>> {
        let aspect = schema.setting("aspect_ratio").unwrap();
        let hack = schema.setting("widescreen_hack").unwrap();
        let mut game = FilePlan::game(GAME, Style::Ini);
        let (aspect_choice, layer) = if on {
            ("1", Layer::Widescreen)
        } else {
            ("3", Layer::Game)
        };
        game.settings.push(Wanted {
            setting: aspect,
            section: "Video_Settings".into(),
            choice: aspect_choice.into(),
            layer,
        });
        if on {
            game.settings.push(Wanted {
                setting: hack,
                section: "Video_Settings".into(),
                choice: "False".into(),
                layer: Layer::Widescreen,
            });
            game.keys
                .push(("Core".into(), "EnableCheats".into(), "True".into()));
            game.lines
                .push(("Gecko_Enabled".into(), "$16:9 Widescreen v2".into()));
        }
        vec![game]
    }

    fn run_ws(dir: &Path, schema: &Schema, on: bool) -> Report {
        let mut applied = Applied::load(dir);
        let report = merge(dir, &widescreen_plan(schema, on), &mut applied).unwrap();
        applied.save(dir).unwrap();
        report
    }

    #[test]
    fn widescreen_on_then_off_brings_the_games_stretch_back() {
        let dir = tempdir();
        let schema = Schema::dolphin();
        fs::create_dir_all(dir.path().join("GameSettings")).unwrap();
        // The user's own enabled code stays whatever happens.
        fs::write(
            dir.path().join(GAME),
            "[Gecko_Enabled]\n$Mine\n[Core]\nCPUThread = False\n",
        )
        .unwrap();
        run_ws(dir.path(), &schema, false);
        assert!(read(dir.path(), GAME).contains("AspectRatio = 3"));
        let report = run_ws(dir.path(), &schema, true);
        assert!(
            report.adopted.is_empty() && report.kept.is_empty(),
            "{report:?}"
        );
        assert_golden(
            Path::new("emusettings/merge/widescreen/on.ini"),
            &read(dir.path(), GAME),
        );
        let report = run_ws(dir.path(), &schema, false);
        assert!(
            report.adopted.is_empty() && report.kept.is_empty(),
            "{report:?}"
        );
        assert_eq!(
            read(dir.path(), GAME),
            "[Gecko_Enabled]\n$Mine\n[Core]\nCPUThread = False\n[Video_Settings]\nAspectRatio = 3\n"
        );
    }

    #[test]
    fn a_change_made_while_widescreen_is_on_is_kept() {
        let dir = tempdir();
        let schema = Schema::dolphin();
        run_ws(dir.path(), &schema, true);
        // In Dolphin's game properties: another aspect ratio, cheats off, the code unticked.
        let text = read(dir.path(), GAME)
            .replace("AspectRatio = 1", "AspectRatio = 2")
            .replace("EnableCheats = True", "EnableCheats = False")
            .replace("$16:9 Widescreen v2\n", "");
        fs::write(dir.path().join(GAME), &text).unwrap();
        let report = run_ws(dir.path(), &schema, true);
        assert!(report.adopted.is_empty(), "{report:?}");
        assert_eq!(report.kept.len(), 3, "{report:?}");
        assert_eq!(read(dir.path(), GAME), text);
        // Still theirs at the next launch, and nothing more is logged.
        assert_eq!(run_ws(dir.path(), &schema, true), Report::default());
        assert_eq!(read(dir.path(), GAME), text);
        // Turned off in Hearth: the normal layers supply the aspect ratio again.
        run_ws(dir.path(), &schema, false);
        let off = read(dir.path(), GAME);
        assert!(off.contains("AspectRatio = 3"), "{off}");
        assert!(
            off.contains("EnableCheats = False"),
            "the user's value stays: {off}"
        );
    }
}
