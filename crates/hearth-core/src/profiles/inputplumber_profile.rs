//! Hearth mapping → InputPlumber device profile (`device_profile_v1` YAML).
//!
//! With InputPlumber, remapping happens before the game: the emulator always reads the same
//! virtual pad laid out like the console's shipped default ([`VirtualLayout`]), with emulator
//! actions on spare buttons, and the profile routes each physical input to the virtual input
//! the emulator reads for the user's choice. Loading another profile is instant, so a change
//! made while the game runs applies at once.
//!
//! Checked against InputPlumber 0.82 (`src/config/capability_map.rs`, `src/input/event/value.rs`):
//! a mapped source loses its own meaning (so swaps work), unmapped sources pass through
//! unchanged, and a mapping without targets drops the input. Every physical input is therefore
//! mapped, unbound ones to nothing. What a profile can't express (per-axis inversion, stick
//! deadzone and sensitivity, trigger mode, vibration) stays in the emulator's config.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::controls::layers::{UserControls, UserLayers, resolve_layers};
use crate::controls::model::InputKind;
use crate::controls::{
    ControllerKind, EmulatorAction, GameKey, HearthAction, Mapping, PadInput, Target,
};
use crate::systems::{ConsoleControls, DIRECTIONS, System};

/// D-Bus event the Capture button sends while a game runs (Hearth listens for it).
pub const CAPTURE_EVENT: &str = "hearth_capture";
/// Stick travel or trigger pull that counts as pressing a button.
const PRESS_THRESHOLD: f32 = 0.5;
/// Extras of the virtual pad, in the order emulator actions take them.
const SPARE_EXTRAS: [PadInput; 5] = [
    PadInput::Paddle1,
    PadInput::Paddle2,
    PadInput::Paddle3,
    PadInput::Paddle4,
    PadInput::Share,
];
/// Standard buttons emulator actions may take when the console's layout leaves them free,
/// least used first.
const SPARE_BUTTONS: [PadInput; 10] = [
    PadInput::Select,
    PadInput::L3,
    PadInput::R3,
    PadInput::L1,
    PadInput::R1,
    PadInput::North,
    PadInput::West,
    PadInput::East,
    PadInput::South,
    PadInput::Start,
];

/// Where each console control sits on the virtual pad, and which spare input carries each
/// emulator action. Fixed per system and emulator, so the emulator's config never changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualLayout {
    /// Console control → the virtual input that drives it (the shipped default's binding).
    controls: BTreeMap<String, PadInput>,
    /// Emulator action → the spare virtual input that triggers it.
    spares: BTreeMap<EmulatorAction, PadInput>,
}

impl VirtualLayout {
    /// The layout of `system` for an emulator supporting `actions`: its shipped default
    /// mapping. `None` if the system has no controls.
    pub fn for_system(system: &System, actions: &[EmulatorAction]) -> Option<Self> {
        let key = GameKey::for_rom(&system.id, Path::new(""), "");
        let shipped = resolve_layers(
            system,
            &key,
            &UserControls::none(),
            ControllerKind::Generic,
            UserLayers::NONE,
        )?;
        Some(Self::from_default(&shipped.mapping, actions))
    }

    /// The layout given by a default mapping: each console control stays where it puts it,
    /// and `actions` take the free inputs (extras first).
    pub fn from_default(default: &Mapping, actions: &[EmulatorAction]) -> Self {
        let controls: BTreeMap<String, PadInput> = default
            .binds
            .iter()
            .filter_map(|(input, target)| match target {
                Target::Control(control) => Some((control.clone(), *input)),
                _ => None,
            })
            .collect();
        let used: Vec<PadInput> = default
            .expanded()
            .binds
            .iter()
            .filter(|(_, t)| matches!(t, Target::Control(_)))
            .map(|(i, _)| *i)
            .collect();
        let free = SPARE_EXTRAS
            .into_iter()
            .chain(SPARE_BUTTONS.into_iter().filter(|i| !used.contains(i)));
        let spares = EmulatorAction::ALL
            .into_iter()
            .filter(|a| actions.contains(a))
            .zip(free)
            .collect();
        Self { controls, spares }
    }

