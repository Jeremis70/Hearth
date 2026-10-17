//! One controls file (a layer): parsing and validation against a system's controls.
//!
//! A layer only holds what it changes. Errors name the file and line; a file with errors is
//! skipped as a whole, warnings are reported and the rest of the file applies.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml::Spanned;

use super::model::{EmulatorAction, InputKind, PadInput, Target, TriggerMode};
use crate::systems::{ConsoleControls, ControlKind};

/// A problem in a controls file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The file.
    pub file: PathBuf,
    /// 1-based line, if known.
    pub line: Option<usize>,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{line}: {}", self.file.display(), self.message),
            None => write!(f, "{}: {}", self.file.display(), self.message),
        }
    }
}

/// One stick's changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StickChanges {
    /// New `invert_x`.
    pub invert_x: Option<bool>,
    /// New `invert_y`.
    pub invert_y: Option<bool>,
    /// New `deadzone`.
    pub deadzone: Option<f32>,
    /// New `sensitivity`.
    pub sensitivity: Option<f32>,
}

/// What one file changes, validated.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layer {
    /// Input → target, in file order.
    pub binds: Vec<(PadInput, Target)>,
    /// Left stick changes.
    pub left_stick: StickChanges,
    /// Right stick changes.
    pub right_stick: StickChanges,
    /// New trigger mode.
    pub trigger_mode: Option<TriggerMode>,
    /// New trigger threshold.
    pub trigger_threshold: Option<f32>,
    /// New vibration setting.
    pub vibration: Option<bool>,
    /// New swap_sticks setting.
    pub swap_sticks: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    player1: Option<PlayerDoc>,
    /// Console declarations: only in shipped files (`defaults`).
    console: Option<Spanned<toml::Value>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlayerDoc {
    vibration: Option<bool>,
    swap_sticks: Option<bool>,
    bind: Option<BTreeMap<String, Spanned<String>>>,
    left_stick: Option<StickDoc>,
    right_stick: Option<StickDoc>,
    triggers: Option<TriggerDoc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StickDoc {
    invert_x: Option<bool>,
    invert_y: Option<bool>,
    deadzone: Option<Spanned<f32>>,
    sensitivity: Option<Spanned<f32>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerDoc {
    mode: Option<Spanned<String>>,
    threshold: Option<Spanned<f32>>,
}

/// Parses and validates `text` (the contents of `file`) for a system with `controls`.
/// `shipped`: a `data/controls` file, which may also declare the console's controls.
/// Returns the layer and its warnings, or every error found.
pub fn parse_layer(
    file: &Path,
    text: &str,
    controls: &ConsoleControls,
    shipped: bool,
) -> Result<(Layer, Vec<Problem>), Vec<Problem>> {
    let problem = |span: Option<Range<usize>>, message: String| Problem {
        file: file.to_path_buf(),
        line: span.map(|s| line_of(text, s.start)),
        message,
    };
    let doc: Doc = toml::from_str(text)
        .map_err(|err| vec![problem(err.span(), err.message().trim().to_owned())])?;
    let mut layer = Layer::default();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if let Some(console) = doc.console.filter(|_| !shipped) {
        return Err(vec![problem(
            Some(console.span()),
            "console controls are declared by Hearth (data/controls); remove [[console]]".into(),
        )]);
    }
    let Some(player) = doc.player1 else {
        return Ok((layer, warnings));
    };
    layer.vibration = player.vibration;
    layer.swap_sticks = player.swap_sticks;

    // Sort binds by position in the file, so "first in the file" is well defined.
    let mut binds: Vec<_> = player.bind.unwrap_or_default().into_iter().collect();
    binds.sort_by_key(|(_, value)| value.span().start);
    for (name, value) in binds {
        let span = Some(value.span());
        if name == "guide" {
            warnings.push(problem(
                span,
                "guide belongs to Hearth and can't be bound; ignored".into(),
            ));
            continue;
        }
        let Some(input) = PadInput::from_name(&name) else {
            let names: Vec<_> = PadInput::ALL.iter().map(|i| i.name()).collect();
            errors.push(problem(
                span,
                format!(
                    "unknown input '{name}' (expected one of: {})",
                    names.join(", ")
                ),
            ));
            continue;
        };
        match parse_target(input, value.get_ref(), controls) {
            Ok(target) => {
                if let Some((other, _)) = layer
                    .binds
                    .iter()
                    .find(|(_, t)| *t == target && target != Target::None)
                {
                    warnings.push(problem(
                        span,
                        format!(
                            "{target} is already bound to {other} in this file; {input} ignored"
                        ),
                    ));
                } else {
                    layer.binds.push((input, target));
                }
            }
            Err(message) => errors.push(problem(span, message)),
        }
    }

    for (doc, changes) in [
        (player.left_stick, &mut layer.left_stick),
        (player.right_stick, &mut layer.right_stick),
    ] {
        let Some(doc) = doc else { continue };
        changes.invert_x = doc.invert_x;
        changes.invert_y = doc.invert_y;
        changes.deadzone = in_range(doc.deadzone, "deadzone", 0.0, 0.9, &problem, &mut errors);
        changes.sensitivity = in_range(
            doc.sensitivity,
            "sensitivity",
            0.5,
            2.0,
            &problem,
            &mut errors,
        );
    }
    if let Some(triggers) = player.triggers {
        if let Some(mode) = triggers.mode {
            match mode.get_ref().as_str() {
                "analog" => layer.trigger_mode = Some(TriggerMode::Analog),
                "digital" => layer.trigger_mode = Some(TriggerMode::Digital),
                other => errors.push(problem(
                    Some(mode.span()),
                    format!("unknown trigger mode '{other}' (expected \"analog\" or \"digital\")"),
                )),
            }
        }
        layer.trigger_threshold = in_range(
            triggers.threshold,
            "threshold",
            0.05,
            1.0,
            &problem,
            &mut errors,
        );
    }

    if errors.is_empty() {
        Ok((layer, warnings))
    } else {
        Err(errors)
    }
}

fn in_range(
    value: Option<Spanned<f32>>,
    name: &str,
    min: f32,
    max: f32,
    problem: &impl Fn(Option<Range<usize>>, String) -> Problem,
    errors: &mut Vec<Problem>,
) -> Option<f32> {
    let value = value?;
    let v = *value.get_ref();
    if (min..=max).contains(&v) {
        Some(v)
    } else {
        errors.push(problem(
            Some(value.span()),
            format!("{name} = {v} is out of range ({min}–{max})"),
        ));
        None
    }
}

/// Parses a bind value: `"none"`, `"@action"` or a console control compatible with `input`.
fn parse_target(
    input: PadInput,
    value: &str,
    controls: &ConsoleControls,
) -> Result<Target, String> {
    if value == "none" {
        return Ok(Target::None);
    }
    if let Some(action) = value.strip_prefix('@') {
        let action = EmulatorAction::from_name(action).ok_or_else(|| {
            let names: Vec<_> = EmulatorAction::ALL
                .iter()
                .map(|a| format!("@{}", a.name()))
                .collect();
            format!(
                "unknown action '{value}' (expected one of: {})",
                names.join(", ")
            )
        })?;
        return match input.kind() {
            InputKind::Button | InputKind::Trigger => Ok(Target::Action(action)),
            _ => Err(format!("{input} can't drive an action; bind a button")),
        };
    }
    let Some(control) = controls.find(value) else {
        let names: Vec<_> = controls
            .declared()
            .iter()
            .map(|c| format!("\"{}\"", c.name))
            .collect();
        return Err(format!(
            "unknown control '{value}' (expected one of: {}, a stick or D-pad direction like \"D-pad up\", \"none\" or an @action)",
            names.join(", ")
        ));
    };
    let (name, kind) = (control.name.as_str(), control.kind);
    let fits = match input.kind() {
        InputKind::Button | InputKind::Trigger => {
            matches!(kind, ControlKind::Button | ControlKind::Trigger)
        }
        InputKind::Stick | InputKind::Dpad => {
            matches!(kind, ControlKind::Stick | ControlKind::Dpad)
        }
    };
    if fits {
        Ok(Target::Control(name.to_owned()))
    } else {
        Err(format!(
            "{input} can't drive {name} (a stick or D-pad drives a stick or D-pad, a button drives a button)"
        ))
    }
}

/// 1-based line of byte offset `at`.
fn line_of(text: &str, at: usize) -> usize {
    text.get(..at)
        .map_or(1, |head| head.matches('\n').count() + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SystemId;
    use crate::systems::Systems;

    fn gc() -> ConsoleControls {
        Systems::builtin()
            .unwrap()
            .get(&SystemId::new("gc"))
            .unwrap()
            .controls
            .clone()
    }

    fn parse(text: &str) -> Result<(Layer, Vec<Problem>), Vec<Problem>> {
        parse_layer(Path::new("t.toml"), text, &gc(), false)
    }

    #[test]
    fn parses_binds_and_options() {
        let (layer, warnings) = parse(
            "[player1]\nvibration = false\n[player1.bind]\nsouth = \"a\"\nl1 = \"@fast_forward_hold\"\nr1 = \"none\"\nright_stick = \"C-stick\"\n[player1.right_stick]\ninvert_x = true\ndeadzone = 0.1\n[player1.triggers]\nmode = \"digital\"\n",
        )
        .unwrap();
        assert!(warnings.is_empty());
        assert_eq!(
            layer.binds,
            [
                (PadInput::South, Target::Control("A".into())),
                (
                    PadInput::L1,
                    Target::Action(EmulatorAction::FastForwardHold)
                ),
                (PadInput::R1, Target::None),
                (PadInput::RightStick, Target::Control("C-stick".into())),
            ]
        );
        assert_eq!(layer.vibration, Some(false));
        assert_eq!(layer.right_stick.invert_x, Some(true));
        assert_eq!(layer.right_stick.deadzone, Some(0.1));
        assert_eq!(layer.left_stick, StickChanges::default());
        assert_eq!(layer.trigger_mode, Some(TriggerMode::Digital));
    }

    #[test]
    fn unknown_names_are_errors_with_lines() {
        let errors =
            parse("[player1.bind]\nsouth = \"A\"\nnorth = \"Zz\"\nfoo = \"B\"\n").unwrap_err();
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].line, Some(3));
        assert!(
            errors[0].message.contains("unknown control 'Zz'"),
            "{}",
            errors[0]
        );
        assert!(errors[0].message.contains("\"C-stick\""));
        assert_eq!(errors[1].line, Some(4));
        assert!(
            errors[1]
                .to_string()
                .starts_with("t.toml:4: unknown input 'foo'")
        );
    }

    #[test]
    fn unknown_keys_and_players_are_errors() {
        let errors = parse("[player1.left_stick]\ninvert_z = true\n").unwrap_err();
        assert_eq!(errors[0].line, Some(2));
        assert!(errors[0].message.contains("invert_z"), "{}", errors[0]);
        let errors = parse("[player2.bind]\nsouth = \"A\"\n").unwrap_err();
        assert!(errors[0].message.contains("player2"), "{}", errors[0]);
    }

    #[test]
    fn wrong_kinds_and_ranges_are_errors() {
        let errors = parse(
            "[player1.bind]\nleft_stick = \"A\"\nsouth = \"Main stick\"\nleft_stick = \"x\"\n",
        );
        assert!(errors.is_err());
        let errors = parse("[player1.right_stick]\ndeadzone = 1.5\n").unwrap_err();
        assert!(errors[0].message.contains("out of range"), "{}", errors[0]);
        let errors = parse("[player1.bind]\nleft_stick = \"@screenshot\"\n").unwrap_err();
        assert!(errors[0].message.contains("can't drive an action"));
    }

    #[test]
    fn directions_bind_to_buttons_and_console_is_shipped_only() {
        let (layer, _) = parse("[player1.bind]\ndpad_up = \"Z\"\nl3 = \"C-stick left\"\n").unwrap();
        assert_eq!(
            layer.binds[1],
            (PadInput::L3, Target::Control("C-stick left".into()))
        );
        let errors =
            parse("[[console]]\nname = \"Q\"\nkind = \"button\"\nlabel = \"Q\"\n").unwrap_err();
        assert!(errors[0].message.contains("[[console]]"), "{}", errors[0]);
    }

    #[test]
    fn guide_and_duplicates_are_warnings() {
        let (layer, warnings) =
            parse("[player1.bind]\nguide = \"Start\"\nsouth = \"A\"\neast = \"A\"\n").unwrap();
        assert_eq!(
            layer.binds,
            [(PadInput::South, Target::Control("A".into()))]
        );
        assert_eq!(warnings.len(), 2);
        assert_eq!(warnings[0].line, Some(2));
        assert!(warnings[1].message.contains("already bound to south"));
    }
}
