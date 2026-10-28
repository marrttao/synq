//! Music library: background scanner and folder watcher.
//!
//! A worker thread owns its own database connection, keeps `tracks` in sync
//! with `music/` and sends full snapshots to the UI over a channel.

mod scan;
mod tags;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};

use crate::db::{Db, Track};

/// File events are batched for this long before a rescan.
const DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub enum LibraryEvent {
    /// A scan started.
    Scanning,
    /// The full, sorted track list after a scan.
    Tracks(Vec<Track>),
    /// Something went wrong; shown in the status line.
    Error(String),
}

/// Start the worker. The receiver gets events until the UI drops it.
pub fn spawn(music: PathBuf, db_path: PathBuf) -> Receiver<LibraryEvent> {
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new().name("library".into()).spawn({
        let tx = tx.clone();
        move || run(&music, &db_path, &tx)
    });
    if let Err(e) = spawned {
        tracing::error!("cannot start library thread: {e}");
        // A send error only means the UI is already gone.
        let _ = tx.send(LibraryEvent::Error(format!("library thread: {e}")));
    }
    rx
}

fn run(music: &Path, db_path: &Path, tx: &Sender<LibraryEvent>) {
    let mut db = match Db::open(db_path) {
        Ok(db) => db,
        Err(e) => {
            tracing::error!("{e}");
            let _ = tx.send(LibraryEvent::Error(format!("database: {e}")));
            return;
        }
    };
    let mut bad = scan::BadFiles::new();

    // Watch before the first scan so nothing slips through in between.
    let (fs_tx, fs_rx) = mpsc::channel();
    let _watcher = start_watcher(music, fs_tx, tx);

    loop {
        if tx.send(LibraryEvent::Scanning).is_err() {
            return;
        }
        match scan::sync(&mut db, music, &mut bad) {
            Ok(stats) => tracing::info!(?stats, "library scan finished"),
            Err(e) => {
                tracing::error!("library scan: {e}");
                let _ = tx.send(LibraryEvent::Error(format!("scan: {e}")));
            }
        }
        match db.tracks() {
            Ok(tracks) => {
                if tx.send(LibraryEvent::Tracks(tracks)).is_err() {
                    return;
                }
            }
            Err(e) => {
                tracing::error!("library query: {e}");
                let _ = tx.send(LibraryEvent::Error(format!("database: {e}")));
            }
        }

        // Block until something changes, then let the burst settle.
        if fs_rx.recv().is_err() {
            // No watcher: the initial scan is all we get.
            return;
        }
        loop {
            match fs_rx.recv_timeout(DEBOUNCE) {
                Ok(()) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

fn start_watcher(
    music: &Path,
    fs_tx: Sender<()>,
    ui_tx: &Sender<LibraryEvent>,
) -> Option<notify::RecommendedWatcher> {
    let handler = move |res: notify::Result<notify::Event>| match res {
        // Reads of a file are not changes.
        Ok(ev) if matches!(ev.kind, EventKind::Access(_)) => {}
        Ok(_) => {
            let _ = fs_tx.send(());
        }
        Err(e) => tracing::warn!("watcher: {e}"),
    };
    let result = notify::recommended_watcher(handler).and_then(|mut w| {
        w.watch(music, RecursiveMode::Recursive)?;
        Ok(w)
    });
    match result {
        Ok(w) => Some(w),
        Err(e) => {
            tracing::error!("cannot watch {}: {e}", music.display());
            let _ = ui_tx.send(LibraryEvent::Error(format!(
                "folder watching unavailable: {e}"
            )));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Instant;

    /// Wait for a `Tracks` snapshot satisfying `pred`.
    fn wait_for(rx: &Receiver<LibraryEvent>, pred: impl Fn(&[Track]) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(LibraryEvent::Tracks(t)) = rx.recv_timeout(Duration::from_millis(200))
                && pred(&t)
            {
                return true;
            }
        }
        false
    }

    #[test]
    fn watcher_picks_up_new_and_removed_files() {
        let tmp = tempfile::tempdir().unwrap();
        let music = tmp.path().join("music");
        fs::create_dir_all(&music).unwrap();
        let rx = spawn(music.clone(), tmp.path().join("library.db"));
        assert!(wait_for(&rx, |t| t.is_empty()), "initial scan");

        let file = music.join("live.wav");
        fs::write(&file, crate::testutil::wav_bytes(300)).unwrap();
        assert!(
            wait_for(&rx, |t| t.len() == 1 && !t[0].missing),
            "new file not detected"
        );

        fs::remove_file(&file).unwrap();
        assert!(
            wait_for(&rx, |t| t.len() == 1 && t[0].missing),
            "removed file not flagged"
        );
    }
}
