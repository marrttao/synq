//! Playlists and favorites.
//!
//! The database is the source of truth; the folders under `playlists/` and
//! `favorites/` mirror it with hard links (or copies when linking fails).
//! Files in `music/` are only ever read.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use thiserror::Error;

use crate::db::{Db, DbError, LinkEntry};
use crate::paths::Paths;

const MAX_NAME_LEN: usize = 100;

#[derive(Debug, Error)]
pub enum PlaylistError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("file error: {0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    InvalidName(&'static str),
    #[error("a playlist named \"{0}\" already exists")]
    Duplicate(String),
    #[error("playlist no longer exists")]
    NoSuchPlaylist,
    #[error("track no longer exists")]
    NoSuchTrack,
    #[error("\"{0}\" is missing from music/, it cannot be linked")]
    SourceMissing(String),
}

type Result<T> = std::result::Result<T, PlaylistError>;

/// Where tracks are collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Playlist(i64),
    Favorites,
}

// ---- names ----------------------------------------------------------------

/// Trim and validate a playlist name; it becomes a folder name on every OS.
pub fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(PlaylistError::InvalidName("name cannot be empty"));
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(PlaylistError::InvalidName("name is too long"));
    }
    if name
        .chars()
        .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
    {
        return Err(PlaylistError::InvalidName(
            "name cannot contain \\ / : * ? \" < > |",
        ));
    }
    if name.ends_with('.') {
        return Err(PlaylistError::InvalidName("name cannot end with a dot"));
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ["COM", "LPT"].iter().any(|p| {
            stem.strip_prefix(p)
                .is_some_and(|n| n.len() == 1 && n != "0" && n.bytes().all(|b| b.is_ascii_digit()))
        });
    if reserved {
        return Err(PlaylistError::InvalidName(
            "this name is reserved by Windows",
        ));
    }
    Ok(name.to_owned())
}

/// "song.mp3" -> "song (2).mp3", "song (3).mp3", ... until it is not taken.
/// Comparison is case-insensitive because Windows folders are.
fn unique_name(dir: &Path, wanted: &str, taken: &HashSet<String>) -> String {
    let is_free =
        |n: &str| !taken.contains(&n.to_lowercase()) && fs::symlink_metadata(dir.join(n)).is_err();
    if is_free(wanted) {
        return wanted.to_owned();
    }
    let path = Path::new(wanted);
    let stem = path
        .file_stem()
        .map_or_else(|| wanted.to_owned(), |s| s.to_string_lossy().into_owned());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|n| is_free(n))
        .unwrap_or_else(|| wanted.to_owned())
}

// ---- links ----------------------------------------------------------------

/// Hard-link `src` to `dest`; fall back to a plain copy if linking fails.
/// Never overwrites an existing file.
fn link_file(src: &Path, dest: &Path) -> io::Result<()> {
    match fs::hard_link(src, dest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(e) => {
            tracing::info!(
                "hard link {} -> {} failed ({e}), copying instead",
                src.display(),
                dest.display()
            );
            let mut from = fs::File::open(src)?;
            let mut to = OpenOptions::new().write(true).create_new(true).open(dest)?;
            let copied = io::copy(&mut from, &mut to);
            if copied.is_err() {
                // Do not leave a truncated copy behind (it is our own file).
                drop(to);
                let _ = fs::remove_file(dest);
            }
            copied.map(|_| ())
        }
    }
}

fn file_name_of(rel_path: &str) -> String {
    Path::new(rel_path)
        .file_name()
        .map_or_else(|| rel_path.to_owned(), |n| n.to_string_lossy().into_owned())
}

// ---- manager --------------------------------------------------------------

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Links that were missing on disk and have been created again.
    pub recreated: usize,
    /// Entries whose source file is gone; nothing can be linked.
    pub unavailable: usize,
    /// Files in a collection folder that the database does not know. Kept.
    pub unexpected: Vec<PathBuf>,
}

pub struct Manager {
    db: Db,
    music: PathBuf,
    playlists_dir: PathBuf,
    favorites_dir: PathBuf,
}

impl Manager {
    pub fn new(db: Db, paths: &Paths) -> Self {
        Self {
            db,
            music: paths.music.clone(),
            playlists_dir: paths.playlists.clone(),
            favorites_dir: paths.favorites.clone(),
        }
    }

    fn playlist_dir(&self, name: &str) -> PathBuf {
        self.playlists_dir.join(name)
    }

