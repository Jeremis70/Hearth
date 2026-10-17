//! Hearth mapping → RetroArch remap file, hotkey binds and core options for player 1.
//!
//! RetroArch reads the controller through its joypad autoconfig, which gives RetroPad buttons
//! by position, the same positions as the Hearth pad. A core remap file
//! (`remaps/<core>/<core>.rmp`, RetroArch 1.22.2 `input_remapping_load_file`) then sends each
//! RetroPad input to the RetroPad id the core reads for the console control. Every input is
//! written, since a missing key means "unchanged". `-1` unbinds. Mapping a half-axis onto the
//! opposite half inverts it (`input_driver.c`). Emulator actions are RetroArch hotkeys, which
//! take the controller's raw button numbers, so they come from the device's autoconfig file.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use super::input::{Capabilities, Translation};
use crate::controls::model::InputKind;
use crate::controls::{ControllerIdentity, EmulatorAction, Mapping, PadInput, Target};

/// What the RetroArch translator honors (mupen64plus-next: deadzone and sensitivity only for
/// the N64 stick, through core options).
pub const CAPABILITIES: Capabilities = Capabilities {
    emulator: "RetroArch",
    actions: &[
        EmulatorAction::FastForwardHold,
        EmulatorAction::FastForwardToggle,
        EmulatorAction::SaveState,
        EmulatorAction::LoadState,
        EmulatorAction::Screenshot,
        EmulatorAction::Rewind,
    ],
    stick_deadzone: [true, false],
    stick_sensitivity: [true, false],
    analog_triggers: false,
    vibration: true,
};

// RetroPad ids used by the core tables (libretro.h: B 0, Y 1, Select 2, Start 3, Up 4, Down 5,
// Left 6, Right 7, A 8, X 9, L 10, R 11, L2 12, R2 13, L3 14, R3 15).
const B: i32 = 0;
const Y: i32 = 1;
const START: i32 = 3;
const UP: i32 = 4;
const DOWN: i32 = 5;
const LEFT: i32 = 6;
const RIGHT: i32 = 7;
const L: i32 = 10;
const R: i32 = 11;
const L2: i32 = 12;
/// First analog half-axis id: left X+, X-, Y+, Y-, then right X+, X-, Y+, Y-. Y+ is down.
const ANALOG_LEFT: i32 = 16;
const ANALOG_RIGHT: i32 = 20;
const UNBOUND: i32 = -1;

/// How a core reads a console's controls, as RetroPad ids.
#[derive(Debug, Clone, Copy)]
pub struct CoreTable {
    /// Core's `library_name`, which names the remap folder and file.
    pub library_name: &'static str,
    controls: &'static [(&'static str, CoreInput)],
}

#[derive(Debug, Clone, Copy)]
enum CoreInput {
    Button(i32),
    /// Four ids: up, down, left, right.
    Pad([i32; 4]),
    /// Analog stick, by its first half-axis id.
    Stick(i32),
}

/// mupen64plus-next with `mupen64plus-alt-map = False` (its default):
/// `custom/mupen64plus-core/plugin/emulate_game_controller_via_libretro.c`. The right analog
/// stick drives the C buttons: left → C-left, up → C-up.
pub const MUPEN64PLUS_NEXT: CoreTable = CoreTable {
    library_name: "Mupen64Plus-Next",
    controls: &[
        ("A", CoreInput::Button(B)),
        ("B", CoreInput::Button(Y)),
        ("Z", CoreInput::Button(L2)),
        ("L", CoreInput::Button(L)),
        ("R", CoreInput::Button(R)),
        ("Start", CoreInput::Button(START)),
        ("C-up", CoreInput::Button(ANALOG_RIGHT + 3)),
        ("C-down", CoreInput::Button(ANALOG_RIGHT + 2)),
        ("C-left", CoreInput::Button(ANALOG_RIGHT + 1)),
        ("C-right", CoreInput::Button(ANALOG_RIGHT)),
        ("D-pad", CoreInput::Pad([UP, DOWN, LEFT, RIGHT])),
        ("Stick", CoreInput::Stick(ANALOG_LEFT)),
        ("C-buttons", CoreInput::Stick(ANALOG_RIGHT)),
    ],
};

