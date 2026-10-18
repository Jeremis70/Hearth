//! The controls page: fills `ControlsData` from a controls editor and applies the player's
//! changes (saved at once). The same page serves a console (Settings, system layer) and the
//! running game (overlay or Home, game layer).

use std::path::PathBuf;
use std::rc::Rc;

use hearth_core::controls::edit::{Editor, Scope, Setting};
use hearth_core::controls::{EmulatorAction, HearthAction, Mapping, PadInput, Target, TriggerMode};
use hearth_core::profiles::input::Capabilities;
use hearth_core::profiles::{dolphin_input, retroarch_input};
use hearth_core::systems::{ControlKind, EmulatorKind, System};
use hearth_platform::input::{Analog, Connection, InputControl, PadInfo, Power};
use slint::{Model, ModelRc, VecModel};

use crate::callouts::{self, Align, Placed, Point};
use crate::controller_layout::{Layout, Side};
use crate::controls::GameInput;
use crate::{Callout, Choice, ControlsData, OptionRow};

/// Pushes a changed mapping to a game that is already running.
///
/// Without InputPlumber the emulator config is regenerated at the next launch only: Dolphin
/// reads its controller config when a game boots (`Core.cpp`, `Pad::LoadConfig`) and has no
/// way to reload it from outside; RetroArch 1.22 loads remap files with the content and its
/// network commands can't load one. With InputPlumber the remapping is a profile that is
/// loaded again as the game comes back to the front ([`InputPlumberApplier`]); options it
/// can't express stay in the emulator config.
pub trait MappingApplier {
    /// Whether a running game picks up remapped buttons right away.
    fn is_live(&self) -> bool;
    /// Applies the running game's controls, resolved again from every layer.
    fn apply(&self, input: &GameInput) -> ApplyOutcome;
}

/// What [`MappingApplier::apply`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The running game uses the new mapping now.
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

    fn apply(&self, _input: &GameInput) -> ApplyOutcome {
        ApplyOutcome::NextLaunch
    }
}

/// The running game's InputPlumber profile, rewritten in place: the input backend loads it
/// every time the game comes back to the front.
pub struct InputPlumberApplier {
    /// Where the running game's profile lives (the path given to the input backend).
    pub path: PathBuf,
    /// The input backend.
    pub input: InputControl,
}

impl MappingApplier for InputPlumberApplier {
    fn is_live(&self) -> bool {
        // False once InputPlumber went away for good (gilrs took over).
        self.input.is_live()
    }

    fn apply(&self, input: &GameInput) -> ApplyOutcome {
        let (true, Some(profile)) = (self.is_live(), &input.profile) else {
            return ApplyOutcome::NextLaunch;
        };
        let written = self
            .path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&self.path, &profile.yaml));
        match written {
            Ok(()) => {
                self.input.set_game_profile(Some(self.path.clone()));
                ApplyOutcome::Applied
            }
            Err(err) => {
                tracing::warn!(path = %self.path.display(), "writing the controller profile failed: {err}");
                ApplyOutcome::NextLaunch
            }
        }
    }
}

/// An open controls page.
pub struct Page {
    editor: Editor,
    /// The running game's title when the page is scoped to it.
    game_title: Option<String>,
    /// Whether the running game uses this page's mapping.
    pub affects_running: bool,
    /// Whether remapped buttons reach a running game at once (InputPlumber). Options only
    /// the emulator can do still wait for the next launch.
    live: bool,
    caps: Capabilities,
    emulator: EmulatorKind,
    layout: Layout,
    /// The controller's extra inputs (touchpad, share, paddles), listed below the drawing.
    extras: Vec<PadInput>,
    /// Something was saved since the page opened.
    pub changed: bool,
    /// An option only the emulator config holds was changed since the page opened.
    pub changed_next_launch: bool,
}

