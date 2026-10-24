//! Dolphin profile (GameCube, Wii), isolated from the user's own Dolphin setup.
//!
//! Dolphin runs with `-u <root>`, so `~/.config/dolphin-emu` and `~/.local/share/dolphin-emu`
//! are never read or written. Settings are passed as `-C` overrides, which Dolphin keeps in a
//! command-line layer it never saves. Controller mappings and hotkeys can't be set that way, so
//! `GCPadNew.ini` and `Hotkeys.ini` are generated from Hearth's controls files on every launch
//! (`dolphin_input`).
//!
//! The user directory is the user profile's (`profiles/<id>/emulators/dolphin`); the game
//! saves live in the profile's `saves/`: GameCube memory cards in `saves/gc/` (GCI folders
//! `Card A/<region>/`, or raw cards `MemoryCardA.<region>.raw`, whichever the profile's
//! Dolphin.ini selects; both paths are always given) and the Wii NAND in `saves/wii/nand/`.
//! Dolphin's caches (shaders etc.) stay in the user directory, so per profile: Dolphin has no
//! setting for their location (`D_CACHE_IDX` follows `-u`).
//!
//! Every key and flag was checked against Dolphin 2609 (`Core/Config/MainSettings.cpp`,
//! `UICommon/CommandLineParse.cpp`, `DolphinQt/Main.cpp`). The save paths too: a configured
//! GCI folder gets `/<USA|JPN|EUR>` appended (`GetGCIFolderPath`), a configured raw card gets
//! `.<region>` inserted before `.raw` (`GetMemcardPath`), and `NANDRootPath` sets
//! `D_WIIROOT_IDX` (`UICommon::InitCustomPaths`).

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::input::Translation;
use crate::runtime::LaunchCommand;
use crate::users::ProfileDirs;

/// Program name of a native install.
pub const PROGRAM: &str = "dolphin-emu";
/// Flatpak application id.
pub const FLATPAK_APP: &str = "org.DolphinEmu.dolphin-emu";

/// `-C <System>.<Section>.<Key>=<Value>` overrides.
const SETTINGS: &[(&str, &str)] = &[
    // Render window fullscreen, no cursor. Batch mode already forces RenderToMain off.
    ("Dolphin.Display.Fullscreen", "True"),
    ("Dolphin.Interface.CursorVisibility", "0"), // ShowCursor::Never
    // No on-screen messages ("Video Info: …" at boot, memory card saves).
    ("Dolphin.Interface.OnScreenDisplayMessages", "False"),
    // No dialogs: stop confirmation, NKit warning, panic alerts (logged instead), analytics.
    ("Dolphin.Interface.ConfirmStop", "False"),
    ("Dolphin.Interface.SkipNKitWarning", "True"),
    ("Dolphin.Interface.UsePanicHandlers", "False"),
    ("Dolphin.Analytics.PermissionAsked", "True"),
    ("Dolphin.Analytics.Enabled", "False"),
    // No update check (batch mode skips it too).
    ("Dolphin.AutoUpdate.UpdateTrack", ""),
    // Discord Rich Presence registers a URL handler on startup: it writes
    // ~/.local/share/applications/discord-<id>.desktop and edits ~/.config/mimeapps.list.
    ("Dolphin.General.UseDiscordPresence", "False"),
    // Hearth freezes the game with SIGSTOP; Dolphin must not pause by itself.
    ("Dolphin.Interface.PauseOnFocusLost", "False"),
    // Read the pad even if Dolphin thinks its window isn't focused (gamescope focus isn't
    // always X11 focus). Harmless: whenever Hearth is in front, the game is frozen.
    ("Dolphin.Input.BackgroundInput", "True"),
    // Log controller hotplug ("Added device: SDL/0/<name>") to stdout, i.e. hearth-launch.log.
    ("Logger.Logs.CI", "True"),
    ("Logger.Options.Verbosity", "3"), // LogLevel::LWARNING: notices, errors, warnings
    // Port 1: standard controller (SIDEVICE_GC_CONTROLLER); ports 2-4: nothing.
    ("Dolphin.Core.SIDevice0", "6"),
    ("Dolphin.Core.SIDevice1", "0"),
    ("Dolphin.Core.SIDevice2", "0"),
    ("Dolphin.Core.SIDevice3", "0"),
];

/// How Dolphin is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DolphinInstall {
    /// `dolphin-emu` found in `PATH`.
    Native(PathBuf),
    /// The Flathub package.
    Flatpak,
}

impl DolphinInstall {
    /// Finds Dolphin: `dolphin-emu` in `PATH` first, then the Flatpak (system or user install).
    /// Doesn't run anything.
    pub fn detect() -> Option<Self> {
        let mut flatpak_roots = vec![PathBuf::from("/var/lib/flatpak")];
        if let Some(dirs) = directories::BaseDirs::new() {
            flatpak_roots.push(dirs.data_dir().join("flatpak"));
        }
        Self::detect_in(std::env::var_os("PATH").as_deref(), &flatpak_roots)
    }

