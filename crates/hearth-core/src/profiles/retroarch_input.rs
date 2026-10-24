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
    // mupen64plus-next's core options tune the N64 analog stick only.
    stick_tuning: Some(&["Stick"]),
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
        if let Some(raw) = self.virtual_pad_extra(input) {
            return Some(("btn", raw));
        }
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

/// Keys a Hearth copy of a joypad profile keeps: the RetroPad binds and the device match.
/// Everything else (hotkeys, the menu toggle on Guide) is dropped, so no pad button can reach
/// RetroArch's menu.
fn kept_in_copy(key: &str) -> bool {
    const PAD: [&str; 16] = [
        "b", "a", "y", "x", "l", "r", "l2", "r2", "l3", "r3", "start", "select", "up", "down",
        "left", "right",
    ];
    const STICKS: [&str; 8] = [
        "l_x_plus",
        "l_x_minus",
        "l_y_plus",
        "l_y_minus",
        "r_x_plus",
        "r_x_minus",
        "r_y_plus",
        "r_y_minus",
    ];
    if matches!(
        key,
        "input_driver"
            | "input_device"
            | "input_device_display_name"
            | "input_vendor_id"
            | "input_product_id"
    ) {
        return true;
    }
    let Some(bind) = key.strip_prefix("input_") else {
        return false;
    };
    let bind = bind.strip_suffix("_label").unwrap_or(bind);
    PAD.iter()
        .any(|p| [format!("{p}_btn"), format!("{p}_axis")].contains(&bind.to_owned()))
        || STICKS.iter().any(|s| bind == format!("{s}_axis"))
}

/// InputPlumber's virtual Elite 2 pad as RetroArch's `sdl2` joypad driver reads it: SDL
/// GameController buttons (A 0, B 1, X 2, Y 3, Back 4, Guide 5, Start 6, sticks 7–8,
/// shoulders 9–10, D-pad 11–14, Misc1 15, paddles 16–19) and axes (sticks 0–3, triggers 4–5).
/// Hearth's own profile: no hotkey, no menu toggle.
const VIRTUAL_PAD_PROFILE: &str = "\
# Generated by Hearth: InputPlumber's virtual pad for RetroArch's sdl2 joypad driver.
input_driver = \"sdl2\"
input_device = \"Xbox One Elite 2 Controller\"
input_vendor_id = \"1118\"
input_product_id = \"2816\"
input_b_btn = \"0\"
input_a_btn = \"1\"
input_y_btn = \"2\"
input_x_btn = \"3\"
input_select_btn = \"4\"
input_start_btn = \"6\"
input_l3_btn = \"7\"
input_r3_btn = \"8\"
input_l_btn = \"9\"
input_r_btn = \"10\"
input_up_btn = \"11\"
input_down_btn = \"12\"
input_left_btn = \"13\"
input_right_btn = \"14\"
input_l2_axis = \"+4\"
input_r2_axis = \"+5\"
input_l_x_plus_axis = \"+0\"
input_l_x_minus_axis = \"-0\"
input_l_y_plus_axis = \"+1\"
input_l_y_minus_axis = \"-1\"
input_r_x_plus_axis = \"+2\"
input_r_x_minus_axis = \"-2\"
input_r_y_plus_axis = \"+3\"
input_r_y_minus_axis = \"-3\"
";

impl Autoconfig {
    /// Hearth's profile for InputPlumber's virtual pad (`sdl2` driver).
    pub fn virtual_pad() -> Self {
        Self::parse("sdl2/Hearth virtual pad.cfg".into(), VIRTUAL_PAD_PROFILE)
    }

    /// The joypad driver this profile is for (`udev`, `sdl2`).
    pub fn driver(&self) -> &str {
        self.values
            .get("input_driver")
            .map_or("udev", String::as_str)
    }

    /// The file Hearth puts in RetroArch's joypad folder for this profile, relative to the
    /// RetroArch root: `autoconfig/<driver>/<name>`, with only the RetroPad binds and the
    /// device match kept (see [`kept_in_copy`]).
    pub fn hearth_copy(&self) -> (String, String) {
        let name = self
            .path
            .file_name()
            .map_or_else(|| "pad.cfg".into(), |n| n.to_string_lossy().into_owned());
        let mut text = format!(
            "# Generated by Hearth from {}, without hotkeys or the menu toggle.\n",
            self.path.display()
        );
        let mut keys: Vec<_> = self
            .values
            .iter()
            .filter(|(k, _)| kept_in_copy(k))
            .collect();
        keys.sort();
        for (key, value) in keys {
            let _ = writeln!(text, "{key} = \"{value}\"");
        }
        (format!("autoconfig/{}/{name}", self.driver()), text)
    }

