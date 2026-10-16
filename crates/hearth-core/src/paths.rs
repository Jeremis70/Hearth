//! Where Hearth keeps its config, data and cache.
//!
//! In development `HEARTH_HOME` points at a single folder (usually `./.dev-data`) holding
//! `config/`, `data/` and `cache/`. Without it, the XDG directories are used.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;

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

/// Resolved app directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirs {
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
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
                })
            }
        }
    }

    fn under(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
        }
    }

    /// Creates the directories if they don't exist yet.
    pub fn create_all(&self) -> Result<(), PathsError> {
        for dir in [&self.config, &self.data, &self.cache] {
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
