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
const MIGRATIONS: &[&str] = &[
    r#"
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
"#,
    // 2: name of the hard link (or copy) inside the playlist / favorites folder.
    r#"
ALTER TABLE playlist_tracks ADD COLUMN link_name TEXT NOT NULL DEFAULT '';
ALTER TABLE favorites ADD COLUMN link_name TEXT NOT NULL DEFAULT '';
"#,
];

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

/// A track's membership in a playlist or in favorites.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkEntry {
    pub track_id: i64,
    /// File name of the hard link inside the collection's folder ("" if unset).
    pub link_name: String,
    /// Source file, relative to music/.
    pub rel_path: String,
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

    /// Library path (relative to music/) and missing flag of a track.
    pub fn track_file(&self, id: i64) -> DbResult<Option<(String, bool)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT path, missing FROM tracks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
            )
            .optional()?)
    }

    // ---- settings -------------------------------------------------------

    pub fn setting(&self, key: &str) -> DbResult<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> DbResult<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- playlists ------------------------------------------------------

    pub fn create_playlist(&self, name: &str) -> DbResult<i64> {
        self.conn
            .execute("INSERT INTO playlists (name) VALUES (?1)", [name])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_playlist(&self, id: i64, name: &str) -> DbResult<()> {
        self.conn.execute(
            "UPDATE playlists SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        Ok(())
    }

    /// Removes the playlist and (by cascade) its track links.
    pub fn delete_playlist(&self, id: i64) -> DbResult<()> {
        self.conn
            .execute("DELETE FROM playlists WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn playlists(&self) -> DbResult<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM playlists ORDER BY name COLLATE NOCASE, id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn playlist_name(&self, id: i64) -> DbResult<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT name FROM playlists WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Entries of a playlist in play order.
    pub fn playlist_entries(&self, playlist: i64) -> DbResult<Vec<LinkEntry>> {
        self.link_entries(
            "SELECT pt.track_id, pt.link_name, t.path, t.missing
             FROM playlist_tracks pt JOIN tracks t ON t.id = pt.track_id
             WHERE pt.playlist_id = ?1 ORDER BY pt.position, pt.track_id",
            params![playlist],
        )
    }

    /// Returns false when the track was already in the playlist.
    pub fn playlist_add(&self, playlist: i64, track: i64, link_name: &str) -> DbResult<bool> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO playlist_tracks (playlist_id, track_id, position, link_name)
             VALUES (?1, ?2, COALESCE((SELECT MAX(position) + 1 FROM playlist_tracks
                                       WHERE playlist_id = ?1), 0), ?3)",
            params![playlist, track, link_name],
        )?;
        Ok(n > 0)
    }

    /// Returns the link name of the removed entry, if there was one.
    pub fn playlist_remove(&self, playlist: i64, track: i64) -> DbResult<Option<String>> {
        let name = self
            .conn
            .query_row(
                "SELECT link_name FROM playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
                params![playlist, track],
                |r| r.get(0),
            )
            .optional()?;
        self.conn.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
            params![playlist, track],
        )?;
        Ok(name)
    }

    pub fn set_playlist_link_name(&self, playlist: i64, track: i64, name: &str) -> DbResult<()> {
        self.conn.execute(
            "UPDATE playlist_tracks SET link_name = ?3 WHERE playlist_id = ?1 AND track_id = ?2",
            params![playlist, track, name],
        )?;
        Ok(())
    }

    // ---- favorites ------------------------------------------------------

    pub fn favorite_entries(&self) -> DbResult<Vec<LinkEntry>> {
        self.link_entries(
            "SELECT f.track_id, f.link_name, t.path, t.missing
             FROM favorites f JOIN tracks t ON t.id = f.track_id
             ORDER BY f.added_at, f.track_id",
            [],
        )
    }

    /// Returns false when the track was already a favorite.
    pub fn favorite_add(&self, track: i64, link_name: &str) -> DbResult<bool> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO favorites (track_id, added_at, link_name) VALUES (?1, ?2, ?3)",
            params![track, now_ms, link_name],
        )?;
        Ok(n > 0)
    }

    pub fn favorite_remove(&self, track: i64) -> DbResult<Option<String>> {
        let name = self
            .conn
            .query_row(
                "SELECT link_name FROM favorites WHERE track_id = ?1",
                [track],
                |r| r.get(0),
            )
            .optional()?;
        self.conn
            .execute("DELETE FROM favorites WHERE track_id = ?1", [track])?;
        Ok(name)
    }

    pub fn set_favorite_link_name(&self, track: i64, name: &str) -> DbResult<()> {
        self.conn.execute(
            "UPDATE favorites SET link_name = ?2 WHERE track_id = ?1",
            params![track, name],
        )?;
        Ok(())
    }

    fn link_entries(&self, sql: &str, p: impl rusqlite::Params) -> DbResult<Vec<LinkEntry>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(p, |r| {
            Ok(LinkEntry {
                track_id: r.get(0)?,
                link_name: r.get(1)?,
                rel_path: r.get(2)?,
                missing: r.get::<_, i64>(3)? != 0,
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
