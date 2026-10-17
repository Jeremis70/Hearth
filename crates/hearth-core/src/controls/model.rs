//! The Hearth pad, console controls, emulator actions and a resolved mapping.

use std::collections::BTreeMap;
use std::fmt;

/// An input of the Hearth pad, named by position (Nintendo's A is Xbox's B, both are `East`).
/// Matches SDL's gamepad model. Guide is Hearth's and never an input here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PadInput {
    /// Bottom face button (Xbox A, PlayStation Cross).
    South,
    /// Right face button (Xbox B, PlayStation Circle).
    East,
    /// Left face button (Xbox X, PlayStation Square).
    West,
    /// Top face button (Xbox Y, PlayStation Triangle).
    North,
    /// Left bumper.
    L1,
    /// Right bumper.
    R1,
    /// Left trigger (analog).
    L2,
    /// Right trigger (analog).
    R2,
    /// Left stick click.
    L3,
    /// Right stick click.
    R3,
    /// Start / Menu / Options.
    Start,
    /// Select / View / Create.
    Select,
    /// The whole D-pad.
    Dpad,
    /// D-pad up.
    DpadUp,
    /// D-pad down.
    DpadDown,
    /// D-pad left.
    DpadLeft,
    /// D-pad right.
    DpadRight,
    /// The whole left stick.
    LeftStick,
    /// The whole right stick.
    RightStick,
    /// Touchpad click (DualShock 4, DualSense), when present.
    Touchpad,
    /// Share / Capture button (Xbox Series Share, Switch Pro Capture; SDL's misc1), when present.
    Share,
    /// Back paddle 1, when present.
    Paddle1,
    /// Back paddle 2, when present.
    Paddle2,
    /// Back paddle 3, when present.
    Paddle3,
    /// Back paddle 4, when present.
    Paddle4,
}

/// What kind of control an input is, which decides what it can be bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// On/off.
    Button,
    /// Analog 0..1 (also usable as a button).
    Trigger,
    /// Four directions, digital.
    Dpad,
    /// Two analog axes.
    Stick,
}

impl PadInput {
    /// Every input, in file order.
    pub const ALL: [PadInput; 25] = [
        Self::South,
        Self::East,
        Self::West,
        Self::North,
        Self::L1,
        Self::R1,
        Self::L2,
        Self::R2,
        Self::L3,
        Self::R3,
        Self::Start,
        Self::Select,
        Self::Dpad,
        Self::DpadUp,
        Self::DpadDown,
        Self::DpadLeft,
        Self::DpadRight,
        Self::LeftStick,
        Self::RightStick,
        Self::Touchpad,
        Self::Share,
        Self::Paddle1,
        Self::Paddle2,
        Self::Paddle3,
        Self::Paddle4,
    ];

    /// The four D-pad direction inputs with their direction names.
    pub const DPAD_DIRECTIONS: [(PadInput, &'static str); 4] = [
        (Self::DpadUp, "up"),
        (Self::DpadDown, "down"),
        (Self::DpadLeft, "left"),
        (Self::DpadRight, "right"),
    ];

    /// Name used in files.
    pub fn name(self) -> &'static str {
        match self {
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
            Self::North => "north",
            Self::L1 => "l1",
            Self::R1 => "r1",
            Self::L2 => "l2",
            Self::R2 => "r2",
            Self::L3 => "l3",
            Self::R3 => "r3",
            Self::Start => "start",
            Self::Select => "select",
            Self::Dpad => "dpad",
            Self::DpadUp => "dpad_up",
            Self::DpadDown => "dpad_down",
            Self::DpadLeft => "dpad_left",
            Self::DpadRight => "dpad_right",
            Self::LeftStick => "left_stick",
            Self::RightStick => "right_stick",
            Self::Touchpad => "touchpad",
            Self::Share => "share",
            Self::Paddle1 => "paddle1",
            Self::Paddle2 => "paddle2",
            Self::Paddle3 => "paddle3",
            Self::Paddle4 => "paddle4",
        }
    }

    /// Parses a file name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|i| i.name() == name)
    }

    /// The input's kind.
    pub fn kind(self) -> InputKind {
        match self {
            Self::L2 | Self::R2 => InputKind::Trigger,
            Self::Dpad => InputKind::Dpad,
            Self::LeftStick | Self::RightStick => InputKind::Stick,
            _ => InputKind::Button,
        }
    }

    /// Only some controllers have it; ignored when absent.
    pub fn is_extra(self) -> bool {
        matches!(
            self,
            Self::Touchpad
                | Self::Share
                | Self::Paddle1
                | Self::Paddle2
                | Self::Paddle3
                | Self::Paddle4
        )
    }
}