    /// Raw button numbers of InputPlumber's virtual Elite 2 pad extras, which the RetroPad binds
    /// don't cover: SDL GameController Misc1 and paddles (checked on InputPlumber 0.82 with
    /// SDL; see `docs/spikes/inputplumber.md`).
    fn virtual_pad_extra(&self, input: PadInput) -> Option<&'static str> {
        let ids = (
            self.values.get("input_vendor_id").map(String::as_str),
            self.values.get("input_product_id").map(String::as_str),
        );
        if ids != (Some("1118"), Some("2816")) || self.driver() != "sdl2" {
            return None;
        }
        Some(match input {
            PadInput::Share => "15",
            PadInput::Paddle1 => "16",
            PadInput::Paddle2 => "17",
            PadInput::Paddle3 => "18",
            PadInput::Paddle4 => "19",
            _ => return None,
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
        // The N64 analog stick's options, whichever physical stick drives it.
        let stick = mapping.stick_options("Stick");
        let step5 = |v: f32, min: f32, max: f32| ((v * 20.0).round() * 5.0).clamp(min, max);
        let deadzone = step5(stick.deadzone, 0.0, 30.0);
        if (deadzone - stick.deadzone * 100.0).abs() > 0.01 {
            out.warnings.push(format!(
                "RetroArch: the Stick deadzone {} becomes {deadzone}% (mupen64plus-next allows 0–30 in steps of 5)",
                stick.deadzone
            ));
        }
        let sensitivity = step5(stick.sensitivity, 50.0, 150.0);
        out.core_options
            .push(("mupen64plus-astick-deadzone".into(), format!("{deadzone}")));
        out.core_options.push((
            "mupen64plus-astick-sensitivity".into(),
            format!("{sensitivity}"),
        ));
    }
    out
}

/// Sets the four half-axes of physical stick `input`: `targets` are the ids for X+, X-, Y+, Y-
/// before inversion, which comes from the console stick it drives.
fn stick_axes(mapping: &Mapping, input: PadInput, targets: [i32; 4], axes: &mut [i32; 8]) {
    let options = mapping.options_of_physical(input);
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
    let first = if input == PadInput::LeftStick { 0 } else { 4 };
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
    use crate::controls::{GameKey, UserControls, resolve};
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
        let resolved = resolve(
            n64,
            &key,
            &UserControls::test_at(user),
            crate::controls::ControllerKind::Generic,
        )
        .unwrap();
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
    fn stick_options_follow_their_stick_and_actions() {
        let mapping = n64_mapping(Some(
            "[player1.bind]\nleft_stick = \"C-buttons\"\nright_stick = \"Stick\"\nl3 = \"@fast_forward_hold\"\nselect = \"@save_state\"\neast = \"C-down\"\nl2 = \"@rewind\"\n[player1.sticks.\"C-buttons\"]\ninvert_y = true\ndeadzone = 0.2\n[player1.sticks.\"Stick\"]\ndeadzone = 0.25\n",
        ));
        let autoconfig = Autoconfig::parse("ds.cfg".into(), DUALSENSE);
        let t = translate(&mapping, &MUPEN64PLUS_NEXT, Some(&autoconfig));
        // The physical right stick drives the N64 stick, the left one the C buttons, with the C
        // buttons' Y inverted; the N64 stick keeps its own deadzone on the right.
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
        assert!(
            t.core_options
                .contains(&("mupen64plus-astick-deadzone".into(), "25".into()))
        );
        assert_eq!(t.warnings.len(), 1, "{:?}", t.warnings);
        assert!(
            t.warnings[0].contains("the C-buttons deadzone"),
            "{:?}",
            t.warnings
        );
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
    fn virtual_pad_extras_have_raw_numbers() {
        let elite = Autoconfig::virtual_pad();
        assert_eq!(elite.driver(), "sdl2");
        assert_eq!(elite.raw(PadInput::Share), Some(("btn", "15")));
        assert_eq!(elite.raw(PadInput::Paddle1), Some(("btn", "16")));
        assert_eq!(elite.raw(PadInput::Paddle4), Some(("btn", "19")));
        assert_eq!(elite.raw(PadInput::Select), Some(("btn", "4")));
        let ds = Autoconfig::parse("ds.cfg".into(), DUALSENSE);
        assert_eq!(ds.raw(PadInput::Paddle1), None);
    }

    #[test]
    fn hearth_copies_drop_hotkeys_and_the_menu_toggle() {
        let ds = Autoconfig::parse(
            "/usr/share/libretro/autoconfig/udev/Sony DualSense.cfg".into(),
            &format!(
                "{DUALSENSE}input_menu_toggle_btn = \"10\"\ninput_enable_hotkey_btn = \"10\"\ninput_l_x_plus_axis = \"+0\"\ninput_b_btn_label = \"Cross\"\n"
            ),
        );
        let (path, text) = ds.hearth_copy();
        assert_eq!(path, "autoconfig/udev/Sony DualSense.cfg");
        assert!(text.contains("input_b_btn = \"0\"\n"));
        assert!(text.contains("input_l_x_plus_axis = \"+0\"\n"));
        assert!(text.contains("input_vendor_id = \"1356\"\n"));
        assert!(text.contains("input_b_btn_label"));
        assert!(!text.contains("input_menu_toggle"), "{text}");
        assert!(!text.contains("input_enable_hotkey"), "{text}");
        let (path, text) = Autoconfig::virtual_pad().hearth_copy();
        assert_eq!(path, "autoconfig/sdl2/Hearth virtual pad.cfg");
        assert!(text.contains("input_driver = \"sdl2\""));
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
