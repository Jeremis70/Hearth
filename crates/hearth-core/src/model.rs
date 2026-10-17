//! The game model shared by every library source and runtime.

use std::fmt;
use std::path::PathBuf;

/// Stable identifier of a game across sources (e.g. `steam:620`, `roms:gc/zelda-tp`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GameId(String);

impl GameId {
    /// Wraps an identifier string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GameId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifier of a system ("gc", "n64", "steam"), as in `systems.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Deserialize)]
#[serde(transparent)]
pub struct SystemId(String);

impl SystemId {
    /// Wraps a system identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SystemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One playable game, whatever its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    /// Stable identifier.
    pub id: GameId,
    /// Display title.
    pub title: String,
    /// System the game runs on.
    pub system: SystemId,
    /// The ROM or disc image to launch.
    pub path: PathBuf,
}
