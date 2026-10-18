//! The controls page: fills `ControlsData` from a controls editor and applies the player's
//! changes (saved at once). The same page serves a console (Settings, system layer) and the
//! running game (overlay or Home, game layer).

use std::rc::Rc;

use hearth_core::controls::edit::{Editor, Scope, Setting, Stick};
use hearth_core::controls::{EmulatorAction, HearthAction, Mapping, PadInput, Target, TriggerMode};
use hearth_core::profiles::input::Capabilities;
use hearth_core::profiles::{dolphin_input, retroarch_input};
use hearth_core::systems::{ControlKind, EmulatorKind, System};
use slint::{ModelRc, VecModel};

use crate::callouts::{self, Align, Placed, Point};
use crate::controller_layout::{Layout, Side};
use crate::{Callout, Choice, ControlsData, OptionRow, TestInput};

/// Pushes a changed mapping to a game that is already running.
///
/// Today's implementation regenerates the emulator config at the next launch only: Dolphin
/// reads its controller config when a game boots (`Core.cpp`, `Pad::LoadConfig`) and has no
/// way to reload it from outside; RetroArch 1.22 loads remap files with the content and its
/// network commands can't load one. InputPlumber can remap live (next round).
pub trait MappingApplier {
    /// Whether a running game picks up changes right away.
    fn is_live(&self) -> bool;
    /// Applies `mapping` to the running game of `system`.
    fn apply(&self, system: &System, mapping: &Mapping) -> ApplyOutcome;
}

/// What [`MappingApplier::apply`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The running game uses the new mapping now.
    #[expect(
        dead_code,
        reason = "produced by live appliers (InputPlumber, next round)"
    )]
    Applied,
    /// The game uses it from its next launch.
    NextLaunch,
}

/// Emulator config files, regenerated at every launch (`crate::controls::game_input`).
pub struct EmulatorConfigApplier;

impl MappingApplier for EmulatorConfigApplier {
    fn is_live(&self) -> bool {
        false
    }

    fn apply(&self, _system: &System, _mapping: &Mapping) -> ApplyOutcome {
        ApplyOutcome::NextLaunch
    }
}

/// An open controls page.
pub struct Page {
    editor: Editor,
    /// The running game's title when the page is scoped to it.
    game_title: Option<String>,
    /// Whether the running game uses this page's mapping.
    affects_running: bool,
    /// Whether changes reach a running game at once.
    live: bool,
    caps: Capabilities,
    emulator: EmulatorKind,
    layout: Layout,
    /// The controller's extra inputs (touchpad, share, paddles), listed below the drawing.
    extras: Vec<PadInput>,
    /// Something was saved since the page opened.
    pub changed: bool,
}

/// Deadzone steps the emulator can do: (maximum, step).
fn deadzone_range(kind: EmulatorKind) -> (f32, f32) {
    match kind {
        // mupen64plus-next's core option: 0–30 % in steps of 5.
        EmulatorKind::Retroarch => (0.30, 0.05),
        EmulatorKind::Dolphin => (0.50, 0.05),
    }
}

fn capabilities(kind: EmulatorKind) -> Capabilities {
    match kind {
        EmulatorKind::Dolphin => dolphin_input::CAPABILITIES,
        EmulatorKind::Retroarch => retroarch_input::CAPABILITIES,
    }
}

fn emulator_kind(system: &System) -> EmulatorKind {
    system
        .emulator()
        .map_or(EmulatorKind::Retroarch, |e| e.kind)
}

impl Page {
    /// A page for `editor`. `game_title` set: scoped to that (running) game. `live`: the
    /// applier reaches a running game at once ([`MappingApplier::is_live`]).
    pub fn new(
        editor: Editor,
        game_title: Option<String>,
        affects_running: bool,
        live: bool,
        extras: Vec<PadInput>,
    ) -> Self {
        let emulator = emulator_kind(editor.system());
        Self {
            editor,
            game_title,
            affects_running,
            live,
            caps: capabilities(emulator),
            emulator,
            layout: Layout::generic(),
            extras,
            changed: false,
        }
    }