/// What every option that only the emulator config can hold adds to its hint while it would
/// only reach the running game at its next launch.
const NEXT_LAUNCH: &str = "Applies next time you start the game.";

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
            changed_next_launch: false,
        }
    }

    /// Why `input` can't do anything on this page's console, if it can't.
    fn unreadable(&self, input: PadInput) -> Option<&'static str> {
        // InputPlumber routes every physical input onto the virtual pad the emulator reads.
        (input.is_extra() && self.emulator == EmulatorKind::Retroarch && !self.live)
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
        let input = PadInput::from_name(placed.input).expect("the controls page lists pad inputs");
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
        Callout {
            label: label.into(),
            hint: hint.into(),
            unassigned,
            changed: target != below,
            ..geometry(placed)
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
        let extras: Vec<&'static str> = self.extras.iter().map(|i| i.name()).collect();
        let placed = callouts::place(&self.layout, &extras);
        set_geometry(data, &self.layout, &placed);
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
        // Every option below lives in the emulator config: with live remapping it is the only
        // thing still waiting for the next launch, so each option says so.
        let next_launch = self.affects_running && self.live;
        let mut push =
            |id: &str, label: String, value: String, hint: &str, changed: bool, kind: i32| {
                let hint = match (next_launch && id != "reset", hint.ends_with('.')) {
                    (false, _) => hint.to_owned(),
                    (true, true) => format!("{hint} {NEXT_LAUNCH}"),
                    (true, false) => format!("{hint}. {NEXT_LAUNCH}"),
                };
                rows.push(OptionRow {
                    id: id.into(),
                    label: label.into(),
                    value: value.into(),
                    hint: hint.into(),
                    changed,
                    kind,
                });
            };
        // Options belong to the console stick: they follow it to whichever physical stick
        // drives it. Ids: "stick:<console stick>:<option>".
        let (sticks, labels): (Vec<String>, Vec<String>) = self
            .system()
            .controls
            .declared()
            .iter()
            .filter(|c| c.kind == ControlKind::Stick)
            .map(|c| (c.name.clone(), c.label.clone()))
            .unzip();
        for (stick, label) in sticks.iter().zip(&labels) {
            let (opts, base_opts) = (m.stick_options(stick), base.stick_options(stick));
            let place = match m.physical_stick_of(stick) {
                Some(PadInput::LeftStick) => {
                    "It is on the left stick; this follows it if you move it."
                }
                Some(_) => "It is on the right stick; this follows it if you move it.",
                None => "No stick drives it right now.",
            };
            push(
                &format!("stick:{stick}:invert_x"),
                format!("{label}: invert left/right"),
                on_off(opts.invert_x).into(),
                &format!("Flips the {label} left and right. {place}"),
                opts.invert_x != base_opts.invert_x,
                0,
            );
            push(
                &format!("stick:{stick}:invert_y"),
                format!("{label}: invert up/down"),
                on_off(opts.invert_y).into(),
                &format!("Flips the {label} up and down. {place}"),
                opts.invert_y != base_opts.invert_y,
                0,
            );
        }
        for (stick, label) in sticks.iter().zip(&labels) {
            if !self.caps.can_tune(stick) {
                continue;
            }
            let (v, b) = (
                m.stick_options(stick).deadzone,
                base.stick_options(stick).deadzone,
            );
            push(
                &format!("stick:{stick}:deadzone"),
                format!("{label}: deadzone"),
                pct(v),
                "How far the stick must move before the game notices. Raise it if the game moves by itself",
                (v - b).abs() > 0.001,
                1,
            );
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
            // gilrs can't see the touchpad (a separate device); InputPlumber can.
            if input != PadInput::Touchpad || self.live {
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
            self.changed_next_launch = true;
            return Ok("Back to defaults".into());
        }
        let m = self.editor.current().mapping;
        let (max_dz, dz_step) = deadzone_range(emulator_kind(self.system()));
        let stepped = |v: f32, step_size: f32, min: f32, max: f32| {
            let dir = if step < 0 { -1.0 } else { 1.0 };
            ((v / step_size).round() * step_size + dir * step_size).clamp(min, max)
        };
        if let Some((stick, field)) = id.strip_prefix("stick:").and_then(|r| r.rsplit_once(':')) {
            let opts = m.stick_options(stick);
            let stick = stick.to_owned();
            let setting = match field {
                "invert_x" => Setting::InvertX(stick, !opts.invert_x),
                "invert_y" => Setting::InvertY(stick, !opts.invert_y),
                "deadzone" if step == 0 => return Ok(String::new()),
                "deadzone" => {
                    Setting::Deadzone(stick, stepped(opts.deadzone, dz_step, 0.0, max_dz))
                }
                _ => return Err(format!("unknown option {id}")),
            };
            self.editor.set(setting).map_err(|e| e.to_string())?;
            self.changed = true;
            self.changed_next_launch = true;
            return Ok(String::new());
        }
        let setting = match id.split_once('.') {
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
            _ if id == "vibration" => Setting::Vibration(!m.vibration),
            _ => return Err(format!("unknown option {id}")),
        };
        self.editor.set(setting).map_err(|e| e.to_string())?;
        self.changed = true;
        self.changed_next_launch = true;
        Ok(String::new())
    }
}

/// A callout's place on the page, with empty text and no live state.
fn geometry(placed: &Placed) -> Callout {
    let [line_start, elbow, end] = placed.leader.unwrap_or([Point { x: 0.0, y: 0.0 }; 3]);
    Callout {
        input: placed.input.into(),
        label: "".into(),
        hint: "".into(),
        unassigned: false,
        changed: false,
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
        held: false,
        bar: -1.0,
        has_dot: false,
        dot_x: 0.0,
        dot_y: 0.0,
    }
}

/// The drawing's place, the column sizes and where going down from each column lands.
fn set_geometry(data: &ControlsData<'_>, layout: &Layout, placed: &[Placed]) {
    let draw = callouts::drawing(layout);
    data.set_draw_x(draw.x);
    data.set_draw_y(draw.y);
    data.set_draw_w(draw.w);
    data.set_draw_h(draw.h);
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
}

/// The button test: the controls page's drawing and callouts showing each input's live state.
pub struct TestPage {
    placed: Vec<Placed>,
    callouts: Rc<VecModel<Callout>>,
}