impl fmt::Display for PadInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An optional extra an emulator offers, bound to an otherwise unused button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EmulatorAction {
    /// Run unthrottled while held.
    FastForwardHold,
    /// Toggle unthrottled.
    FastForwardToggle,
    /// Save state slot 1.
    SaveState,
    /// Load state slot 1.
    LoadState,
    /// Screenshot into the emulator's folder.
    Screenshot,
    /// Rewind while held.
    Rewind,
}

impl EmulatorAction {
    /// Every action.
    pub const ALL: [EmulatorAction; 6] = [
        Self::FastForwardHold,
        Self::FastForwardToggle,
        Self::SaveState,
        Self::LoadState,
        Self::Screenshot,
        Self::Rewind,
    ];

    /// Name used in files, without the `@`.
    pub fn name(self) -> &'static str {
        match self {
            Self::FastForwardHold => "fast_forward_hold",
            Self::FastForwardToggle => "fast_forward_toggle",
            Self::SaveState => "save_state",
            Self::LoadState => "load_state",
            Self::Screenshot => "screenshot",
            Self::Rewind => "rewind",
        }
    }

    /// Parses a file name (without the `@`).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.name() == name)
    }

    /// What the UI shows.
    pub fn label(self) -> &'static str {
        match self {
            Self::FastForwardHold => "Fast-forward (hold)",
            Self::FastForwardToggle => "Fast-forward (toggle)",
            Self::SaveState => "Save state",
            Self::LoadState => "Load state",
            Self::Screenshot => "Screenshot",
            Self::Rewind => "Rewind (hold)",
        }
    }

    /// One line explaining it.
    pub fn hint(self) -> &'static str {
        match self {
            Self::FastForwardHold => "Runs the game as fast as possible while held",
            Self::FastForwardToggle => {
                "Press once to run the game as fast as possible, again to stop"
            }
            Self::SaveState => "Saves the game exactly where you are (one slot per game)",
            Self::LoadState => "Goes back to the saved state; unsaved progress is lost",
            Self::Screenshot => "Saves a picture of the screen",
            Self::Rewind => "Plays the game backwards while held",
        }
    }
}

/// A Hearth feature on a controller button. Hearth reads the button itself; it is never sent
/// to the game or the emulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HearthAction {
    /// Screenshots and clips (for now a notice that they're coming).
    Capture,
}

impl HearthAction {
    /// Every Hearth action.
    pub const ALL: [HearthAction; 1] = [Self::Capture];

    /// Name used in files, without the `@`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Capture => "capture",
        }
    }

    /// Parses a file name (without the `@`).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.name() == name)
    }

    /// What the UI shows.
    pub fn label(self) -> &'static str {
        match self {
            Self::Capture => "Capture",
        }
    }

    /// One line explaining it.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Capture => {
                "Hearth's capture button: screenshots and clips. The game never sees this button"
            }
        }
    }
}

/// What an input drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Nothing (explicitly unbound).
    None,
    /// A console control, by its canonical name from `systems.toml`.
    Control(String),
    /// An emulator action.
    Action(EmulatorAction),
    /// A Hearth action: the emulator gets nothing for this input.
    Hearth(HearthAction),
}

impl Target {
    /// Parses an `@name` value (without the `@`): a Hearth or an emulator action.
    pub fn action(name: &str) -> Option<Self> {
        HearthAction::from_name(name)
            .map(Self::Hearth)
            .or_else(|| EmulatorAction::from_name(name).map(Self::Action))
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Control(name) => f.write_str(name),
            Self::Action(action) => write!(f, "@{}", action.name()),
            Self::Hearth(action) => write!(f, "@{}", action.name()),
        }
    }
}

/// Per-stick options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StickOptions {
    /// Flip left/right.
    pub invert_x: bool,
    /// Flip up/down.
    pub invert_y: bool,
    /// Travel ignored around the center, 0.0–0.9.
    pub deadzone: f32,
    /// Output scale, 0.5–2.0 (1.0 = unchanged; above 1 reaches full tilt sooner).
    pub sensitivity: f32,
}

