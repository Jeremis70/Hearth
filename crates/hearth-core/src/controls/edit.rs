//! Changing a user layer file from the UI: one setting at a time, saved immediately.
//!
//! The file only ever holds differences from the layers below it: a value set back to what
//! those layers give is removed. Edits go through `toml_edit`, so comments, order and
//! formatting of hand-edited files are kept.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, value};

use super::device::ControllerKind;
use super::key::GameKey;
use super::layers::{Resolved, UserLayers, game_file, resolve_layers, system_file};
use super::model::{InputKind, Mapping, PadInput, Target, TriggerMode};
use crate::systems::{ControlKind, System};

/// Errors saving a controls change.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    /// The system has no controls.
    #[error("{0} has no controls to edit")]
    NoControls(String),
    /// The user file can't be read or written.
    #[error("{path}: {source}")]
    Io {
        /// File.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// The user file isn't valid TOML (fix or delete it).
    #[error("{path}: {message}")]
    Parse {
        /// File.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
    /// Stick options for something that isn't one of the console's sticks (by its canonical
    /// name).
    #[error("{0} isn't a stick of this console")]
    NotAStick(String),
    /// A target the input can't drive.
    #[error("{input} can't drive {target}")]
    Incompatible {
        /// Input.
        input: PadInput,
        /// Target.
        target: Target,
    },
}

/// Which file the edits go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every game of the system (`systems/<system>.toml`).
    System,
    /// One game (`games/<system>/<serial>.toml`).
    Game,
}

/// One option change. Stick options name the console stick ("C-stick"): they follow it to
/// whichever physical stick drives it.
#[derive(Debug, Clone, PartialEq)]
pub enum Setting {
    /// Rumble.
    Vibration(bool),
    /// Invert a console stick's X.
    InvertX(String, bool),
    /// Invert a console stick's Y.
    InvertY(String, bool),
    /// A console stick's deadzone.
    Deadzone(String, f32),
    /// A console stick's sensitivity.
    Sensitivity(String, f32),
    /// Trigger mode.
    TriggerMode(TriggerMode),
    /// Trigger threshold.
    TriggerThreshold(f32),
}

/// What an assignment did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assigned {
    /// Another input that had the target and now has the input's previous one.
    pub swapped: Option<(PadInput, Target)>,
}

/// Edits one user layer of a system (or of one game of it).
#[derive(Debug, Clone)]
pub struct Editor {
    system: System,
    key: GameKey,
    user_dir: PathBuf,
    /// Controller type, for its default bindings.
    pad: ControllerKind,
    scope: Scope,
}

impl Editor {
    /// An editor for `scope`. `key` identifies the game (any game of the system for
    /// [`Scope::System`]; it only matters for the game layer).
    pub fn new(
        system: &System,
        key: GameKey,
        user_dir: &Path,
        pad: ControllerKind,
        scope: Scope,
    ) -> Result<Self, EditError> {
        if system.controls.is_empty() {
            return Err(EditError::NoControls(system.name.clone()));
        }
        Ok(Self {
            system: system.clone(),
            key,
            user_dir: user_dir.to_path_buf(),
            pad,
            scope,
        })
    }

    /// The scope.
    pub fn scope(&self) -> Scope {
        self.scope
    }

    /// The system.
    pub fn system(&self) -> &System {
        &self.system
    }

    /// The file edits go to.
    pub fn path(&self) -> PathBuf {
        match self.scope {
            Scope::System => system_file(&self.system, &self.user_dir),
            Scope::Game => game_file(&self.key, &self.user_dir),
        }
    }

    fn layers(&self, including_own: bool) -> UserLayers {
        match self.scope {
            Scope::System => UserLayers {
                system: including_own,
                game: false,
            },
            Scope::Game => UserLayers {
                system: true,
                game: including_own,
            },
        }
    }

    fn resolve(&self, layers: UserLayers) -> Resolved {
        resolve_layers(&self.system, &self.key, &self.user_dir, self.pad, layers)
            .expect("the system has controls (checked in new)")
    }

    /// The mapping as edited (this scope's file and everything below).
    pub fn current(&self) -> Resolved {
        self.resolve(self.layers(true))
    }

    /// The mapping without this scope's file: what "reset" goes back to.
    pub fn base(&self) -> Mapping {
        self.resolve(self.layers(false)).mapping
    }

    /// The shipped defaults alone (console, then controller type).
    pub fn shipped(&self) -> Mapping {
        self.resolve(UserLayers::NONE).mapping
    }

