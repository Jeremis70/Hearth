//! RetroArch profile, fully isolated from the user's own RetroArch setup.
//!
//! Hearth generates its own `retroarch.cfg` under `root` and launches
//! `retroarch --config <cfg> -L <core> <rom>`, so `~/.config/retroarch` is never read or
//! written. Every writable directory lives under `root` (the user profile's RetroArch folder),
//! except the game saves (the profile's `saves/<system>/`, one flat folder per system, whatever
//! the core) and the system directory (BIOS files, shared by every profile). Joypad profiles
//! too: RetroArch
//! reads `<root>/autoconfig`, where each launch puts only the profile Hearth chose (InputPlumber's
//! virtual pad, or a copy of the system profile of the physical pad), so no built-in profile
//! ever loads.
//!
//! The Guide button must only ever reach Hearth. Autoconfig profiles bind it to the menu
//! (`input_menu_toggle_btn`), and RetroArch falls back to the profile whenever our bind is
//! `nul`, so every hotkey's gamepad bind is set to a button that doesn't exist instead.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::input::Translation;
use crate::model::SystemId;
use crate::runtime::LaunchCommand;
use crate::users::ProfileDirs;

/// Where Arch's `retroarch-autoconfig-udev` package installs joypad profiles.
pub const SYSTEM_AUTOCONFIG_DIR: &str = "/usr/share/libretro/autoconfig";

/// Gamepad button index no controller has: binding hotkeys to it disables them without
/// falling back to autoconfig binds.
const NO_SUCH_BUTTON: &str = "999";
/// Axis no controller has, for the same reason.
const NO_SUCH_AXIS: &str = "+99";

/// RetroArch 1.22 hotkeys (`DECLARE_META_BIND` in `input/input_driver.c`).
const HOTKEYS: &[&str] = &[
    "ai_service",
    "audio_mute",
    "cheat_index_minus",
    "cheat_index_plus",
    "cheat_toggle",
    "close_content",
    "desktop_menu_toggle",
    "disk_eject_toggle",
    "disk_next",
    "disk_prev",
    "enable_hotkey",
    "exit_emulator",
    "fps_toggle",
    "frame_advance",
    "game_focus_toggle",
    "grab_mouse_toggle",
    "halt_replay",
    "hold_fast_forward",
    "hold_slowmotion",
    "load_state",
    "menu_toggle",
    "netplay_fade_chat_toggle",
    "netplay_game_watch",
    "netplay_host_toggle",
    "netplay_ping_toggle",
    "netplay_player_chat",
    "next_replay_checkpoint",
    "osk_toggle",
    "overlay_next",
    "pause_toggle",
    "play_replay",
    "preempt_toggle",
    "prev_replay_checkpoint",
    "record_replay",
    "recording_toggle",
    "replay_slot_decrease",
    "replay_slot_increase",
    "reset",
    "rewind",
    "runahead_toggle",
    "save_replay_checkpoint",
    "save_state",
    "screenshot",
    "shader_hold",
    "shader_next",
    "shader_prev",
    "shader_toggle",
    "state_slot_decrease",
    "state_slot_increase",
    "streaming_toggle",
    "toggle_fast_forward",
    "toggle_fullscreen",
    "toggle_slowmotion",
    "toggle_statistics",
    "toggle_vrr_runloop",
    "turbo_fire_toggle",
    "volume_down",
    "volume_up",
];

/// Writable subdirectories of the profile root, by config key. (`savefile_directory` and
/// `system_directory` are set apart: see [`RetroArchProfile`].)
const DIRS: &[(&str, &str)] = &[
    ("savestate_directory", "states"),
    ("rgui_config_directory", "config"),
    ("input_remapping_directory", "remaps"),
    ("log_dir", "logs"),
    ("screenshot_directory", "screenshots"),
    ("cache_directory", "cache"),
    ("playlist_directory", "playlists"),
    ("thumbnails_directory", "thumbnails"),
    ("core_assets_directory", "downloads"),
    ("recording_output_directory", "recordings"),
    ("runtime_log_directory", "logs/runtime"),
    ("cheat_database_path", "cheats"),
];

/// Writable files under the profile root, by config key.
const FILES: &[(&str, &str)] = &[
    ("core_options_path", "config/retroarch-core-options.cfg"),
    ("content_history_path", "playlists/history.lpl"),
    ("content_favorites_path", "playlists/favorites.lpl"),
    ("content_music_history_path", "playlists/music_history.lpl"),
    ("content_video_history_path", "playlists/video_history.lpl"),
    ("content_image_history_path", "playlists/image_history.lpl"),
];

