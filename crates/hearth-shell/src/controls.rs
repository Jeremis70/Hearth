//! A game's controls at launch: resolve its layers, pick the controller, translate for its
//! emulator. Shared by the session (every launch) and `--print-controls`.

use std::fmt::Write as _;
use std::path::Path;

use hearth_core::controls::layers::stick_setting;
use hearth_core::controls::{
    ControllerIdentity, GameKey, PadInput, Resolved, UserControls, resolve,
};
use hearth_core::model::Game;
use hearth_core::profiles::input::Translation;
use hearth_core::profiles::inputplumber_profile::{self, Profile, VirtualLayout};
use hearth_core::profiles::retroarch_input::{self, Autoconfig};
use hearth_core::profiles::{RetroArchProfile, dolphin_input};
use hearth_core::systems::{ControlKind, EmulatorKind, System};
use hearth_platform::input::PadInfo;

/// Which controller the emulator reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadTarget {
    /// The real controller (gilrs): the emulator's config does the remapping.
    Physical,
    /// InputPlumber's virtual pad: the emulator's config is fixed (the console's default
    /// layout) and an InputPlumber profile does the remapping.
    Virtual,
}

/// Everything about a game's controls for one launch.
#[derive(Debug, Clone)]
pub struct GameInput {
    /// Per-game key (`gc/GZ2E01`).
    pub key: GameKey,
    /// Resolved mapping, `None` if the system declares no controls.
    pub resolved: Option<Resolved>,
    /// The console's sticks (canonical names), whose options the mapping holds.
    pub sticks: Vec<String>,
    /// Player 1's controller.
    pub device: ControllerIdentity,
    /// The controller the emulator reads (`device`, or InputPlumber's virtual pad).
    pub emulator_device: ControllerIdentity,
    /// Generated emulator configuration.
    pub translation: Translation,
    /// InputPlumber profile doing the remapping ([`PadTarget::Virtual`]).
    pub profile: Option<Profile>,
    /// Where the RetroArch joypad profile came from, if used.
    pub autoconfig: Option<std::path::PathBuf>,
}

/// Player 1's controller: the first connected pad, else the fallback; then `device.toml` in
/// `device_dir` (shared by every profile).
pub fn device(pads: &[PadInfo], device_dir: &Path) -> ControllerIdentity {
    let identity = match pads.first() {
        Some(pad) => ControllerIdentity::from_pad(&pad.os_name, pad.vendor_id, pad.product_id),
        None => {
            tracing::warn!("no controller connected; generating controls for the default pad");
            ControllerIdentity::fallback()
        }
    };
    identity.with_overrides(device_dir)
}