    /// Puts `target` on `input`. If another input had it, that input gets `input`'s previous
    /// target (a swap); `Target::None` just unbinds.
    pub fn assign(&self, input: PadInput, target: Target) -> Result<Assigned, EditError> {
        self.check_fits(input, &target)?;
        let current = self.current().mapping.expanded();
        let previous = current.binds.get(&input).cloned().unwrap_or(Target::None);
        let other = (target != Target::None)
            .then(|| {
                current
                    .binds
                    .iter()
                    .find(|(i, t)| **t == target && **i != input)
                    .map(|(i, _)| *i)
            })
            .flatten();
        let base = self.base().expanded();
        let mut doc = self.load()?;
        let mut writes = vec![(input, target)];
        if let Some(other) = other {
            writes.push((other, previous.clone()));
        }
        for (i, t) in &writes {
            let path = ["player1", "bind", i.name()];
            if base.binds.get(i).unwrap_or(&Target::None) == t {
                remove(&mut doc, &path);
            } else {
                set(&mut doc, &path, value(t.to_string()));
            }
        }
        self.save(&doc)?;
        Ok(Assigned {
            swapped: other.map(|o| (o, previous)),
        })
    }

    fn check_fits(&self, input: PadInput, target: &Target) -> Result<(), EditError> {
        let fits = match target {
            Target::None => true,
            Target::Action(_) | Target::Hearth(_) => {
                matches!(input.kind(), InputKind::Button | InputKind::Trigger)
            }
            Target::Control(name) => self.system.controls.find(name).is_some_and(|c| {
                use crate::systems::ControlKind as K;
                match input.kind() {
                    InputKind::Button | InputKind::Trigger => {
                        matches!(c.kind, K::Button | K::Trigger)
                    }
                    InputKind::Stick | InputKind::Dpad => matches!(c.kind, K::Stick | K::Dpad),
                }
            }),
        };
        if fits {
            Ok(())
        } else {
            Err(EditError::Incompatible {
                input,
                target: target.clone(),
            })
        }
    }

    /// Changes one option.
    pub fn set(&self, setting: Setting) -> Result<(), EditError> {
        let base = self.base();
        if let Setting::InvertX(stick, _)
        | Setting::InvertY(stick, _)
        | Setting::Deadzone(stick, _)
        | Setting::Sensitivity(stick, _) = &setting
            && !self
                .system
                .controls
                .find(stick)
                .is_some_and(|c| c.kind == ControlKind::Stick && c.name == *stick)
        {
            return Err(EditError::NotAStick(stick.clone()));
        }
        let (path, new, old): (Vec<&str>, Item, Item) = match &setting {
            Setting::Vibration(v) => (
                vec!["player1", "vibration"],
                value(*v),
                value(base.vibration),
            ),
            Setting::InvertX(s, v) => (
                vec!["player1", "sticks", s, "invert_x"],
                value(*v),
                value(base.stick_options(s).invert_x),
            ),
            Setting::InvertY(s, v) => (
                vec!["player1", "sticks", s, "invert_y"],
                value(*v),
                value(base.stick_options(s).invert_y),
            ),
            Setting::Deadzone(s, v) => (
                vec!["player1", "sticks", s, "deadzone"],
                value(round(*v)),
                value(round(base.stick_options(s).deadzone)),
            ),
            Setting::Sensitivity(s, v) => (
                vec!["player1", "sticks", s, "sensitivity"],
                value(round(*v)),
                value(round(base.stick_options(s).sensitivity)),
            ),
            Setting::TriggerMode(m) => (
                vec!["player1", "triggers", "mode"],
                value(mode_name(*m)),
                value(mode_name(base.triggers.mode)),
            ),
            Setting::TriggerThreshold(v) => (
                vec!["player1", "triggers", "threshold"],
                value(round(*v)),
                value(round(base.triggers.threshold)),
            ),
        };
        let mut doc = self.load()?;
        if new.to_string() == old.to_string() {
            remove(&mut doc, &path);
        } else {
            set(&mut doc, &path, new);
        }
        self.save(&doc)
    }

