//! Hearth's own settings file, `<config>/hearth.toml`.
//!
//! ```toml
//! [art]
//! steamgriddb_key = "…"                 # HEARTH_STEAMGRIDDB_KEY wins over this
//!
//! [packs]
//! console_icons = "my-console-icons"    # missing: Hearth's bundled defaults
//! ui_sounds = "ps5-sounds"
//!
//! [audio]
//! ui_volume = 80                        # 0–100, a perceptual (dB) scale
//! music_volume = 70
//! mute = false
//! music_in_overlay = true
//! game_themes = true
//! ```
//!
//! A missing file is an empty config. Settings change it through `toml_edit`, so comments and
//! anything written by hand (the API key) survive.

use std::fs;
use std::io;
use std::ops::Range;
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
    #[error("{path}{}: {message}", line.map(|l| format!(", line {l}")).unwrap_or_default())]
    Parse {
        /// File.
        path: PathBuf,
        /// Line of the problem (1-based), when the parser knows it.
        line: Option<usize>,
        /// Parser message, with any quoted file content taken out (it could be the API key).
        message: String,
    },
}

impl ConfigError {
    fn parse(path: &Path, text: &str, message: &str, span: Option<Range<usize>>) -> Self {
        let line = span.map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
        Self::Parse {
            path: path.to_path_buf(),
            line,
            message: redact(message),
        }
    }

    /// The line a parse error is on.
    pub fn line(&self) -> Option<usize> {
        match self {
            Self::Parse { line, .. } => *line,
            Self::Io { .. } => None,
        }
    }
}

/// `message` with everything between backticks or double quotes replaced by "…": parser
/// messages quote values from the file ("invalid type: integer `123`"), and a value in
/// `hearth.toml` may be a secret.
fn redact(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut open: Option<char> = None;
    for c in message.chars() {
        match open {
            Some(quote) if c == quote => {
                out.push('…');
                out.push(c);
                open = None;
            }
            Some(_) => {}
            None => {
                out.push(c);
                if c == '`' || c == '"' {
                    open = Some(c);
                }
            }
        }
    }
    if open.is_some() {
        out.push('…');
    }
    out
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
    audio: AudioSettings,
}

/// Hearth's own volumes and music options (`[audio]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// UI sounds volume, 0–100.
    pub ui_volume: u8,
    /// Music (ambient and game themes) volume, 0–100.
    pub music_volume: u8,
    /// Silences everything Hearth plays.
    pub mute: bool,
    /// Music plays in the overlay over a frozen game.
    pub music_in_overlay: bool,
    /// A focused game's theme plays instead of the ambient.
    pub game_themes: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            ui_volume: 80,
            music_volume: 70,
            mute: false,
            music_in_overlay: true,
            game_themes: true,
        }
    }
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
        toml::from_str(&text)
            .map_err(|err| ConfigError::parse(path, &text, err.message(), err.span()))
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

impl Config {
    /// The `[audio]` settings (volumes clamped to 100).
    pub fn audio(&self) -> AudioSettings {
        AudioSettings {
            ui_volume: self.audio.ui_volume.min(100),
            music_volume: self.audio.music_volume.min(100),
            ..self.audio
        }
    }

    /// Replaces the audio settings, in memory only.
    pub fn set_audio(&mut self, audio: AudioSettings) {
        self.audio = audio;
    }
}

/// Saves the pack choice for `category` into `path`, keeping everything else in the file.
/// `None` removes the entry (Hearth's defaults).
pub fn save_pack(
    path: &Path,
    category: PackCategory,
    pack: Option<&str>,
) -> Result<(), ConfigError> {
    edit_table(path, "packs", |packs| match pack {
        Some(id) => {
            packs.insert(category.key(), value(id));
        }
        None => {
            packs.remove(category.key());
        }
    })
}

/// Saves `[audio]` into `path`, keeping everything else in the file.
pub fn save_audio(path: &Path, audio: &AudioSettings) -> Result<(), ConfigError> {
    edit_table(path, "audio", |table| {
        table.insert("ui_volume", value(i64::from(audio.ui_volume)));
        table.insert("music_volume", value(i64::from(audio.music_volume)));
        table.insert("mute", value(audio.mute));
        table.insert("music_in_overlay", value(audio.music_in_overlay));
        table.insert("game_themes", value(audio.game_themes));
    })
}

/// Changes one table of the file through `toml_edit` (comments and other tables survive).
fn edit_table(path: &Path, name: &str, edit: impl FnOnce(&mut Table)) -> Result<(), ConfigError> {
    let io_err = |source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(io_err(err)),
    };
    let mut doc: DocumentMut = text.parse().map_err(|err: toml_edit::TomlError| {
        ConfigError::parse(path, &text, err.message(), err.span())
    })?;
    let table = doc
        .entry(name)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| ConfigError::Parse {
            path: path.to_path_buf(),
            line: None,
            message: format!("[{name}] isn't a table"),
        })?;
    edit(table);
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
    fn audio_defaults_save_and_clamp() {
        let dir = tempdir();
        let path = dir.path().join("hearth.toml");
        assert_eq!(
            Config::load(&path).unwrap().audio(),
            AudioSettings::default()
        );
        fs::write(
            &path,
            "[packs]\nui_sounds = \"ps5\"\n[audio]\nui_volume = 250\n",
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.audio().ui_volume, 100);
        assert_eq!(config.audio().music_volume, 70);
        let audio = AudioSettings {
            ui_volume: 30,
            music_volume: 0,
            mute: true,
            music_in_overlay: false,
            game_themes: false,
        };
        save_audio(&path, &audio).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.audio(), audio);
        assert_eq!(config.pack(PackCategory::UiSounds), Some("ps5"));
    }

    #[test]
    fn wrong_types_are_errors() {
        let dir = tempdir();
        let path = dir.path().join("hearth.toml");
        fs::write(&path, "[packs]\nconsole_icons = 3\n").unwrap();
        let err = Config::load(&path).unwrap_err();
        assert_eq!(err.line(), Some(2));
    }

    #[test]
    fn parse_errors_name_the_line_and_never_the_values() {
        let dir = tempdir();
        let path = dir.path().join("hearth.toml");
        // The classic mistake: the key not quoted.
        fs::write(&path, "# mine\n[art]\nsteamgriddb_key = abc123secret\n").unwrap();
        let err = Config::load(&path).unwrap_err();
        assert_eq!(err.line(), Some(3));
        assert!(!err.to_string().contains("abc123secret"), "{err}");
        // Wrong types quote the value in the parser's message.
        fs::write(&path, "[art]\nsteamgriddb_key = 123456789\n").unwrap();
        let err = Config::load(&path).unwrap_err();
        assert_eq!(err.line(), Some(2));
        assert!(!err.to_string().contains("123456789"), "{err}");
        // Saving a pack into a broken file fails and leaves it alone.
        fs::write(&path, "[art]\nsteamgriddb_key = abc123secret\n").unwrap();
        let err = save_pack(&path, PackCategory::ConsoleIcons, Some("x")).unwrap_err();
        assert!(!err.to_string().contains("abc123secret"), "{err}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[art]\nsteamgriddb_key = abc123secret\n"
        );
    }

    #[test]
    fn redact_hides_quoted_parts() {
        assert_eq!(
            redact("invalid type: integer `12`, expected a string"),
            "invalid type: integer `…`, expected a string"
        );
        assert_eq!(redact("unknown \"abc"), "unknown \"…");
        assert_eq!(redact("plain"), "plain");
    }
}