    fn target_dir(&self, target: Target) -> Result<PathBuf> {
        match target {
            Target::Favorites => Ok(self.favorites_dir.clone()),
            Target::Playlist(id) => {
                let name = self
                    .db
                    .playlist_name(id)?
                    .ok_or(PlaylistError::NoSuchPlaylist)?;
                Ok(self.playlist_dir(&name))
            }
        }
    }

    fn entries(&self, target: Target) -> Result<Vec<LinkEntry>> {
        Ok(match target {
            Target::Favorites => self.db.favorite_entries()?,
            Target::Playlist(id) => self.db.playlist_entries(id)?,
        })
    }

    fn name_taken(&self, name: &str, except: Option<i64>) -> Result<bool> {
        let lower = name.to_lowercase();
        Ok(self
            .db
            .playlists()?
            .iter()
            .any(|(id, n)| Some(*id) != except && n.to_lowercase() == lower))
    }

    pub fn create_playlist(&self, name: &str) -> Result<i64> {
        let name = validate_name(name)?;
        if self.name_taken(&name, None)? {
            return Err(PlaylistError::Duplicate(name));
        }
        fs::create_dir_all(self.playlist_dir(&name))?;
        Ok(self.db.create_playlist(&name)?)
    }

    pub fn rename_playlist(&self, id: i64, new_name: &str) -> Result<()> {
        let new_name = validate_name(new_name)?;
        let old_name = self
            .db
            .playlist_name(id)?
            .ok_or(PlaylistError::NoSuchPlaylist)?;
        if self.name_taken(&new_name, Some(id))? {
            return Err(PlaylistError::Duplicate(new_name));
        }
        let (old_dir, new_dir) = (self.playlist_dir(&old_name), self.playlist_dir(&new_name));
        if old_name != new_name {
            if old_dir.is_dir() {
                // A pure case change ("a" -> "A") is a rename on the same
                // folder on Windows, which fs::rename handles.
                fs::rename(&old_dir, &new_dir)?;
            } else {
                fs::create_dir_all(&new_dir)?;
            }
        }
        self.db.rename_playlist(id, &new_name)?;
        Ok(())
    }

    /// Delete the playlist and the links it created. Other files someone put
    /// into the folder are left alone (and so is the folder then).
    pub fn delete_playlist(&self, id: i64) -> Result<()> {
        let dir = self.target_dir(Target::Playlist(id))?;
        let entries = self.db.playlist_entries(id)?;
        self.db.delete_playlist(id)?;
        for e in &entries {
            remove_link(&dir, &e.link_name);
        }
        if dir.is_dir()
            && let Err(e) = fs::remove_dir(&dir)
        {
            tracing::warn!("playlist folder {} not removed: {e}", dir.display());
        }
        Ok(())
    }

    /// Add a track and create its link. Returns false if it was already there.
    pub fn add(&self, target: Target, track: i64) -> Result<bool> {
        let (rel_path, missing) = self
            .db
            .track_file(track)?
            .ok_or(PlaylistError::NoSuchTrack)?;
        if missing {
            return Err(PlaylistError::SourceMissing(file_name_of(&rel_path)));
        }
        let dir = self.target_dir(target)?;
        let taken = self.taken_names(target)?;
        fs::create_dir_all(&dir)?;
        let name = unique_name(&dir, &file_name_of(&rel_path), &taken);
        let inserted = match target {
            Target::Favorites => self.db.favorite_add(track, &name)?,
            Target::Playlist(id) => self.db.playlist_add(id, track, &name)?,
        };
        if !inserted {
            return Ok(false);
        }
        if let Err(e) = link_file(&self.music.join(&rel_path), &dir.join(&name)) {
            // Keep database and folder consistent: undo the row.
            self.remove_row(target, track)?;
            return Err(e.into());
        }
        Ok(true)
    }

    /// Remove a track's membership and its link; the music file is untouched.
    pub fn remove(&self, target: Target, track: i64) -> Result<bool> {
        let dir = self.target_dir(target)?;
        let Some(name) = self.remove_row(target, track)? else {
            return Ok(false);
        };
        remove_link(&dir, &name);
        Ok(true)
    }

    fn remove_row(&self, target: Target, track: i64) -> Result<Option<String>> {
        Ok(match target {
            Target::Favorites => self.db.favorite_remove(track)?,
            Target::Playlist(id) => self.db.playlist_remove(id, track)?,
        })
    }

    fn taken_names(&self, target: Target) -> Result<HashSet<String>> {
        Ok(self
            .entries(target)?
            .into_iter()
            .map(|e| e.link_name.to_lowercase())
            .collect())
    }