impl CoreTable {
    /// How the core reads `control`, including a stick's or D-pad's single direction
    /// ("D-pad up" → the Up button, "C-buttons left" → the right stick's X- half-axis).
    fn find(&self, control: &str) -> Option<CoreInput> {
        if let Some((_, input)) = self.controls.iter().find(|(c, _)| *c == control) {
            return Some(*input);
        }
        let (parent, dir) = control.rsplit_once(' ')?;
        let dir = ["up", "down", "left", "right"]
            .iter()
            .position(|d| *d == dir)?;
        match self.controls.iter().find(|(c, _)| *c == parent)?.1 {
            CoreInput::Pad(ids) => Some(CoreInput::Button(ids[dir])),
            // Up = Y-, down = Y+, left = X-, right = X+.
            CoreInput::Stick(base) => Some(CoreInput::Button(base + [3, 2, 1, 0][dir])),
            CoreInput::Button(_) => None,
        }
    }
}

/// The core table for a system, if Hearth knows its core.
pub fn core_table(system: &str) -> Option<CoreTable> {
    (system == "n64").then_some(MUPEN64PLUS_NEXT)
}

/// Remap key suffix of each button-like Hearth input (RetroPad position).
fn remap_key(input: PadInput) -> Option<&'static str> {
    Some(match input {
        PadInput::South => "b",
        PadInput::East => "a",
        PadInput::West => "y",
        PadInput::North => "x",
        PadInput::L1 => "l",
        PadInput::R1 => "r",
        PadInput::L2 => "l2",
        PadInput::R2 => "r2",
        PadInput::L3 => "l3",
        PadInput::R3 => "r3",
        PadInput::Start => "start",
        PadInput::Select => "select",
        PadInput::DpadUp => "up",
        PadInput::DpadDown => "down",
        PadInput::DpadLeft => "left",
        PadInput::DpadRight => "right",
        _ => return None,
    })
}

/// The 16 RetroPad button slots, in the remap file's order.
const BUTTON_SLOTS: [&str; 16] = [
    "b", "y", "select", "start", "up", "down", "left", "right", "a", "x", "l", "r", "l2", "r2",
    "l3", "r3",
];

/// RetroArch hotkey name of an action.
fn hotkey(action: EmulatorAction) -> &'static str {
    match action {
        EmulatorAction::FastForwardHold => "hold_fast_forward",
        EmulatorAction::FastForwardToggle => "toggle_fast_forward",
        EmulatorAction::SaveState => "save_state",
        EmulatorAction::LoadState => "load_state",
        EmulatorAction::Screenshot => "screenshot",
        EmulatorAction::Rewind => "rewind",
    }
}

/// A joypad autoconfig profile: `key = "value"` lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Autoconfig {
    /// The file.
    pub path: PathBuf,
    values: HashMap<String, String>,
}