    /// Back to the layers below: deletes this scope's file.
    pub fn reset(&self) -> Result<(), EditError> {
        let path = self.path();
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(EditError::Io { path, source }),
        }
    }

    /// Whether this scope's file sets the setting at `path` under `[player1]`
    /// (`["bind", "south"]`, `["sticks", "C-stick", "invert_x"]`).
    pub fn overrides(&self, path: &[&str]) -> bool {
        let Ok(doc) = self.load() else {
            return false;
        };
        let mut item = doc.as_item().get("player1");
        for part in path {
            item = item.and_then(|i| i.get(part));
        }
        item.is_some()
    }

    fn load(&self) -> Result<DocumentMut, EditError> {
        let path = self.path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
            Err(source) => return Err(EditError::Io { path, source }),
        };
        text.parse()
            .map_err(|err: toml_edit::TomlError| EditError::Parse {
                path,
                message: err.message().to_owned(),
            })
    }

    fn save(&self, doc: &DocumentMut) -> Result<(), EditError> {
        let path = self.path();
        let io_err = |source| EditError::Io {
            path: path.clone(),
            source,
        };
        if is_empty(doc.as_table()) && !doc.to_string().contains('#') {
            return match fs::remove_file(&path) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => Err(io_err(err)),
                _ => Ok(()),
            };
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(io_err)?;
        }
        let mut text = doc.to_string();
        if self.scope == Scope::Game && !text.starts_with('#') {
            text = format!(
                "# Controls for {} only (Hearth).\n{text}",
                self.key.as_string()
            );
        }
        fs::write(&path, text).map_err(io_err)
    }
}

fn mode_name(mode: TriggerMode) -> &'static str {
    match mode {
        TriggerMode::Analog => "analog",
        TriggerMode::Digital => "digital",
    }
}

/// Two decimals, so 0.15 doesn't become 0.15000000596.
fn round(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

/// Sets `path`. Tables are created as needed; a table that gets its first own value is
/// placed at the end of the file, so hand-written content and its comments stay first.
/// Intermediate tables without own values stay implicit (no empty `[player1]` header).
fn set(doc: &mut DocumentMut, path: &[&str], item: Item) {
    let next = last_position(doc.as_table()) + 1;
    let (last, parents) = path.split_last().expect("non-empty path");
    let mut table = doc.as_table_mut();
    for part in parents {
        let entry = table.entry(part).or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        });
        table = entry
            .as_table_mut()
            .expect("controls files hold tables here");
    }
    if table.is_implicit() || !has_values(table) {
        table.set_implicit(false);
        table.set_position(Some(next));
    }
    table.insert(last, item);
}

fn last_position(table: &Table) -> isize {
    table
        .iter()
        .filter_map(|(_, item)| item.as_table())
        .map(|t| t.position().unwrap_or(0).max(last_position(t)))
        .max()
        .unwrap_or(0)
}

fn has_values(table: &Table) -> bool {
    table.iter().any(|(_, item)| !item.is_table())
}

/// Removes `path`, then any table it leaves empty.
fn remove(doc: &mut DocumentMut, path: &[&str]) {
    fn walk(table: &mut Table, path: &[&str]) {
        match path {
            [] => {}
            [last] => {
                table.remove(last);
            }
            [first, rest @ ..] => {
                if let Some(child) = table.get_mut(first).and_then(Item::as_table_mut) {
                    walk(child, rest);
                    if is_empty(child) {
                        table.remove(first);
                    } else if !has_values(child) {
                        child.set_implicit(true);
                    }
                }
            }
        }
    }
    walk(doc.as_table_mut(), path);
}

