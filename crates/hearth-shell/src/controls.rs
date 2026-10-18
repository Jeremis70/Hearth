//! A game's controls at launch: resolve its layers, pick the controller, translate for its
//! emulator. Shared by the session (every launch) and `--print-controls`.

use std::fmt::Write as _;
use std::path::Path;

use hearth_core::controls::layers::stick_setting;
use hearth_core::controls::{ControllerIdentity, GameKey, PadInput, Resolved, resolve};
use hearth_core::model::Game;
use hearth_core::profiles::input::Translation;
use hearth_core::profiles::retroarch_input::{self, Autoconfig};
use hearth_core::profiles::{RetroArchProfile, dolphin_input};
use hearth_core::systems::{ControlKind, EmulatorKind, System};
use hearth_platform::input::PadInfo;

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
    /// Generated emulator configuration.
    pub translation: Translation,
    /// Where the RetroArch joypad profile came from, if used.
    pub autoconfig: Option<std::path::PathBuf>,
}

/// Player 1's controller: the first connected pad, else the fallback; then `device.toml`.
pub fn device(pads: &[PadInfo], controls_dir: &Path) -> ControllerIdentity {
    let identity = match pads.first() {
        Some(pad) => ControllerIdentity::from_pad(&pad.os_name, pad.vendor_id, pad.product_id),
        None => {
            tracing::warn!("no controller connected; generating controls for the default pad");
            ControllerIdentity::fallback()
        }
    };
    identity.with_overrides(controls_dir)
}

/// Resolves and translates `game`'s controls for `kind`.
pub fn game_input(
    system: &System,
    game: &Game,
    kind: EmulatorKind,
    controls_dir: &Path,
    device: ControllerIdentity,
    retroarch: &RetroArchProfile,
) -> GameInput {
    let key = GameKey::for_rom(&system.id, &game.path, &game.title);
    let resolved = resolve(system, &key, controls_dir, device.kind);
    let mapping = resolved.as_ref().map(|r| &r.mapping);
    let mut autoconfig = None;
    let translation = match kind {
        EmulatorKind::Dolphin => {
            // Only GameCube controls exist for now; Wii games get an unbound pad.
            let mapping = mapping.filter(|_| system.id.as_str() == "gc");
            dolphin_input::translate(mapping, &device)
        }
        EmulatorKind::Retroarch => {
            match (mapping, retroarch_input::core_table(system.id.as_str())) {
                (Some(mapping), Some(core)) => {
                    let found = Autoconfig::find(retroarch.autoconfig_dir(), &device);
                    autoconfig = found.as_ref().map(|a| a.path.clone());
                    retroarch_input::translate(mapping, &core, found.as_ref())
                }
                _ => Translation {
                    warnings: vec![format!(
                        "RetroArch: no controls translation for {}; using RetroArch's defaults",
                        system.name
                    )],
                    ..Translation::default()
                },
            }
        }
    };
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
        translation,
        autoconfig,
    }
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
