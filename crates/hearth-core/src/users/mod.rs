//! User profiles: what belongs to one person rather than to the console.
//!
//! Each profile has a folder, `<home>/profiles/<id>/`, holding its `profile.toml`, its game
//! saves (`saves/<system>/`), its controls files and its emulator directories. Playtime,
//! sessions, last played and favorites live in the library database, keyed by the profile id.
//! Everything else (library, art, picks, packs, settings, BIOS files) is shared by every
//! profile.
//!
//! There is one active profile per run, `default` unless `--profile` says otherwise. The
//! controls resolver and every save path are keyed by the profile, so guests and per-controller
//! users can be added later without moving data.
//!
//! (Not to be confused with [`crate::profiles`], which drives emulators.)

pub mod migrate;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::model::SystemId;

/// Errors about profiles.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    /// The id isn't 1–32 lowercase letters, digits or `-`.
    #[error("invalid profile id {0:?}: use 1-32 lowercase letters, digits or '-'")]
    InvalidId(String),
    /// A profile file or folder couldn't be read or written.
    #[error("{path}: {source}")]
    Io {
        /// File or folder.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// `profile.toml` isn't valid.
    #[error("{path}: {message}")]
    Parse {
        /// File.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
}

/// A profile's id, also its folder name: 1–32 lowercase ASCII letters, digits or `-`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProfileId(String);

impl ProfileId {
    /// The id of the profile every existing install's data moves into.
    pub const DEFAULT: &'static str = "default";

    /// Checks and wraps `id`.
    pub fn new(id: &str) -> Result<Self, ProfileError> {
        let valid = (1..=32).contains(&id.len())
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if valid {
            Ok(Self(id.to_owned()))
        } else {
            Err(ProfileError::InvalidId(id.to_owned()))
        }
    }

    /// The `default` profile.
    pub fn default_profile() -> Self {
        Self(Self::DEFAULT.to_owned())
    }

    /// The id as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ProfileId {
    fn default() -> Self {
        Self::default_profile()
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ProfileId {
    type Err = ProfileError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// `profile.toml`: who the profile is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserProfile {
    /// Shown name.
    pub name: String,
    /// When the profile was created.
    pub created: toml::value::Datetime,
}

impl UserProfile {
    /// A new profile named `name`, created now.
    pub fn new(name: impl Into<String>) -> Self {
        use chrono::{Datelike as _, Timelike as _};
        use toml::value::{Date, Datetime, Offset, Time};
        let now = chrono::Utc::now();
        // Every field fits: chrono's ranges are TOML's.
        let created = Datetime {
            date: Some(Date {
                year: u16::try_from(now.year()).unwrap_or(0),
                month: u8::try_from(now.month()).unwrap_or(1),
                day: u8::try_from(now.day()).unwrap_or(1),
            }),
            time: Some(Time {
                hour: u8::try_from(now.hour()).unwrap_or(0),
                minute: u8::try_from(now.minute()).unwrap_or(0),
                second: Some(u8::try_from(now.second()).unwrap_or(0)),
                nanosecond: None,
            }),
            offset: Some(Offset::Z),
        };
        Self {
            name: name.into(),
            created,
        }
    }
}

/// The folders of one profile, plus the shared emulator folders its emulators read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDirs {
    id: ProfileId,
    root: PathBuf,
    shared_emulators: PathBuf,
}

impl ProfileDirs {
    /// The profile `id` under `profiles` (`<home>/profiles`), using the shared emulator folder
    /// `shared_emulators` (`<home>/emulators`). Paths are made absolute: emulators run with
    /// their own working directory and the Flatpak sandbox needs absolute paths.
    pub fn new(id: ProfileId, profiles: &Path, shared_emulators: &Path) -> Self {
        let absolute = |p: PathBuf| std::path::absolute(&p).unwrap_or(p);
        Self {
            root: absolute(profiles.join(id.as_str())),
            shared_emulators: absolute(shared_emulators.to_path_buf()),
            id,
        }
    }

    /// The profile's id.
    pub fn id(&self) -> &ProfileId {
        &self.id
    }

    /// `profiles/<id>`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `profile.toml`.
    pub fn profile_file(&self) -> PathBuf {
        self.root.join("profile.toml")
    }

    /// What games save, one folder per system: `saves/<system>/`.
    pub fn saves(&self) -> PathBuf {
        self.root.join("saves")
    }

    /// The saves of one system.
    pub fn system_saves(&self, system: &SystemId) -> PathBuf {
        self.saves().join(system.as_str())
    }

    /// GameCube memory cards: GCI folders `Card A/<region>/` and raw cards
    /// `MemoryCardA.<region>.raw`.
    pub fn gc_saves(&self) -> PathBuf {
        self.saves().join("gc")
    }

    /// The Wii's NAND (Wii saves live in it).
    pub fn wii_nand(&self) -> PathBuf {
        self.saves().join("wii").join("nand")
    }

    /// The profile's controls files: `systems/<system>.toml`, `games/<system>/<serial>.toml`.
    pub fn controls(&self) -> PathBuf {
        self.root.join("controls")
    }

    /// The profile's emulator directories.
    pub fn emulators(&self) -> PathBuf {
        self.root.join("emulators")
    }

    /// The profile's RetroArch directory (config, states, remaps, logs).
    pub fn retroarch(&self) -> PathBuf {
        self.emulators().join("retroarch")
    }

    /// The profile's Dolphin user directory (config, states, cache, logs).
    pub fn dolphin(&self) -> PathBuf {
        self.emulators().join("dolphin")
    }

    /// Where backups go (`<home>/backup`, beside `profiles/` in both layouts).
    pub fn backup_root(&self) -> PathBuf {
        self.root
            .parent()
            .and_then(Path::parent)
            .map_or_else(|| self.root.join("backup"), |home| home.join("backup"))
    }

    /// An emulator's folder shared by every profile (`<home>/emulators/<name>`): the console
    /// settings, BIOS files.
    pub fn shared_emulator(&self, name: &str) -> PathBuf {
        self.shared_emulators.join(name)
    }

    /// RetroArch's system directory (BIOS and core files), shared by every profile.
    pub fn retroarch_system(&self) -> PathBuf {
        self.shared_emulators.join("retroarch").join("system")
    }

    /// Creates the profile's folder and `profile.toml` if missing, and reads it. A new
    /// profile has no controls files (the shipped defaults apply) and no saves.
    pub fn ensure(&self) -> Result<UserProfile, ProfileError> {
        let io_err = |path: &Path| {
            let path = path.to_path_buf();
            move |source| ProfileError::Io { path, source }
        };
        fs::create_dir_all(&self.root).map_err(io_err(&self.root))?;
        let file = self.profile_file();
        match fs::read_to_string(&file) {
            Ok(text) => toml::from_str(&text).map_err(|err| ProfileError::Parse {
                path: file.clone(),
                message: err.message().to_owned(),
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let name = if self.id.as_str() == ProfileId::DEFAULT {
                    "Default".to_owned()
                } else {
                    self.id.as_str().to_owned()
                };
                let profile = UserProfile::new(name);
                let text = toml::to_string(&profile).unwrap_or_default();
                write_atomic(&file, text.as_bytes()).map_err(io_err(&file))?;
                tracing::info!(profile = %self.id, path = %file.display(), "created profile");
                Ok(profile)
            }
            Err(err) => Err(io_err(&file)(err)),
        }
    }
}

/// Writes `bytes` to a temporary file next to `path`, syncs it and renames it over `path`, so
/// a crash leaves either the old file or the new one.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    sync_parent(path)
}

/// Syncs the directory holding `path`, so a rename into it survives a power cut.
pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => fs::File::open(dir)?.sync_all(),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    #[test]
    fn ids_are_folder_safe() {
        for good in ["default", "test2", "guest-1", &"a".repeat(32)] {
            assert!(ProfileId::new(good).is_ok(), "{good}");
        }
        for bad in ["", "Default", "../x", "a b", "é", &"a".repeat(33), "a/b"] {
            assert!(ProfileId::new(bad).is_err(), "{bad:?}");
        }
        assert_eq!(ProfileId::default().as_str(), "default");
    }

    #[test]
    fn dirs_follow_the_layout() {
        let p = ProfileDirs::new(
            ProfileId::new("test2").unwrap(),
            Path::new("/h/profiles"),
            Path::new("/h/emulators"),
        );
        assert_eq!(p.root(), Path::new("/h/profiles/test2"));
        assert_eq!(
            p.profile_file(),
            Path::new("/h/profiles/test2/profile.toml")
        );
        assert_eq!(
            p.system_saves(&SystemId::new("n64")),
            Path::new("/h/profiles/test2/saves/n64")
        );
        assert_eq!(p.gc_saves(), Path::new("/h/profiles/test2/saves/gc"));
        assert_eq!(p.wii_nand(), Path::new("/h/profiles/test2/saves/wii/nand"));
        assert_eq!(p.controls(), Path::new("/h/profiles/test2/controls"));
        assert_eq!(
            p.retroarch(),
            Path::new("/h/profiles/test2/emulators/retroarch")
        );
        assert_eq!(
            p.dolphin(),
            Path::new("/h/profiles/test2/emulators/dolphin")
        );
        assert_eq!(
            p.retroarch_system(),
            Path::new("/h/emulators/retroarch/system")
        );
    }

    #[test]
    fn relative_homes_become_absolute() {
        let p = ProfileDirs::new(
            ProfileId::default(),
            Path::new("./.dev-data/profiles"),
            Path::new("./.dev-data/emulators"),
        );
        assert!(p.root().is_absolute());
        assert!(p.retroarch_system().is_absolute());
    }

    #[test]
    fn ensure_creates_the_profile_once() {
        let dir = tempdir();
        let p = ProfileDirs::new(
            ProfileId::default(),
            &dir.path().join("profiles"),
            &dir.path().join("emulators"),
        );
        let first = p.ensure().unwrap();
        assert_eq!(first.name, "Default");
        let text = fs::read_to_string(p.profile_file()).unwrap();
        assert!(text.contains("name = \"Default\""), "{text}");
        assert!(text.contains("created = 20"), "a TOML datetime: {text}");
        fs::write(
            p.profile_file(),
            "name = \"Jérémie\"\ncreated = 2026-10-24T12:00:00Z\n",
        )
        .unwrap();
        assert_eq!(p.ensure().unwrap().name, "Jérémie", "never rewritten");
        let other = ProfileDirs::new(
            ProfileId::new("test2").unwrap(),
            &dir.path().join("profiles"),
            &dir.path().join("emulators"),
        );
        assert_eq!(other.ensure().unwrap().name, "test2");
        assert!(
            !other.controls().exists(),
            "no controls files: shipped defaults"
        );
    }
}
