//! Scanning `music/` and bringing the database in line with it.
//!
//! The music folder is only ever read, never written to.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::db::{Db, DbResult, TrackRecord};

use super::tags;

/// Extensions symphonia can decode (checked again by lofty on read).
pub const EXTENSIONS: &[&str] = &["mp3", "flac", "wav", "ogg", "m4a", "aac"];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub added: usize,
    pub updated: usize,
    pub missing: usize,
    pub skipped: usize,
}

/// Files that failed to parse, keyed by (path, size, mtime) so that an
/// unchanged broken file is reported once, not on every rescan.
pub type BadFiles = HashSet<(String, u64, i64)>;

struct Found {
    rel: String,
    abs: PathBuf,
    size: u64,
    mtime_ms: i64,
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<Found>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("cannot read {}: {e}", dir.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `DirEntry::file_type` does not follow symlinks: no link loops.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            walk(&path, root, out);
        } else if kind.is_file() {
            if !is_supported(&path) {
                tracing::debug!("skipping non-audio file {}", path.display());
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let Some(rel) = rel_string(rel) else {
                tracing::warn!("skipping file with non-UTF-8 name: {}", path.display());
                continue;
            };
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            out.push(Found {
                rel,
                abs: path,
                size: meta.len(),
                mtime_ms,
            });
        }
    }
}

/// Relative path with '/' separators on every platform.
fn rel_string(rel: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    parts.map(|p| p.join("/"))
}

/// Make the database reflect the current contents of `music`.
pub fn sync(db: &mut Db, music: &Path, bad: &mut BadFiles) -> DbResult<SyncStats> {
    let mut found = Vec::new();
    walk(music, music, &mut found);

    let mut stats = SyncStats::default();
    // Tag reading is slow, so it happens before the write transaction.
    let mut records = Vec::new();
    let mut unchanged_missing = Vec::new();
    for f in &found {
        let known = db.fingerprint(&f.rel)?;
        if let Some(fp) = known
            && fp.size == f.size
            && fp.mtime_ms == f.mtime_ms
        {
            if fp.missing {
                unchanged_missing.push(f.rel.clone());
            }
            continue;
        }
        let key = (f.rel.clone(), f.size, f.mtime_ms);
        if bad.contains(&key) {
            stats.skipped += 1;
            continue;
        }
        match tags::read_tags(&f.abs) {
            Ok(info) => {
                if known.is_some() {
                    stats.updated += 1;
                } else {
                    stats.added += 1;
                }
                records.push(TrackRecord {
                    path: f.rel.clone(),
                    size: f.size,
                    mtime_ms: f.mtime_ms,
                    title: info.title,
                    artist: info.artist,
                    album: info.album,
                    duration_ms: info.duration_ms,
                });
            }
            Err(e) => {
                tracing::warn!("skipping {}: {e}", f.abs.display());
                bad.insert(key);
                stats.skipped += 1;
            }
        }
    }

    let present: HashSet<&str> = found.iter().map(|f| f.rel.as_str()).collect();
    let stored = db.all_paths()?;
    let gone: Vec<&String> = stored
        .iter()
        .filter(|p| !present.contains(p.as_str()))
        .collect();
    stats.missing = gone.len();

    db.transaction(|db| {
        for r in &records {
            db.upsert_track(r)?;
        }
        for p in &unchanged_missing {
            db.mark_present(p)?;
        }
        for p in &gone {
            db.mark_missing(p)?;
        }
        Ok(())
    })?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::wav_bytes;

    #[test]
    fn extension_filter() {
        assert!(is_supported(Path::new("a/b.MP3")));
        assert!(is_supported(Path::new("b.flac")));
        assert!(!is_supported(Path::new("b.txt")));
        assert!(!is_supported(Path::new("noext")));
    }

    #[test]
    fn sync_tracks_added_changed_removed_and_restored_files() {
        let tmp = tempfile::tempdir().unwrap();
        let music = tmp.path().join("music");
        fs::create_dir_all(music.join("sub")).unwrap();
        let mut db = Db::open_in_memory().unwrap();
        let mut bad = BadFiles::new();

        fs::write(music.join("one.wav"), wav_bytes(1000)).unwrap();
        fs::write(music.join("sub/two.wav"), wav_bytes(500)).unwrap();
        fs::write(music.join("notes.txt"), "not audio").unwrap();
        fs::write(music.join("broken.mp3"), "not really an mp3").unwrap();

        let s = sync(&mut db, &music, &mut bad).unwrap();
        assert_eq!((s.added, s.updated, s.missing, s.skipped), (2, 0, 0, 1));
        let tracks = db.tracks().unwrap();
        assert_eq!(tracks.len(), 2);
        let one = tracks.iter().find(|t| t.path == "one.wav").unwrap();
        assert_eq!(one.title, "one");
        assert_eq!(one.artist, tags::UNKNOWN_ARTIST);
        assert_eq!(one.duration_ms, 1000);
        assert!(tracks.iter().any(|t| t.path == "sub/two.wav"));

        // Nothing changed: nothing to do, broken file is not re-reported.
        let s = sync(&mut db, &music, &mut bad).unwrap();
        assert_eq!(
            s,
            SyncStats {
                skipped: 1,
                ..SyncStats::default()
            }
        );

        // Changed file is re-read.
        fs::write(music.join("one.wav"), wav_bytes(2000)).unwrap();
        let s = sync(&mut db, &music, &mut bad).unwrap();
        assert_eq!((s.added, s.updated), (0, 1));
        let one = db
            .tracks()
            .unwrap()
            .into_iter()
            .find(|t| t.path == "one.wav");
        assert_eq!(one.map(|t| t.duration_ms), Some(2000));

        // Removed file is flagged, not deleted.
        fs::remove_file(music.join("sub/two.wav")).unwrap();
        let s = sync(&mut db, &music, &mut bad).unwrap();
        assert_eq!(s.missing, 1);
        let tracks = db.tracks().unwrap();
        assert_eq!(tracks.len(), 2);
        assert!(
            tracks
                .iter()
                .find(|t| t.path == "sub/two.wav")
                .unwrap()
                .missing
        );

        // Restored file comes back without a re-read.
        fs::write(music.join("sub/two.wav"), wav_bytes(500)).unwrap();
        sync(&mut db, &music, &mut bad).unwrap();
        assert!(db.tracks().unwrap().iter().all(|t| !t.missing));
    }

    #[test]
    fn missing_music_folder_marks_everything_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let music = tmp.path().join("music");
        fs::create_dir_all(&music).unwrap();
        let mut db = Db::open_in_memory().unwrap();
        let mut bad = BadFiles::new();
        fs::write(music.join("a.wav"), wav_bytes(100)).unwrap();
        sync(&mut db, &music, &mut bad).unwrap();
        fs::remove_dir_all(&music).unwrap();
        sync(&mut db, &music, &mut bad).unwrap();
        assert!(db.tracks().unwrap().iter().all(|t| t.missing));
    }
}
