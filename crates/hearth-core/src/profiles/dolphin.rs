//! Dolphin profile (GameCube, Wii), isolated from the user's own Dolphin setup.
//!
//! Dolphin runs with `-u <root>`, so `~/.config/dolphin-emu` and `~/.local/share/dolphin-emu`
//! are never read or written. Controller mappings and hotkeys are whole files Hearth owns:
//! `GCPadNew.ini` and `Hotkeys.ini` are generated from Hearth's controls files on every launch
//! (`dolphin_input`).
//!
//! Other settings are merged into Dolphin's own files at launch (`emusettings::merge`), by
//! Dolphin's layers (`Common/Config/Enums.h`, lowest first: Base < CommandLine < GlobalGame <
//! LocalGame):
//! - Hearth's locked keys go into `Config/Dolphin.ini` / `Logger.ini` (so a hand edit is
//!   restored) and stay `-C` overrides, the command-line layer Dolphin never saves;
//! - console settings (shared by every profile) go into the Base files, `Dolphin.ini` and
//!   `GFX.ini`, never `-C`: the command line beats Base, so a value later changed in Dolphin's
//!   own UI would be ignored;
//! - a game's overrides go into `GameSettings/<game id>.ini` (LocalGame), which beats Dolphin's
//!   shipped game fixes (`Sys/GameSettings`, GlobalGame) as well. Locked `[Core]` keys found
//!   there are removed (LocalGame would beat the command line).
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
use crate::emusettings::merge::{FilePlan, Layer, Wanted};
use crate::emusettings::schema::{ConfigFile, game_section};
use crate::emusettings::store::Applied;
use crate::emusettings::{ConfigDoc, EmulatorSettings, Gpu, Report, Schema, Style, backup};
use crate::runtime::LaunchCommand;
use crate::users::ProfileDirs;

/// Program name of a native install.
pub const PROGRAM: &str = "dolphin-emu";
/// Flatpak application id.
pub const FLATPAK_APP: &str = "org.DolphinEmu.dolphin-emu";

/// Hearth's locked keys, as `<System>.<Section>.<Key>` (also the `-C` override syntax) and
/// value. Everything else in Dolphin's files is the user's.
const LOCKED: &[(&str, &str)] = &[
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
    settings: EmulatorSettings,
    /// `<home>/backup`, for the one-time copy before Hearth first merges into the files.
    backup_root: PathBuf,
    /// `<profile id>/dolphin`, the backup's folder.
    backup_label: String,
}

/// A game Dolphin is launched with, for its own settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRef {
    /// The disc's game id (`GZ2E01`): `GameSettings/<id>.ini`.
    pub id: String,
    /// Hearth's data for the game (`<data>/games/gc/GZ2E01`): its `emulator.toml`.
    pub dir: PathBuf,
}

impl GameRef {
    /// `game`'s reference, if its disc header has a game id (every GameCube and Wii disc
    /// does). Its data folder is the one of its Customize picks (by art key).
    pub fn for_game(game: &crate::model::Game, picks: &crate::custom::CustomStore) -> Option<Self> {
        let serial = crate::rom_header::read(&game.system, &game.path)
            .ok()?
            .serial
            .filter(|s| s.len() == 6)?;
        let key = picks.key_of(game);
        // Per-game data only under the header key (`gc/GZ2E01`), never a name hash.
        key.as_str().ends_with(&format!("/{serial}")).then(|| Self {
            dir: picks.dir(&key),
            id: serial,
        })
    }
}

/// Config files Hearth merges into, relative to the user directory.
const MAIN_INI: &str = "Config/Dolphin.ini";
const GFX_INI: &str = "Config/GFX.ini";
const LOGGER_INI: &str = "Config/Logger.ini";

