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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// The controls of a console's controller that a mapping can bind to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleControls {
    /// Buttons.
    #[serde(default)]
    pub buttons: Vec<String>,
    /// Analog triggers.
    #[serde(default)]
    pub triggers: Vec<String>,
    /// Sticks.
    #[serde(default)]
    pub sticks: Vec<String>,
    /// D-pads.
    #[serde(default)]
    pub dpads: Vec<String>,
}

impl ConsoleControls {
    /// Looks a control up, ignoring case; returns its canonical name and kind.
    pub fn find(&self, name: &str) -> Option<(&str, ControlKind)> {
        self.all()
            .find(|(control, _)| control.eq_ignore_ascii_case(name))
    }

    /// Every control with its kind.
    pub fn all(&self) -> impl Iterator<Item = (&str, ControlKind)> {
        self.buttons
            .iter()
            .map(|c| (c.as_str(), ControlKind::Button))
            .chain(
                self.triggers
                    .iter()
                    .map(|c| (c.as_str(), ControlKind::Trigger)),
            )
            .chain(self.sticks.iter().map(|c| (c.as_str(), ControlKind::Stick)))
            .chain(self.dpads.iter().map(|c| (c.as_str(), ControlKind::Dpad)))
    }

    /// Whether the system declares no controls (no mapping is generated).
    pub fn is_empty(&self) -> bool {
        self.all().next().is_none()
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
    /// Controls a mapping can bind to.
    #[serde(default)]
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

    /// Parses a systems file.
    pub fn parse(text: &str) -> Result<Self, SystemsError> {
        Ok(toml::from_str(text)?)
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
        assert_eq!(
            gc.controls.find("c-stick"),
            Some(("C-stick", ControlKind::Stick))
        );
        assert_eq!(gc.controls.find("L"), Some(("L", ControlKind::Trigger)));
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