    /// The spare input carrying `action`, if the emulator supports it.
    pub fn spare(&self, action: EmulatorAction) -> Option<PadInput> {
        self.spares.get(&action).copied()
    }

    /// What the emulator's config is generated from: the layout's bindings, every supported
    /// action on its spare input, and the user's options (they belong to the console sticks,
    /// which never move on the virtual pad).
    pub fn emulator_mapping(&self, user: &Mapping) -> Mapping {
        let mut binds: BTreeMap<PadInput, Target> = self
            .controls
            .iter()
            .map(|(control, input)| (*input, Target::Control(control.clone())))
            .collect();
        for (action, input) in &self.spares {
            binds.insert(*input, Target::Action(*action));
        }
        Mapping {
            binds,
            sticks: user.sticks.clone(),
            triggers: user.triggers,
            vibration: user.vibration,
        }
    }

    /// Where `control` is on the virtual pad: an input, or one direction of a stick.
    fn locate(&self, control: &str, controls: &ConsoleControls) -> Option<Place> {
        if let Some(input) = self.controls.get(control) {
            return Some(Place::Input(*input));
        }
        if let Some(other) = controls.find(control).and_then(|c| c.same_as) {
            return self.locate(&other, controls);
        }
        let (parent, dir) = controls.split_direction(control)?;
        let dir = DIRECTIONS.iter().position(|d| *d == dir)?;
        match *self.controls.get(&parent.name)? {
            PadInput::Dpad => Some(Place::Input(PadInput::DPAD_DIRECTIONS[dir].0)),
            stick @ (PadInput::LeftStick | PadInput::RightStick) => {
                Some(Place::Direction(stick, DIRECTIONS[dir]))
            }
            _ => None,
        }
    }
}

/// A place on the virtual pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Input(PadInput),
    /// One direction of a stick.
    Direction(PadInput, &'static str),
}

/// A generated profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    /// The YAML document.
    pub yaml: String,
    /// Bindings that couldn't be expressed (each also logged at launch).
    pub warnings: Vec<String>,
}

