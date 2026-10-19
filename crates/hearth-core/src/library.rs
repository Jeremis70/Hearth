//! The game library, persisted in SQLite: games found by scanning, when they were last played
//! and for how long, favorites, and the artwork pipeline's results and retry queue.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::art::{ArtKey, ArtKind};
use crate::model::{Game, GameId, SystemId};

/// First retry delay for art that failed on the network; doubles with each attempt.
const ART_RETRY_BASE: Duration = Duration::from_secs(5 * 60);
/// Longest delay between art retries.
const ART_RETRY_MAX: Duration = Duration::from_secs(6 * 60 * 60);

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
    /// Marked as a favorite.
    pub favorite: bool,
}

/// A game that a sync removed from the library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedGame {
    /// The game as it was stored.
    pub game: Game,
    /// Its art key, if the art pipeline identified it.
    pub art_key: Option<ArtKey>,
}

/// What the art pipeline found for one art key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtStatus {
    /// Matched a SteamGridDB game; `kinds` are the kinds of art it had (and that were cached).
    Matched {
        /// SteamGridDB game id, kept permanently.
        sgdb_id: u64,
        /// Its name on SteamGridDB.
        sgdb_name: String,
        /// Kinds of art found.
        kinds: Vec<ArtKind>,
    },
    /// No search result was close enough. Not retried until the user refreshes the art.
    NoMatch,
}

/// A key waiting for the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedArt {
    /// The key.
    pub key: ArtKey,
    /// Failed attempts so far.
    pub attempts: u32,
    /// Unix time of the next try.
    pub next_try: i64,
    /// The last error.
    pub last_error: String,
}