impl Autoconfig {
    /// Parses an autoconfig file's text.
    pub fn parse(path: PathBuf, text: &str) -> Self {
        let values = text
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once('=')?;
                Some((
                    key.trim().to_owned(),
                    value.trim().trim_matches('"').to_owned(),
                ))
            })
            .collect();
        Self { path, values }
    }

    /// Finds the profile for `device` in `<dir>/udev`: by USB ids, else by name.
    pub fn find(dir: &Path, device: &ControllerIdentity) -> Option<Self> {
        let mut by_name = None;
        for entry in fs::read_dir(dir.join("udev")).ok()?.flatten() {
            let path = entry.path();
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let config = Self::parse(path, &text);
            let id = |key: &str| config.values.get(key).and_then(|v| v.parse::<u16>().ok());
            if device.vendor_id.is_some()
                && id("input_vendor_id") == device.vendor_id
                && id("input_product_id") == device.product_id
            {
                return Some(config);
            }
            let named = config
                .values
                .iter()
                .any(|(k, v)| k.starts_with("input_device") && *v == device.os_name);
            if named && by_name.is_none() {
                by_name = Some(config);
            }
        }
        by_name
    }

    /// Raw `(key suffix, value)` binding of a Hearth input: `("btn", "4")` or `("axis", "+2")`.
    fn raw(&self, input: PadInput) -> Option<(&'static str, &str)> {
        let name = match input {
            PadInput::South => "b",
            PadInput::East => "a",
            PadInput::West => "y",
            PadInput::North => "x",
            PadInput::L1 => "l",
            PadInput::R1 => "r",
            PadInput::L2 => "l2",
            PadInput::R2 => "r2",
            PadInput::L3 => "l3",
            PadInput::R3 => "r3",
            PadInput::Start => "start",
            PadInput::Select => "select",
            PadInput::DpadUp => "up",
            PadInput::DpadDown => "down",
            PadInput::DpadLeft => "left",
            PadInput::DpadRight => "right",
            _ => return None,
        };
        ["btn", "axis"].into_iter().find_map(|kind| {
            self.values
                .get(&format!("input_{name}_{kind}"))
                .map(|v| (kind, v.as_str()))
        })
    }
}