    fn detect_in(path: Option<&OsStr>, flatpak_roots: &[PathBuf]) -> Option<Self> {
        if let Some(program) = path.and_then(|p| find_in_path(p, PROGRAM)) {
            return Some(Self::Native(program));
        }
        flatpak_roots
            .iter()
            .any(|root| root.join("app").join(FLATPAK_APP).join("current").is_dir())
            .then_some(Self::Flatpak)
    }
}

/// The first executable file named `name` in a `PATH`-style list.
fn find_in_path(path: &OsStr, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|file| {
            file.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// A Dolphin user directory driven by Hearth, for one user profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinProfile {
    root: PathBuf,
    /// GameCube memory cards.
    gc_saves: PathBuf,
    /// The Wii NAND.
    wii_nand: PathBuf,
    /// What the Flatpak sandbox may write: the user profile's folder.
    writable: PathBuf,
    install: Option<DolphinInstall>,
}

impl DolphinProfile {
    /// The Dolphin of user profile `user` (its paths are absolute already). `install` is `None`
    /// when Dolphin isn't installed.
    pub fn new(user: &ProfileDirs, install: Option<DolphinInstall>) -> Self {
        Self {
            root: user.dolphin(),
            gc_saves: user.gc_saves(),
            wii_nand: user.wii_nand(),
            writable: user.root().to_path_buf(),
            install,
        }
    }

    /// Dolphin's user directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `-C` overrides putting the game saves in the profile's `saves/`.
    fn save_settings(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("Dolphin.Core.GCIFolderAPath", self.gc_saves.join("Card A")),
            ("Dolphin.Core.GCIFolderBPath", self.gc_saves.join("Card B")),
            (
                "Dolphin.Core.MemcardAPath",
                self.gc_saves.join("MemoryCardA.raw"),
            ),
            (
                "Dolphin.Core.MemcardBPath",
                self.gc_saves.join("MemoryCardB.raw"),
            ),
            ("Dolphin.General.NANDRootPath", self.wii_nand.clone()),
        ]
    }

    /// How Dolphin is installed, if it is.
    pub fn install(&self) -> Option<&DolphinInstall> {
        self.install.as_ref()
    }

    /// Writes the generated input files (controller mapping, hotkeys) into the user directory.
    pub fn prepare(&self, input: &Translation) -> io::Result<()> {
        fs::create_dir_all(self.root.join("Logs"))?;
        fs::create_dir_all(&self.gc_saves)?;
        fs::create_dir_all(&self.wii_nand)?;
        for (rel, text) in &input.files {
            let path = self.root.join(rel);
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(path, text)?;
        }
        Ok(())
    }

    /// The command that runs `rom`, logging to `<root>/Logs/hearth-launch.log`, or `None` if
    /// Dolphin isn't installed.
    pub fn command(&self, rom: &Path) -> Option<LaunchCommand> {
        let mut dolphin_args: Vec<OsString> =
            vec!["-u".into(), self.root.clone().into(), "-b".into()];
        for (key, value) in SETTINGS {
            dolphin_args.push("-C".into());
            dolphin_args.push(format!("{key}={value}").into());
        }
        for (key, path) in self.save_settings() {
            let mut arg = OsString::from(format!("{key}="));
            arg.push(path);
            dolphin_args.push("-C".into());
            dolphin_args.push(arg);
        }
        dolphin_args.push("-e".into());
        dolphin_args.push(rom.into());

        let (program, args) = match self.install.as_ref()? {
            DolphinInstall::Native(program) => (program.clone().into_os_string(), dolphin_args),
            DolphinInstall::Flatpak => {
                let mut args: Vec<OsString> = vec!["run".into()];
                args.push(filesystem_arg(&self.writable, ""));
                if let Some(dir) = rom.parent() {
                    args.push(filesystem_arg(dir, ":ro"));
                }
                args.push(FLATPAK_APP.into());
                args.extend(dolphin_args);
                ("flatpak".into(), args)
            }
        };
        Some(LaunchCommand {
            program,
            args,
            log: Some(self.root.join("Logs/hearth-launch.log")),
            ..LaunchCommand::default()
        })
    }
}

