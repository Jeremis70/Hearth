//! Where Hearth keeps its config, data and cache.
//!
//! In development `HEARTH_HOME` points at a single folder (usually `./.dev-data`) holding
//! `config/`, `data/` and `cache/`. Without it, the XDG directories are used.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use nix::fcntl::{Flock, FlockArg};

/// Environment variable that overrides every Hearth directory.
pub const HEARTH_HOME_ENV: &str = "HEARTH_HOME";

/// Errors while resolving or creating the app directories.
#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    /// `HEARTH_HOME` is unset and no home directory could be found for XDG paths.
    #[error("no home directory found; set {HEARTH_HOME_ENV}")]
    NoHome,
    /// `HEARTH_HOME` is set but empty.
    #[error("{HEARTH_HOME_ENV} is set but empty")]
    EmptyHome,
    /// A directory could not be created.
    #[error("cannot create {path}: {source}")]
    Create {
        /// Directory that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
}

/// Errors taking the single-instance lock.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another Hearth holds the lock.
    #[error("Hearth is already running (pid {pid}); not starting another")]
    Running {
        /// The running instance's pid, as written in the lock file ("unknown" if unreadable).
        pid: String,
    },
    /// The lock file couldn't be opened or written.
    #[error("cannot use lock file {path}: {source}")]
    Io {
        /// Lock file.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
}

/// Held for the whole life of a Hearth process: a second one can't take it.
///
/// An exclusive `flock` on `<data>/hearth.lock`. The kernel releases it when the process
/// dies, so a lock left by a crashed Hearth (whose pid is still in the file) is simply taken
/// over.
#[derive(Debug)]
pub struct InstanceLock {
    _lock: Flock<File>,
    path: PathBuf,
}

impl InstanceLock {
    /// Takes the lock in `dir` and writes our pid into it.
    pub fn acquire(dir: &Path) -> Result<Self, LockError> {
        let path = dir.join("hearth.lock");
        let io_err = |source| LockError::Io {
            path: path.clone(),
            source,
        };
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(io_err)?;
        let mut lock = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(lock) => lock,
            Err((mut file, _)) => {
                let mut pid = String::new();
                let _ = file.read_to_string(&mut pid);
                let pid = pid.trim();
                return Err(LockError::Running {
                    pid: if pid.is_empty() {
                        "unknown".into()
                    } else {
                        pid.into()
                    },
                });
            }
        };
        lock.set_len(0).map_err(io_err)?;
        lock.rewind().map_err(io_err)?;
        writeln!(lock, "{}", std::process::id()).map_err(io_err)?;
        lock.flush().map_err(io_err)?;
        Ok(Self { _lock: lock, path })
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// `~/Games`, the default folder scanned for games.
pub fn default_games_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().join("Games"))
}

/// Resolved app directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirs {
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    retroarch: PathBuf,
    dolphin: PathBuf,
    runtime: PathBuf,
    packs: PathBuf,
}

impl DataDirs {
    /// Resolves the directories from the process environment without touching the disk.
    pub fn from_env() -> Result<Self, PathsError> {
        Self::resolve(std::env::var_os(HEARTH_HOME_ENV))
    }

    /// Resolves the directories from an optional `HEARTH_HOME` value.
    pub fn resolve(hearth_home: Option<OsString>) -> Result<Self, PathsError> {
        match hearth_home {
            Some(home) if home.is_empty() => Err(PathsError::EmptyHome),
            Some(home) => Ok(Self::under(Path::new(&home))),
            None => {
                let dirs = ProjectDirs::from("", "", "hearth").ok_or(PathsError::NoHome)?;
                Ok(Self {
                    config: dirs.config_dir().to_path_buf(),
                    data: dirs.data_dir().to_path_buf(),
                    cache: dirs.cache_dir().to_path_buf(),
                    retroarch: dirs.data_dir().join("retroarch"),
                    dolphin: dirs.data_dir().join("dolphin"),
                    runtime: dirs.data_dir().join("runtime"),
                    packs: dirs.data_dir().join("packs"),
                })
            }
        }
    }