impl Default for StickOptions {
    fn default() -> Self {
        Self {
            invert_x: false,
            invert_y: false,
            deadzone: 0.0,
            sensitivity: 1.0,
        }
    }
}

/// How analog triggers reach the console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TriggerMode {
    /// Analog travel plus a digital click at the threshold (GameCube style).
    #[default]
    Analog,
    /// Fully on past the threshold, off below.
    Digital,
}

/// Trigger options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriggerOptions {
    /// Analog or digital.
    pub mode: TriggerMode,
    /// Travel where the digital press happens, 0.05–1.0.
    pub threshold: f32,
}

impl Default for TriggerOptions {
    fn default() -> Self {
        Self {
            mode: TriggerMode::Analog,
            threshold: 0.9,
        }
    }
}

/// One player's resolved mapping.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapping {
    /// Input → target. Inputs not listed drive nothing.
    pub binds: BTreeMap<PadInput, Target>,
    /// Left stick options (applied to the physical left stick, before swapping).
    pub left_stick: StickOptions,
    /// Right stick options.
    pub right_stick: StickOptions,
    /// Trigger options.
    pub triggers: TriggerOptions,
    /// Rumble.
    pub vibration: bool,
    /// Physical left stick drives what is bound to `right_stick`, and the other way around.
    pub swap_sticks: bool,
}

impl Default for Mapping {
    fn default() -> Self {
        Self {
            binds: BTreeMap::new(),
            left_stick: StickOptions::default(),
            right_stick: StickOptions::default(),
            triggers: TriggerOptions::default(),
            vibration: true,
            swap_sticks: false,
        }
    }
}

impl Mapping {
    /// What `input` drives (after `swap_sticks`), if anything.
    pub fn target(&self, input: PadInput) -> Option<&Target> {
        self.binds.get(&input).filter(|t| **t != Target::None)
    }

    /// The mapping with a whole-D-pad binding (`dpad = "D-pad"`) split into its four
    /// directions (`dpad_up = "D-pad up"`, …). A direction bound on its own keeps its binding.
    pub fn expanded(&self) -> Mapping {
        let mut out = self.clone();
        let Some(whole) = out.binds.remove(&PadInput::Dpad) else {
            return out;
        };
        for (input, dir) in PadInput::DPAD_DIRECTIONS {
            out.binds.entry(input).or_insert_with(|| match &whole {
                Target::Control(name) => Target::Control(format!("{name} {dir}")),
                _ => Target::None,
            });
        }
        out
    }

    /// The physical stick whose movement drives the binding of `stick` (`LeftStick` or
    /// `RightStick`), and that stick's options.
    pub fn stick_source(&self, stick: PadInput) -> (PadInput, StickOptions) {
        let physical = match (stick, self.swap_sticks) {
            (PadInput::LeftStick, false) | (PadInput::RightStick, true) => PadInput::LeftStick,
            _ => PadInput::RightStick,
        };
        let options = if physical == PadInput::LeftStick {
            self.left_stick
        } else {
            self.right_stick
        };
        (physical, options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for input in PadInput::ALL {
            assert_eq!(PadInput::from_name(input.name()), Some(input));
        }
        for action in EmulatorAction::ALL {
            assert_eq!(EmulatorAction::from_name(action.name()), Some(action));
        }
        assert_eq!(PadInput::from_name("guide"), None);
    }

    #[test]
    fn expanding_the_dpad_keeps_explicit_directions() {
        let mut m = Mapping::default();
        m.binds
            .insert(PadInput::Dpad, Target::Control("D-pad".into()));
        m.binds
            .insert(PadInput::DpadUp, Target::Control("Z".into()));
        let e = m.expanded();
        assert_eq!(e.binds.get(&PadInput::Dpad), None);
        assert_eq!(e.binds[&PadInput::DpadUp], Target::Control("Z".into()));
        assert_eq!(
            e.binds[&PadInput::DpadLeft],
            Target::Control("D-pad left".into())
        );
    }

    #[test]
    fn swap_sticks_exchanges_physical_sources() {
        let mut m = Mapping {
            right_stick: StickOptions {
                invert_x: true,
                ..StickOptions::default()
            },
            ..Mapping::default()
        };
        assert_eq!(m.stick_source(PadInput::RightStick).0, PadInput::RightStick);
        m.swap_sticks = true;
        let (physical, options) = m.stick_source(PadInput::LeftStick);
        assert_eq!(physical, PadInput::RightStick);
        assert!(options.invert_x);
    }
}