/// Settings that make RetroArch a silent, fullscreen part of Hearth.
const SETTINGS: &[(&str, &str)] = &[
    ("config_save_on_exit", "false"),
    ("video_fullscreen", "true"),
    ("video_windowed_fullscreen", "true"),
    // gl + X11 context: works inside gamescope's Xwayland and on a desktop's Xwayland.
    ("video_driver", "gl"),
    ("video_context_driver", "x"),
    ("input_joypad_driver", "udev"),
    ("input_autodetect_enable", "true"),
    ("input_menu_toggle_gamepad_combo", "0"),
    ("input_quit_gamepad_combo", "0"),
    ("quit_press_twice", "false"),
    ("pause_nonactive", "false"),
    ("menu_pause_libretro", "true"),
    ("global_core_options", "true"),
    ("history_list_enable", "false"),
    ("content_runtime_log", "false"),
    ("content_runtime_log_aggregate", "false"),
    // retroarch.log under <root>/logs: shows video init and joypad autoconfiguration.
    ("log_verbosity", "true"),
    ("frontend_log_level", "1"),
    ("libretro_log_level", "1"),
    ("log_to_file", "true"),
    ("log_to_file_timestamp", "false"),
    ("network_cmd_enable", "false"),
    ("menu_show_load_content_animation", "false"),
    ("notification_show_autoconfig", "false"),
    ("video_font_enable", "false"),
    ("savestate_auto_save", "false"),
    ("savestate_auto_load", "false"),
    // Saves go straight into `saves/<system>/`, not into a folder per core or per ROM folder.
    ("sort_savefiles_enable", "false"),
    ("sort_savefiles_by_content_enable", "false"),
];

/// A RetroArch installation driven by Hearth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchProfile {
    program: OsString,
    root: PathBuf,
    /// Game saves, one folder per system.
    saves: PathBuf,
    /// BIOS and core system files.
    system_dir: PathBuf,
    autoconfig_dir: PathBuf,
}

impl RetroArchProfile {
    /// Profile writing everything under `root` (saves in `root/saves/<system>`, BIOS files in
    /// `root/system`), reading joypad profiles from the system. Paths are made absolute.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = absolute(root.into());
        Self {
            program: "retroarch".into(),
            saves: root.join("saves"),
            system_dir: root.join("system"),
            root,
            autoconfig_dir: PathBuf::from(SYSTEM_AUTOCONFIG_DIR),
        }
    }

    /// The RetroArch of user profile `user`: its own folder and saves, the shared BIOS folder.
    pub fn for_profile(user: &ProfileDirs) -> Self {
        Self {
            saves: user.saves(),
            system_dir: user.retroarch_system(),
            ..Self::new(user.retroarch())
        }
    }

    /// RetroArch's folder (config, states, remaps, logs).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where games of `system` save (`.srm` files).
    pub fn saves_dir(&self, system: &SystemId) -> PathBuf {
        self.saves.join(system.as_str())
    }

    /// BIOS and core system files.
    pub fn system_dir(&self) -> &Path {
        &self.system_dir
    }

    /// Uses another (read-only) folder of system joypad profiles.
    pub fn with_autoconfig_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.autoconfig_dir = dir.into();
        self
    }

    /// The generated config file.
    pub fn config_path(&self) -> PathBuf {
        self.root.join("retroarch.cfg")
    }

    /// The system's joypad profiles (read-only), which Hearth copies from.
    pub fn autoconfig_dir(&self) -> &Path {
        &self.autoconfig_dir
    }

    /// The joypad profile folder RetroArch reads: Hearth's, refilled on every launch.
    pub fn hearth_autoconfig_dir(&self) -> PathBuf {
        self.root.join("autoconfig")
    }

    /// Whether joypad profiles are installed (without them RetroArch maps no controller).
    pub fn has_autoconfig(&self) -> bool {
        self.autoconfig_dir.join("udev").is_dir()
    }

    /// The config file contents for a game of `system`.
    pub fn config_text(&self, system: &SystemId) -> String {
        self.config_text_with(system, &[])
    }

    /// The config file contents for a game of `system`, with `overrides` (input settings)
    /// replacing or adding keys.
    pub fn config_text_with(&self, system: &SystemId, overrides: &[(String, String)]) -> String {
        let mut out = String::from("# Generated by Hearth; changes are overwritten on launch.\n");
        let mut written = Vec::new();
        let mut line = |key: &str, value: &str| {
            let value = overrides
                .iter()
                .rev()
                .find(|(k, _)| k == key)
                .map_or(value, |(_, v)| v.as_str());
            let _ = writeln!(out, "{key} = \"{value}\"");
            written.push(key.to_owned());
        };
        for (key, value) in SETTINGS {
            line(key, value);
        }
        line(
            "joypad_autoconfig_dir",
            &self.hearth_autoconfig_dir().to_string_lossy(),
        );
        line(
            "savefile_directory",
            &self.saves_dir(system).to_string_lossy(),
        );
        line("system_directory", &self.system_dir.to_string_lossy());
        for (key, sub) in DIRS.iter().chain(FILES) {
            line(key, &self.root.join(sub).to_string_lossy());
        }
        for hotkey in HOTKEYS {
            line(&format!("input_{hotkey}"), "nul");
            line(&format!("input_{hotkey}_btn"), NO_SUCH_BUTTON);
            line(&format!("input_{hotkey}_axis"), NO_SUCH_AXIS);
            line(&format!("input_{hotkey}_mbtn"), "nul");
        }
        for (key, value) in overrides {
            if !written.contains(key) {
                let _ = writeln!(out, "{key} = \"{value}\"");
                written.push(key.clone());
            }
        }
        out
    }

    /// Writes the config with the generated input settings, the remap file (removing any
    /// other remap of that core, so game or folder remaps can't take precedence) and the core
    /// options for a game of `system`, and creates the writable directories.
    pub fn prepare(&self, system: &SystemId, input: &Translation) -> io::Result<PathBuf> {
        for (_, sub) in DIRS {
            fs::create_dir_all(self.root.join(sub))?;
        }
        fs::create_dir_all(self.saves_dir(system))?;
        fs::create_dir_all(&self.system_dir)?;
        let path = self.config_path();
        fs::write(&path, self.config_text_with(system, &input.settings))?;
        // Only this launch's profile: an old one could still match and win.
        match fs::remove_dir_all(self.hearth_autoconfig_dir()) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        for (rel, text) in &input.files {
            let file = self.root.join(rel);
            if let Some(dir) = file.parent() {
                fs::create_dir_all(dir)?;
                for entry in fs::read_dir(dir)?.flatten() {
                    let stale = entry.path();
                    if stale.extension().is_some_and(|e| e == "rmp") && stale != file {
                        fs::remove_file(stale)?;
                    }
                }
            }
            fs::write(file, text)?;
        }
        if !input.core_options.is_empty() {
            let options = self.root.join("config/retroarch-core-options.cfg");
            let old = fs::read_to_string(&options).unwrap_or_default();
            fs::write(&options, merge_options(&old, &input.core_options))?;
        }
        Ok(path)
    }

    /// The command that runs `rom` with `core`, logging to `<root>/logs/hearth-launch.log`.
    pub fn command(&self, core: &Path, rom: &Path) -> LaunchCommand {
        LaunchCommand {
            program: self.program.clone(),
            args: vec![
                "--config".into(),
                self.config_path().into(),
                "-L".into(),
                core.into(),
                rom.into(),
            ],
            log: Some(self.root.join("logs/hearth-launch.log")),
            ..LaunchCommand::default()
        }
    }
}