    /// Make the folders match the database: create what is missing, report
    /// (but never delete) what is extra.
    pub fn sync_all(&self) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        fs::create_dir_all(&self.playlists_dir)?;
        fs::create_dir_all(&self.favorites_dir)?;
        let mut known_dirs = HashSet::new();
        for (id, name) in self.db.playlists()? {
            known_dirs.insert(name.to_lowercase());
            self.sync_target(Target::Playlist(id), &mut report)?;
        }
        self.sync_target(Target::Favorites, &mut report)?;

        for entry in fs::read_dir(&self.playlists_dir)?.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !known_dirs.contains(&name) {
                tracing::warn!(
                    "{} is not a known playlist, leaving it alone",
                    entry.path().display()
                );
                report.unexpected.push(entry.path());
            }
        }
        Ok(report)
    }

    fn sync_target(&self, target: Target, report: &mut SyncReport) -> Result<()> {
        let dir = self.target_dir(target)?;
        fs::create_dir_all(&dir)?;
        let entries = self.entries(target)?;
        let mut taken: HashSet<String> = entries
            .iter()
            .filter(|e| !e.link_name.is_empty())
            .map(|e| e.link_name.to_lowercase())
            .collect();

        for e in &entries {
            let present =
                !e.link_name.is_empty() && fs::symlink_metadata(dir.join(&e.link_name)).is_ok();
            if present {
                continue;
            }
            let src = self.music.join(&e.rel_path);
            if e.missing || !src.is_file() {
                tracing::warn!("{} is missing, cannot link it", src.display());
                report.unavailable += 1;
                continue;
            }
            // Rows from before link names existed get a fresh name.
            let name = if e.link_name.is_empty() {
                unique_name(&dir, &file_name_of(&e.rel_path), &taken)
            } else {
                e.link_name.clone()
            };
            match link_file(&src, &dir.join(&name)) {
                Ok(()) => {
                    taken.insert(name.to_lowercase());
                    if name != e.link_name {
                        match target {
                            Target::Favorites => {
                                self.db.set_favorite_link_name(e.track_id, &name)?
                            }
                            Target::Playlist(id) => {
                                self.db.set_playlist_link_name(id, e.track_id, &name)?;
                            }
                        }
                    }
                    report.recreated += 1;
                }
                Err(err) => tracing::warn!("cannot link {}: {err}", src.display()),
            }
        }

        for f in fs::read_dir(&dir)?.flatten() {
            if !taken.contains(&f.file_name().to_string_lossy().to_lowercase()) {
                tracing::warn!("unexpected file {}, leaving it alone", f.path().display());
                report.unexpected.push(f.path());
            }
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let mut playlists = Vec::new();
        for (id, name) in self.db.playlists()? {
            let tracks = self
                .db
                .playlist_entries(id)?
                .into_iter()
                .map(|e| e.track_id)
                .collect();
            playlists.push(PlaylistInfo { id, name, tracks });
        }
        let favorites = self
            .db
            .favorite_entries()?
            .into_iter()
            .map(|e| e.track_id)
            .collect();
        Ok(Snapshot {
            playlists,
            favorites,
        })
    }
}

/// Delete a link we created; a failure is only logged.
fn remove_link(dir: &Path, name: &str) {
    if name.is_empty() {
        return;
    }
    let path = dir.join(name);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("cannot remove {}: {e}", path.display()),
    }
}

// ---- background worker ----------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistInfo {
    pub id: i64,
    pub name: String,
    /// Library track ids in playlist order.
    pub tracks: Vec<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub playlists: Vec<PlaylistInfo>,
    pub favorites: Vec<i64>,
}

#[derive(Debug)]
pub enum Command {
    Create(String),
    Rename { id: i64, name: String },
    Delete(i64),
    Add { target: Target, track: i64 },
    Remove { target: Target, track: i64 },
}

#[derive(Debug)]
pub enum Event {
    Snapshot(Snapshot),
    /// Something worth telling the user (shown in the status line).
    Notice(String),
    Error(String),
}

pub struct Handle {
    tx: Sender<Command>,
    pub events: Receiver<Event>,
}

impl Handle {
    pub fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_err() {
            tracing::error!("playlist thread is gone");
        }
    }
}

