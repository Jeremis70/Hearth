//! Hearth's own settings file, `<config>/hearth.toml`.
//!
//! ```toml
//! [art]
//! steamgriddb_key = "…"                 # HEARTH_STEAMGRIDDB_KEY wins over this
//!
//! [packs]
//! console_icons = "my-console-icons"    # missing: Hearth's bundled defaults
//! ui_sounds = "ps5-sounds"
//! ```
//!
//! A missing file is an empty config. Settings change it through `toml_edit`, so comments and
//! anything written by hand (the API key) survive.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table, value};

/// Environment variable holding the SteamGridDB API key; wins over the config file.
pub const STEAMGRIDDB_KEY_ENV: &str = "HEARTH_STEAMGRIDDB_KEY";

/// Errors reading or writing the config file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file can't be read or written.
    #[error("{path}: {source}")]
    Io {
        /// File.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// The file isn't valid TOML, or has a value of the wrong type.
    #[error("{path}: {message}")]
    Parse {
        /// File.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
}

/// The kinds of asset packs a user can pick one of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackCategory {
    /// Console icons (`icons/consoles/<system>`).
    ConsoleIcons,
    /// UI sounds (`sounds/<name>`).
    UiSounds,
}

impl PackCategory {
    /// Both categories.
    pub const ALL: [Self; 2] = [Self::ConsoleIcons, Self::UiSounds];

    /// The key under `[packs]`.
    pub fn key(self) -> &'static str {
        match self {
            Self::ConsoleIcons => "console_icons",
            Self::UiSounds => "ui_sounds",
        }
    }
}

/// The parsed config. Every field is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Config {
    art: ArtConfig,
    packs: PacksConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct ArtConfig {
    steamgriddb_key: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct PacksConfig {
    console_icons: Option<String>,
    ui_sounds: Option<String>,
}

impl Config {
    /// Reads `path`; a missing file is the default config.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|err| ConfigError::Parse {
            path: path.to_path_buf(),
            message: err.message().to_owned(),
        })
    }

    /// The SteamGridDB API key: `env` (the value of [`STEAMGRIDDB_KEY_ENV`]) if set, else the
    /// file's. Blank keys count as none.
    pub fn steamgriddb_key(&self, env: Option<String>) -> Option<String> {
        env.or_else(|| self.art.steamgriddb_key.clone())
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
    }

    /// The selected pack id for `category`; `None` means Hearth's defaults.
    pub fn pack(&self, category: PackCategory) -> Option<&str> {
        let id = match category {
            PackCategory::ConsoleIcons => &self.packs.console_icons,
            PackCategory::UiSounds => &self.packs.ui_sounds,
        };
        id.as_deref().filter(|id| !id.is_empty())
    }

    /// Selects `pack` (or the defaults, with `None`) for `category`, in memory only.
    pub fn set_pack(&mut self, category: PackCategory, pack: Option<&str>) {
        let id = pack.map(str::to_owned);
        match category {
            PackCategory::ConsoleIcons => self.packs.console_icons = id,
            PackCategory::UiSounds => self.packs.ui_sounds = id,
        }
    }
}

/// Saves the pack choice for `category` into `path`, keeping everything else in the file.
/// `None` removes the entry (Hearth's defaults).
pub fn save_pack(
    path: &Path,
    category: PackCategory,
    pack: Option<&str>,
) -> Result<(), ConfigError> {
    let io_err = |source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(io_err(err)),
    };
    let mut doc: DocumentMut =
        text.parse()
            .map_err(|err: toml_edit::TomlError| ConfigError::Parse {
                path: path.to_path_buf(),
                message: err.message().to_owned(),
            })?;
    let packs = doc
        .entry("packs")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| ConfigError::Parse {
            path: path.to_path_buf(),
            message: "[packs] isn't a table".into(),
        })?;
    match pack {
        Some(id) => {
            packs.insert(category.key(), value(id));
        }
        None => {
            packs.remove(category.key());
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    fs::write(path, doc.to_string()).map_err(io_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    #[test]
    fn missing_file_is_default() {
        let dir = tempdir();
        let config = Config::load(&dir.path().join("hearth.toml")).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.steamgriddb_key(None), None);
        assert_eq!(config.pack(PackCategory::ConsoleIcons), None);
    }

    #[test]
    fn env_key_wins_and_blank_keys_are_none() {
        let dir = tempdir();
        let path = dir.path().join("hearth.toml");
        fs::write(&path, "[art]\nsteamgriddb_key = \" file-key \"\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.steamgriddb_key(None).as_deref(), Some("file-key"));
        assert_eq!(
            config.steamgriddb_key(Some("env-key".into())).as_deref(),
            Some("env-key")
        );
        fs::write(&path, "[art]\nsteamgriddb_key = \"\"\n").unwrap();
        assert_eq!(Config::load(&path).unwrap().steamgriddb_key(None), None);
    }

    #[test]
    fn saving_a_pack_keeps_comments_and_the_key() {
        let dir = tempdir();
        let path = dir.path().join("hearth.toml");
        fs::write(&path, "# mine\n[art]\nsteamgriddb_key = \"k\" # secret\n").unwrap();
        save_pack(&path, PackCategory::ConsoleIcons, Some("my-icons")).unwrap();
        save_pack(&path, PackCategory::UiSounds, Some("ps5")).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# mine") && text.contains("# secret"),
            "{text}"
        );
        let config = Config::load(&path).unwrap();
        assert_eq!(config.pack(PackCategory::ConsoleIcons), Some("my-icons"));
        assert_eq!(config.pack(PackCategory::UiSounds), Some("ps5"));
        assert_eq!(config.steamgriddb_key(None).as_deref(), Some("k"));

        save_pack(&path, PackCategory::ConsoleIcons, None).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.pack(PackCategory::ConsoleIcons), None);
        assert_eq!(config.pack(PackCategory::UiSounds), Some("ps5"));
    }

    #[test]
    fn wrong_types_are_errors() {
        let dir = tempdir();
        let path = dir.path().join("hearth.toml");
        fs::write(&path, "[packs]\nconsole_icons = 3\n").unwrap();
        assert!(matches!(
            Config::load(&path),
            Err(ConfigError::Parse { .. })
        ));
    }
}
