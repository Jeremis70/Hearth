//! Systems (consoles) and how to launch their games, from `data/systems.toml`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::model::SystemId;

/// The systems file shipped with Hearth.
const BUILTIN: &str = include_str!("../../../data/systems.toml");

/// Errors loading systems.
#[derive(Debug, thiserror::Error)]
pub enum SystemsError {
    /// The TOML is invalid.
    #[error("invalid systems.toml: {0}")]
    Parse(#[from] toml::de::Error),
}

/// What to do with a game's processes while Hearth is in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SuspendPolicy {
    /// Freeze the process group with SIGSTOP; SIGCONT on resume.
    Sigstop,
    /// Leave it running (online games, Steam).
    #[default]
    None,
}

/// Which launcher an emulator entry uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmulatorKind {
    /// RetroArch with a libretro core, launched through the RetroArch profile.
    Retroarch,
    /// Dolphin (GameCube, Wii), launched through the Dolphin profile.
    Dolphin,
}

/// One way to run a system's games.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Emulator {
    /// Identifier, referenced by `default_emulator`.
    pub id: String,
    /// Launcher kind.
    pub kind: EmulatorKind,
    /// libretro core (RetroArch only).
    pub core: Option<PathBuf>,
}

/// What kind of control a console control is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlKind {
    /// On/off.
    Button,
    /// Analog trigger with a digital click (GameCube L/R).
    Trigger,
    /// Analog stick.
    Stick,
    /// Four-way pad.
    Dpad,
}

/// One control of a console's controller, as declared in `data/controls/<system>.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleControl {
    /// Name used in controls files ("Main stick").
    pub name: String,
    /// Kind.
    pub kind: ControlKind,
    /// What the UI shows ("C-stick (camera)").
    pub label: String,
    /// One line for players who never held this console's controller.
    #[serde(default)]
    pub hint: Option<String>,
    /// Another control that drives the same console input (the N64's C-up is the C-buttons
    /// stick pushed up). Lets a remapping layer that only knows where the other one sits route
    /// this one too.
    #[serde(default)]
    pub same_as: Option<String>,
}

/// Directions of a stick or D-pad, as suffixes of their derived button controls.
pub const DIRECTIONS: [&str; 4] = ["up", "down", "left", "right"];

/// The controls of a console's controller that a mapping can bind to. Every stick and D-pad
/// control `X` also has four button controls `X up`, `X down`, `X left`, `X right`, so a
/// single button can push one direction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsoleControls {
    controls: Vec<ConsoleControl>,
}

impl ConsoleControls {
    /// From the declared controls, in display order.
    pub fn new(controls: Vec<ConsoleControl>) -> Self {
        Self { controls }
    }

    /// Looks a control up, ignoring case, including derived direction buttons.
    pub fn find(&self, name: &str) -> Option<ConsoleControl> {
        if let Some(c) = self
            .controls
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
        {
            return Some(c.clone());
        }
        let (parent, dir) = name.rsplit_once(' ')?;
        let dir = DIRECTIONS
            .into_iter()
            .find(|d| d.eq_ignore_ascii_case(dir))?;
        let parent = self.controls.iter().find(|c| {
            c.name.eq_ignore_ascii_case(parent)
                && matches!(c.kind, ControlKind::Stick | ControlKind::Dpad)
        })?;
        Some(Self::direction(parent, dir))
    }

    fn direction(parent: &ConsoleControl, dir: &str) -> ConsoleControl {
        ConsoleControl {
            name: format!("{} {dir}", parent.name),
            kind: ControlKind::Button,
            label: format!("{} {dir}", parent.label),
            hint: parent.hint.clone(),
            same_as: None,
        }
    }

    /// If `name` is a derived direction button, its parent control and direction.
    pub fn split_direction<'a>(&self, name: &'a str) -> Option<(&ConsoleControl, &'a str)> {
        let (parent, dir) = name.rsplit_once(' ')?;
        let parent = self.controls.iter().find(|c| {
            c.name == parent && matches!(c.kind, ControlKind::Stick | ControlKind::Dpad)
        })?;
        DIRECTIONS.contains(&dir).then_some((parent, dir))
    }

    /// The declared controls (without derived directions), in display order.
    pub fn declared(&self) -> &[ConsoleControl] {
        &self.controls
    }

    /// Every bindable control: declared ones, then each stick's and D-pad's directions.
    pub fn all(&self) -> Vec<ConsoleControl> {
        let mut all = self.controls.clone();
        for parent in &self.controls {
            if matches!(parent.kind, ControlKind::Stick | ControlKind::Dpad) {
                all.extend(DIRECTIONS.iter().map(|d| Self::direction(parent, d)));
            }
        }
        all
    }

    /// Whether the system declares no controls (no mapping is generated).
    pub fn is_empty(&self) -> bool {
        self.controls.is_empty()
    }
}

