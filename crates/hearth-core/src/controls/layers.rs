//! Resolving a game's mapping: shipped default → user system override → user game override.
//! Every value remembers which layer set it.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::file::{Layer, Problem, StickChanges, parse_layer};
use super::key::GameKey;
use super::model::{Mapping, StickOptions, Target};
use crate::systems::System;

/// Where a value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Built-in default (no shipped file for this system).
    Builtin,
    /// The shipped default, `data/controls/<system>.toml`.
    Default(String),
    /// The user's override for every game of the system.
    System(PathBuf),
    /// The user's override for this game.
    Game(PathBuf),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin => f.write_str("built-in"),
            Self::Default(path) => write!(f, "default ({path})"),
            Self::System(path) => write!(f, "system ({})", path.display()),
            Self::Game(path) => write!(f, "game ({})", path.display()),
        }
    }
}

/// A resolved mapping with the origin of each value.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// The mapping.
    pub mapping: Mapping,
    /// Setting path (`bind.south`, `right_stick.invert_x`, …) → the layer that set it.
    pub sources: BTreeMap<String, Source>,
    /// Layers that were applied, in order.
    pub applied: Vec<Source>,
    /// Errors (their file was skipped) and warnings.
    pub problems: Vec<Problem>,
    /// Layers skipped because of errors.
    pub skipped: usize,
}

impl Resolved {
    /// Where `path` was set (built-in default if no layer touched it).
    pub fn source(&self, path: &str) -> &Source {
        self.sources.get(path).unwrap_or(&Source::Builtin)
    }

    /// Whether some layer had errors and was skipped.
    pub fn has_errors(&self) -> bool {
        self.skipped > 0
    }
}

/// Folder of the user's controls files: `<config>/controls`, holding `systems/<system>.toml`
/// and `games/<system>/<serial>.toml`.
pub fn user_dir(config: &Path) -> PathBuf {
    config.join("controls")
}