/// Resolves and translates `game`'s controls for `kind` with `user`'s files, for the emulator
/// reading `pad`.
pub fn game_input(
    system: &System,
    game: &Game,
    kind: EmulatorKind,
    user: &UserControls,
    device: ControllerIdentity,
    retroarch: &RetroArchProfile,
    pad: PadTarget,
) -> GameInput {
    let key = GameKey::for_rom(&system.id, &game.path, &game.title);
    let resolved = resolve(system, &key, user, device.kind);
    let user_mapping = resolved.as_ref().map(|r| &r.mapping);
    let actions = match kind {
        EmulatorKind::Dolphin => dolphin_input::CAPABILITIES.actions,
        EmulatorKind::Retroarch => retroarch_input::CAPABILITIES.actions,
    };
    // With the virtual pad the emulator always gets the console's default layout; the
    // user's mapping becomes the profile.
    let (emulator_mapping, profile, emulator_device) = match pad {
        PadTarget::Physical => (user_mapping.cloned(), None, device.clone()),
        PadTarget::Virtual => {
            let layout = VirtualLayout::for_system(system, actions);
            match (user_mapping, layout) {
                (Some(mapping), Some(layout)) => (
                    Some(layout.emulator_mapping(mapping)),
                    Some(inputplumber_profile::translate(
                        mapping,
                        &layout,
                        &system.controls,
                        &key.as_string(),
                    )),
                    ControllerIdentity::virtual_pad(),
                ),
                _ => (None, None, ControllerIdentity::virtual_pad()),
            }
        }
    };
    let mapping = emulator_mapping.as_ref();
    let mut autoconfig = None;
    let mut translation = match kind {
        EmulatorKind::Dolphin => {
            // Only GameCube controls exist for now; Wii games get an unbound pad.
            let mapping = mapping.filter(|_| system.id.as_str() == "gc");
            dolphin_input::translate(mapping, &emulator_device)
        }
        EmulatorKind::Retroarch => {
            // RetroArch reads only the joypad profile Hearth gives it (never a built-in one,
            // whose Guide opens its menu): the virtual pad's, or the physical pad's copy.
            let found = match pad {
                PadTarget::Virtual => Some(Autoconfig::virtual_pad()),
                PadTarget::Physical => {
                    Autoconfig::find(retroarch.autoconfig_dir(), &emulator_device)
                }
            };
            autoconfig = found.as_ref().map(|a| a.path.clone());
            let mut translation = match (mapping, retroarch_input::core_table(system.id.as_str())) {
                (Some(mapping), Some(core)) => {
                    retroarch_input::translate(mapping, &core, found.as_ref())
                }
                _ => Translation {
                    warnings: vec![format!(
                        "RetroArch: no controls translation for {}; using RetroArch's defaults",
                        system.name
                    )],
                    ..Translation::default()
                },
            };
            match &found {
                Some(found) => {
                    translation.files.push(found.hearth_copy());
                    translation
                        .settings
                        .push(("input_joypad_driver".into(), found.driver().into()));
                }
                None => translation.warnings.push(format!(
                    "RetroArch: no joypad profile for \"{}\"; the controller won't work in the game",
                    emulator_device.os_name
                )),
            }
            translation
        }
    };
    if pad == PadTarget::Virtual {
        translation.env.extend(only_the_virtual_pad());
    }
    if let Some(profile) = &profile {
        translation
            .warnings
            .extend(profile.warnings.iter().cloned());
    }
    let sticks = system
        .controls
        .declared()
        .iter()
        .filter(|c| c.kind == ControlKind::Stick)
        .map(|c| c.name.clone())
        .collect();
    GameInput {
        key,
        resolved,
        sticks,
        device,
        emulator_device,
        translation,
        profile,
        autoconfig,
    }
}

/// The emulator sees InputPlumber's virtual pad and nothing else: no raw controller (it shows
/// up for a moment when a pad reconnects, before InputPlumber hides it) and no other virtual
/// device. SDL's allowlist, honored by Dolphin (SDL3) and RetroArch's `sdl2` driver.
fn only_the_virtual_pad() -> Vec<(String, String)> {
    let pad = ControllerIdentity::virtual_pad();
    let id = |v: Option<u16>| v.unwrap_or_default();
    vec![(
        "SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT".into(),
        format!("0x{:04x}/0x{:04x}", id(pad.vendor_id), id(pad.product_id)),
    )]
}

impl GameInput {
    /// Logs problems and warnings; returns a short message for a toast if a file had errors.
    pub fn report(&self) -> Option<String> {
        for problem in self.resolved.iter().flat_map(|r| &r.problems) {
            tracing::warn!("controls: {problem}");
        }
        for warning in &self.translation.warnings {
            tracing::warn!("controls: {warning}");
        }
        let resolved = self.resolved.as_ref()?;
        resolved.has_errors().then(|| {
            let first = resolved
                .problems
                .first()
                .map(ToString::to_string)
                .unwrap_or_default();
            format!("Controls file skipped: {first}")
        })
    }

