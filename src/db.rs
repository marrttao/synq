//! SQLite schema, migrations and track queries.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema is newer than this build (version {0})")]
    TooNew(i64),
}

pub type DbResult<T> = Result<T, DbError>;

/// Each entry upgrades the schema by one version. Never edit old entries.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE tracks (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,   -- relative to music/, '/' separated
    size        INTEGER NOT NULL,
    mtime_ms    INTEGER NOT NULL,
    title       TEXT NOT NULL,
    artist      TEXT NOT NULL,
    album       TEXT NOT NULL DEFAULT '',
    duration_ms INTEGER NOT NULL DEFAULT 0,
    missing     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_tracks_sort ON tracks (artist COLLATE NOCASE, album COLLATE NOCASE, title COLLATE NOCASE);

CREATE TABLE playlists (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, track_id)
);

CREATE TABLE favorites (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    added_at INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#];

/// A track as stored in the database.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: i64,
    /// Path relative to the music folder, '/' separated.
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub missing: bool,
}

/// Fields written when a file is new or changed.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRecord {
    pub path: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
}

/// What the scanner needs to decide whether a file must be re-read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    pub size: u64,
    pub mtime_ms: i64,
    pub missing: bool,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Self> {
        Self::init(Connection::open(path)?)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> DbResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> DbResult<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        // WAL lets other connections (UI) read while the scanner writes.
        // In-memory databases ignore it, which is fine.
        let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> DbResult<()> {
        let current: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let latest = MIGRATIONS.len() as i64;
        if current > latest {
            return Err(DbError::TooNew(current));
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i as i64 + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn fingerprint(&self, path: &str) -> DbResult<Option<Fingerprint>> {
        Ok(self
            .conn
            .query_row(
                "SELECT size, mtime_ms, missing FROM tracks WHERE path = ?1",
                [path],
                |r| {
                    Ok(Fingerprint {
                        size: r.get::<_, i64>(0)? as u64,
                        mtime_ms: r.get(1)?,
                        missing: r.get::<_, i64>(2)? != 0,
                    })
                },
            )
            .optional()?)
    }

    /// Insert a new track or update a changed one; clears the missing flag.
    pub fn upsert_track(&self, t: &TrackRecord) -> DbResult<()> {
        self.conn.execute(
            "INSERT INTO tracks (path, size, mtime_ms, title, artist, album, duration_ms, missing)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
             ON CONFLICT(path) DO UPDATE SET
                size = excluded.size, mtime_ms = excluded.mtime_ms,
                title = excluded.title, artist = excluded.artist,
                album = excluded.album, duration_ms = excluded.duration_ms,
                missing = 0",
            params![
                t.path,
                t.size as i64,
                t.mtime_ms,
                t.title,
                t.artist,
                t.album,
                t.duration_ms as i64
            ],
        )?;
        Ok(())
    }

    /// Clear the missing flag of a file that is present again unchanged.
    pub fn mark_present(&self, path: &str) -> DbResult<()> {
        self.conn
            .execute("UPDATE tracks SET missing = 0 WHERE path = ?1", [path])?;
        Ok(())
    }

    /// Every path currently stored (missing or not).
    pub fn all_paths(&self) -> DbResult<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT path FROM tracks")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Flag a track whose file vanished. The row (and playlist links) stay.
    pub fn mark_missing(&self, path: &str) -> DbResult<()> {
        self.conn
            .execute("UPDATE tracks SET missing = 1 WHERE path = ?1", [path])?;
        Ok(())
    }

    pub fn tracks(&self) -> DbResult<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, title, artist, album, duration_ms, missing FROM tracks
             ORDER BY artist COLLATE NOCASE, album COLLATE NOCASE, title COLLATE NOCASE, path",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Track {
                id: r.get(0)?,
                path: r.get(1)?,
                title: r.get(2)?,
                artist: r.get(3)?,
                album: r.get(4)?,
                duration_ms: r.get::<_, i64>(5)? as u64,
                missing: r.get::<_, i64>(6)? != 0,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Run several writes atomically (one fsync for a whole scan).
    pub fn transaction<T>(&mut self, f: impl FnOnce(&Db) -> DbResult<T>) -> DbResult<T> {
        self.conn.execute_batch("BEGIN")?;
        match f(self) {
            Ok(v) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                // Rollback failure would only repeat the original problem.
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn record(path: &str, title: &str) -> TrackRecord {
        TrackRecord {
            path: path.into(),
            size: 10,
            mtime_ms: 1000,
            title: title.into(),
            artist: "A".into(),
            album: String::new(),
            duration_ms: 5000,
        }
    }

    #[test]
    fn migrations_create_schema_and_are_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("library.db");
        drop(Db::open(&path).unwrap());
        let db = Db::open(&path).unwrap();
        let version: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        for table in [
            "tracks",
            "playlists",
            "playlist_tracks",
            "favorites",
            "settings",
        ] {
            let n: i64 = db
                .conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "table {table} missing");
        }
    }

    #[test]
    fn newer_schema_is_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 999).unwrap();
        assert!(matches!(Db::init(conn), Err(DbError::TooNew(999))));
    }

    #[test]
    fn upsert_updates_and_clears_missing() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_track(&record("a.mp3", "One")).unwrap();
        db.mark_missing("a.mp3").unwrap();
        assert!(db.tracks().unwrap()[0].missing);

        db.upsert_track(&record("a.mp3", "Renamed")).unwrap();
        let tracks = db.tracks().unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Renamed");
        assert!(!tracks[0].missing);
    }

    #[test]
    fn tracks_sorted_case_insensitively() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_track(&record("b.mp3", "beta")).unwrap();
        db.upsert_track(&record("a.mp3", "Alpha")).unwrap();
        db.upsert_track(&record("c.mp3", "gamma")).unwrap();
        let titles: Vec<_> = db.tracks().unwrap().into_iter().map(|t| t.title).collect();
        assert_eq!(titles, ["Alpha", "beta", "gamma"]);
    }

    #[test]
    fn failed_transaction_rolls_back() {
        let mut db = Db::open_in_memory().unwrap();
        let res: DbResult<()> = db.transaction(|db| {
            db.upsert_track(&record("a.mp3", "One"))?;
            Err(DbError::TooNew(0))
        });
        assert!(res.is_err());
        assert!(db.tracks().unwrap().is_empty());
    }
}