fn file_rel(file: ConfigFile) -> &'static str {
    match file {
        ConfigFile::Gfx => GFX_INI,
        ConfigFile::Logger => LOGGER_INI,
        ConfigFile::Main | ConfigFile::RetroArch => MAIN_INI,
    }
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
            settings: EmulatorSettings::new(
                Schema::dolphin(),
                &user.shared_emulator("dolphin"),
                None,
            ),
            backup_root: user.backup_root(),
            backup_label: format!("{}/dolphin", user.id()),
        }
    }

    /// Recommends the preset for `gpu` (the box's GPU, see `emusettings::gpu`).
    pub fn with_gpu(mut self, gpu: Option<Gpu>) -> Self {
        let shared_dir = self
            .settings
            .shared_path()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        self.settings = EmulatorSettings::new(self.settings.schema().clone(), &shared_dir, gpu);
        self
    }

    /// Dolphin's settings (schema, console base, recommended preset).
    pub fn settings(&self) -> &EmulatorSettings {
        &self.settings
    }

    /// Dolphin's own folder of shipped game settings (`Sys/GameSettings`), if found.
    pub fn sys_game_settings(&self) -> Option<PathBuf> {
        let candidates: Vec<PathBuf> = match self.install.as_ref()? {
            DolphinInstall::Native(program) => {
                let prefix = program.parent()?.parent()?;
                vec![prefix.join("share/dolphin-emu/Sys/GameSettings")]
            }
            DolphinInstall::Flatpak => {
                let mut roots = vec![PathBuf::from("/var/lib/flatpak")];
                if let Some(dirs) = directories::BaseDirs::new() {
                    roots.push(dirs.data_dir().join("flatpak"));
                }
                roots
                    .into_iter()
                    .map(|r| {
                        r.join("app")
                            .join(FLATPAK_APP)
                            .join("current/active/files/share/dolphin-emu/Sys/GameSettings")
                    })
                    .collect()
            }
        };
        candidates.into_iter().find(|dir| dir.is_dir())
    }

    /// The settings Dolphin's shipped game INIs set for game `id` (setting id → choice id),
    /// read the way Dolphin does: `<first letter>.ini`, `<first 3>.ini`, `<id>.ini`, later
    /// files winning (`ConfigLoaders/GameConfigLoader.cpp`). Values Hearth has no choice for
    /// are left out.
    pub fn shipped_game_values(&self, id: &str) -> std::collections::BTreeMap<String, String> {
        let mut out = std::collections::BTreeMap::new();
        let Some(dir) = self.sys_game_settings() else {
            return out;
        };
        let mut names = Vec::new();
        if id.len() == 6 {
            names.push(format!("{}.ini", &id[..1]));
            names.push(format!("{}.ini", &id[..3]));
        }
        names.push(format!("{id}.ini"));
        for name in names {
            let Ok(doc) = ConfigDoc::load(&dir.join(name), Style::Ini) else {
                continue;
            };
            for setting in &self.settings.schema().settings {
                let Some(section) = game_section(setting.file, &setting.section) else {
                    continue;
                };
                if !setting
                    .all_keys()
                    .iter()
                    .any(|k| doc.get(&section, k).is_some())
                {
                    continue;
                }
                if let Some(choice) = setting.read(|k| doc.get(&section, k).map(str::to_owned)) {
                    out.insert(setting.id.clone(), choice.to_owned());
                }
            }
        }
        out
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

    /// Hearth's locked keys as (file, section, key, value), save paths included.
    fn locked(&self) -> Vec<(&'static str, String, String, String)> {
        let mut out: Vec<_> = LOCKED
            .iter()
            .filter_map(|(path, value)| {
                let mut parts = path.splitn(3, '.');
                let file = match parts.next()? {
                    "Dolphin" => MAIN_INI,
                    "Logger" => LOGGER_INI,
                    _ => return None,
                };
                Some((
                    file,
                    parts.next()?.to_owned(),
                    parts.next()?.to_owned(),
                    (*value).to_owned(),
                ))
            })
            .collect();
        for (path, value) in self.save_settings() {
            let mut parts = path.splitn(3, '.').skip(1);
            if let (Some(section), Some(key)) = (parts.next(), parts.next()) {
                out.push((
                    MAIN_INI,
                    section.to_owned(),
                    key.to_owned(),
                    value.to_string_lossy().into_owned(),
                ));
            }
        }
        out
    }

    /// What Hearth will merge into Dolphin's files for a launch of `game` (or of no game in
    /// particular): locked keys, console settings and the game's overrides.
    pub fn plans(&self, game: Option<&GameRef>) -> Vec<FilePlan<'_>> {
        let schema = self.settings.schema();
        let shared = self.settings.shared();
        let resolved = self.settings.resolved(&shared);
        let overrides = game.map(|g| self.settings.game(&g.dir)).unwrap_or_default();
        let mut main = FilePlan::new(MAIN_INI, Style::Ini);
        let mut logger = FilePlan::new(LOGGER_INI, Style::Ini);
        let mut gfx = FilePlan::new(GFX_INI, Style::Ini);
        let mut per_game =
            game.map(|g| FilePlan::game(format!("GameSettings/{}.ini", g.id), Style::Ini));
        for (file, section, key, value) in self.locked() {
            let plan = if file == LOGGER_INI {
                &mut logger
            } else {
                &mut main
            };
            if let Some(game) = per_game.as_mut()
                && section == "Core"
            {
                game.strip.push((section.clone(), key.clone()));
            }
            plan.lock(&section, &key, value);
        }
        for setting in &schema.settings {
            let (choice, _) = resolved.console(setting);
            let plan = if setting.file == ConfigFile::Gfx {
                &mut gfx
            } else {
                &mut main
            };
            debug_assert_eq!(file_rel(setting.file), plan.rel);
            plan.settings.push(Wanted {
                setting,
                section: setting.section.clone(),
                choice,
                layer: Layer::Console,
            });
            if let (Some(plan), Some(choice)) = (per_game.as_mut(), overrides.get(&setting.id))
                && setting.scope == crate::emusettings::Scope::PerGame
                && setting.choice(choice).is_some()
                && let Some(section) = game_section(setting.file, &setting.section)
            {
                plan.settings.push(Wanted {
                    setting,
                    section,
                    choice: choice.clone(),
                    layer: Layer::Game,
                });
            }
        }
        let mut plans = vec![main, logger, gfx];
        plans.extend(per_game);
        plans
    }

    /// Writes the generated input files (controller mapping, hotkeys) into the user directory
    /// and merges Hearth's keys and settings into Dolphin's config files (backing them up the
    /// first time). Settings changed in Dolphin itself are kept and stored back.
    pub fn prepare(&self, input: &Translation, game: Option<&GameRef>) -> io::Result<Report> {
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
        let plans = self.plans(game);
        let mut applied = Applied::load(&self.root);
        let rels: Vec<String> = plans.iter().map(|p| p.rel.clone()).collect();
        backup::back_up_once(
            &mut applied,
            &self.backup_root,
            &self.backup_label,
            &self.root,
            &rels,
        )?;
        let report = crate::emusettings::merge::merge(&self.root, &plans, &mut applied)?;
        applied.save(&self.root)?;
        report.log("dolphin");
        self.settings
            .adopt(&report, game.map(|g| g.dir.as_path()))?;
        Ok(report)
    }

    /// The command that runs `rom`, logging to `<root>/Logs/hearth-launch.log`, or `None` if
    /// Dolphin isn't installed.
    pub fn command(&self, rom: &Path) -> Option<LaunchCommand> {
        let mut dolphin_args: Vec<OsString> =
            vec!["-u".into(), self.root.clone().into(), "-b".into()];
        for (key, value) in LOCKED {
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
        profile.prepare(&input, None).unwrap();
        assert!(user.dolphin().join("Config/GCPadNew.ini").is_file());
        assert!(user.dolphin().join("Logs").is_dir());
        assert!(user.gc_saves().is_dir());
        assert!(user.wii_nand().is_dir());
    }

    /// A profile in a temporary home, recommending Balanced (this laptop's Iris Xe).
    fn temp_profile(home: &Path) -> (ProfileDirs, DolphinProfile) {
        let user = ProfileDirs::new(
            crate::users::ProfileId::default(),
            &home.join("profiles"),
            &home.join("emulators"),
        );
        let profile = DolphinProfile::new(&user, None).with_gpu(Some(Gpu {
            vendor: 0x8086,
            device: 0x9a49,
        }));
        (user, profile)
    }

    fn tp(home: &Path) -> GameRef {
        GameRef {
            id: "GZ2E01".into(),
            dir: home.join("data/games/gc/GZ2E01"),
        }
    }

    /// Dolphin's merged files, with the temporary home replaced by `<home>`.
    fn assert_files(name: &str, home: &Path, user: &ProfileDirs) {
        let home = std::path::absolute(home).unwrap();
        for rel in [MAIN_INI, GFX_INI, LOGGER_INI, "GameSettings/GZ2E01.ini"] {
            let text = fs::read_to_string(user.dolphin().join(rel)).unwrap_or_default();
            let text = text.replace(home.to_str().unwrap(), "<home>");
            let file = rel.rsplit('/').next().unwrap();
            crate::profiles::input::assert_golden(
                Path::new(&format!("emusettings/dolphin/{name}/{file}")),
                &text,
            );
        }
    }

    #[test]
    fn fresh_profile_gets_locked_keys_and_the_recommended_preset() {
        let dir = tempdir();
        let (user, profile) = temp_profile(dir.path());
        let report = profile
            .prepare(&Translation::default(), Some(&tp(dir.path())))
            .unwrap();
        assert!(report.adopted.is_empty() && report.restored.is_empty());
        assert_files("fresh", dir.path(), &user);
        // Nothing to back up the first time: no files existed.
        assert!(!user.backup_root().exists());
        assert!(Applied::load(&user.dolphin()).backed_up);
    }

    #[test]
    fn hand_edits_and_settings_from_hearth() {
        let dir = tempdir();
        let (user, profile) = temp_profile(dir.path());
        // Before Hearth merged anything: the user's Dolphin.ini as Dolphin left it.
        let config = user.dolphin().join("Config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("Dolphin.ini"),
            "[Analytics]\nID = 7a30\n[DSP]\nDSPThread = True\n",
        )
        .unwrap();
        profile.prepare(&Translation::default(), None).unwrap();
        // Backed up once, with checksums.
        let backups: Vec<_> = fs::read_dir(user.backup_root())
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(backups.len(), 1);
        assert!(
            backups[0]
                .path()
                .join("default/dolphin/Config/Dolphin.ini")
                .is_file()
        );

        // Hand edits: a key of the user's, a locked key, a setting.
        let edit = |rel: &str, from: &str, to: &str| {
            let path = user.dolphin().join(rel);
            let text = fs::read_to_string(&path).unwrap();
            assert!(text.contains(from), "{rel}: {from}");
            fs::write(path, text.replace(from, to)).unwrap();
        };
        edit(MAIN_INI, "DSPThread = True", "DSPThread = False");
        edit(MAIN_INI, "Fullscreen = True", "Fullscreen = False");
        edit(GFX_INI, "InternalResolution = 2", "InternalResolution = 3");
        let report = profile.prepare(&Translation::default(), None).unwrap();
        assert_eq!(report.restored, ["Config/Dolphin.ini [Display] Fullscreen"]);
        assert_eq!(report.adopted.len(), 1);
        // The setting changed in the file is now the console's (Custom).
        let shared = profile.settings().shared();
        assert_eq!(
            shared.values.get("internal_resolution").map(String::as_str),
            Some("3")
        );
        assert_eq!(
            profile.settings().resolved(&shared).status().as_deref(),
            Some("Custom")
        );
        assert_files("hand_edited", dir.path(), &user);
    }

    #[test]
    fn game_overrides_go_to_the_game_ini() {
        let dir = tempdir();
        let (user, profile) = temp_profile(dir.path());
        let game = tp(dir.path());
        let mut overrides = crate::emusettings::GameOverrides::default();
        overrides.set("dolphin", "internal_resolution", Some("4"));
        overrides.set("dolphin", "arbitrary_mipmaps", Some("False"));
        overrides.set("dolphin", "backend", Some("Vulkan")); // console only: ignored
        overrides.save(&game.dir).unwrap();
        let mut shared = profile.settings().shared();
        crate::emusettings::store::set_preset(profile.settings().schema(), &mut shared, "quality");
        profile.settings().save_shared(&shared).unwrap();
        // A locked key in the game INI would beat the command line.
        let ini = user.dolphin().join("GameSettings/GZ2E01.ini");
        fs::create_dir_all(ini.parent().unwrap()).unwrap();
        fs::write(&ini, "[Core]\nSIDevice0 = 0\nCPUThread = False\n").unwrap();
        profile
            .prepare(&Translation::default(), Some(&game))
            .unwrap();
        assert_files("per_game", dir.path(), &user);

        // Reset to console defaults: the game's values leave the INI at the next launch.
        crate::emusettings::GameOverrides::default()
            .save(&game.dir)
            .unwrap();
        profile
            .prepare(&Translation::default(), Some(&game))
            .unwrap();
        assert_eq!(
            fs::read_to_string(&ini).unwrap(),
            "[Core]\nCPUThread = False\n[Video_Settings]\n[Video_Enhancements]\n"
        );
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
