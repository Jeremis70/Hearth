//! RetroArch profile, fully isolated from the user's own RetroArch setup.
//!
//! Hearth generates its own `retroarch.cfg` under `root` and launches
//! `retroarch --config <cfg> -L <core> <rom>`, so `~/.config/retroarch` is never read or
//! written. Every writable directory lives under `root`; the only outside path is the joypad
//! autoconfig folder, which is read-only.
//!
//! The Guide button must only ever reach Hearth. Autoconfig profiles bind it to the menu
//! (`input_menu_toggle_btn`), and RetroArch falls back to the profile whenever our bind is
//! `nul`, so every hotkey's gamepad bind is set to a button that doesn't exist instead.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::runtime::LaunchCommand;

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

/// Writable subdirectories of the profile root, by config key.
const DIRS: &[(&str, &str)] = &[
    ("savefile_directory", "saves"),
    ("savestate_directory", "states"),
    ("system_directory", "system"),
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
];

/// A RetroArch installation driven by Hearth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchProfile {
    program: OsString,
    root: PathBuf,
    autoconfig_dir: PathBuf,
}

impl RetroArchProfile {
    /// Profile writing everything under `root`, reading joypad profiles from the system.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            program: "retroarch".into(),
            root: root.into(),
            autoconfig_dir: PathBuf::from(SYSTEM_AUTOCONFIG_DIR),
        }
    }

    /// Uses another (read-only) joypad autoconfig folder.
    pub fn with_autoconfig_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.autoconfig_dir = dir.into();
        self
    }

    /// The generated config file.
    pub fn config_path(&self) -> PathBuf {
        self.root.join("retroarch.cfg")
    }

    /// The joypad autoconfig folder RetroArch will read.
    pub fn autoconfig_dir(&self) -> &Path {
        &self.autoconfig_dir
    }

    /// Whether joypad profiles are installed (without them RetroArch maps no controller).
    pub fn has_autoconfig(&self) -> bool {
        self.autoconfig_dir.join("udev").is_dir()
    }

    /// The config file contents.
    pub fn config_text(&self) -> String {
        let mut out = String::from("# Generated by Hearth; changes are overwritten on launch.\n");
        let mut line = |key: &str, value: &str| {
            let _ = writeln!(out, "{key} = \"{value}\"");
        };
        for (key, value) in SETTINGS {
            line(key, value);
        }
        line(
            "joypad_autoconfig_dir",
            &self.autoconfig_dir.to_string_lossy(),
        );
        for (key, sub) in DIRS.iter().chain(FILES) {
            line(key, &self.root.join(sub).to_string_lossy());
        }
        for hotkey in HOTKEYS {
            line(&format!("input_{hotkey}"), "nul");
            line(&format!("input_{hotkey}_btn"), NO_SUCH_BUTTON);
            line(&format!("input_{hotkey}_axis"), NO_SUCH_AXIS);
            line(&format!("input_{hotkey}_mbtn"), "nul");
        }
        out
    }

    /// Writes the config and creates the writable directories.
    pub fn prepare(&self) -> io::Result<PathBuf> {
        for (_, sub) in DIRS {
            fs::create_dir_all(self.root.join(sub))?;
        }
        let path = self.config_path();
        fs::write(&path, self.config_text())?;
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

    #[test]
    fn writable_paths_stay_under_root() {
        let profile = RetroArchProfile::new("/dev-data/retroarch");
        for (key, value) in values(&profile.config_text()) {
            if value.starts_with('/') && key != "joypad_autoconfig_dir" {
                assert!(value.starts_with("/dev-data/retroarch/"), "{key} = {value}");
            }
        }
    }

    #[test]
    fn guide_and_hotkeys_are_unreachable() {
        let text = RetroArchProfile::new("/r").config_text();
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
        assert_eq!(get("joypad_autoconfig_dir"), Some(SYSTEM_AUTOCONFIG_DIR));
        // No gamepad hotkey is left unbound ("nul" would fall back to autoconfig).
        assert!(!v.iter().any(|(k, val)| k.ends_with("_btn") && val == "nul"));
    }

    #[test]
    fn prepare_writes_config_and_dirs() {
        let dir = tempdir();
        let profile = RetroArchProfile::new(dir.path().join("retroarch"));
        let cfg = profile.prepare().unwrap();
        assert!(cfg.is_file());
        assert!(dir.path().join("retroarch/saves").is_dir());
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
}
