//! Fake "recently played" games for the M0 home screen.
//!
//! TODO(M2): replace with the library's recent-games query.

use std::path::PathBuf;

use crate::model::{Game, GameId, SystemId};

/// An sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// A demo game with the extra data the home hero zone shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoGame {
    /// The game itself.
    pub game: Game,
    /// Short system label for badges ("GameCube").
    pub system_short: String,
    /// Human playtime ("12 h played").
    pub playtime: String,
    /// Human last-played ("last played yesterday").
    pub last_played: String,
    /// Two colors for the placeholder art gradient.
    pub art: [Rgb; 2],
}

/// Six recently played games, most recent first.
pub fn recent_games() -> Vec<DemoGame> {
    [
        (
            "gc-tp",
            "Twilight Princess",
            "gc",
            "GameCube",
            "12 h played",
            "last played yesterday",
            [Rgb(0x3B, 0x5B, 0x3A), Rgb(0x1A, 0x22, 0x18)],
        ),
        (
            "n64-oot",
            "Ocarina of Time",
            "n64",
            "N64",
            "31 h played",
            "last played 3 days ago",
            [Rgb(0x2E, 0x6F, 0x6A), Rgb(0x13, 0x24, 0x2B)],
        ),
        (
            "steam-hades",
            "Hades",
            "steam",
            "Steam",
            "48 h played",
            "last played last week",
            [Rgb(0x8C, 0x2F, 0x22), Rgb(0x2A, 0x10, 0x0E)],
        ),
        (
            "snes-sm",
            "Super Metroid",
            "snes",
            "SNES",
            "6 h played",
            "last played 2 weeks ago",
            [Rgb(0x5A, 0x2E, 0x6E), Rgb(0x1B, 0x10, 0x26)],
        ),
        (
            "ps2-sotc",
            "Shadow of the Colossus",
            "ps2",
            "PS2",
            "9 h played",
            "last played last month",
            [Rgb(0x8A, 0x7A, 0x5C), Rgb(0x26, 0x22, 0x1C)],
        ),
        (
            "3ds-mm",
            "Majora's Mask 3D",
            "3ds",
            "3DS",
            "15 h played",
            "last played 2 months ago",
            [Rgb(0x9C, 0x5A, 0x1E), Rgb(0x2B, 0x17, 0x0C)],
        ),
    ]
    .into_iter()
    .map(
        |(id, title, system, short, playtime, last_played, art)| DemoGame {
            game: Game {
                id: GameId::new(id),
                title: title.to_owned(),
                system: SystemId::new(system),
                path: PathBuf::new(),
            },
            system_short: short.to_owned(),
            playtime: playtime.to_owned(),
            last_played: last_played.to_owned(),
            art,
        },
    )
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_unique_recent_games() {
        let games = recent_games();
        assert_eq!(games.len(), 6);
        let mut ids: Vec<_> = games.iter().map(|g| g.game.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 6);
    }
}