/// Resolves the mapping of a game of `system` with per-game `key`, reading user files under
/// `user_dir`. `None` if the system declares no controls.
pub fn resolve(system: &System, key: &GameKey, user_dir: &Path) -> Option<Resolved> {
    if system.controls.is_empty() {
        return None;
    }
    let mut files: Vec<(Source, String)> = Vec::new();
    if let Some(text) = super::defaults::shipped(system.id.as_str()) {
        files.push((
            Source::Default(format!("data/controls/{}.toml", system.id)),
            text.to_owned(),
        ));
    }
    let system_file = user_dir.join("systems").join(format!("{}.toml", system.id));
    let game_file = key
        .candidates()
        .into_iter()
        .map(|rel| user_dir.join("games").join(rel))
        .find(|path| path.is_file());
    let mut problems = Vec::new();
    for (path, source) in [
        (Some(system_file.clone()), Source::System(system_file)),
        (
            game_file.clone(),
            Source::Game(game_file.unwrap_or_default()),
        ),
    ] {
        let Some(path) = path else { continue };
        match fs::read_to_string(&path) {
            Ok(text) => files.push((source, text)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => problems.push(Problem {
                file: path,
                line: None,
                message: format!("can't read: {err}"),
            }),
        }
    }

    let mut resolved = Resolved {
        mapping: Mapping::default(),
        sources: BTreeMap::new(),
        applied: Vec::new(),
        problems,
        skipped: 0,
    };
    for (source, text) in files {
        let path = match &source {
            Source::Default(p) => PathBuf::from(p),
            Source::System(p) | Source::Game(p) => p.clone(),
            Source::Builtin => PathBuf::new(),
        };
        let shipped = matches!(source, Source::Default(_));
        match parse_layer(&path, &text, &system.controls, shipped) {
            Ok((layer, warnings)) => {
                resolved.problems.extend(warnings);
                resolved.apply(&layer, &source, &path);
                resolved.applied.push(source);
            }
            Err(errors) => {
                resolved.problems.extend(errors);
                resolved.skipped += 1;
            }
        }
    }
    Some(resolved)
}

impl Resolved {
    fn apply(&mut self, layer: &Layer, source: &Source, path: &Path) {
        for (input, target) in &layer.binds {
            if *target != Target::None {
                let taken = self
                    .mapping
                    .binds
                    .iter()
                    .find(|(other, t)| *t == target && *other != input)
                    .map(|(other, _)| *other);
                if let Some(other) = taken {
                    self.problems.push(Problem {
                        file: path.to_path_buf(),
                        line: None,
                        message: format!(
                            "{target} was bound to {other} by {}; now on {input} only",
                            self.source(&format!("bind.{other}"))
                        ),
                    });
                    self.mapping.binds.insert(other, Target::None);
                    self.sources.insert(format!("bind.{other}"), source.clone());
                }
            }
            self.mapping.binds.insert(*input, target.clone());
            self.sources.insert(format!("bind.{input}"), source.clone());
        }
        let sources = &mut self.sources;
        let mut set = |key: &str| {
            sources.insert(key.to_owned(), source.clone());
        };
        let mapping = &mut self.mapping;
        apply_stick(
            &mut mapping.left_stick,
            &layer.left_stick,
            "left_stick",
            &mut set,
        );
        apply_stick(
            &mut mapping.right_stick,
            &layer.right_stick,
            "right_stick",
            &mut set,
        );
        if let Some(mode) = layer.trigger_mode {
            mapping.triggers.mode = mode;
            set("triggers.mode");
        }
        if let Some(threshold) = layer.trigger_threshold {
            mapping.triggers.threshold = threshold;
            set("triggers.threshold");
        }
        if let Some(vibration) = layer.vibration {
            mapping.vibration = vibration;
            set("vibration");
        }
        if let Some(swap) = layer.swap_sticks {
            mapping.swap_sticks = swap;
            set("swap_sticks");
        }
    }
}

fn apply_stick(
    stick: &mut StickOptions,
    changes: &StickChanges,
    name: &str,
    set: &mut impl FnMut(&str),
) {
    if let Some(v) = changes.invert_x {
        stick.invert_x = v;
        set(&format!("{name}.invert_x"));
    }
    if let Some(v) = changes.invert_y {
        stick.invert_y = v;
        set(&format!("{name}.invert_y"));
    }
    if let Some(v) = changes.deadzone {
        stick.deadzone = v;
        set(&format!("{name}.deadzone"));
    }
    if let Some(v) = changes.sensitivity {
        stick.sensitivity = v;
        set(&format!("{name}.sensitivity"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::model::{EmulatorAction, PadInput};
    use crate::model::SystemId;
    use crate::systems::Systems;
    use crate::testdir::tempdir;

    fn system(id: &str) -> System {
        Systems::builtin()
            .unwrap()
            .get(&SystemId::new(id))
            .unwrap()
            .clone()
    }

    fn tp_key(dir: &Path) -> GameKey {
        let iso = dir.join("tp.iso");
        let mut data = b"GZ2E01".to_vec();
        data.resize(0x40, 0);
        fs::write(&iso, data).unwrap();
        GameKey::for_rom(&SystemId::new("gc"), &iso, "TP")
    }

    fn write(path: PathBuf, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn shipped_defaults_parse_cleanly() {
        let dir = tempdir();
        for id in ["gc", "n64"] {
            let key = GameKey::for_rom(&SystemId::new(id), &dir.path().join("none"), "x");
            let r = resolve(&system(id), &key, dir.path()).unwrap();
            assert!(r.problems.is_empty(), "{id}: {:?}", r.problems);
            assert_eq!(r.applied.len(), 1);
        }
        assert!(
            resolve(
                &system("wii"),
                &GameKey::for_rom(&SystemId::new("wii"), Path::new("/x"), "x"),
                dir.path()
            )
            .is_none()
        );
    }

    #[test]
    fn game_layer_overrides_with_provenance() {
        let dir = tempdir();
        let user = dir.path().join("controls");
        write(
            user.join("games/gc/GZ2E01.toml"),
            "[player1.right_stick]\ninvert_x = true\n",
        );
        let r = resolve(&system("gc"), &tp_key(dir.path()), &user).unwrap();
        assert!(r.mapping.right_stick.invert_x);
        assert!(!r.mapping.left_stick.invert_x);
        assert!(matches!(r.source("right_stick.invert_x"), Source::Game(_)));
        assert!(matches!(r.source("bind.south"), Source::Default(_)));
        assert_eq!(
            r.mapping.target(PadInput::L1),
            Some(&Target::Action(EmulatorAction::FastForwardHold))
        );
    }

    #[test]
    fn region_free_game_file_and_system_layer() {
        let dir = tempdir();
        let user = dir.path().join("controls");
        write(
            user.join("systems/gc.toml"),
            "[player1]\nvibration = false\n",
        );
        write(
            user.join("games/gc/GZ2.toml"),
            "[player1.bind]\neast = \"A\"\n",
        );
        let r = resolve(&system("gc"), &tp_key(dir.path()), &user).unwrap();
        assert!(!r.mapping.vibration);
        assert!(matches!(r.source("vibration"), Source::System(_)));
        // A moved from south to east; south is now unbound, with a warning.
        assert_eq!(
            r.mapping.target(PadInput::East),
            Some(&Target::Control("A".into()))
        );
        assert_eq!(r.mapping.target(PadInput::South), None);
        assert_eq!(r.problems.len(), 1);
        assert!(r.problems[0].message.contains("A was bound to south"));
        assert_eq!(r.applied.len(), 3);
    }

    #[test]
    fn broken_layer_is_skipped_others_apply() {
        let dir = tempdir();
        let user = dir.path().join("controls");
        write(
            user.join("games/gc/GZ2E01.toml"),
            "[player1.bind]\nsouth = \"Zz\"\n[player1.right_stick]\ninvert_x = true\n",
        );
        let r = resolve(&system("gc"), &tp_key(dir.path()), &user).unwrap();
        assert!(r.has_errors());
        assert!(!r.mapping.right_stick.invert_x);
        assert_eq!(
            r.mapping.target(PadInput::South),
            Some(&Target::Control("A".into()))
        );
        assert_eq!(r.problems[0].line, Some(2));
    }
}