/// `--filesystem=<dir><mode>` for `flatpak run`.
fn filesystem_arg(dir: &Path, mode: &str) -> OsString {
    let mut arg = OsString::from("--filesystem=");
    arg.push(dir);
    arg.push(mode);
    arg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    fn user() -> ProfileDirs {
        ProfileDirs::new(
            crate::users::ProfileId::default(),
            Path::new("/h/profiles"),
            Path::new("/h/emulators"),
        )
    }

    fn native() -> DolphinProfile {
        DolphinProfile::new(
            &user(),
            Some(DolphinInstall::Native("/usr/bin/dolphin-emu".into())),
        )
    }

    const ROOT: &str = "/h/profiles/default/emulators/dolphin";

    #[test]
    fn native_command_uses_isolated_user_dir_and_batch_mode() {
        let cmd = native().command(Path::new("/games/gc/Game.iso")).unwrap();
        assert_eq!(cmd.program, "/usr/bin/dolphin-emu");
        let args: Vec<_> = cmd.args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args[..3], ["-u", ROOT, "-b"]);
        assert_eq!(args[args.len() - 2..], ["-e", "/games/gc/Game.iso"]);
        for expected in [
            "Dolphin.Display.Fullscreen=True",
            "Dolphin.Interface.ConfirmStop=False",
            "Dolphin.Interface.PauseOnFocusLost=False",
            "Dolphin.Analytics.PermissionAsked=True",
            "Dolphin.Core.SIDevice0=6",
            "Dolphin.Core.SIDevice3=0",
            "Dolphin.General.UseDiscordPresence=False",
            "Dolphin.Interface.OnScreenDisplayMessages=False",
        ] {
            let at = args.iter().position(|a| *a == expected).unwrap();
            assert_eq!(args[at - 1], "-C", "{expected}");
        }
        assert_eq!(
            cmd.log.as_deref(),
            Some(Path::new(
                "/h/profiles/default/emulators/dolphin/Logs/hearth-launch.log"
            ))
        );
    }

    #[test]
    fn saves_go_to_the_profile() {
        let cmd = native().command(Path::new("/games/gc/Game.iso")).unwrap();
        let args: Vec<_> = cmd.args.iter().map(|a| a.to_str().unwrap()).collect();
        for expected in [
            "Dolphin.Core.GCIFolderAPath=/h/profiles/default/saves/gc/Card A",
            "Dolphin.Core.GCIFolderBPath=/h/profiles/default/saves/gc/Card B",
            "Dolphin.Core.MemcardAPath=/h/profiles/default/saves/gc/MemoryCardA.raw",
            "Dolphin.Core.MemcardBPath=/h/profiles/default/saves/gc/MemoryCardB.raw",
            "Dolphin.General.NANDRootPath=/h/profiles/default/saves/wii/nand",
        ] {
            let at = args.iter().position(|a| *a == expected).unwrap();
            assert_eq!(args[at - 1], "-C", "{expected}");
        }
        // Every path Dolphin gets is the profile's.
        for arg in &args[..args.len() - 1] {
            if let Some((_, path)) = arg.split_once('=').filter(|(_, p)| p.starts_with('/')) {
                assert!(path.starts_with("/h/profiles/default/"), "{arg}");
            } else if arg.starts_with('/') {
                assert!(arg.starts_with("/h/profiles/default/"), "{arg}");
            }
        }
    }

    #[test]
    fn flatpak_command_grants_user_dir_and_rom_folder() {
        let profile = DolphinProfile::new(&user(), Some(DolphinInstall::Flatpak));
        let cmd = profile.command(Path::new("/games/gc/Game.iso")).unwrap();
        assert_eq!(cmd.program, "flatpak");
        let args: Vec<_> = cmd.args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args[..6],
            [
                "run",
                "--filesystem=/h/profiles/default",
                "--filesystem=/games/gc:ro",
                FLATPAK_APP,
                "-u",
                ROOT
            ]
        );
    }

    #[test]
    fn missing_dolphin_has_no_command() {
        let profile = DolphinProfile::new(&user(), None);
        assert!(profile.command(Path::new("/g.iso")).is_none());
    }

    #[test]
    fn relative_root_is_made_absolute() {
        let user = ProfileDirs::new(
            crate::users::ProfileId::default(),
            Path::new("./.dev-data/profiles"),
            Path::new("./.dev-data/emulators"),
        );
        let profile = DolphinProfile::new(&user, None);
        assert!(profile.root().is_absolute());
        assert!(
            profile
                .root()
                .ends_with(".dev-data/profiles/default/emulators/dolphin")
        );
    }

    #[test]
    fn prepare_writes_input_files_under_root() {
        let dir = tempdir();
        let user = ProfileDirs::new(
            crate::users::ProfileId::default(),
            &dir.path().join("profiles"),
            &dir.path().join("emulators"),
        );
        let profile = DolphinProfile::new(&user, None);
        let input = Translation {
            files: vec![("Config/GCPadNew.ini".into(), "[GCPad1]\n".into())],
            ..Translation::default()
        };
        profile.prepare(&input).unwrap();
        assert!(user.dolphin().join("Config/GCPadNew.ini").is_file());
        assert!(user.dolphin().join("Logs").is_dir());
        assert!(user.gc_saves().is_dir());
        assert!(user.wii_nand().is_dir());
    }

    #[test]
    fn detection_prefers_path_then_flatpak() {
        let dir = tempdir();
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let program = bin.join(PROGRAM);
        fs::write(&program, "").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let flatpak = dir.path().join("flatpak");
        fs::create_dir_all(flatpak.join("app").join(FLATPAK_APP).join("current")).unwrap();
        let roots = [flatpak];

        assert_eq!(
            DolphinInstall::detect_in(Some(bin.as_os_str()), &roots),
            Some(DolphinInstall::Native(program))
        );
        let empty = dir.path().join("empty");
        assert_eq!(
            DolphinInstall::detect_in(Some(empty.as_os_str()), &roots),
            Some(DolphinInstall::Flatpak)
        );
        assert_eq!(DolphinInstall::detect_in(None, &[empty]), None);
    }
}