    /// Why `input` can't do anything on this page's console, if it can't.
    fn unreadable(&self, input: PadInput) -> Option<&'static str> {
        (input.is_extra() && self.emulator == EmulatorKind::Retroarch)
            .then_some("RetroArch can't read this button, so it does nothing in these games.")
    }

    fn system(&self) -> &System {
        self.editor.system()
    }

    fn short(&self) -> &str {
        &self.system().short
    }

    /// What a target is called on this console.
    fn target_label(&self, target: &Target) -> Option<(String, String)> {
        match target {
            Target::None => None,
            Target::Action(a) => Some((a.label().to_owned(), a.hint().to_owned())),
            Target::Hearth(a) => Some((a.label().to_owned(), a.hint().to_owned())),
            Target::Control(name) => self
                .system()
                .controls
                .find(name)
                .map(|c| (c.label.clone(), c.hint.clone().unwrap_or_default())),
        }
    }

    fn callout(&self, placed: &Placed, current: &Mapping, base: &Mapping) -> Callout {
        let input = placed.input;
        let target = current.binds.get(&input).cloned().unwrap_or(Target::None);
        let below = base.binds.get(&input).cloned().unwrap_or(Target::None);
        let (label, hint, unassigned) = match self.target_label(&target) {
            _ if self.unreadable(input).is_some() => (
                "—".to_owned(),
                self.unreadable(input).unwrap_or_default().to_owned(),
                true,
            ),
            Some((label, hint)) => (label, hint, false),
            None => (
                "—".to_owned(),
                "Does nothing in this game. Choose Change to give it a function.".to_owned(),
                true,
            ),
        };
        let [line_start, elbow, end] = placed.leader.unwrap_or([Point { x: 0.0, y: 0.0 }; 3]);
        Callout {
            input: input.name().into(),
            label: label.into(),
            hint: hint.into(),
            unassigned,
            changed: target != below,
            side: match placed.side {
                Side::Left => 0,
                Side::Right => 1,
                Side::Bottom => 2,
            },
            glyph_last: placed.align == Align::GlyphLast,
            bx: placed.label.x,
            by: placed.label.y,
            bw: placed.label.w,
            bh: placed.label.h,
            has_line: placed.leader.is_some(),
            lx: line_start.x,
            ly: line_start.y,
            ex: elbow.x,
            ey: elbow.y,
            ax: end.x,
            ay: end.y,
        }
    }

    /// Fills the page's data.
    pub fn fill(&self, data: &ControlsData<'_>) {
        let resolved = self.editor.current();
        let current = resolved.mapping.expanded();
        let base_mapping = self.editor.base();
        let base = base_mapping.expanded();
        let system = self.system().name.clone();
        data.set_title(self.short().into());
        let scope = match &self.game_title {
            Some(title) => format!("{title} only"),
            None => format!("All {} games", self.short()),
        };
        data.set_scope_line(scope.into());
        data.set_note(
            if self.affects_running && !self.live {
                "Applies next time you start the game"
            } else {
                ""
            }
            .into(),
        );
        data.set_changed_label(
            match self.editor.scope() {
                Scope::System => "Changed from Hearth's default",
                Scope::Game => "Set for this game only",
            }
            .into(),
        );
        data.set_reset_message(
            match self.editor.scope() {
                Scope::System => {
                    format!("Every {system} control and option goes back to Hearth's defaults.")
                }
                Scope::Game => format!("This game goes back to your {} settings.", self.short()),
            }
            .into(),
        );
        let draw = callouts::drawing(&self.layout);
        data.set_draw_x(draw.x);
        data.set_draw_y(draw.y);
        data.set_draw_w(draw.w);
        data.set_draw_h(draw.h);
        let placed = callouts::place(&self.layout, &self.extras);
        let count = |side| {
            let n = placed.iter().filter(|p| p.side == side).count();
            i32::try_from(n).unwrap_or(0)
        };
        data.set_left_count(count(Side::Left));
        data.set_right_count(count(Side::Right));
        // Going down from a column lands on the bottom callout nearest the middle on that side.
        let first_bottom = count(Side::Left) + count(Side::Right);
        let middle = callouts::PAGE_W / 2.0;
        let bottom: Vec<_> = placed.iter().filter(|p| p.side == Side::Bottom).collect();
        let left_of_middle = bottom
            .iter()
            .filter(|p| p.label.x + p.label.w / 2.0 < middle)
            .count();
        let at = |i: usize| first_bottom + i32::try_from(i).unwrap_or(0);
        data.set_down_left(at(left_of_middle.saturating_sub(1)));
        data.set_down_right(at(left_of_middle.min(bottom.len().saturating_sub(1))));
        data.set_callouts(model(
            placed
                .iter()
                .map(|p| self.callout(p, &current, &base))
                .collect(),
        ));
        data.set_options(model(self.options(&resolved.mapping, &base_mapping)));
    }

    fn options(&self, m: &Mapping, base: &Mapping) -> Vec<OptionRow> {
        let mut rows = Vec::new();
        let on_off = |b: bool| if b { "On" } else { "Off" };
        let pct = |v: f32| format!("{:.0} %", v * 100.0);
        let mut push =
            |id: &str, label: String, value: String, hint: &str, changed: bool, kind: i32| {
                rows.push(OptionRow {
                    id: id.into(),
                    label: label.into(),
                    value: value.into(),
                    hint: hint.into(),
                    changed,
                    kind,
                });
            };
        for (stick, name, opts, base_opts) in [
            ("left_stick", "Left stick", m.left_stick, base.left_stick),
            (
                "right_stick",
                "Right stick",
                m.right_stick,
                base.right_stick,
            ),
        ] {
            let what = self.stick_function(m, stick);
            push(
                &format!("{stick}.invert_x"),
                format!("{name}: invert left/right"),
                on_off(opts.invert_x).into(),
                &format!("Flips {what} left and right"),
                opts.invert_x != base_opts.invert_x,
                0,
            );
            push(
                &format!("{stick}.invert_y"),
                format!("{name}: invert up/down"),
                on_off(opts.invert_y).into(),
                &format!("Flips {what} up and down"),
                opts.invert_y != base_opts.invert_y,
                0,
            );
        }
        push(
            "swap_sticks",
            "Swap sticks".into(),
            on_off(m.swap_sticks).into(),
            "The left stick does what the right one did, and the other way around",
            m.swap_sticks != base.swap_sticks,
            0,
        );
        for (i, (stick, name, v, b)) in [
            (
                "left_stick",
                "Left stick",
                m.left_stick.deadzone,
                base.left_stick.deadzone,
            ),
            (
                "right_stick",
                "Right stick",
                m.right_stick.deadzone,
                base.right_stick.deadzone,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if self.caps.stick_deadzone[i] {
                push(
                    &format!("{stick}.deadzone"),
                    format!("{name}: deadzone"),
                    pct(v),
                    "How far the stick must move before the game notices. Raise it if the game moves by itself",
                    (v - b).abs() > 0.001,
                    1,
                );
            }
        }
        let has_triggers = self
            .system()
            .controls
            .declared()
            .iter()
            .any(|c| c.kind == ControlKind::Trigger);
        if self.caps.analog_triggers && has_triggers {
            push(
                "triggers.mode",
                "Triggers".into(),
                match m.triggers.mode {
                    TriggerMode::Analog => "Analog",
                    TriggerMode::Digital => "Digital",
                }
                .into(),
                "Analog: pressure counts, like the original. Digital: fully pressed past the click point",
                m.triggers.mode != base.triggers.mode,
                1,
            );
            push(
                "triggers.threshold",
                "Trigger click point".into(),
                pct(m.triggers.threshold),
                "How far a trigger goes down before it clicks",
                (m.triggers.threshold - base.triggers.threshold).abs() > 0.001,
                1,
            );
        }
        if self.caps.vibration {
            push(
                "vibration",
                "Vibration".into(),
                on_off(m.vibration).into(),
                "Rumble when the game asks for it",
                m.vibration != base.vibration,
                0,
            );
        }
        let reset = match self.editor.scope() {
            Scope::System => format!("Reset {} to defaults", self.short()),
            Scope::Game => format!("Reset this game to {} defaults", self.short()),
        };
        push(
            "reset",
            reset,
            String::new(),
            "Undo every change on this page",
            false,
            2,
        );
        rows
    }

    /// "the camera" style description of what a physical stick drives.
    fn stick_function(&self, m: &Mapping, stick: &str) -> String {
        let input = if stick == "left_stick" {
            PadInput::LeftStick
        } else {
            PadInput::RightStick
        };
        let driving = [PadInput::LeftStick, PadInput::RightStick]
            .into_iter()
            .find(|i| m.stick_source(*i).0 == input)
            .and_then(|i| m.target(i).cloned());
        match driving.and_then(|t| self.target_label(&t)) {
            Some((label, _)) => format!("the {label}"),
            None => "this stick".into(),
        }
    }

    /// Everything `input` can do, with the index of its current target.
    pub fn choices(&self, input: PadInput) -> (Vec<Choice>, usize) {
        let current = self.editor.current().mapping.expanded();
        let now = current.binds.get(&input).cloned().unwrap_or(Target::None);
        let mut targets: Vec<Target> = Vec::new();
        let controls = &self.system().controls;
        let stick_like = matches!(input, PadInput::LeftStick | PadInput::RightStick);
        let usable = self.unreadable(input).is_none();
        for c in controls.declared().iter().filter(|_| usable) {
            let fits = match c.kind {
                ControlKind::Stick | ControlKind::Dpad => stick_like,
                ControlKind::Button | ControlKind::Trigger => !stick_like,
            };
            if fits {
                targets.push(Target::Control(c.name.clone()));
            }
        }
        if !stick_like && usable {
            for c in controls.all().into_iter().skip(controls.declared().len()) {
                targets.push(Target::Control(c.name));
            }
            // Hearth reads buttons through gilrs, which can't see the touchpad.
            if input != PadInput::Touchpad {
                targets.extend(HearthAction::ALL.map(Target::Hearth));
            }
            for action in EmulatorAction::ALL {
                if self.caps.actions.contains(&action) {
                    targets.push(Target::Action(action));
                }
            }
        }
        targets.push(Target::None);
        let index = targets.iter().position(|t| *t == now).unwrap_or(0);
        let choices = targets
            .iter()
            .map(|t| {
                let (label, hint) = self.target_label(t).unwrap_or_else(|| {
                    (
                        "Nothing".to_owned(),
                        "The button does nothing in the game".to_owned(),
                    )
                });
                Choice {
                    id: t.to_string().into(),
                    label: label.into(),
                    hint: hint.into(),
                    current: *t == now,
                }
            })
            .collect();
        (choices, index)
    }

    /// Puts the choice `id` on `input`; returns feedback for the page.
    pub fn choose(&mut self, input: PadInput, id: &str) -> Result<String, String> {
        let target = match id {
            "none" => Target::None,
            _ => id
                .strip_prefix('@')
                .and_then(Target::action)
                .unwrap_or_else(|| Target::Control(id.to_owned())),
        };
        let label = |t: &Target| {
            self.target_label(t)
                .map_or_else(|| "Nothing".to_owned(), |(l, _)| l)
        };
        let new_label = label(&target);
        let done = self
            .editor
            .assign(input, target)
            .map_err(|e| e.to_string())?;
        let message = match done.swapped {
            Some((_, Target::None)) => format!("{new_label} moved to this button"),
            Some((_, previous)) => format!("{} and {} swapped", label(&previous), new_label),
            None => String::new(),
        };
        self.changed = true;
        Ok(message)
    }

    /// Changes option `id` by `step` (-1/+1, 0 = toggle or activate).
    pub fn change_option(&mut self, id: &str, step: i32) -> Result<String, String> {
        if id == "reset" {
            self.editor.reset().map_err(|e| e.to_string())?;
            self.changed = true;
            return Ok("Back to defaults".into());
        }
        let m = self.editor.current().mapping;
        let stick = |name: &str| {
            if name == "left_stick" {
                Stick::Left
            } else {
                Stick::Right
            }
        };
        let (max_dz, dz_step) = deadzone_range(emulator_kind(self.system()));
        let stepped = |v: f32, step_size: f32, min: f32, max: f32| {
            let dir = if step < 0 { -1.0 } else { 1.0 };
            ((v / step_size).round() * step_size + dir * step_size).clamp(min, max)
        };
        let setting = match id.split_once('.') {
            Some((s, "invert_x")) => {
                let opts = if s == "left_stick" {
                    m.left_stick
                } else {
                    m.right_stick
                };
                Setting::InvertX(stick(s), !opts.invert_x)
            }
            Some((s, "invert_y")) => {
                let opts = if s == "left_stick" {
                    m.left_stick
                } else {
                    m.right_stick
                };
                Setting::InvertY(stick(s), !opts.invert_y)
            }
            Some((s, "deadzone")) => {
                if step == 0 {
                    return Ok(String::new());
                }
                let opts = if s == "left_stick" {
                    m.left_stick
                } else {
                    m.right_stick
                };
                Setting::Deadzone(stick(s), stepped(opts.deadzone, dz_step, 0.0, max_dz))
            }
            Some(("triggers", "mode")) => Setting::TriggerMode(match m.triggers.mode {
                TriggerMode::Analog => TriggerMode::Digital,
                TriggerMode::Digital => TriggerMode::Analog,
            }),
            Some(("triggers", "threshold")) => {
                if step == 0 {
                    return Ok(String::new());
                }
                Setting::TriggerThreshold(stepped(m.triggers.threshold, 0.05, 0.5, 1.0))
            }
            _ if id == "swap_sticks" => Setting::SwapSticks(!m.swap_sticks),
            _ if id == "vibration" => Setting::Vibration(!m.vibration),
            _ => return Err(format!("unknown option {id}")),
        };
        self.editor.set(setting).map_err(|e| e.to_string())?;
        self.changed = true;
        Ok(String::new())
    }

    /// Applies the page's mapping to the running game, if it uses it.
    pub fn apply(&self, applier: &dyn MappingApplier) -> Option<ApplyOutcome> {
        self.affects_running
            .then(|| applier.apply(self.system(), &self.editor.current().mapping))
    }
}

/// The button test's inputs, lit when held.
pub fn test_inputs(pressed: &[&str]) -> ModelRc<TestInput> {
    let layout = Layout::generic();
    model(
        layout
            .anchors
            .iter()
            .filter(|a| a.input != PadInput::L3 && a.input != PadInput::R3)
            .map(|a| {
                let name = a.input.name();
                let held = pressed.contains(&name)
                    || (a.input == PadInput::LeftStick && pressed.contains(&"l3"))
                    || (a.input == PadInput::RightStick && pressed.contains(&"r3"));
                TestInput {
                    input: name.into(),
                    ax: a.x,
                    ay: a.y,
                    pressed: held,
                }
            })
            .collect(),
    )
}

/// The drawing size, for the button test.
pub fn set_drawing(data: &ControlsData<'_>) {
    let layout = Layout::generic();
    data.set_drawing_width(layout.width);
    data.set_drawing_height(layout.height);
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}