/// Generates the remap file, hotkey settings and core options for `mapping` on `core`.
pub fn translate(
    mapping: &Mapping,
    core: &CoreTable,
    autoconfig: Option<&Autoconfig>,
) -> Translation {
    let mut out = Translation::default();
    out.warnings.extend(CAPABILITIES.check(mapping));
    let mapping = &mapping.expanded();
    let mut buttons: HashMap<&str, i32> = BUTTON_SLOTS.iter().map(|s| (*s, UNBOUND)).collect();
    // Physical stick half-axes: [l_x+, l_x-, l_y+, l_y-, r_x+, r_x-, r_y+, r_y-].
    let mut axes = [UNBOUND; 8];
    let mut any_action = false;

    for (input, target) in &mapping.binds {
        let control = match target {
            // Hearth reads its own buttons; RetroArch gets nothing for them.
            Target::None | Target::Hearth(_) => continue,
            Target::Action(action) => {
                if !CAPABILITIES.actions.contains(action) {
                    continue;
                }
                any_action = true;
                bind_hotkey(*action, *input, autoconfig, &mut out);
                continue;
            }
            Target::Control(control) => control,
        };
        let Some(core_input) = core.find(control) else {
            out.warnings.push(format!(
                "RetroArch: {} has no {control}; {input} skipped",
                core.library_name
            ));
            continue;
        };
        match (input.kind(), core_input) {
            (InputKind::Button | InputKind::Trigger, CoreInput::Button(id)) => {
                if let Some(slot) = remap_key(*input) {
                    buttons.insert(slot, id);
                } else {
                    out.warnings
                        .push(format!("RetroArch can't read {input}; it does nothing"));
                }
            }
            (InputKind::Dpad, CoreInput::Pad(ids)) => {
                for (slot, id) in ["up", "down", "left", "right"].into_iter().zip(ids) {
                    buttons.insert(slot, id);
                }
            }
            (InputKind::Dpad, CoreInput::Stick(base)) => {
                // Up = Y-, down = Y+, left = X-, right = X+.
                for (slot, id) in ["up", "down", "left", "right"].into_iter().zip([
                    base + 3,
                    base + 2,
                    base + 1,
                    base,
                ]) {
                    buttons.insert(slot, id);
                }
            }
            (InputKind::Stick, CoreInput::Stick(base)) => {
                stick_axes(
                    mapping,
                    *input,
                    [base, base + 1, base + 2, base + 3],
                    &mut axes,
                );
            }
            (InputKind::Stick, CoreInput::Pad([up, down, left, right])) => {
                stick_axes(mapping, *input, [right, left, down, up], &mut axes);
            }
            _ => out
                .warnings
                .push(format!("RetroArch: {input} can't drive {control}; skipped")),
        }
    }

    let mut rmp = String::from(
        "# Generated by Hearth from its controls files on every launch; edits are overwritten.\n",
    );
    for slot in BUTTON_SLOTS {
        let _ = writeln!(rmp, "input_player1_btn_{slot} = \"{}\"", buttons[slot]);
    }
    for (name, id) in [
        "l_x+", "l_x-", "l_y+", "l_y-", "r_x+", "r_x-", "r_y+", "r_y-",
    ]
    .into_iter()
    .zip(axes)
    {
        let _ = writeln!(rmp, "input_player1_stk_{name} = \"{id}\"");
    }
    rmp.push_str("input_remap_port_p1 = \"0\"\ninput_player1_analog_dpad_mode = \"0\"\n");
    let folder = core.library_name;
    out.files
        .push((format!("remaps/{folder}/{folder}.rmp"), rmp));

    let set = |out: &mut Translation, k: &str, v: &str| out.settings.push((k.into(), v.into()));
    set(&mut out, "input_remap_binds_enable", "true");
    set(&mut out, "auto_remaps_enable", "true");
    set(&mut out, "remap_save_on_exit", "false");
    set(&mut out, "input_remap_sort_by_controller_enable", "false");
    if any_action {
        // Hotkeys need no enable button (bound to a nonexistent one by default).
        set(&mut out, "input_enable_hotkey_btn", "nul");
        set(&mut out, "input_enable_hotkey_axis", "nul");
    }
    let rewind = mapping
        .binds
        .values()
        .any(|t| *t == Target::Action(EmulatorAction::Rewind));
    set(
        &mut out,
        "rewind_enable",
        if rewind { "true" } else { "false" },
    );
    set(
        &mut out,
        "input_rumble_gain",
        if mapping.vibration { "100" } else { "0" },
    );

    // mupen64plus-next stick options: 0–30 % deadzone, 50–150 % sensitivity, steps of 5.
    if core.library_name == MUPEN64PLUS_NEXT.library_name {
        let left = mapping.left_stick;
        let step5 = |v: f32, min: f32, max: f32| ((v * 20.0).round() * 5.0).clamp(min, max);
        let deadzone = step5(left.deadzone, 0.0, 30.0);
        if (deadzone - left.deadzone * 100.0).abs() > 0.01 {
            out.warnings.push(format!(
                "RetroArch: left_stick.deadzone {} becomes {deadzone}% (mupen64plus-next allows 0–30 in steps of 5)",
                left.deadzone
            ));
        }
        let sensitivity = step5(left.sensitivity, 50.0, 150.0);
        out.core_options
            .push(("mupen64plus-astick-deadzone".into(), format!("{deadzone}")));
        out.core_options.push((
            "mupen64plus-astick-sensitivity".into(),
            format!("{sensitivity}"),
        ));
    }
    out
}

/// Sets the four half-axes of the physical stick behind `input`: `targets` are the ids for
/// X+, X-, Y+, Y- before inversion.
fn stick_axes(mapping: &Mapping, input: PadInput, targets: [i32; 4], axes: &mut [i32; 8]) {
    let (physical, options) = mapping.stick_source(input);
    let [x_plus, x_minus, y_plus, y_minus] = targets;
    let (x_plus, x_minus) = if options.invert_x {
        (x_minus, x_plus)
    } else {
        (x_plus, x_minus)
    };
    let (y_plus, y_minus) = if options.invert_y {
        (y_minus, y_plus)
    } else {
        (y_plus, y_minus)
    };
    let first = if physical == PadInput::LeftStick {
        0
    } else {
        4
    };
    axes[first..first + 4].copy_from_slice(&[x_plus, x_minus, y_plus, y_minus]);
}