fn absolute(path: PathBuf) -> PathBuf {
    std::path::absolute(&path).unwrap_or(path)
}

/// `old` (RetroArch `key = "value"` lines) with `options` replaced or appended.
fn merge_options(old: &str, options: &[(String, String)]) -> String {
    let mut out = String::new();
    for line in old.lines() {
        let key = line.split_once('=').map(|(k, _)| k.trim());
        if !key.is_some_and(|k| options.iter().any(|(o, _)| o == k)) {
            out.push_str(line);
            out.push('\n');
        }
    }
    for (key, value) in options {
        let _ = writeln!(out, "{key} = \"{value}\"");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    fn values(text: &str) -> Vec<(String, String)> {
        text.lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let (k, v) = l.split_once(" = ")?;
                Some((k.to_owned(), v.trim_matches('"').to_owned()))
            })
            .collect()
    }

    fn n64() -> SystemId {
        SystemId::new("n64")
    }

    #[test]
    fn writable_paths_stay_under_root() {
        let profile = RetroArchProfile::new("/dev-data/retroarch");
        for (key, value) in values(&profile.config_text(&n64())) {
            if value.starts_with('/') {
                assert!(value.starts_with("/dev-data/retroarch/"), "{key} = {value}");
            }
        }
    }

    #[test]
    fn a_user_profile_keeps_saves_apart_and_shares_the_bios() {
        let user = ProfileDirs::new(
            crate::users::ProfileId::default(),
            Path::new("/h/profiles"),
            Path::new("/h/emulators"),
        );
        let profile = RetroArchProfile::for_profile(&user);
        let v = values(&profile.config_text(&n64()));
        let get = |k: &str| v.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(
            get("savefile_directory"),
            Some("/h/profiles/default/saves/n64")
        );
        assert_eq!(get("sort_savefiles_enable"), Some("false"));
        assert_eq!(
            get("system_directory"),
            Some("/h/emulators/retroarch/system")
        );
        assert_eq!(
            get("savestate_directory"),
            Some("/h/profiles/default/emulators/retroarch/states")
        );
        // Every path is the profile's or the shared one: nothing left in the old
        // `<home>/retroarch` folder, nothing relative.
        for (key, value) in &v {
            if value.contains('/') {
                assert!(
                    value.starts_with("/h/profiles/default/") || value.starts_with("/h/emulators/"),
                    "{key} = {value}"
                );
            }
        }
    }

    #[test]
    fn relative_roots_are_made_absolute() {
        let profile = RetroArchProfile::new("./.dev-data/retroarch");
        assert!(profile.config_path().is_absolute());
        assert!(profile.saves_dir(&n64()).is_absolute());
    }

    #[test]
    fn guide_and_hotkeys_are_unreachable() {
        let text = RetroArchProfile::new("/r").config_text(&n64());
        let v = values(&text);
        let get = |k: &str| v.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("input_menu_toggle_btn"), Some(NO_SUCH_BUTTON));
        assert_eq!(get("input_menu_toggle"), Some("nul"));
        assert_eq!(get("input_exit_emulator"), Some("nul"));
        assert_eq!(get("input_exit_emulator_btn"), Some(NO_SUCH_BUTTON));
        assert_eq!(get("input_enable_hotkey_btn"), Some(NO_SUCH_BUTTON));
        assert_eq!(get("input_toggle_fast_forward"), Some("nul"));
        assert_eq!(get("input_menu_toggle_gamepad_combo"), Some("0"));
        assert_eq!(get("input_quit_gamepad_combo"), Some("0"));
        assert_eq!(get("config_save_on_exit"), Some("false"));
        assert_eq!(get("video_fullscreen"), Some("true"));
        assert_eq!(get("joypad_autoconfig_dir"), Some("/r/autoconfig"));
        // No gamepad hotkey is left unbound ("nul" would fall back to autoconfig).
        assert!(!v.iter().any(|(k, val)| k.ends_with("_btn") && val == "nul"));
    }

    #[test]
    fn prepare_writes_config_and_dirs() {
        let dir = tempdir();
        let profile = RetroArchProfile::new(dir.path().join("retroarch"));
        let cfg = profile.prepare(&n64(), &Translation::default()).unwrap();
        assert!(cfg.is_file());
        assert!(dir.path().join("retroarch/saves/n64").is_dir());
        assert!(dir.path().join("retroarch/logs/runtime").is_dir());
        let cmd = profile.command(Path::new("/core.so"), Path::new("/rom.z64"));
        assert_eq!(cmd.program, "retroarch");
        assert_eq!(
            cmd.args,
            [
                "--config".into(),
                cfg.into_os_string(),
                "-L".into(),
                "/core.so".into(),
                "/rom.z64".into()
            ] as [OsString; 5]
        );
    }

    #[test]
    fn prepare_applies_input_translation() {
        let dir = tempdir();
        let profile = RetroArchProfile::new(dir.path().join("ra"));
        let remaps = dir.path().join("ra/remaps/Core");
        fs::create_dir_all(&remaps).unwrap();
        fs::write(remaps.join("Some Game.rmp"), "old").unwrap();
        fs::create_dir_all(dir.path().join("ra/config")).unwrap();
        fs::write(
            dir.path().join("ra/config/retroarch-core-options.cfg"),
            "core-a = \"1\"\ncore-b = \"2\"\n",
        )
        .unwrap();
        let stale = profile.hearth_autoconfig_dir().join("udev/Old Pad.cfg");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, "input_device = \"Old\"").unwrap();
        let input = Translation {
            files: vec![
                ("remaps/Core/Core.rmp".into(), "new".into()),
                ("autoconfig/sdl2/Pad.cfg".into(), "pad".into()),
            ],
            settings: vec![
                ("input_hold_fast_forward_btn".into(), "11".into()),
                ("rewind_enable".into(), "true".into()),
            ],
            core_options: vec![("core-b".into(), "5".into())],
            ..Translation::default()
        };
        let cfg = profile.prepare(&n64(), &input).unwrap();
        // Only this launch's joypad profile is left.
        assert!(!stale.exists());
        assert!(
            profile
                .hearth_autoconfig_dir()
                .join("sdl2/Pad.cfg")
                .is_file()
        );
        let v = values(&fs::read_to_string(cfg).unwrap());
        let get = |k: &str| {
            v.iter()
                .filter(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(get("input_hold_fast_forward_btn"), ["11"]);
        assert_eq!(get("rewind_enable"), ["true"]);
        assert_eq!(get("input_save_state_btn"), [NO_SUCH_BUTTON]);
        assert!(!remaps.join("Some Game.rmp").exists());
        assert_eq!(fs::read_to_string(remaps.join("Core.rmp")).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(dir.path().join("ra/config/retroarch-core-options.cfg")).unwrap(),
            "core-a = \"1\"\ncore-b = \"5\"\n"
        );
    }
}