/// A console or platform.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct System {
    /// Identifier ("n64").
    pub id: SystemId,
    /// Full name ("Nintendo 64").
    pub name: String,
    /// Short label for badges ("N64").
    pub short: String,
    /// File extensions, lowercase, without the dot.
    pub extensions: Vec<String>,
    /// Folder names under a games root that hold this system's games.
    pub folders: Vec<String>,
    /// Emulator used unless the user picks another.
    pub default_emulator: String,
    /// Suspend behaviour while Hearth is in front.
    #[serde(default)]
    pub suspend: SuspendPolicy,
    /// Controls a mapping can bind to, from `data/controls/<system>.toml`.
    #[serde(skip)]
    pub controls: ConsoleControls,
    /// Available emulators.
    #[serde(rename = "emulator", default)]
    pub emulators: Vec<Emulator>,
}

impl System {
    /// Whether `path` has one of this system's extensions (case-insensitive).
    pub fn accepts(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| self.extensions.iter().any(|x| x.eq_ignore_ascii_case(ext)))
    }

    /// The default emulator entry, if it exists.
    pub fn emulator(&self) -> Option<&Emulator> {
        self.emulators
            .iter()
            .find(|e| e.id == self.default_emulator)
    }
}

/// All known systems.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Systems {
    #[serde(rename = "system", default)]
    systems: Vec<System>,
}

impl Systems {
    /// The systems shipped with Hearth.
    pub fn builtin() -> Result<Self, SystemsError> {
        Self::parse(BUILTIN)
    }

    /// Parses a systems file; console controls come from the shipped controls files.
    pub fn parse(text: &str) -> Result<Self, SystemsError> {
        let mut systems: Self = toml::from_str(text)?;
        for system in &mut systems.systems {
            if let Some(text) = crate::controls::defaults::shipped(system.id.as_str()) {
                system.controls = crate::controls::defaults::console_controls(text)?;
            }
        }
        Ok(systems)
    }

    /// Looks a system up by id.
    pub fn get(&self, id: &SystemId) -> Option<&System> {
        self.systems.iter().find(|s| &s.id == id)
    }

    /// Iterates over all systems.
    pub fn iter(&self) -> impl Iterator<Item = &System> {
        self.systems.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_has_n64_with_retroarch_core() {
        let systems = Systems::builtin().unwrap();
        let n64 = systems.get(&SystemId::new("n64")).unwrap();
        assert_eq!(n64.short, "N64");
        assert_eq!(n64.suspend, SuspendPolicy::Sigstop);
        let emu = n64.emulator().unwrap();
        assert_eq!(emu.kind, EmulatorKind::Retroarch);
        assert!(
            emu.core
                .as_ref()
                .unwrap()
                .ends_with("mupen64plus_next_libretro.so")
        );
    }

    #[test]
    fn builtin_has_gamecube_and_wii_with_dolphin() {
        let systems = Systems::builtin().unwrap();
        for id in ["gc", "wii"] {
            let system = systems.get(&SystemId::new(id)).unwrap();
            assert_eq!(system.suspend, SuspendPolicy::Sigstop);
            assert_eq!(system.emulator().unwrap().kind, EmulatorKind::Dolphin);
            assert!(system.accepts(Path::new("Game (USA).iso")));
        }
        let gc = systems.get(&SystemId::new("gc")).unwrap();
        assert_eq!(gc.short, "GameCube");
        let c_stick = gc.controls.find("c-stick").unwrap();
        assert_eq!(
            (c_stick.name.as_str(), c_stick.kind),
            ("C-stick", ControlKind::Stick)
        );
        assert_eq!(gc.controls.find("L").unwrap().kind, ControlKind::Trigger);
        let up = gc.controls.find("d-pad UP").unwrap();
        assert_eq!(
            (up.name.as_str(), up.kind),
            ("D-pad up", ControlKind::Button)
        );
        assert!(gc.controls.find("Z up").is_none());
        assert!(gc.controls.declared().iter().all(|c| !c.label.is_empty()));
        assert_eq!(gc.controls.all().len(), gc.controls.declared().len() + 12);
        assert!(
            systems
                .get(&SystemId::new("wii"))
                .unwrap()
                .controls
                .is_empty()
        );
        assert_eq!(gc.folders, ["gc", "gamecube"]);
    }

    #[test]
    fn extensions_match_case_insensitively() {
        let systems = Systems::builtin().unwrap();
        let n64 = systems.get(&SystemId::new("n64")).unwrap();
        assert!(n64.accepts(Path::new("Zelda.Z64")));
        assert!(n64.accepts(Path::new("a/b/game.v64")));
        assert!(!n64.accepts(Path::new("readme.txt")));
        assert!(!n64.accepts(Path::new("noext")));
    }
}