fn is_empty(table: &Table) -> bool {
    table
        .iter()
        .all(|(_, item)| item.as_table().is_some_and(is_empty))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::EmulatorAction;
    use crate::model::SystemId;
    use crate::systems::Systems;
    use crate::testdir::tempdir;

    struct Fixture {
        _dir: tempfile::TempDir,
        user: PathBuf,
        gc: System,
        key: GameKey,
    }

    fn fixture() -> Fixture {
        let dir = tempdir();
        let iso = dir.path().join("tp.iso");
        let mut data = b"GZ2E01".to_vec();
        data.resize(0x40, 0);
        fs::write(&iso, data).unwrap();
        let gc = Systems::builtin()
            .unwrap()
            .get(&SystemId::new("gc"))
            .unwrap()
            .clone();
        let key = GameKey::for_rom(&gc.id, &iso, "TP");
        let user = dir.path().join("controls");
        Fixture {
            _dir: dir,
            user,
            gc,
            key,
        }
    }

    fn editor(f: &Fixture, scope: Scope) -> Editor {
        Editor::new(
            &f.gc,
            f.key.clone(),
            &f.user,
            ControllerKind::Generic,
            scope,
        )
        .unwrap()
    }

    #[test]
    fn reassigning_swaps_and_writes_only_differences() {
        let f = fixture();
        let e = editor(&f, Scope::System);
        // Z moves from R1 to the unused L1: R1 is left with L1's nothing.
        let done = e.assign(PadInput::L1, Target::Control("Z".into())).unwrap();
        assert_eq!(done.swapped, Some((PadInput::R1, Target::None)));
        assert_eq!(
            fs::read_to_string(e.path()).unwrap(),
            "[player1.bind]\nl1 = \"Z\"\nr1 = \"none\"\n"
        );
        // Moving it back restores the default: the file goes away.
        e.assign(PadInput::R1, Target::Control("Z".into())).unwrap();
        assert!(!e.path().exists());
        // A on east: south gets east's B.
        let done = e
            .assign(PadInput::East, Target::Control("A".into()))
            .unwrap();
        assert_eq!(
            done.swapped,
            Some((PadInput::South, Target::Control("B".into())))
        );
        let m = e.current().mapping;
        assert_eq!(
            m.target(PadInput::South),
            Some(&Target::Control("B".into()))
        );
        // An emulator action is added by hand like any other function.
        e.assign(
            PadInput::L1,
            Target::Action(EmulatorAction::FastForwardHold),
        )
        .unwrap();
        assert!(
            fs::read_to_string(e.path())
                .unwrap()
                .contains("l1 = \"@fast_forward_hold\"\n")
        );
    }

    #[test]
    fn nothing_and_dpad_directions() {
        let f = fixture();
        let e = editor(&f, Scope::System);
        e.assign(PadInput::L1, Target::None).unwrap();
        e.assign(PadInput::DpadUp, Target::Control("Y".into()))
            .unwrap();
        let m = e.current().mapping.expanded();
        assert_eq!(m.target(PadInput::L1), None);
        assert_eq!(
            m.target(PadInput::DpadUp),
            Some(&Target::Control("Y".into()))
        );
        // Y left north, which got D-pad up.
        assert_eq!(
            m.target(PadInput::North),
            Some(&Target::Control("D-pad up".into()))
        );
        assert_eq!(
            m.target(PadInput::DpadDown),
            Some(&Target::Control("D-pad down".into()))
        );
        assert!(matches!(
            e.assign(PadInput::LeftStick, Target::Control("A".into())),
            Err(EditError::Incompatible { .. })
        ));
    }

    #[test]
    fn options_and_game_scope_and_reset() {
        let f = fixture();
        let game = editor(&f, Scope::Game);
        game.set(Setting::InvertX("C-stick".into(), true)).unwrap();
        game.set(Setting::Deadzone("Main stick".into(), 0.15))
            .unwrap();
        assert_eq!(game.path(), f.user.join("games/gc/GZ2E01.toml"));
        let text = fs::read_to_string(game.path()).unwrap();
        assert!(
            text.contains("[player1.sticks.C-stick]\ninvert_x = true\n"),
            "{text}"
        );
        assert!(
            text.contains("[player1.sticks.\"Main stick\"]\ndeadzone = 0.15\n"),
            "{text}"
        );
        assert!(game.overrides(&["sticks", "C-stick", "invert_x"]));
        assert!(!game.overrides(&["bind", "south"]));
        // Only console sticks have stick options.
        assert!(matches!(
            game.set(Setting::InvertX("D-pad".into(), true)),
            Err(EditError::NotAStick(_))
        ));
        // The system layer doesn't see the game's file.
        let system = editor(&f, Scope::System);
        assert!(!system.current().mapping.stick_options("C-stick").invert_x);
        assert!(game.current().mapping.stick_options("C-stick").invert_x);
        // Setting back to the value below removes it.
        game.set(Setting::Deadzone("Main stick".into(), 0.0))
            .unwrap();
        assert!(
            !fs::read_to_string(game.path())
                .unwrap()
                .contains("deadzone")
        );
        game.reset().unwrap();
        assert!(!game.path().exists());
        assert!(!game.current().mapping.stick_options("C-stick").invert_x);
    }

    #[test]
    fn hand_edited_files_keep_comments_and_order() {
        let f = fixture();
        let e = editor(&f, Scope::Game);
        let original = "# My TP tweaks\n\n[player1.sticks.C-stick]\n# camera felt reversed\ninvert_x = true\n\n[player1.bind]\nselect = \"@screenshot\"  # handy\n";
        fs::create_dir_all(e.path().parent().unwrap()).unwrap();
        fs::write(e.path(), original).unwrap();
        e.set(Setting::Vibration(false)).unwrap();
        let text = fs::read_to_string(e.path()).unwrap();
        assert!(
            text.starts_with(
                "# My TP tweaks\n\n[player1.sticks.C-stick]\n# camera felt reversed\ninvert_x = true\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("select = \"@screenshot\"  # handy\n"),
            "{text}"
        );
        assert!(text.contains("vibration = false"), "{text}");
        e.set(Setting::Vibration(true)).unwrap();
        assert_eq!(fs::read_to_string(e.path()).unwrap(), original);
    }
}