/// Start the worker: it checks the folders against the database, sends the
/// first snapshot, then serves commands.
pub fn spawn(paths: &Paths) -> Handle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (ev_tx, ev_rx) = mpsc::channel();
    let paths = paths.clone();
    let spawned = thread::Builder::new().name("playlists".into()).spawn({
        let ev_tx = ev_tx.clone();
        move || {
            let db = match Db::open(&paths.db) {
                Ok(db) => db,
                Err(e) => {
                    tracing::error!("{e}");
                    let _ = ev_tx.send(Event::Error(format!("database: {e}")));
                    return;
                }
            };
            let manager = Manager::new(db, &paths);
            match manager.sync_all() {
                Ok(r) => tracing::info!(?r, "playlist folders checked"),
                Err(e) => {
                    tracing::error!("playlist sync: {e}");
                    let _ = ev_tx.send(Event::Error(format!("playlist folders: {e}")));
                }
            }
            send_snapshot(&manager, &ev_tx);
            while let Ok(cmd) = cmd_rx.recv() {
                match execute(&manager, cmd) {
                    Ok(Some(notice)) => {
                        let _ = ev_tx.send(Event::Notice(notice));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!("playlist command failed: {e}");
                        let _ = ev_tx.send(Event::Error(e.to_string()));
                    }
                }
                send_snapshot(&manager, &ev_tx);
            }
        }
    });
    if let Err(e) = spawned {
        tracing::error!("cannot start playlist thread: {e}");
        let _ = ev_tx.send(Event::Error(format!("playlist thread: {e}")));
    }
    Handle {
        tx: cmd_tx,
        events: ev_rx,
    }
}

fn send_snapshot(manager: &Manager, tx: &Sender<Event>) {
    match manager.snapshot() {
        Ok(s) => {
            let _ = tx.send(Event::Snapshot(s));
        }
        Err(e) => {
            tracing::error!("playlist snapshot: {e}");
            let _ = tx.send(Event::Error(e.to_string()));
        }
    }
}