/// Delay before retry number `attempts + 1`: 5 min, 10, 20… up to 6 h.
pub fn art_retry_delay(attempts: u32) -> Duration {
    let factor = 1u32 << attempts.saturating_sub(1).min(16);
    ART_RETRY_BASE.saturating_mul(factor).min(ART_RETRY_MAX)
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
        // Schema version 0: games only; 1: favorites, art keys, art result and queue tables.
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(
                "BEGIN;
                 ALTER TABLE games ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE games ADD COLUMN art_key TEXT;
                 CREATE TABLE IF NOT EXISTS art (
                     art_key TEXT PRIMARY KEY,
                     status TEXT NOT NULL,
                     sgdb_id INTEGER,
                     sgdb_name TEXT,
                     kinds TEXT NOT NULL DEFAULT '',
                     updated_at INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS art_queue (
                     art_key TEXT PRIMARY KEY,
                     attempts INTEGER NOT NULL,
                     next_try INTEGER NOT NULL,
                     last_error TEXT NOT NULL
                 );
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        Ok(Self { conn })
    }

    /// Makes the stored games match a fresh scan: new games are added, titles and paths
    /// updated, games no longer found removed (with their history). Returns the removed games.
    pub fn sync(&mut self, games: &[Game]) -> Result<Vec<RemovedGame>, LibraryError> {
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
        let removed = {
            let mut stmt = tx.prepare(
                "SELECT id, title, system, path, art_key FROM games
                 WHERE id NOT IN (SELECT id FROM seen)",
            )?;
            stmt.query_map([], |row| {
                Ok(RemovedGame {
                    game: Game {
                        id: GameId::new(row.get::<_, String>(0)?),
                        title: row.get(1)?,
                        system: SystemId::new(row.get::<_, String>(2)?),
                        path: PathBuf::from(row.get::<_, String>(3)?),
                    },
                    art_key: row.get::<_, Option<String>>(4)?.map(ArtKey::from_stored),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        tx.execute(
            "DELETE FROM games WHERE id NOT IN (SELECT id FROM seen)",
            [],
        )?;
        tx.commit()?;
        Ok(removed)
    }

    /// All games, most recently played first, then never-played games by title.
    pub fn recent(&self) -> Result<Vec<LibraryEntry>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, system, path, last_played, playtime_secs, favorite FROM games
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
                favorite: row.get::<_, i64>(6)? != 0,
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

    /// Marks or unmarks a game as a favorite.
    pub fn set_favorite(&self, id: &GameId, favorite: bool) -> Result<(), LibraryError> {
        self.conn.execute(
            "UPDATE games SET favorite = ?2 WHERE id = ?1",
            params![id.as_str(), i64::from(favorite)],
        )?;
        Ok(())
    }

    /// Records which art key a game has (so its art can be found when it is removed).
    pub fn set_art_key(&self, id: &GameId, key: &ArtKey) -> Result<(), LibraryError> {
        self.conn.execute(
            "UPDATE games SET art_key = ?2 WHERE id = ?1",
            params![id.as_str(), key.as_str()],
        )?;
        Ok(())
    }

    /// Whether any game in the library still has this art key.
    pub fn art_key_in_use(&self, key: &ArtKey) -> Result<bool, LibraryError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM games WHERE art_key = ?1 LIMIT 1",
                [key.as_str()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The art pipeline's result for `key`, if it got one.
    pub fn art_status(&self, key: &ArtKey) -> Result<Option<ArtStatus>, LibraryError> {
        let row = self
            .conn
            .query_row(
                "SELECT status, sgdb_id, sgdb_name, kinds FROM art WHERE art_key = ?1",
                [key.as_str()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(
            row.and_then(|(status, id, name, kinds)| match status.as_str() {
                "matched" => Some(ArtStatus::Matched {
                    sgdb_id: id.and_then(|id| u64::try_from(id).ok())?,
                    sgdb_name: name.unwrap_or_default(),
                    kinds: kinds.split(',').filter_map(ArtKind::from_name).collect(),
                }),
                "no_match" => Some(ArtStatus::NoMatch),
                _ => None,
            }),
        )
    }

    /// Stores the art pipeline's result for `key` and takes it off the queue.
    pub fn set_art_status(
        &self,
        key: &ArtKey,
        status: &ArtStatus,
        now: i64,
    ) -> Result<(), LibraryError> {
        let (name, id, sgdb_name, kinds) = match status {
            ArtStatus::Matched {
                sgdb_id,
                sgdb_name,
                kinds,
            } => (
                "matched",
                i64::try_from(*sgdb_id).ok(),
                Some(sgdb_name.as_str()),
                kinds.iter().map(|k| k.name()).collect::<Vec<_>>().join(","),
            ),
            ArtStatus::NoMatch => ("no_match", None, None, String::new()),
        };
        self.conn.execute(
            "INSERT INTO art (art_key, status, sgdb_id, sgdb_name, kinds, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(art_key) DO UPDATE SET status = ?2, sgdb_id = ?3, sgdb_name = ?4,
                 kinds = ?5, updated_at = ?6",
            params![key.as_str(), name, id, sgdb_name, kinds, now],
        )?;
        self.dequeue_art(key)
    }

    /// Forgets everything about `key`'s art (result and queue entry).
    pub fn forget_art(&self, key: &ArtKey) -> Result<(), LibraryError> {
        self.conn
            .execute("DELETE FROM art WHERE art_key = ?1", [key.as_str()])?;
        self.dequeue_art(key)
    }

    /// Puts `key` on the queue after a network or HTTP error, or bumps its attempt count;
    /// the next try waits [`art_retry_delay`]. Returns the entry.
    pub fn queue_art(
        &self,
        key: &ArtKey,
        error: &str,
        now: i64,
    ) -> Result<QueuedArt, LibraryError> {
        let attempts: u32 = self
            .conn
            .query_row(
                "SELECT attempts FROM art_queue WHERE art_key = ?1",
                [key.as_str()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0)
            + 1;
        let delay = i64::try_from(art_retry_delay(attempts).as_secs()).unwrap_or(i64::MAX);
        let entry = QueuedArt {
            key: key.clone(),
            attempts,
            next_try: now.saturating_add(delay),
            last_error: error.to_owned(),
        };
        self.conn.execute(
            "INSERT INTO art_queue (art_key, attempts, next_try, last_error) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(art_key) DO UPDATE SET attempts = ?2, next_try = ?3, last_error = ?4",
            params![key.as_str(), entry.attempts, entry.next_try, entry.last_error],
        )?;
        Ok(entry)
    }

    /// Takes `key` off the queue.
    pub fn dequeue_art(&self, key: &ArtKey) -> Result<(), LibraryError> {
        self.conn
            .execute("DELETE FROM art_queue WHERE art_key = ?1", [key.as_str()])?;
        Ok(())
    }

    /// The whole queue, soonest first.
    pub fn art_queue(&self) -> Result<Vec<QueuedArt>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT art_key, attempts, next_try, last_error FROM art_queue ORDER BY next_try",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(QueuedArt {
                key: ArtKey::from_stored(r.get::<_, String>(0)?),
                attempts: r.get(1)?,
                next_try: r.get(2)?,
                last_error: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
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

    #[test]
    fn version_0_databases_are_migrated_and_keep_their_history() {
        let dir = tempdir();
        let path = dir.path().join("library.sqlite3");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE games (id TEXT PRIMARY KEY, title TEXT NOT NULL,
                     system TEXT NOT NULL, path TEXT NOT NULL, last_played INTEGER,
                     playtime_secs INTEGER NOT NULL DEFAULT 0);
                 INSERT INTO games VALUES ('a', 'Banjo', 'n64', '/b.z64', 7, 60);",
            )
            .unwrap();
        }
        let lib = Library::open(&path).unwrap();
        let entries = lib.recent().unwrap();
        assert_eq!(entries[0].playtime, Duration::from_secs(60));
        assert!(!entries[0].favorite);
        lib.set_favorite(&GameId::new("a"), true).unwrap();
        drop(lib);
        // Opening again doesn't migrate twice.
        let lib = Library::open(&path).unwrap();
        assert!(lib.recent().unwrap()[0].favorite);
    }

    #[test]
    fn sync_reports_removed_games_with_their_art_key() {
        let dir = tempdir();
        let mut lib = Library::open(&dir.path().join("l.sqlite3")).unwrap();
        lib.sync(&[game("a", "Banjo"), game("b", "Zelda")]).unwrap();
        let key = ArtKey::new(&SystemId::new("n64"), Some("NBKE"), "");
        lib.set_art_key(&GameId::new("a"), &key).unwrap();
        assert!(lib.art_key_in_use(&key).unwrap());
        let removed = lib.sync(&[game("b", "Zelda")]).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].game.title, "Banjo");
        assert_eq!(removed[0].art_key, Some(key.clone()));
        assert!(!lib.art_key_in_use(&key).unwrap());
        assert!(lib.sync(&[game("b", "Zelda")]).unwrap().is_empty());
    }

    #[test]
    fn art_results_are_stored_and_clear_the_queue() {
        let dir = tempdir();
        let lib = Library::open(&dir.path().join("l.sqlite3")).unwrap();
        let key = ArtKey::new(&SystemId::new("gc"), Some("GZ2E01"), "");
        assert_eq!(lib.art_status(&key).unwrap(), None);

        lib.queue_art(&key, "offline", 100).unwrap();
        let matched = ArtStatus::Matched {
            sgdb_id: 4242,
            sgdb_name: "The Legend of Zelda: Twilight Princess".into(),
            kinds: vec![ArtKind::Grid, ArtKind::Hero, ArtKind::Logo],
        };
        lib.set_art_status(&key, &matched, 200).unwrap();
        assert_eq!(lib.art_status(&key).unwrap(), Some(matched));
        assert!(lib.art_queue().unwrap().is_empty());

        lib.set_art_status(&key, &ArtStatus::NoMatch, 300).unwrap();
        assert_eq!(lib.art_status(&key).unwrap(), Some(ArtStatus::NoMatch));
        lib.forget_art(&key).unwrap();
        assert_eq!(lib.art_status(&key).unwrap(), None);
    }

    #[test]
    fn needs_art_queue_backs_off() {
        let dir = tempdir();
        let lib = Library::open(&dir.path().join("l.sqlite3")).unwrap();
        let a = ArtKey::new(&SystemId::new("gc"), Some("GALE01"), "");
        let b = ArtKey::new(&SystemId::new("n64"), None, "Pikmin");
        let first = lib.queue_art(&a, "dns", 1_000).unwrap();
        assert_eq!((first.attempts, first.next_try), (1, 1_000 + 300));
        let second = lib.queue_art(&a, "timeout", 2_000).unwrap();
        assert_eq!((second.attempts, second.next_try), (2, 2_000 + 600));
        assert_eq!(second.last_error, "timeout");
        lib.queue_art(&b, "HTTP 503", 1_000).unwrap();
        let queue = lib.art_queue().unwrap();
        assert_eq!(
            queue.iter().map(|q| q.key.clone()).collect::<Vec<_>>(),
            [b.clone(), a.clone()]
        );
        lib.dequeue_art(&b).unwrap();
        assert_eq!(lib.art_queue().unwrap().len(), 1);

        assert_eq!(art_retry_delay(1), Duration::from_secs(300));
        assert_eq!(art_retry_delay(3), Duration::from_secs(1_200));
        assert_eq!(art_retry_delay(40), Duration::from_secs(6 * 3_600));
    }
}