    /// Human-readable dump: mapping with the layer of each value, then the generated files.
    pub fn describe(&self, game: &Game) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{} [{}]", game.title, self.key.as_string());
        let _ = writeln!(out, "rom: {}", game.path.display());
        let _ = writeln!(
            out,
            "controller: {:?} \"{}\" (SDL: \"{}\")",
            self.device.kind, self.device.os_name, self.device.sdl_name
        );
        if self.emulator_device != self.device {
            let _ = writeln!(
                out,
                "emulator reads: \"{}\" (SDL: \"{}\"), remapped by InputPlumber",
                self.emulator_device.os_name, self.emulator_device.sdl_name
            );
        }
        if let Some(path) = &self.autoconfig {
            let _ = writeln!(out, "joypad profile: {}", path.display());
        }
        match &self.resolved {
            None => out.push_str("\nno controls for this system\n"),
            Some(r) => {
                out.push_str("\nlayers:\n");
                for source in &r.applied {
                    let _ = writeln!(out, "  {source}");
                }
                out.push_str("\nbindings:\n");
                for input in PadInput::ALL {
                    if let Some(target) = r.mapping.binds.get(&input) {
                        let path = format!("bind.{input}");
                        let _ = writeln!(
                            out,
                            "  {:<12} → {:<22} {}",
                            input.name(),
                            target.to_string(),
                            r.source(&path)
                        );
                    }
                }
                out.push_str("\noptions:\n");
                let m = &r.mapping;
                let options: [(&str, String); 3] = [
                    ("vibration", m.vibration.to_string()),
                    (
                        "triggers.mode",
                        format!("{:?}", m.triggers.mode).to_lowercase(),
                    ),
                    ("triggers.threshold", m.triggers.threshold.to_string()),
                ];
                for (name, value) in &options {
                    let _ = writeln!(out, "  {name:<24} = {value:<8} {}", r.source(name));
                }
                // Stick options belong to the console stick and follow it.
                out.push_str("\nsticks:\n");
                for stick in &self.sticks {
                    let on = m
                        .physical_stick_of(stick)
                        .map_or("not bound".to_owned(), |i| format!("on {i}"));
                    let _ = writeln!(out, "  {stick} ({on})");
                    let o = m.stick_options(stick);
                    for (field, value) in [
                        ("invert_x", o.invert_x.to_string()),
                        ("invert_y", o.invert_y.to_string()),
                        ("deadzone", o.deadzone.to_string()),
                        ("sensitivity", o.sensitivity.to_string()),
                    ] {
                        let _ = writeln!(
                            out,
                            "    {field:<22} = {value:<8} {}",
                            r.source(&stick_setting(stick, field))
                        );
                    }
                }
                if !r.problems.is_empty() {
                    out.push_str("\nproblems:\n");
                    for p in &r.problems {
                        let _ = writeln!(out, "  {p}");
                    }
                }
            }
        }
        if !self.translation.warnings.is_empty() {
            out.push_str("\ntranslation warnings:\n");
            for w in &self.translation.warnings {
                let _ = writeln!(out, "  {w}");
            }
        }
        for (path, text) in &self.translation.files {
            let _ = writeln!(out, "\n--- {path}");
            out.push_str(text);
        }
        if let Some(profile) = &self.profile {
            out.push_str("\n--- InputPlumber profile\n");
            out.push_str(&profile.yaml);
        }
        for (key, value) in &self.translation.env {
            let _ = writeln!(out, "\nenvironment: {key}={value}");
        }
        if !self.translation.settings.is_empty() {
            out.push_str("\n--- retroarch.cfg (input settings)\n");
            for (k, v) in &self.translation.settings {
                let _ = writeln!(out, "{k} = \"{v}\"");
            }
        }
        if !self.translation.core_options.is_empty() {
            out.push_str("\n--- config/retroarch-core-options.cfg (set keys)\n");
            for (k, v) in &self.translation.core_options {
                let _ = writeln!(out, "{k} = \"{v}\"");
            }
        }
        out
    }
}