/// Run one command; the returned text is shown to the user.
fn execute(m: &Manager, cmd: Command) -> Result<Option<String>> {
    Ok(match cmd {
        Command::Create(name) => {
            m.create_playlist(&name)?;
            Some(format!("Created playlist \"{}\"", name.trim()))
        }
        Command::Rename { id, name } => {
            m.rename_playlist(id, &name)?;
            Some(format!("Renamed to \"{}\"", name.trim()))
        }
        Command::Delete(id) => {
            m.delete_playlist(id)?;
            Some("Playlist deleted".to_owned())
        }
        Command::Add { target, track } => Some(if m.add(target, track)? {
            "Added".to_owned()
        } else {
            "Already there".to_owned()
        }),
        Command::Remove { target, track } => {
            m.remove(target, track)?;
            Some("Removed".to_owned())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TrackRecord;
    use crate::testutil::wav_bytes;

    struct Env {
        _tmp: tempfile::TempDir,
        paths: Paths,
        m: Manager,
    }

    /// Temp BASE with `files` as tracks (ids 1..) in music/, in-memory DB.
    fn env(files: &[&str]) -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        paths.ensure_dirs().unwrap();
        let db = Db::open_in_memory().unwrap();
        for f in files {
            let abs = paths.music.join(f);
            fs::create_dir_all(abs.parent().unwrap()).unwrap();
            fs::write(&abs, wav_bytes(100)).unwrap();
            db.upsert_track(&TrackRecord {
                path: (*f).to_owned(),
                size: 1,
                mtime_ms: 1,
                title: (*f).to_owned(),
                artist: "A".into(),
                album: String::new(),
                duration_ms: 100,
            })
            .unwrap();
        }
        let m = Manager::new(db, &paths);
        Env {
            _tmp: tmp,
            paths,
            m,
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn name_validation() {
        assert_eq!(validate_name("  Road trip ").unwrap(), "Road trip");
        for bad in [
            "", "   ", "a/b", "a\\b", "a:b", "what?", "dot.", "con", "NUL.txt", "com1", "x\ty",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(validate_name("com0").is_ok());
        assert!(validate_name(&"x".repeat(101)).is_err());
    }

    #[test]
    fn create_makes_folder_and_rejects_duplicates_case_insensitively() {
        let e = env(&[]);
        e.m.create_playlist("Rock").unwrap();
        assert!(e.paths.playlists.join("Rock").is_dir());
        assert!(matches!(
            e.m.create_playlist("rock"),
            Err(PlaylistError::Duplicate(_))
        ));
        assert!(matches!(
            e.m.create_playlist("a/b"),
            Err(PlaylistError::InvalidName(_))
        ));
    }

    #[test]
    fn adding_creates_a_hard_link_that_shares_the_file() {
        let e = env(&["one.wav"]);
        let pl = e.m.create_playlist("P").unwrap();
        assert!(e.m.add(Target::Playlist(pl), 1).unwrap());
        let link = e.paths.playlists.join("P").join("one.wav");
        assert!(link.is_file());
        assert_eq!(
            fs::read(&link).unwrap(),
            fs::read(e.paths.music.join("one.wav")).unwrap()
        );
        // Adding twice is a no-op.
        assert!(!e.m.add(Target::Playlist(pl), 1).unwrap());
        assert_eq!(names(&e.paths.playlists.join("P")), ["one.wav"]);
    }

    #[test]
    fn link_is_a_real_hard_link_not_a_copy() {
        // Needs a filesystem with hard links (NTFS, ext4, tmpfs, ...).
        let e = env(&["one.wav"]);
        let pl = e.m.create_playlist("P").unwrap();
        e.m.add(Target::Playlist(pl), 1).unwrap();
        let link = e.paths.playlists.join("P").join("one.wav");
        // Writing through the original shows up in the link: same file.
        fs::write(e.paths.music.join("one.wav"), b"changed").unwrap();
        assert_eq!(fs::read(&link).unwrap(), b"changed");
    }

    #[test]
    fn link_file_never_overwrites_and_leaves_nothing_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src.wav");
        fs::write(&src, b"data").unwrap();
        // Linking into a folder that does not exist fails; so does the copy,
        // and nothing is left behind.
        let dest = tmp.path().join("no-such-dir").join("x.wav");
        assert!(link_file(&src, &dest).is_err());
        assert!(!dest.exists());
        // An existing destination is never overwritten.
        let existing = tmp.path().join("exists.wav");
        fs::write(&existing, b"keep").unwrap();
        assert!(link_file(&src, &existing).is_err());
        assert_eq!(fs::read(&existing).unwrap(), b"keep");
    }

    #[test]
    fn same_file_names_get_a_numeric_suffix() {
        let e = env(&["a/song.wav", "b/song.wav", "c/song.wav"]);
        let pl = e.m.create_playlist("P").unwrap();
        for id in 1..=3 {
            e.m.add(Target::Playlist(pl), id).unwrap();
        }
        assert_eq!(
            names(&e.paths.playlists.join("P")),
            ["song (2).wav", "song (3).wav", "song.wav"]
        );
    }

    #[test]
    fn removing_deletes_only_the_link() {
        let e = env(&["one.wav", "two.wav"]);
        let pl = e.m.create_playlist("P").unwrap();
        e.m.add(Target::Playlist(pl), 1).unwrap();
        e.m.add(Target::Playlist(pl), 2).unwrap();
        assert!(e.m.remove(Target::Playlist(pl), 1).unwrap());
        assert_eq!(names(&e.paths.playlists.join("P")), ["two.wav"]);
        assert!(e.paths.music.join("one.wav").is_file());
        assert!(!e.m.remove(Target::Playlist(pl), 1).unwrap());
    }

    #[test]
    fn favorites_use_their_own_folder() {
        let e = env(&["one.wav"]);
        assert!(e.m.add(Target::Favorites, 1).unwrap());
        assert_eq!(names(&e.paths.favorites), ["one.wav"]);
        assert!(e.m.remove(Target::Favorites, 1).unwrap());
        assert!(names(&e.paths.favorites).is_empty());
        assert!(e.paths.music.join("one.wav").is_file());
    }

    #[test]
    fn rename_moves_the_folder() {
        let e = env(&["one.wav"]);
        let pl = e.m.create_playlist("Old").unwrap();
        e.m.add(Target::Playlist(pl), 1).unwrap();
        e.m.rename_playlist(pl, "New").unwrap();
        assert!(!e.paths.playlists.join("Old").exists());
        assert_eq!(names(&e.paths.playlists.join("New")), ["one.wav"]);
        // New links go to the new folder.
        e.m.remove(Target::Playlist(pl), 1).unwrap();
        e.m.add(Target::Playlist(pl), 1).unwrap();
        assert_eq!(names(&e.paths.playlists.join("New")), ["one.wav"]);
    }

    #[test]
    fn rename_to_existing_name_is_rejected() {
        let e = env(&[]);
        e.m.create_playlist("A").unwrap();
        let b = e.m.create_playlist("B").unwrap();
        assert!(matches!(
            e.m.rename_playlist(b, "a"),
            Err(PlaylistError::Duplicate(_))
        ));
        // Changing only the case of its own name is fine.
        e.m.rename_playlist(b, "b").unwrap();
    }

    #[test]
    fn delete_removes_links_and_folder_but_not_music() {
        let e = env(&["one.wav"]);
        let pl = e.m.create_playlist("P").unwrap();
        e.m.add(Target::Playlist(pl), 1).unwrap();
        e.m.delete_playlist(pl).unwrap();
        assert!(!e.paths.playlists.join("P").exists());
        assert!(e.paths.music.join("one.wav").is_file());
        assert!(e.m.snapshot().unwrap().playlists.is_empty());
    }

    #[test]
    fn delete_keeps_foreign_files_in_the_folder() {
        let e = env(&["one.wav"]);
        let pl = e.m.create_playlist("P").unwrap();
        e.m.add(Target::Playlist(pl), 1).unwrap();
        fs::write(e.paths.playlists.join("P").join("mine.txt"), "x").unwrap();
        e.m.delete_playlist(pl).unwrap();
        assert_eq!(names(&e.paths.playlists.join("P")), ["mine.txt"]);
    }

    #[test]
    fn sync_recreates_missing_links_and_folders() {
        let e = env(&["one.wav", "two.wav"]);
        let pl = e.m.create_playlist("P").unwrap();
        e.m.add(Target::Playlist(pl), 1).unwrap();
        e.m.add(Target::Favorites, 2).unwrap();
        fs::remove_dir_all(e.paths.playlists.join("P")).unwrap();
        fs::remove_file(e.paths.favorites.join("two.wav")).unwrap();

        let r = e.m.sync_all().unwrap();
        assert_eq!(r.recreated, 2);
        assert_eq!(names(&e.paths.playlists.join("P")), ["one.wav"]);
        assert_eq!(names(&e.paths.favorites), ["two.wav"]);
        // Second run has nothing to do.
        assert_eq!(e.m.sync_all().unwrap(), SyncReport::default());
    }

    #[test]
    fn sync_reports_but_keeps_unknown_files_and_folders() {
        let e = env(&["one.wav"]);
        let pl = e.m.create_playlist("P").unwrap();
        e.m.add(Target::Playlist(pl), 1).unwrap();
        fs::write(e.paths.playlists.join("P").join("stray.txt"), "x").unwrap();
        fs::create_dir(e.paths.playlists.join("Unknown")).unwrap();

        let r = e.m.sync_all().unwrap();
        assert_eq!(r.unexpected.len(), 2);
        assert!(e.paths.playlists.join("P").join("stray.txt").is_file());
        assert!(e.paths.playlists.join("Unknown").is_dir());
    }

    #[test]
    fn missing_sources_keep_their_rows_and_are_not_linked() {
        let e = env(&["one.wav"]);
        let pl = e.m.create_playlist("P").unwrap();
        e.m.add(Target::Playlist(pl), 1).unwrap();
        // The library notices the file is gone.
        e.m.db.mark_missing("one.wav").unwrap();
        fs::remove_file(e.paths.music.join("one.wav")).unwrap();
        fs::remove_file(e.paths.playlists.join("P").join("one.wav")).unwrap();

        let r = e.m.sync_all().unwrap();
        assert_eq!((r.recreated, r.unavailable), (0, 1));
        assert_eq!(e.m.snapshot().unwrap().playlists[0].tracks, [1]);
        // Adding a missing track is refused.
        assert!(matches!(
            e.m.add(Target::Favorites, 1),
            Err(PlaylistError::SourceMissing(_))
        ));
    }

    #[test]
    fn snapshot_lists_tracks_in_order() {
        let e = env(&["a.wav", "b.wav", "c.wav"]);
        let pl = e.m.create_playlist("P").unwrap();
        for id in [3, 1, 2] {
            e.m.add(Target::Playlist(pl), id).unwrap();
        }
        e.m.add(Target::Favorites, 2).unwrap();
        let s = e.m.snapshot().unwrap();
        assert_eq!(s.playlists[0].tracks, [3, 1, 2]);
        assert_eq!(s.favorites, [2]);
    }

    #[test]
    fn unknown_playlist_or_track_is_an_error_not_a_panic() {
        let e = env(&["a.wav"]);
        assert!(matches!(
            e.m.add(Target::Playlist(99), 1),
            Err(PlaylistError::NoSuchPlaylist)
        ));
        assert!(matches!(
            e.m.add(Target::Favorites, 99),
            Err(PlaylistError::NoSuchTrack)
        ));
        assert!(e.m.delete_playlist(99).is_err());
    }
}