    fn under(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            retroarch: root.join("retroarch"),
            dolphin: root.join("dolphin"),
            runtime: root.join("runtime"),
            packs: root.join("packs"),
        }
    }

    /// Creates the directories if they don't exist yet.
    pub fn create_all(&self) -> Result<(), PathsError> {
        for dir in [
            &self.config,
            &self.data,
            &self.cache,
            &self.retroarch,
            &self.dolphin,
            &self.runtime,
        ] {
            std::fs::create_dir_all(dir).map_err(|source| PathsError::Create {
                path: dir.clone(),
                source,
            })?;
        }
        Ok(())
    }

    /// Configuration files.
    pub fn config(&self) -> &Path {
        &self.config
    }

    /// Persistent data (library database, saves).
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// Disposable cache (art).
    pub fn cache(&self) -> &Path {
        &self.cache
    }

    /// Hearth's isolated RetroArch profile (config, saves, states, logs).
    pub fn retroarch(&self) -> &Path {
        &self.retroarch
    }

    /// Hearth's isolated Dolphin user directory (config, saves, NAND, logs).
    pub fn dolphin(&self) -> &Path {
        &self.dolphin
    }

    /// Files generated for one run and read by other processes (InputPlumber profiles).
    pub fn runtime(&self) -> &Path {
        &self.runtime
    }

    /// Installed asset packs, one folder each (`<packs>/<id>/`). Not created: it only
    /// exists once someone installs a pack.
    pub fn packs(&self) -> &Path {
        &self.packs
    }

    /// Hearth's settings file.
    pub fn config_file(&self) -> PathBuf {
        self.config.join("hearth.toml")
    }

    /// Cached game artwork, one folder per game key.
    pub fn art_cache(&self) -> PathBuf {
        self.cache.join("art")
    }

    /// The library database file.
    pub fn library_db(&self) -> PathBuf {
        self.data.join("library.sqlite3")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hearth_home_holds_all_directories() {
        let dirs = DataDirs::resolve(Some("./.dev-data".into())).unwrap();
        assert_eq!(dirs.config(), Path::new("./.dev-data/config"));
        assert_eq!(dirs.data(), Path::new("./.dev-data/data"));
        assert_eq!(dirs.cache(), Path::new("./.dev-data/cache"));
        assert_eq!(dirs.retroarch(), Path::new("./.dev-data/retroarch"));
        assert_eq!(dirs.dolphin(), Path::new("./.dev-data/dolphin"));
        assert_eq!(dirs.packs(), Path::new("./.dev-data/packs"));
        assert_eq!(dirs.art_cache(), Path::new("./.dev-data/cache/art"));
        assert_eq!(
            dirs.config_file(),
            Path::new("./.dev-data/config/hearth.toml")
        );
        assert_eq!(
            dirs.library_db(),
            Path::new("./.dev-data/data/library.sqlite3")
        );
    }

    #[test]
    fn second_lock_fails_until_the_first_is_dropped() {
        let dir = crate::testdir::tempdir();
        // A stale pid from a crashed instance doesn't matter: nobody holds the flock.
        std::fs::write(dir.path().join("hearth.lock"), "999999\n").unwrap();
        let first = InstanceLock::acquire(dir.path()).unwrap();
        let pid = std::fs::read_to_string(first.path()).unwrap();
        assert_eq!(pid.trim(), std::process::id().to_string());
        match InstanceLock::acquire(dir.path()) {
            Err(LockError::Running { pid }) => assert_eq!(pid, std::process::id().to_string()),
            other => panic!("expected Running, got {other:?}"),
        }
        drop(first);
        assert!(InstanceLock::acquire(dir.path()).is_ok());
    }

    #[test]
    fn empty_hearth_home_is_rejected() {
        assert!(matches!(
            DataDirs::resolve(Some(OsString::new())),
            Err(PathsError::EmptyHome)
        ));
    }

    #[test]
    fn without_hearth_home_uses_xdg_names() {
        // Only checks the resolved names; nothing is created.
        if let Ok(dirs) = DataDirs::resolve(None) {
            assert!(dirs.data().ends_with("hearth"));
            assert!(dirs.config().ends_with("hearth"));
        }
    }
}
