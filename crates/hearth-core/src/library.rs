//! The game library, persisted in SQLite: games found by scanning, when they were last played
//! and for how long, favorites, and the artwork pipeline's results and retry queue.
//!
//! Games and art are shared; play history and favorites belong to a user profile
//! (`profile_games`, `play_sessions`, keyed by [`ProfileId`]).
//!
//! A game's id is its path under the system folder, so moving a ROM (e.g. into its own
//! folder) gives it a new id. A sync recognises that: a game that vanished and a new one with
//! the same system and file name are the same game, and its rows (art key, and every profile's
//! playtime, last played, favorite and sessions) move to the new id. Only an unambiguous pair
//! moves.

use std::collections::{HashMap, HashSet};

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::art::{ArtKey, ArtKind};
use crate::model::{Game, GameId, SystemId};
use crate::users::ProfileId;

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
    /// The library is from before profiles and holds games: only the move into profiles
    /// (`crate::users::migrate`, after its backup) may upgrade it.
    #[error("the library is from before profiles; it is upgraded by the move into profiles")]
    NeedsProfileUpgrade,
}

/// A game plus one profile's play history.
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

/// A SteamGridDB game the matcher found for an art key (for "Wrong game?").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtCandidate {
    /// SteamGridDB game id.
    pub sgdb_id: u64,
    /// Its name there.
    pub name: String,
    /// The search that found it (the ROM header's name or the file name).
    pub query: String,
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
    ///
    /// A library from before profiles (version < 4) that holds games is refused
    /// ([`LibraryError::NeedsProfileUpgrade`]) and left untouched: its history moves to the
    /// default profile only through the migration, which backs it up first
    /// ([`Library::open_upgrading`]).
    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        Self::open_with(path, false)
    }

    /// Like [`Library::open`], also upgrading a library from before profiles (its history
    /// goes to the default profile). For the migration, after its backup.
    pub fn open_upgrading(path: &Path) -> Result<Self, LibraryError> {
        Self::open_with(path, true)
    }

    fn open_with(path: &Path, profile_upgrade: bool) -> Result<Self, LibraryError> {
        let mut conn = Connection::open(path)?;
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
        // Schema version 0: games only; 1: favorites, art keys, art result and queue tables;
        // 2: the game's own folder; 3: the matcher's candidates; 4: play history and favorites
        // per profile (existing history goes to the default profile).
        // The version is read under the write lock, so two threads opening the database at
        // once (library and art worker) don't both migrate.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: i32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            tx.execute_batch(
                "ALTER TABLE games ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0;
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
                 PRAGMA user_version = 1;",
            )?;
        }
        if version < 2 {
            tx.execute_batch(
                "ALTER TABLE games ADD COLUMN folder TEXT;
                 PRAGMA user_version = 2;",
            )?;
        }
        if version < 3 {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS art_candidates (
                     art_key TEXT NOT NULL,
                     rank INTEGER NOT NULL,
                     sgdb_id INTEGER NOT NULL,
                     name TEXT NOT NULL,
                     query TEXT NOT NULL,
                     PRIMARY KEY (art_key, sgdb_id)
                 );
                 PRAGMA user_version = 3;",
            )?;
        }
        if version < 4 {
            let games: i64 = tx.query_row("SELECT COUNT(*) FROM games", [], |r| r.get(0))?;
            if games > 0 && !profile_upgrade {
                // Dropping the transaction rolls back the earlier steps too.
                return Err(LibraryError::NeedsProfileUpgrade);
            }
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS profile_games (
                     profile_id TEXT NOT NULL,
                     game_id TEXT NOT NULL,
                     last_played INTEGER,
                     playtime_secs INTEGER NOT NULL DEFAULT 0,
                     favorite INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY (profile_id, game_id)
                 );
                 CREATE TABLE IF NOT EXISTS play_sessions (
                     profile_id TEXT NOT NULL,
                     game_id TEXT NOT NULL,
                     ended_at INTEGER NOT NULL,
                     secs INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS play_sessions_game
                     ON play_sessions (profile_id, game_id);",
            )?;
            tx.execute(
                "INSERT INTO profile_games (profile_id, game_id, last_played, playtime_secs,
                     favorite)
                 SELECT ?1, id, last_played, playtime_secs, favorite FROM games
                 WHERE last_played IS NOT NULL OR playtime_secs != 0 OR favorite != 0",
                [ProfileId::DEFAULT],
            )?;
            tx.execute_batch(
                "ALTER TABLE games DROP COLUMN last_played;
                 ALTER TABLE games DROP COLUMN playtime_secs;
                 ALTER TABLE games DROP COLUMN favorite;
                 PRAGMA user_version = 4;",
            )?;
        }
        tx.commit()?;
        Ok(Self { conn })
    }

    /// A transaction that will write, taking the write lock first (`BEGIN IMMEDIATE`). A
    /// deferred one reads under a shared lock, then upgrades at its first write; if another
    /// connection holds the write lock at that moment (the art worker opening the library at
    /// startup), SQLite fails the upgrade at once with "database is locked" instead of waiting
    /// out the busy timeout, since waiting could deadlock. Taking the lock up front waits.
    fn write_transaction(&mut self) -> rusqlite::Result<rusqlite::Transaction<'_>> {
        self.conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
    }

    /// Makes the stored games match a fresh scan: new games are added, titles and paths
    /// updated, games no longer found removed (with every profile's history). Returns the
    /// removed games.
    pub fn sync(&mut self, games: &[Game]) -> Result<Vec<RemovedGame>, LibraryError> {
        let tx = self.write_transaction()?;
        move_renamed(&tx, games)?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS seen (id TEXT PRIMARY KEY); DELETE FROM seen;",
        )?;
        for game in games {
            tx.execute(
                "INSERT INTO games (id, title, system, path, folder) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET title = ?2, system = ?3, path = ?4, folder = ?5",
                params![
                    game.id.as_str(),
                    game.title,
                    game.system.as_str(),
                    game.path.to_string_lossy(),
                    game.folder
                        .as_ref()
                        .map(|f| f.to_string_lossy().into_owned())
                ],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO seen (id) VALUES (?1)",
                [game.id.as_str()],
            )?;
        }
        let removed = {
            let mut stmt = tx.prepare(
                "SELECT id, title, system, path, art_key, folder FROM games
                 WHERE id NOT IN (SELECT id FROM seen)",
            )?;
            stmt.query_map([], |row| {
                Ok(RemovedGame {
                    game: Game {
                        id: GameId::new(row.get::<_, String>(0)?),
                        title: row.get(1)?,
                        system: SystemId::new(row.get::<_, String>(2)?),
                        path: PathBuf::from(row.get::<_, String>(3)?),
                        folder: row.get::<_, Option<String>>(5)?.map(PathBuf::from),
                    },
                    art_key: row.get::<_, Option<String>>(4)?.map(ArtKey::from_stored),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        tx.execute_batch(
            "DELETE FROM profile_games WHERE game_id NOT IN (SELECT id FROM seen);
             DELETE FROM play_sessions WHERE game_id NOT IN (SELECT id FROM seen);
             DELETE FROM games WHERE id NOT IN (SELECT id FROM seen);",
        )?;
        tx.commit()?;
        Ok(removed)
    }

    /// All games with `profile`'s history, most recently played first, then never-played
    /// games by title.
    pub fn recent(&self, profile: &ProfileId) -> Result<Vec<LibraryEntry>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT g.id, g.title, g.system, g.path, p.last_played,
                    COALESCE(p.playtime_secs, 0), COALESCE(p.favorite, 0), g.folder
             FROM games g LEFT JOIN profile_games p ON p.game_id = g.id AND p.profile_id = ?1
             ORDER BY p.last_played IS NULL, p.last_played DESC, g.title",
        )?;
        let rows = stmt.query_map([profile.as_str()], |row| {
            Ok(LibraryEntry {
                game: Game {
                    id: GameId::new(row.get::<_, String>(0)?),
                    title: row.get(1)?,
                    system: SystemId::new(row.get::<_, String>(2)?),
                    path: PathBuf::from(row.get::<_, String>(3)?),
                    folder: row.get::<_, Option<String>>(7)?.map(PathBuf::from),
                },
                last_played: row.get(4)?,
                playtime: Duration::from_secs(row.get::<_, i64>(5)?.max(0).unsigned_abs()),
                favorite: row.get::<_, i64>(6)? != 0,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Looks up one game with `profile`'s history.
    pub fn get(
        &self,
        profile: &ProfileId,
        id: &GameId,
    ) -> Result<Option<LibraryEntry>, LibraryError> {
        Ok(self.recent(profile)?.into_iter().find(|e| &e.game.id == id))
    }

    /// Adds `profile`'s finished session: sets last played to `ended_at`, adds `played` and
    /// logs the session. Nothing happens for a game not in the library.
    pub fn record_session(
        &mut self,
        profile: &ProfileId,
        id: &GameId,
        ended_at: SystemTime,
        played: Duration,
    ) -> Result<(), LibraryError> {
        let ended = ended_at
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        let secs = i64::try_from(played.as_secs()).unwrap_or(i64::MAX);
        let tx = self.write_transaction()?;
        let added = tx.execute(
            "INSERT INTO profile_games (profile_id, game_id, last_played, playtime_secs)
             SELECT ?1, id, ?3, ?4 FROM games WHERE id = ?2
             ON CONFLICT(profile_id, game_id) DO UPDATE
                 SET last_played = ?3, playtime_secs = playtime_secs + ?4",
            params![profile.as_str(), id.as_str(), ended, secs],
        )?;
        if added > 0 {
            tx.execute(
                "INSERT INTO play_sessions (profile_id, game_id, ended_at, secs)
                 VALUES (?1, ?2, ?3, ?4)",
                params![profile.as_str(), id.as_str(), ended, secs],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// `profile`'s total playtime of one game, if the game exists.
    pub fn playtime(
        &self,
        profile: &ProfileId,
        id: &GameId,
    ) -> Result<Option<Duration>, LibraryError> {
        let secs: Option<i64> = self
            .conn
            .query_row(
                "SELECT COALESCE(p.playtime_secs, 0) FROM games g
                 LEFT JOIN profile_games p ON p.game_id = g.id AND p.profile_id = ?1
                 WHERE g.id = ?2",
                [profile.as_str(), id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(secs.map(|s| Duration::from_secs(s.max(0).unsigned_abs())))
    }

    /// `profile`'s sessions of one game, oldest first: (Unix time it ended, length).
    pub fn sessions(
        &self,
        profile: &ProfileId,
        id: &GameId,
    ) -> Result<Vec<(i64, Duration)>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT ended_at, secs FROM play_sessions WHERE profile_id = ?1 AND game_id = ?2
             ORDER BY ended_at, rowid",
        )?;
        let rows = stmt.query_map([profile.as_str(), id.as_str()], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Duration::from_secs(r.get::<_, i64>(1)?.max(0).unsigned_abs()),
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Marks or unmarks a game as one of `profile`'s favorites.
    pub fn set_favorite(
        &self,
        profile: &ProfileId,
        id: &GameId,
        favorite: bool,
    ) -> Result<(), LibraryError> {
        self.conn.execute(
            "INSERT INTO profile_games (profile_id, game_id, favorite)
             SELECT ?1, id, ?3 FROM games WHERE id = ?2
             ON CONFLICT(profile_id, game_id) DO UPDATE SET favorite = ?3",
            params![profile.as_str(), id.as_str(), i64::from(favorite)],
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
        self.conn.execute(
            "DELETE FROM art_candidates WHERE art_key = ?1",
            [key.as_str()],
        )?;
        self.dequeue_art(key)
    }

    /// Replaces the games the matcher found for `key` (best first; repeats of one game keep
    /// the first).
    pub fn set_art_candidates(
        &self,
        key: &ArtKey,
        candidates: &[ArtCandidate],
    ) -> Result<(), LibraryError> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        tx.execute(
            "DELETE FROM art_candidates WHERE art_key = ?1",
            [key.as_str()],
        )?;
        for (rank, candidate) in candidates.iter().enumerate() {
            tx.execute(
                "INSERT OR IGNORE INTO art_candidates (art_key, rank, sgdb_id, name, query)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    key.as_str(),
                    i64::try_from(rank).unwrap_or(i64::MAX),
                    i64::try_from(candidate.sgdb_id).unwrap_or(i64::MAX),
                    candidate.name,
                    candidate.query
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The games the matcher found for `key`, best first.
    pub fn art_candidates(&self, key: &ArtKey) -> Result<Vec<ArtCandidate>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT sgdb_id, name, query FROM art_candidates WHERE art_key = ?1 ORDER BY rank",
        )?;
        let rows = stmt.query_map([key.as_str()], |r| {
            Ok(ArtCandidate {
                sgdb_id: u64::try_from(r.get::<_, i64>(0)?).unwrap_or(0),
                name: r.get(1)?,
                query: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
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

/// Moves the rows of games that vanished to the new ids of the same games found elsewhere
/// (same system and file name, one of each), before the sync adds and removes rows.
fn move_renamed(tx: &rusqlite::Transaction<'_>, games: &[Game]) -> Result<(), LibraryError> {
    let file_name = |path: &Path| path.file_name().map(|n| n.to_string_lossy().into_owned());
    let incoming: HashSet<&str> = games.iter().map(|g| g.id.as_str()).collect();
    let mut gone: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut stored = HashSet::new();
    {
        let mut stmt = tx.prepare("SELECT id, system, path FROM games")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, system, path) = row?;
            if !incoming.contains(id.as_str())
                && let Some(name) = file_name(Path::new(&path))
            {
                gone.entry((system, name)).or_default().push(id.clone());
            }
            stored.insert(id);
        }
    }
    let mut new: HashMap<(String, String), Vec<&Game>> = HashMap::new();
    for game in games.iter().filter(|g| !stored.contains(g.id.as_str())) {
        if let Some(name) = file_name(&game.path) {
            new.entry((game.system.as_str().to_owned(), name))
                .or_default()
                .push(game);
        }
    }
    for (key, found) in new {
        let (Some([old]), [game]) = (gone.get(&key).map(Vec::as_slice), found.as_slice()) else {
            continue;
        };
        tracing::info!(from = %old, to = %game.id, "game moved; keeping its history");
        for table in [
            "UPDATE games SET id = ?2 WHERE id = ?1",
            "UPDATE profile_games SET game_id = ?2 WHERE game_id = ?1",
            "UPDATE play_sessions SET game_id = ?2 WHERE game_id = ?1",
        ] {
            tx.execute(table, params![old, game.id.as_str()])?;
        }
    }
    Ok(())
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
            folder: None,
        }
    }

    #[test]
    fn sync_waits_for_another_writer_instead_of_failing() {
        // Startup: the art worker's connection holds the write lock (its own `open`) while
        // the library thread syncs a scan that adds a game.
        let dir = tempdir();
        let path = dir.path().join("l.sqlite3");
        let mut lib = Library::open(&path).unwrap();
        lib.sync(&[game("roms:n64/Zelda.z64", "Zelda")]).unwrap();
        let other = Connection::open(&path).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            other.execute_batch("COMMIT").unwrap();
        });
        held_rx.recv().unwrap();
        let started = std::time::Instant::now();
        // A deferred transaction failed here at once with "database is locked".
        lib.sync(&[
            game("roms:n64/Zelda.z64", "Zelda"),
            game("roms:n64/Mario.z64", "Mario"),
        ])
        .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(200));
        writer.join().unwrap();
        assert_eq!(lib.recent(&ProfileId::default()).unwrap().len(), 2);
    }

    #[test]
    fn a_moved_game_keeps_its_history() {
        let dir = tempdir();
        let mut lib = Library::open(&dir.path().join("l.sqlite3")).unwrap();
        lib.sync(&[
            game("roms:n64/Zelda.z64", "Zelda"),
            game("roms:n64/Mario.z64", "Mario"),
        ])
        .unwrap();
        let old = GameId::new("roms:n64/Zelda.z64");
        lib.record_session(
            &ProfileId::default(),
            &old,
            UNIX_EPOCH + Duration::from_secs(9),
            Duration::from_secs(60),
        )
        .unwrap();
        lib.set_favorite(&ProfileId::default(), &old, true).unwrap();
        let key = ArtKey::new(&SystemId::new("n64"), Some("CZLE"), "");
        lib.set_art_key(&old, &key).unwrap();

        // Zelda moves into its own folder.
        let moved = Game {
            id: GameId::new("roms:n64/Zelda/Zelda.z64"),
            path: PathBuf::from("/games/n64/Zelda/Zelda.z64"),
            folder: Some(PathBuf::from("/games/n64/Zelda")),
            ..game("x", "Zelda")
        };
        let removed = lib
            .sync(&[moved.clone(), game("roms:n64/Mario.z64", "Mario")])
            .unwrap();
        assert!(removed.is_empty(), "nothing removed, so no art deleted");
        let entry = lib.get(&ProfileId::default(), &moved.id).unwrap().unwrap();
        assert_eq!(entry.playtime, Duration::from_secs(60));
        assert_eq!(entry.last_played, Some(9));
        assert!(entry.favorite);
        assert_eq!(entry.game.folder, moved.folder);
        assert!(lib.art_key_in_use(&key).unwrap());
        assert_eq!(lib.get(&ProfileId::default(), &old).unwrap(), None);

        // Two same-named files appearing for one that vanished: ambiguous, nothing moves.
        let copy = |id: &str| Game {
            id: GameId::new(id),
            path: PathBuf::from(format!("/games/n64/{id}/Mario.z64")),
            ..game("x", "Mario")
        };
        let removed = lib.sync(&[moved, copy("a"), copy("b")]).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].game.title, "Mario");
    }

    #[test]
    fn sync_record_and_order() {
        let dir = tempdir();
        let path = dir.path().join("library.sqlite3");
        let mut lib = Library::open(&path).unwrap();
        lib.sync(&[game("a", "Banjo"), game("b", "Zelda"), game("c", "Mario")])
            .unwrap();

        let t = UNIX_EPOCH + Duration::from_secs(1_000);
        lib.record_session(
            &ProfileId::default(),
            &GameId::new("b"),
            t,
            Duration::from_secs(90),
        )
        .unwrap();
        lib.record_session(
            &ProfileId::default(),
            &GameId::new("b"),
            t + Duration::from_secs(5),
            Duration::from_secs(30),
        )
        .unwrap();
        lib.record_session(
            &ProfileId::default(),
            &GameId::new("c"),
            t - Duration::from_secs(5),
            Duration::from_secs(1),
        )
        .unwrap();

        let titles: Vec<_> = lib
            .recent(&ProfileId::default())
            .unwrap()
            .into_iter()
            .map(|e| e.game.title)
            .collect();
        assert_eq!(titles, ["Zelda", "Mario", "Banjo"]);
        assert_eq!(
            lib.playtime(&ProfileId::default(), &GameId::new("b"))
                .unwrap(),
            Some(Duration::from_secs(120))
        );

        // Reopen: history persists; removed games disappear.
        drop(lib);
        let mut lib = Library::open(&path).unwrap();
        lib.sync(&[game("b", "Zelda (renamed)")]).unwrap();
        let entries = lib.recent(&ProfileId::default()).unwrap();
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
        let lib = Library::open_upgrading(&path).unwrap();
        let entries = lib.recent(&ProfileId::default()).unwrap();
        assert_eq!(entries[0].playtime, Duration::from_secs(60));
        assert!(!entries[0].favorite);
        lib.set_favorite(&ProfileId::default(), &GameId::new("a"), true)
            .unwrap();
        drop(lib);
        // Opening again doesn't migrate twice.
        let lib = Library::open(&path).unwrap();
        assert!(lib.recent(&ProfileId::default()).unwrap()[0].favorite);
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
    fn candidates_keep_their_order_and_go_with_the_art() {
        let dir = tempdir();
        let lib = Library::open(&dir.path().join("l.sqlite3")).unwrap();
        let key = ArtKey::from_stored("n64/CZLE");
        let found = |id, name: &str, query: &str| ArtCandidate {
            sgdb_id: id,
            name: name.into(),
            query: query.into(),
        };
        lib.set_art_candidates(
            &key,
            &[
                found(9, "Ocarina of Time", "THE LEGEND OF ZELDA"),
                found(3, "Zelda", "THE LEGEND OF ZELDA"),
                found(
                    9,
                    "Ocarina of Time",
                    "Legend of Zelda, The - Ocarina of Time",
                ),
                found(1, "Majora", "Legend of Zelda, The - Ocarina of Time"),
            ],
        )
        .unwrap();
        let ids: Vec<u64> = lib
            .art_candidates(&key)
            .unwrap()
            .iter()
            .map(|c| c.sgdb_id)
            .collect();
        assert_eq!(ids, [9, 3, 1]);
        lib.forget_art(&key).unwrap();
        assert!(lib.art_candidates(&key).unwrap().is_empty());
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

    #[test]
    fn opening_from_two_threads_at_once_migrates_once() {
        let dir = tempdir();
        let path = dir.path().join("l.sqlite3");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE games (id TEXT PRIMARY KEY, title TEXT NOT NULL,
                     system TEXT NOT NULL, path TEXT NOT NULL, last_played INTEGER,
                     playtime_secs INTEGER NOT NULL DEFAULT 0);",
            )
            .unwrap();
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || Library::open_upgrading(&path).map(|_| ()))
            })
            .collect();
        for t in threads {
            t.join().unwrap().unwrap();
        }
    }

    /// A version 3 database as the dev library had it before profiles.
    fn version_3_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE games (id TEXT PRIMARY KEY, title TEXT NOT NULL,
                 system TEXT NOT NULL, path TEXT NOT NULL, last_played INTEGER,
                 playtime_secs INTEGER NOT NULL DEFAULT 0,
                 favorite INTEGER NOT NULL DEFAULT 0, art_key TEXT, folder TEXT);
             CREATE TABLE art (art_key TEXT PRIMARY KEY, status TEXT NOT NULL, sgdb_id INTEGER,
                 sgdb_name TEXT, kinds TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL);
             CREATE TABLE art_queue (art_key TEXT PRIMARY KEY, attempts INTEGER NOT NULL,
                 next_try INTEGER NOT NULL, last_error TEXT NOT NULL);
             CREATE TABLE art_candidates (art_key TEXT NOT NULL, rank INTEGER NOT NULL,
                 sgdb_id INTEGER NOT NULL, name TEXT NOT NULL, query TEXT NOT NULL,
                 PRIMARY KEY (art_key, sgdb_id));
             INSERT INTO games VALUES ('oot', 'Ocarina of Time', 'n64', '/g/oot.z64', 1000,
                 3738, 1, 'n64/CZLE', NULL);
             INSERT INTO games VALUES ('tp', 'Twilight Princess', 'gc', '/g/tp.iso', 2000,
                 2725, 0, 'gc/GZ2E01', NULL);
             INSERT INTO games VALUES ('new', 'Never Played', 'n64', '/g/n.z64', NULL, 0, 0,
                 NULL, NULL);
             PRAGMA user_version = 3;",
        )
        .unwrap();
    }

    #[test]
    fn version_3_history_goes_to_the_default_profile() {
        let dir = tempdir();
        let path = dir.path().join("l.sqlite3");
        version_3_db(&path);
        let lib = Library::open_upgrading(&path).unwrap();
        let default = ProfileId::default();
        let entries = lib.recent(&default).unwrap();
        let summary: Vec<_> = entries
            .iter()
            .map(|e| {
                (
                    e.game.id.as_str(),
                    e.last_played,
                    e.playtime.as_secs(),
                    e.favorite,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("tp", Some(2000), 2725, false),
                ("oot", Some(1000), 3738, true),
                ("new", None, 0, false),
            ]
        );
        let version: i32 = lib
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 4);
        let columns: Vec<String> = lib
            .conn
            .prepare("SELECT name FROM pragma_table_info('games')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(!columns.iter().any(|c| c == "playtime_secs"), "{columns:?}");
        // Art keys stay shared.
        assert!(
            lib.art_key_in_use(&ArtKey::from_stored("gc/GZ2E01"))
                .unwrap()
        );
        // Another profile starts with nothing.
        let other = ProfileId::new("test2").unwrap();
        assert!(
            lib.recent(&other)
                .unwrap()
                .iter()
                .all(|e| e.last_played.is_none() && e.playtime.is_zero() && !e.favorite)
        );
        drop(lib);
        // Opening again changes nothing.
        let lib = Library::open(&path).unwrap();
        assert_eq!(lib.recent(&default).unwrap(), entries);
    }

    #[test]
    fn profiles_keep_their_own_history() {
        let dir = tempdir();
        let mut lib = Library::open(&dir.path().join("l.sqlite3")).unwrap();
        lib.sync(&[game("a", "A"), game("b", "B")]).unwrap();
        let (me, guest) = (ProfileId::default(), ProfileId::new("guest").unwrap());
        let t = UNIX_EPOCH + Duration::from_secs(100);
        lib.record_session(&me, &GameId::new("a"), t, Duration::from_secs(60))
            .unwrap();
        lib.record_session(&me, &GameId::new("a"), t, Duration::from_secs(30))
            .unwrap();
        lib.record_session(&guest, &GameId::new("b"), t, Duration::from_secs(5))
            .unwrap();
        lib.set_favorite(&guest, &GameId::new("a"), true).unwrap();
        // A game that isn't in the library records nothing.
        lib.record_session(&me, &GameId::new("gone"), t, Duration::from_secs(5))
            .unwrap();

        let a = |p| lib.get(p, &GameId::new("a")).unwrap().unwrap();
        assert_eq!((a(&me).playtime.as_secs(), a(&me).favorite), (90, false));
        assert_eq!(
            (a(&guest).playtime.as_secs(), a(&guest).favorite),
            (0, true)
        );
        assert_eq!(lib.recent(&guest).unwrap()[0].game.id.as_str(), "b");
        assert_eq!(
            lib.sessions(&me, &GameId::new("a")).unwrap(),
            [
                (100, Duration::from_secs(60)),
                (100, Duration::from_secs(30))
            ]
        );
        assert!(lib.sessions(&me, &GameId::new("gone")).unwrap().is_empty());

        // A moved game takes every profile's rows along; a removed one drops them.
        let moved = Game {
            id: GameId::new("a2"),
            path: PathBuf::from("/games/n64/A/A.z64"),
            ..game("a2", "A")
        };
        lib.sync(&[moved]).unwrap();
        let a2 = |p| lib.get(p, &GameId::new("a2")).unwrap().unwrap();
        assert_eq!(a2(&me).playtime.as_secs(), 90);
        assert!(a2(&guest).favorite);
        assert_eq!(lib.sessions(&me, &GameId::new("a2")).unwrap().len(), 2);
        let rows: i64 = lib
            .conn
            .query_row(
                "SELECT COUNT(*) FROM profile_games WHERE game_id = 'b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn a_library_from_before_profiles_is_only_upgraded_by_the_migration() {
        let dir = tempdir();
        let path = dir.path().join("l.sqlite3");
        version_3_db(&path);
        assert!(matches!(
            Library::open(&path),
            Err(LibraryError::NeedsProfileUpgrade)
        ));
        let conn = Connection::open(&path).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 3, "left untouched");
        drop(conn);
        Library::open_upgrading(&path).unwrap();
        assert!(Library::open(&path).is_ok());
        // A new, empty library needs no migration.
        assert!(Library::open(&dir.path().join("new.sqlite3")).is_ok());
    }
}
