//! The game library, persisted in SQLite: games found by scanning, when they were last played
//! and for how long.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Game, GameId, SystemId};

/// Errors from the library database.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// SQLite failed.
    #[error("library database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// A game plus its play history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    /// The game.
    pub game: Game,
    /// Unix time (seconds) the last session ended, if ever played.
    pub last_played: Option<i64>,
    /// Total playtime.
    pub playtime: Duration,
}

/// Handle to the library database. Not `Sync`: open one per thread (SQLite handles the
/// concurrency, with a busy timeout).
#[derive(Debug)]
pub struct Library {
    conn: Connection,
}

impl Library {
    /// Opens (creating if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS games (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                system TEXT NOT NULL,
                path TEXT NOT NULL,
                last_played INTEGER,
                playtime_secs INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        Ok(Self { conn })
    }

    /// Makes the stored games match a fresh scan: new games are added, titles and paths
    /// updated, games no longer found removed (with their history).
    pub fn sync(&mut self, games: &[Game]) -> Result<(), LibraryError> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS seen (id TEXT PRIMARY KEY); DELETE FROM seen;",
        )?;
        for game in games {
            tx.execute(
                "INSERT INTO games (id, title, system, path) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET title = ?2, system = ?3, path = ?4",
                params![
                    game.id.as_str(),
                    game.title,
                    game.system.as_str(),
                    game.path.to_string_lossy()
                ],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO seen (id) VALUES (?1)",
                [game.id.as_str()],
            )?;
        }
        tx.execute(
            "DELETE FROM games WHERE id NOT IN (SELECT id FROM seen)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// All games, most recently played first, then never-played games by title.
    pub fn recent(&self) -> Result<Vec<LibraryEntry>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, system, path, last_played, playtime_secs FROM games
             ORDER BY last_played IS NULL, last_played DESC, title",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(LibraryEntry {
                game: Game {
                    id: GameId::new(row.get::<_, String>(0)?),
                    title: row.get(1)?,
                    system: SystemId::new(row.get::<_, String>(2)?),
                    path: PathBuf::from(row.get::<_, String>(3)?),
                },
                last_played: row.get(4)?,
                playtime: Duration::from_secs(row.get::<_, i64>(5)?.max(0).unsigned_abs()),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Looks up one game.
    pub fn get(&self, id: &GameId) -> Result<Option<LibraryEntry>, LibraryError> {
        Ok(self.recent()?.into_iter().find(|e| &e.game.id == id))
    }

    /// Adds a finished session: sets last played to `ended_at` and adds `played`.
    pub fn record_session(
        &self,
        id: &GameId,
        ended_at: SystemTime,
        played: Duration,
    ) -> Result<(), LibraryError> {
        let ended = ended_at
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        let secs = i64::try_from(played.as_secs()).unwrap_or(i64::MAX);
        self.conn.execute(
            "UPDATE games SET last_played = ?2, playtime_secs = playtime_secs + ?3 WHERE id = ?1",
            params![id.as_str(), ended, secs],
        )?;
        Ok(())
    }

    /// Total playtime of one game, if it exists.
    pub fn playtime(&self, id: &GameId) -> Result<Option<Duration>, LibraryError> {
        let secs: Option<i64> = self
            .conn
            .query_row(
                "SELECT playtime_secs FROM games WHERE id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(secs.map(|s| Duration::from_secs(s.max(0).unsigned_abs())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    fn game(id: &str, title: &str) -> Game {
        Game {
            id: GameId::new(id),
            title: title.into(),
            system: SystemId::new("n64"),
            path: PathBuf::from(format!("/games/n64/{title}.z64")),
        }
    }

    #[test]
    fn sync_record_and_order() {
        let dir = tempdir();
        let path = dir.path().join("library.sqlite3");
        let mut lib = Library::open(&path).unwrap();
        lib.sync(&[game("a", "Banjo"), game("b", "Zelda"), game("c", "Mario")])
            .unwrap();

        let t = UNIX_EPOCH + Duration::from_secs(1_000);
        lib.record_session(&GameId::new("b"), t, Duration::from_secs(90))
            .unwrap();
        lib.record_session(
            &GameId::new("b"),
            t + Duration::from_secs(5),
            Duration::from_secs(30),
        )
        .unwrap();
        lib.record_session(
            &GameId::new("c"),
            t - Duration::from_secs(5),
            Duration::from_secs(1),
        )
        .unwrap();

        let titles: Vec<_> = lib
            .recent()
            .unwrap()
            .into_iter()
            .map(|e| e.game.title)
            .collect();
        assert_eq!(titles, ["Zelda", "Mario", "Banjo"]);
        assert_eq!(
            lib.playtime(&GameId::new("b")).unwrap(),
            Some(Duration::from_secs(120))
        );

        // Reopen: history persists; removed games disappear.
        drop(lib);
        let mut lib = Library::open(&path).unwrap();
        lib.sync(&[game("b", "Zelda (renamed)")]).unwrap();
        let entries = lib.recent().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].game.title, "Zelda (renamed)");
        assert_eq!(entries[0].last_played, Some(1_005));
        assert_eq!(entries[0].playtime, Duration::from_secs(120));
    }
}