fn bind_hotkey(
    action: EmulatorAction,
    input: PadInput,
    autoconfig: Option<&Autoconfig>,
    out: &mut Translation,
) {
    let name = hotkey(action);
    let Some(autoconfig) = autoconfig else {
        out.warnings.push(format!(
            "RetroArch: no joypad profile for this controller, so @{} on {input} can't be bound",
            action.name()
        ));
        return;
    };
    match autoconfig.raw(input) {
        Some((kind, value)) => {
            out.settings
                .push((format!("input_{name}_{kind}"), value.to_owned()));
        }
        None => out.warnings.push(format!(
            "RetroArch: {} doesn't define {input}, so @{} can't be bound",
            autoconfig.path.display(),
            action.name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{GameKey, resolve};
    use crate::model::SystemId;
    use crate::profiles::input::assert_golden;
    use crate::systems::Systems;
    use crate::testdir::tempdir;

    const DUALSENSE: &str = "input_device = \"Sony Interactive Entertainment DualSense Wireless Controller\"\ninput_vendor_id = \"1356\"\ninput_product_id = \"3302\"\ninput_b_btn = \"0\"\ninput_a_btn = \"1\"\ninput_l_btn = \"4\"\ninput_l2_axis = \"+2\"\ninput_select_btn = \"8\"\ninput_l3_btn = \"11\"\n";

    fn n64_mapping(override_text: Option<&str>) -> Mapping {
        let dir = tempdir();
        let user = dir.path().join("controls");
        if let Some(text) = override_text {
            fs::create_dir_all(user.join("systems")).unwrap();
            fs::write(user.join("systems/n64.toml"), text).unwrap();
        }
        let systems = Systems::builtin().unwrap();
        let n64 = systems.get(&SystemId::new("n64")).unwrap();
        let key = GameKey::for_rom(&n64.id, &dir.path().join("none"), "OoT");
        let resolved = resolve(n64, &key, &user, crate::controls::ControllerKind::Generic).unwrap();
        assert!(resolved.problems.is_empty(), "{:?}", resolved.problems);
        resolved.mapping
    }

    fn rmp(t: &Translation) -> &str {
        &t.files[0].1
    }

    fn setting<'a>(t: &'a Translation, key: &str) -> Option<&'a str> {
        t.settings
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn n64_default_is_the_identity_remap() {
        let t = translate(&n64_mapping(None), &MUPEN64PLUS_NEXT, None);
        assert!(t.warnings.is_empty(), "{:?}", t.warnings);
        assert_eq!(t.files[0].0, "remaps/Mupen64Plus-Next/Mupen64Plus-Next.rmp");
        // Every bound RetroPad input keeps its own id; the rest are unbound.
        for (slot, id) in [
            ("b", 0),
            ("y", 1),
            ("start", 3),
            ("up", 4),
            ("l", 10),
            ("r", 11),
            ("l2", 12),
        ] {
            assert!(
                rmp(&t).contains(&format!("input_player1_btn_{slot} = \"{id}\"\n")),
                "{slot}"
            );
        }
        for slot in ["a", "x", "r2", "select", "l3", "r3"] {
            assert!(rmp(&t).contains(&format!("input_player1_btn_{slot} = \"-1\"\n")));
        }
        for (i, half) in [
            "l_x+", "l_x-", "l_y+", "l_y-", "r_x+", "r_x-", "r_y+", "r_y-",
        ]
        .iter()
        .enumerate()
        {
            assert!(rmp(&t).contains(&format!("input_player1_stk_{half} = \"{}\"\n", 16 + i)));
        }
        assert_eq!(
            t.core_options,
            [
                ("mupen64plus-astick-deadzone".into(), "15".into()),
                ("mupen64plus-astick-sensitivity".into(), "100".into())
            ]
        );
        assert_eq!(setting(&t, "remap_save_on_exit"), Some("false"));
        assert_eq!(setting(&t, "input_enable_hotkey_btn"), None);
        assert_golden(
            Path::new("retroarch/n64_default/Mupen64Plus-Next.rmp"),
            rmp(&t),
        );
    }

    #[test]
    fn inversion_swap_and_actions() {
        let mapping = n64_mapping(Some(
            "[player1]\nswap_sticks = true\n[player1.bind]\nl3 = \"@fast_forward_hold\"\nselect = \"@save_state\"\neast = \"C-down\"\nl2 = \"@rewind\"\n[player1.left_stick]\ninvert_y = true\n[player1.right_stick]\ndeadzone = 0.2\n",
        ));
        let autoconfig = Autoconfig::parse("ds.cfg".into(), DUALSENSE);
        let t = translate(&mapping, &MUPEN64PLUS_NEXT, Some(&autoconfig));
        // Swapped: the physical right stick drives the N64 stick, the left one the C buttons,
        // with the left stick's Y inverted.
        assert!(rmp(&t).contains("input_player1_stk_r_x+ = \"16\"\n"));
        assert!(
            rmp(&t).contains("input_player1_stk_l_y+ = \"23\"\ninput_player1_stk_l_y- = \"22\"\n")
        );
        assert!(rmp(&t).contains("input_player1_btn_a = \"22\"\n"));
        // The Z trigger moved away from l2 (now @rewind), so l2 is unbound in the remap.
        assert!(rmp(&t).contains("input_player1_btn_l2 = \"-1\"\n"));
        assert_eq!(setting(&t, "input_hold_fast_forward_btn"), Some("11"));
        assert_eq!(setting(&t, "input_save_state_btn"), Some("8"));
        assert_eq!(setting(&t, "input_rewind_axis"), Some("+2"));
        assert_eq!(setting(&t, "rewind_enable"), Some("true"));
        assert_eq!(setting(&t, "input_enable_hotkey_btn"), Some("nul"));
        assert_eq!(t.warnings.len(), 1, "{:?}", t.warnings);
        assert!(t.warnings[0].contains("right_stick.deadzone"));
    }

    #[test]
    fn autoconfig_found_by_ids_then_name() {
        let dir = tempdir();
        fs::create_dir_all(dir.path().join("udev")).unwrap();
        fs::write(dir.path().join("udev/ds.cfg"), DUALSENSE).unwrap();
        let found = Autoconfig::find(dir.path(), &ControllerIdentity::fallback()).unwrap();
        assert_eq!(found.raw(PadInput::L1), Some(("btn", "4")));
        let by_name = ControllerIdentity::from_pad(
            "Sony Interactive Entertainment DualSense Wireless Controller",
            None,
            None,
        );
        assert!(Autoconfig::find(dir.path(), &by_name).is_some());
        let other = ControllerIdentity::from_pad("Other", Some(1), Some(2));
        assert!(Autoconfig::find(dir.path(), &other).is_none());
    }

    #[test]
    fn capture_button_is_unbound_for_retroarch() {
        let mapping = n64_mapping(Some(
            "[player1.bind]\nsouth = \"@capture\"\nselect = \"@save_state\"\n",
        ));
        let auto = Autoconfig::parse("ds.cfg".into(), DUALSENSE);
        let t = translate(&mapping, &MUPEN64PLUS_NEXT, Some(&auto));
        assert!(t.warnings.is_empty(), "{:?}", t.warnings);
        // South is RetroPad B, which drives N64 A by default: now nothing.
        assert!(rmp(&t).contains("input_player1_btn_b = \"-1\"\n"));
        // Only the real action became a hotkey (the Create button, raw 8).
        assert_eq!(setting(&t, "input_save_state_btn"), Some("8"));
        assert!(
            !t.settings.iter().any(|(_, v)| v == "0"),
            "{:?}",
            t.settings
        );
    }
}