impl TestPage {
    /// A test page listing the drawing's inputs and `extras` ("guide", "share", …).
    pub fn new(extras: &[&'static str]) -> Self {
        let placed = callouts::place(&Layout::generic(), extras);
        let callouts = Rc::new(VecModel::from(
            placed.iter().map(geometry).collect::<Vec<_>>(),
        ));
        Self { placed, callouts }
    }

    /// Fills the page; `controller` is the header line (name, connection, battery).
    pub fn fill(&self, data: &ControlsData<'_>, controller: &str) {
        data.set_title("Button test".into());
        data.set_scope_line(controller.into());
        data.set_note("".into());
        data.set_message("".into());
        set_geometry(data, &Layout::generic(), &self.placed);
        data.set_callouts(ModelRc::from(self.callouts.clone()));
    }

    /// Shows what is held (`pressed`, Hearth pad position names) and the raw analog values.
    pub fn update(&self, pressed: &[&str], analog: &Analog) {
        for (row, placed) in self.placed.iter().enumerate() {
            let mut callout = geometry(placed);
            callout.held = pressed.contains(&placed.input);
            let trigger = match placed.input {
                "l2" => Some(analog.l2),
                "r2" => Some(analog.r2),
                _ => None,
            };
            let stick = match placed.input {
                "left_stick" => Some(analog.left),
                "right_stick" => Some(analog.right),
                _ => None,
            };
            if let Some(value) = trigger {
                callout.label = format!(" · {:.0} %", value * 100.0).into();
                callout.bar = value;
            }
            if let Some((x, y)) = stick {
                callout.label = format!(" · {}, {}", number(x), number(y)).into();
                if let Some((center, radius)) = placed.shape {
                    // Up is positive on the stick, down on the screen.
                    callout.has_dot = true;
                    callout.dot_x = center.x + x.clamp(-1.0, 1.0) * radius * 0.8;
                    callout.dot_y = center.y - y.clamp(-1.0, 1.0) * radius * 0.8;
                }
            }
            if self.callouts.row_data(row).as_ref() != Some(&callout) {
                self.callouts.set_row_data(row, callout);
            }
        }
    }
}

/// A raw axis value for display: two decimals, a real minus sign, no "-0.00".
fn number(v: f32) -> String {
    let v = if v.abs() < 0.005 { 0.0 } else { v };
    format!("{v:.2}").replace('-', "\u{2212}")
}

/// The button test's header line: "DualSense Wireless Controller · Bluetooth · Battery 25 %".
pub fn controller_line(name: &str, pad: Option<&PadInfo>) -> String {
    let Some(pad) = pad else {
        return "No controller connected".to_owned();
    };
    let mut parts = vec![name.to_owned()];
    match pad.connection {
        Connection::Usb => parts.push("USB".to_owned()),
        Connection::Bluetooth => parts.push("Bluetooth".to_owned()),
        Connection::Other => {}
    }
    match pad.power {
        Power::Discharging(p) => parts.push(format!("Battery {p} %")),
        Power::Charging(p) => parts.push(format!("Charging {p} %")),
        Power::Charged => parts.push("Fully charged".to_owned()),
        Power::Wired | Power::Unknown => {}
    }
    parts.join(" · ")
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(page: &TestPage, input: &str) -> Callout {
        let i = page.placed.iter().position(|p| p.input == input).unwrap();
        page.callouts.row_data(i).unwrap()
    }

    #[test]
    fn button_test_shows_held_inputs_and_raw_values() {
        let page = TestPage::new(&["guide"]);
        let analog = Analog {
            left: (0.12, -0.03),
            right: (0.0, 0.0),
            l2: 0.43,
            r2: 0.0,
        };
        page.update(&["south", "l3", "guide"], &analog);
        assert!(row(&page, "south").held);
        assert!(row(&page, "l3").held);
        assert!(row(&page, "guide").held);
        assert!(!row(&page, "east").held);
        let l2 = row(&page, "l2");
        assert_eq!(l2.label, " · 43 %");
        assert!((l2.bar - 0.43).abs() < 1e-6);
        assert!(row(&page, "south").bar < 0.0);
        let left = row(&page, "left_stick");
        assert_eq!(left.label, " · 0.12, \u{2212}0.03");
        assert!(left.has_dot);
        // Right of the ring's center and (stick slightly down) slightly below it.
        let right = row(&page, "right_stick");
        let center = page
            .placed
            .iter()
            .find(|p| p.input == "left_stick")
            .and_then(|p| p.shape)
            .unwrap()
            .0;
        assert!(left.dot_x > center.x && left.dot_y > center.y);
        assert_eq!(right.label, " · 0.00, 0.00");
    }

    #[test]
    fn controller_line_names_connection_and_battery() {
        let pad = PadInfo {
            id: "pad".into(),
            name: "PS5 Controller".into(),
            os_name: String::new(),
            vendor_id: None,
            product_id: None,
            connection: Connection::Bluetooth,
            power: Power::Discharging(25),
        };
        assert_eq!(
            controller_line("DualSense Wireless Controller", Some(&pad)),
            "DualSense Wireless Controller · Bluetooth · Battery 25 %"
        );
        assert_eq!(controller_line("x", None), "No controller connected");
    }
}