/// One side of a profile mapping.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Event {
    Button(&'static str),
    Trigger(&'static str, Option<f32>),
    Axis(&'static str, Option<&'static str>, Option<f32>),
    TouchpadPress,
    DBus(&'static str),
}

/// InputPlumber's name for a button-like input (`GamepadButton` in `capability.rs`). Paddle
/// numbers follow SDL: paddle1 is the upper right one.
///
/// InputPlumber names the top and left face buttons by evdev code, not by position: like
/// xpad, Xbox X (left, PlayStation Square) is `BTN_X`, the same code as `BTN_NORTH`, so it is
/// `North`, and Y (top, Triangle) is `West` (its DualSense driver reports Square as `North`,
/// and its Xbox pads write `North` as code 307, which SDL reads as X).
fn button_name(input: PadInput) -> Option<&'static str> {
    Some(match input {
        PadInput::South => "South",
        PadInput::East => "East",
        PadInput::West => "North",
        PadInput::North => "West",
        PadInput::L1 => "LeftBumper",
        PadInput::R1 => "RightBumper",
        PadInput::L3 => "LeftStick",
        PadInput::R3 => "RightStick",
        PadInput::Start => "Start",
        PadInput::Select => "Select",
        PadInput::DpadUp => "DPadUp",
        PadInput::DpadDown => "DPadDown",
        PadInput::DpadLeft => "DPadLeft",
        PadInput::DpadRight => "DPadRight",
        PadInput::Share => "Screenshot",
        PadInput::Paddle1 => "RightPaddle1",
        PadInput::Paddle2 => "LeftPaddle1",
        PadInput::Paddle3 => "RightPaddle2",
        PadInput::Paddle4 => "LeftPaddle2",
        PadInput::Mute => "Mute",
        _ => return None,
    })
}

/// The physical input as a profile source.
fn source(input: PadInput) -> Option<Event> {
    match input {
        PadInput::L2 => Some(Event::Trigger("LeftTrigger", None)),
        PadInput::R2 => Some(Event::Trigger("RightTrigger", None)),
        PadInput::LeftStick => Some(Event::Axis("LeftStick", None, None)),
        PadInput::RightStick => Some(Event::Axis("RightStick", None, None)),
        PadInput::Touchpad => Some(Event::TouchpadPress),
        _ => button_name(input).map(Event::Button),
    }
}

/// Generates the profile routing `mapping`'s physical inputs onto `layout`. `name` names it in
/// InputPlumber (the game's controls key).
pub fn translate(
    mapping: &Mapping,
    layout: &VirtualLayout,
    controls: &ConsoleControls,
    name: &str,
) -> Profile {
    let mut out = Profile::default();
    let mapping = mapping.expanded();
    let mut entries: Vec<(String, Event, Vec<Event>)> = Vec::new();
    for input in PadInput::ALL {
        if input == PadInput::Dpad {
            continue;
        }
        let Some(from) = source(input) else {
            continue;
        };
        let target = mapping.target(input);
        let label = format!(
            "{input}: {}",
            target.map_or_else(|| "none".to_owned(), ToString::to_string)
        );
        match route(input, target, layout, controls) {
            Ok(Routes::One(to)) => {
                // A trigger pressing a button needs the pull that counts as a press.
                let from = match from {
                    Event::Trigger(name, _)
                        if to.iter().any(|t| !matches!(t, Event::Trigger(..))) =>
                    {
                        Event::Trigger(name, Some(PRESS_THRESHOLD))
                    }
                    other => other,
                };
                entries.push((label, from, to));
            }
            Ok(Routes::Directions(each)) => {
                for (dir, to) in each {
                    let from = match from {
                        Event::Axis(stick, _, _) => {
                            Event::Axis(stick, Some(dir), Some(PRESS_THRESHOLD))
                        }
                        other => other,
                    };
                    entries.push((format!("{label} {dir}"), from, vec![to]));
                }
            }
            Err(warning) => {
                out.warnings.push(warning);
                entries.push((label, from, Vec::new()));
            }
        }
        // A trigger may also report a digital press; only the analog value is routed.
        if let Event::Trigger(trigger, _) = from {
            entries.push((
                format!("{input} (digital)"),
                Event::Button(trigger),
                Vec::new(),
            ));
        }
    }
    out.yaml = yaml(name, &entries);
    out
}

/// Where a physical input goes.
enum Routes {
    /// Every event of the input goes to these targets (none: dropped).
    One(Vec<Event>),
    /// A stick split into four directions, each to its own target.
    Directions(Vec<(&'static str, Event)>),
}

fn route(
    input: PadInput,
    target: Option<&Target>,
    layout: &VirtualLayout,
    controls: &ConsoleControls,
) -> Result<Routes, String> {
    let stick = input.kind() == InputKind::Stick;
    let to_button = |place: PadInput| -> Result<Event, String> {
        match place {
            PadInput::L2 => Ok(Event::Trigger("LeftTrigger", None)),
            PadInput::R2 => Ok(Event::Trigger("RightTrigger", None)),
            other => button_name(other)
                .map(Event::Button)
                .ok_or_else(|| format!("InputPlumber: {input} can't drive {other}; dropped")),
        }
    };
    let routes = match target {
        None => Routes::One(Vec::new()),
        Some(Target::None) => Routes::One(Vec::new()),
        Some(Target::Hearth(HearthAction::Capture)) if !stick => {
            Routes::One(vec![Event::DBus(CAPTURE_EVENT)])
        }
        Some(Target::Action(action)) if !stick => {
            let Some(spare) = layout.spare(*action) else {
                return Err(format!(
                    "this emulator has no @{} action; {input} does nothing",
                    action.name()
                ));
            };
            Routes::One(vec![to_button(spare)?])
        }
        Some(Target::Control(control)) => {
            let place = layout.locate(control, controls).ok_or_else(|| {
                format!("InputPlumber: {control} has no place on the virtual pad; {input} dropped")
            })?;
            match (stick, place) {
                (true, Place::Input(to @ (PadInput::LeftStick | PadInput::RightStick))) => {
                    let name = if to == PadInput::LeftStick {
                        "LeftStick"
                    } else {
                        "RightStick"
                    };
                    Routes::One(vec![Event::Axis(name, None, None)])
                }
                (true, Place::Input(PadInput::Dpad)) => Routes::Directions(
                    PadInput::DPAD_DIRECTIONS
                        .iter()
                        .map(|(dir_input, dir)| Ok((*dir, to_button(*dir_input)?)))
                        .collect::<Result<_, String>>()?,
                ),
                (false, Place::Input(to))
                    if to.kind() != InputKind::Stick && to != PadInput::Dpad =>
                {
                    Routes::One(vec![to_button(to)?])
                }
                (false, Place::Direction(to, dir)) => {
                    let name = if to == PadInput::LeftStick {
                        "LeftStick"
                    } else {
                        "RightStick"
                    };
                    Routes::One(vec![Event::Axis(name, Some(dir), None)])
                }
                _ => {
                    return Err(format!(
                        "InputPlumber: {input} can't drive {control}; dropped"
                    ));
                }
            }
        }
        Some(other) => return Err(format!("InputPlumber: {input} can't do {other}; dropped")),
    };
    Ok(routes)
}

/// The profile document.
fn yaml(name: &str, entries: &[(String, Event, Vec<Event>)]) -> String {
    let mut out = String::from(
        "# Generated by Hearth from its controls files; loaded into InputPlumber while the game\n\
         # runs. Edits are overwritten.\nversion: 1\nkind: DeviceProfile\n",
    );
    let _ = writeln!(
        out,
        "name: {}\nmapping:",
        quoted(&format!("Hearth: {name}"))
    );
    for (label, from, to) in entries {
        let _ = writeln!(out, "  - name: {}", quoted(label));
        out.push_str("    source_event:\n");
        event(&mut out, from, false);
        if to.is_empty() {
            out.push_str("    target_events: []\n");
        } else {
            out.push_str("    target_events:\n");
            for target in to {
                event(&mut out, target, true);
            }
        }
    }
    out
}

/// Writes one event under `source_event:` or, as a list item, under `target_events:`.
fn event(out: &mut String, event: &Event, item: bool) {
    let (first, inner) = if item {
        ("      - ", " ".repeat(10))
    } else {
        ("      ", " ".repeat(8))
    };
    let deeper = format!("{inner}  ");
    match event {
        Event::Button(name) => {
            let _ = writeln!(out, "{first}gamepad:\n{inner}button: {name}");
        }
        Event::Trigger(name, deadzone) => {
            let _ = writeln!(
                out,
                "{first}gamepad:\n{inner}trigger:\n{deeper}name: {name}"
            );
            if let Some(d) = deadzone {
                let _ = writeln!(out, "{deeper}deadzone: {d}");
            }
        }
        Event::Axis(name, direction, deadzone) => {
            let _ = writeln!(out, "{first}gamepad:\n{inner}axis:\n{deeper}name: {name}");
            if let Some(dir) = direction {
                let _ = writeln!(out, "{deeper}direction: {dir}");
            }
            if let Some(d) = deadzone {
                let _ = writeln!(out, "{deeper}deadzone: {d}");
            }
        }
        Event::TouchpadPress => {
            let _ = writeln!(
                out,
                "{first}touchpad:\n{inner}name: CenterPad\n{inner}touch:\n{deeper}button: Press"
            );
        }
        Event::DBus(name) => {
            let _ = writeln!(out, "{first}dbus: {name}");
        }
    }
}

/// A YAML double-quoted string.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::resolve;
    use crate::model::SystemId;
    use crate::profiles::input::assert_golden;
    use crate::profiles::{dolphin_input, retroarch_input};
    use crate::systems::Systems;
    use crate::testdir::tempdir;

    fn system(id: &str) -> System {
        Systems::builtin()
            .unwrap()
            .get(&SystemId::new(id))
            .unwrap()
            .clone()
    }

    /// The resolved mapping for a DualSense, with an optional system override file.
    fn mapping(system: &System, user_file: Option<&str>) -> Mapping {
        let dir = tempdir();
        let user = dir.path().join("controls");
        if let Some(text) = user_file {
            std::fs::create_dir_all(user.join("systems")).unwrap();
            std::fs::write(user.join(format!("systems/{}.toml", system.id)), text).unwrap();
        }
        let key = GameKey::for_rom(&system.id, Path::new("none"), "game");
        let resolved = resolve(
            system,
            &key,
            &UserControls::test_at(user),
            ControllerKind::DualSense,
        )
        .unwrap();
        assert!(resolved.problems.is_empty(), "{:?}", resolved.problems);
        resolved.mapping
    }

    fn profile(id: &str, user_file: Option<&str>, actions: &[EmulatorAction]) -> Profile {
        let system = system(id);
        let layout = VirtualLayout::for_system(&system, actions).unwrap();
        translate(&mapping(&system, user_file), &layout, &system.controls, id)
    }

    /// The target lines of the mapping named `name`.
    fn targets<'a>(yaml: &'a str, name: &str) -> &'a str {
        let start = yaml
            .find(&format!("- name: \"{name}"))
            .unwrap_or_else(|| panic!("{name}"));
        let rest = &yaml[start..];
        let from = rest.find("target_events").unwrap();
        let end = rest[1..].find("  - name:").map_or(rest.len(), |e| e + 1);
        &rest[from..end]
    }

    #[test]
    fn gc_default_matches_golden() {
        let p = profile("gc", None, dolphin_input::CAPABILITIES.actions);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        // Identity for the console's buttons, the DualSense extras routed by the pad layer.
        assert!(targets(&p.yaml, "south: A").contains("button: South"));
        assert!(targets(&p.yaml, "select: @capture").contains("dbus: hearth_capture"));
        assert_eq!(targets(&p.yaml, "l1: none").trim(), "target_events: []");
        assert_golden(Path::new("inputplumber/gc_default.yaml"), &p.yaml);
    }

    #[test]
    fn swaps_and_actions_route_to_the_layout() {
        let p = profile(
            "gc",
            Some(
                "[player1.bind]\nsouth = \"B\"\neast = \"A\"\nleft_stick = \"C-stick\"\nright_stick = \"Main stick\"\nl1 = \"@fast_forward_hold\"\nr3 = \"C-stick up\"\nl2 = \"Z\"\nr1 = \"L\"\n",
            ),
            dolphin_input::CAPABILITIES.actions,
        );
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert!(targets(&p.yaml, "south: B").contains("button: East"));
        assert!(targets(&p.yaml, "east: A").contains("button: South"));
        assert!(targets(&p.yaml, "left_stick: C-stick").contains("name: RightStick"));
        assert!(targets(&p.yaml, "right_stick: Main stick").contains("name: LeftStick"));
        // Fast-forward is the first action: the first paddle.
        assert!(targets(&p.yaml, "l1: @fast_forward_hold").contains("button: RightPaddle1"));
        let up = targets(&p.yaml, "r3: C-stick up");
        assert!(
            up.contains("name: RightStick") && up.contains("direction: up"),
            "{up}"
        );
        // A trigger on a button, a button on a trigger.
        assert!(targets(&p.yaml, "l2: Z").contains("button: RightBumper"));
        assert!(p.yaml.contains("name: LeftTrigger\n          deadzone: 0.5\n    target_events:\n      - gamepad:\n          button: RightBumper"));
        assert!(targets(&p.yaml, "r1: L").contains("name: LeftTrigger"));
        assert_golden(Path::new("inputplumber/gc_swapped.yaml"), &p.yaml);
    }

    #[test]
    fn n64_default_matches_golden_and_aliases_resolve() {
        let p = profile("n64", None, retroarch_input::CAPABILITIES.actions);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert_golden(Path::new("inputplumber/n64_default.yaml"), &p.yaml);
        // C-left on a face button is the C-buttons stick (virtual right stick) pushed left.
        let p = profile(
            "n64",
            Some("[player1.bind]\nnorth = \"C-left\"\n"),
            retroarch_input::CAPABILITIES.actions,
        );
        let left = targets(&p.yaml, "north: C-left");
        assert!(
            left.contains("name: RightStick") && left.contains("direction: left"),
            "{left}"
        );
    }

    #[test]
    fn stick_on_the_dpad_splits_into_directions() {
        let p = profile(
            "gc",
            Some("[player1.bind]\nleft_stick = \"D-pad\"\ndpad = \"Main stick\"\n"),
            &[],
        );
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let up = targets(&p.yaml, "left_stick: D-pad up");
        assert!(up.contains("button: DPadUp"), "{up}");
        assert!(p.yaml.contains("direction: up\n          deadzone: 0.5"));
        // A D-pad direction can push a stick.
        let left = targets(&p.yaml, "dpad_left: Main stick left");
        assert!(left.contains("name: LeftStick") && left.contains("direction: left"));
    }

    #[test]
    fn unsupported_action_is_dropped_with_a_warning() {
        let p = profile(
            "gc",
            Some("[player1.bind]\nl1 = \"@rewind\"\n"),
            dolphin_input::CAPABILITIES.actions,
        );
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
        assert!(p.warnings[0].contains("@rewind"));
        assert_eq!(targets(&p.yaml, "l1: @rewind").trim(), "target_events: []");
    }

    #[test]
    fn spare_inputs_avoid_the_layout() {
        let gc =
            VirtualLayout::for_system(&system("gc"), dolphin_input::CAPABILITIES.actions).unwrap();
        assert_eq!(
            gc.spare(EmulatorAction::FastForwardHold),
            Some(PadInput::Paddle1)
        );
        assert_eq!(
            gc.spare(EmulatorAction::Screenshot),
            Some(PadInput::Paddle4)
        );
        assert_eq!(gc.spare(EmulatorAction::Rewind), None);
        // RetroArch has six actions: the extras, then a button N64 doesn't use.
        let n64 = VirtualLayout::for_system(&system("n64"), retroarch_input::CAPABILITIES.actions)
            .unwrap();
        assert_eq!(
            n64.spare(EmulatorAction::FastForwardToggle),
            Some(PadInput::Paddle2)
        );
        assert_eq!(n64.spare(EmulatorAction::Rewind), Some(PadInput::Select));
        // The emulator gets the shipped layout and every action, never the user's swaps.
        let user = mapping(&system("gc"), Some("[player1.bind]\nsouth = \"B\"\n"));
        let emu = gc.emulator_mapping(&user);
        assert_eq!(emu.binds[&PadInput::South], Target::Control("A".into()));
        assert_eq!(
            emu.binds[&PadInput::Paddle1],
            Target::Action(EmulatorAction::FastForwardHold)
        );
    }

    #[test]
    fn every_physical_input_is_mapped_once() {
        let p = profile("gc", None, &[]);
        for input in PadInput::ALL {
            if input == PadInput::Dpad {
                continue;
            }
            assert!(p.yaml.contains(&format!("- name: \"{input}:")), "{input}");
        }
    }
}
