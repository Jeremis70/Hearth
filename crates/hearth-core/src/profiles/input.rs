//! What the emulator input translators share: their output and what they can honor.

use crate::controls::{EmulatorAction, Mapping, Target, TriggerMode};

/// Generated input configuration for one launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Translation {
    /// Files to write, relative to the emulator's profile directory, with their contents.
    pub files: Vec<(String, String)>,
    /// Main config settings to set (RetroArch), as key/value.
    pub settings: Vec<(String, String)>,
    /// Core option settings to set (RetroArch), as key/value.
    pub core_options: Vec<(String, String)>,
    /// Options or bindings that couldn't be honored (each also logged at launch).
    pub warnings: Vec<String>,
    /// Environment variables for the emulator (the device allowlist with InputPlumber).
    pub env: Vec<(String, String)>,
}

/// What a translator can do; anything else in a mapping is reported and skipped.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    /// Emulator name for messages.
    pub emulator: &'static str,
    /// Supported emulator actions.
    pub actions: &'static [EmulatorAction],
    /// Console sticks whose deadzone and sensitivity it can set, by name; `None`: every one.
    pub stick_tuning: Option<&'static [&'static str]>,
    /// Digital trigger mode.
    pub analog_triggers: bool,
    /// Rumble on/off.
    pub vibration: bool,
}

impl Capabilities {
    /// Whether it can set console stick `stick`'s deadzone and sensitivity.
    pub fn can_tune(&self, stick: &str) -> bool {
        self.stick_tuning
            .is_none_or(|sticks| sticks.contains(&stick))
    }

    /// Warnings for everything in `mapping` this emulator can't honor.
    pub fn check(&self, mapping: &Mapping) -> Vec<String> {
        let mut warnings = Vec::new();
        let emulator = self.emulator;
        for (input, target) in &mapping.binds {
            if let Target::Action(action) = target
                && !self.actions.contains(action)
            {
                warnings.push(format!(
                    "{emulator} has no @{} action; {input} does nothing",
                    action.name()
                ));
            }
        }
        for (name, stick) in &mapping.sticks {
            if self.can_tune(name) {
                continue;
            }
            if stick.deadzone > 0.0 {
                warnings.push(format!("{emulator} can't set the {name} deadzone; ignored"));
            }
            if stick.sensitivity != 1.0 {
                warnings.push(format!(
                    "{emulator} can't set the {name} sensitivity; ignored"
                ));
            }
        }
        if mapping.triggers.mode == TriggerMode::Digital && !self.analog_triggers {
            warnings.push(format!("{emulator} can't change triggers.mode; ignored"));
        }
        if !mapping.vibration && !self.vibration {
            warnings.push(format!("{emulator} can't turn vibration off; ignored"));
        }
        warnings
    }
}

/// Compares `actual` with `tests/golden/<rel>`; `HEARTH_UPDATE_GOLDEN=1` rewrites the file.
#[cfg(test)]
pub fn assert_golden(rel: &std::path::Path, actual: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(rel);
    if std::env::var_os("HEARTH_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "{}: {err} (run with HEARTH_UPDATE_GOLDEN=1 to create it)",
            path.display()
        )
    });
    assert_eq!(
        actual,
        expected,
        "{} differs (HEARTH_UPDATE_GOLDEN=1 to accept)",
        path.display()
    );
}
