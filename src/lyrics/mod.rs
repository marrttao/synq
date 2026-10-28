//! Lyrics: finding, caching and serving them to the UI.
//!
//! Sources are tried in this order:
//! 1. `<track>.lrc` next to the audio file
//! 2. lyrics embedded in the tags
//! 3. the cache in `data/lyrics/` (earlier LRCLIB answers)
//! 4. LRCLIB (if enabled in the config)
//!
//! A background thread does the work so that the UI never waits for disk or
//! network.

mod cache;
pub mod lrc;
mod lrclib;
mod sources;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use cache::{Cache, Cached};
pub use lrc::Lyrics;

use crate::db::Db;

/// Everything the lookup needs to know about a track.
#[derive(Debug, Clone)]
pub struct Request {
    pub track_id: i64,
    /// Path relative to music/, the key for the cache and the saved offset.
    pub rel_path: String,
    pub audio_path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: u64,
    /// Ignore cached answers (including "not found") and ask again.
    pub retry: bool,
}

#[derive(Debug)]
pub enum Command {
    Load(Request),
    /// Remember the timing offset (ms, positive = lyrics later) for a track.
    SetOffset {
        rel_path: String,
        ms: i64,
    },
}

/// Where the lyrics came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    LocalFile,
    Embedded,
    Cache,
    Online,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::LocalFile => ".lrc file",
            Source::Embedded => "tags",
            Source::Cache => "cache",
            Source::Online => "LRCLIB",
        }
    }
}

#[derive(Debug)]
pub enum Event {
    /// Waiting for the network.
    Searching,
    Found {
        track_id: i64,
        lyrics: Lyrics,
        source: Source,
        offset_ms: i64,
    },
    NotFound {
        track_id: i64,
        /// Set when the lookup failed (e.g. offline) instead of finding nothing.
        problem: Option<String>,
    },
}

pub struct Handle {
    tx: Sender<Command>,
    pub events: Receiver<Event>,
}

impl Handle {
    pub fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_err() {
            tracing::error!("lyrics thread is gone");
        }
    }
}

/// Start the lyrics worker.
pub fn spawn(db_path: PathBuf, cache_dir: PathBuf, online: bool) -> Handle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (ev_tx, ev_rx) = mpsc::channel();
    let spawned = thread::Builder::new().name("lyrics".into()).spawn(move || {
        let db = match Db::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                tracing::error!("lyrics: {e}");
                return;
            }
        };
        let resolver = Resolver::new(cache_dir, online);
        while let Ok(first) = cmd_rx.recv() {
            // Skipping through tracks quickly queues up requests; only the
            // newest one is worth answering.
            let mut load = None;
            for cmd in std::iter::once(first).chain(cmd_rx.try_iter()) {
                match cmd {
                    Command::Load(req) => load = Some(req),
                    Command::SetOffset { rel_path, ms } => save_offset(&db, &rel_path, ms),
                }
            }
            if let Some(req) = load {
                let offset_ms = load_offset(&db, &req.rel_path);
                let event = match resolver.resolve(&req, || {
                    let _ = ev_tx.send(Event::Searching);
                }) {
                    Resolved::Found(lyrics, source) => Event::Found {
                        track_id: req.track_id,
                        lyrics,
                        source,
                        offset_ms,
                    },
                    Resolved::NotFound(problem) => Event::NotFound {
                        track_id: req.track_id,
                        problem,
                    },
                };
                if ev_tx.send(event).is_err() {
                    return; // UI is gone
                }
            }
        }
    });
    if let Err(e) = spawned {
        tracing::error!("cannot start lyrics thread: {e}");
    }
    Handle {
        tx: cmd_tx,
        events: ev_rx,
    }
}

fn offset_key(rel_path: &str) -> String {
    format!("lyrics_offset_ms:{rel_path}")
}

fn load_offset(db: &Db, rel_path: &str) -> i64 {
    db.setting(&offset_key(rel_path))
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn save_offset(db: &Db, rel_path: &str, ms: i64) {
    if let Err(e) = db.set_setting(&offset_key(rel_path), &ms.to_string()) {
        tracing::warn!("cannot save lyrics offset: {e}");
    }
}

#[derive(Debug, PartialEq)]
pub enum Resolved {
    Found(Lyrics, Source),
    /// Nothing found; `Some` explains a failed lookup (not cached).
    NotFound(Option<String>),
}

pub struct Resolver {
    cache: Cache,
    online: bool,
    base_url: String,
}

impl Resolver {
    pub fn new(cache_dir: PathBuf, online: bool) -> Self {
        Self {
            cache: Cache::new(cache_dir),
            online,
            base_url: lrclib::BASE_URL.to_owned(),
        }
    }

    /// Walk the source chain. `on_search` is called right before a network
    /// request so the UI can show "searching".
    pub fn resolve(&self, req: &Request, on_search: impl FnOnce()) -> Resolved {
        if let Some(found) = self.local(&req.audio_path) {
            return found;
        }
        if req.retry {
            self.cache.forget(&req.rel_path);
        } else {
            match self.cache.read(&req.rel_path) {
                Some(Cached::Synced(text) | Cached::Plain(text)) => {
                    if let Some(l) = lrc::parse(&text) {
                        return Resolved::Found(l, Source::Cache);
                    }
                }
                Some(Cached::NotFound) => return Resolved::NotFound(None),
                None => {}
            }
        }
        if !self.online {
            return Resolved::NotFound(None);
        }
        // "Unknown Artist" would only produce wrong matches.
        if req.artist == crate::library::UNKNOWN_ARTIST {
            return Resolved::NotFound(None);
        }
        on_search();
        self.fetch_online(req)
    }

    fn local(&self, audio: &Path) -> Option<Resolved> {
        if let Some(l) = sources::local_lrc(audio).and_then(|t| lrc::parse(&t)) {
            return Some(Resolved::Found(l, Source::LocalFile));
        }
        sources::embedded(audio)
            .and_then(|t| lrc::parse(&t))
            .map(|l| Resolved::Found(l, Source::Embedded))
    }

    fn fetch_online(&self, req: &Request) -> Resolved {
        let client = match lrclib::client() {
            Ok(c) => c,
            Err(e) => return Resolved::NotFound(Some(e.to_string())),
        };
        let query = lrclib::Query {
            artist: &req.artist,
            title: &req.title,
            album: &req.album,
            duration_secs: req.duration_secs,
        };
        match lrclib::fetch(&client, &self.base_url, &query) {
            Ok(Some(found)) => {
                // Prefer timed lyrics; fall back to plain text.
                let cached = found
                    .synced
                    .as_deref()
                    .filter(|t| matches!(lrc::parse(t), Some(Lyrics::Synced(_))))
                    .map(|t| (t, true))
                    .or_else(|| found.plain.as_deref().map(|t| (t, false)));
                let Some((text, synced)) = cached else {
                    self.remember_not_found(&req.rel_path);
                    return Resolved::NotFound(None);
                };
                let stored = if synced {
                    self.cache.write_synced(&req.rel_path, text)
                } else {
                    self.cache.write_plain(&req.rel_path, text)
                };
                if let Err(e) = stored {
                    tracing::warn!("cannot cache lyrics: {e}");
                }
                match lrc::parse(text) {
                    Some(l) => Resolved::Found(l, Source::Online),
                    None => Resolved::NotFound(None),
                }
            }
            Ok(None) => {
                self.remember_not_found(&req.rel_path);
                Resolved::NotFound(None)
            }
            Err(e) => {
                tracing::warn!("LRCLIB lookup failed: {e}");
                Resolved::NotFound(Some(e.to_string()))
            }
        }
    }

    fn remember_not_found(&self, rel_path: &str) {
        if let Err(e) = self.cache.write_not_found(rel_path) {
            tracing::warn!("cannot cache lookup result: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::wav_bytes;
    use lrclib::serve_once;
    use std::fs;

    struct Env {
        tmp: tempfile::TempDir,
        resolver: Resolver,
    }

    fn env(online: bool) -> Env {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("song.wav"), wav_bytes(100)).unwrap();
        let resolver = Resolver::new(tmp.path().join("cache"), online);
        Env { tmp, resolver }
    }

    fn request(env: &Env) -> Request {
        Request {
            track_id: 1,
            rel_path: "song.wav".into(),
            audio_path: env.tmp.path().join("song.wav"),
            title: "Song".into(),
            artist: "Band".into(),
            album: "Album".into(),
            duration_secs: 100,
            retry: false,
        }
    }

    fn synced_len(r: &Resolved) -> Option<usize> {
        match r {
            Resolved::Found(Lyrics::Synced(l), _) => Some(l.len()),
            _ => None,
        }
    }

    #[test]
    fn local_lrc_wins_over_everything() {
        let mut e = env(true);
        fs::write(e.tmp.path().join("song.lrc"), "[00:01.00]a\n[00:02.00]b").unwrap();
        // Even a cached answer and an unreachable server do not matter.
        e.resolver.base_url = "http://127.0.0.1:1".into();
        let r = e
            .resolver
            .resolve(&request(&e), || panic!("must not search"));
        assert_eq!(synced_len(&r), Some(2));
        assert!(matches!(r, Resolved::Found(_, Source::LocalFile)));
    }

    #[test]
    fn plain_text_lrc_file_is_shown_as_plain() {
        let e = env(false);
        fs::write(e.tmp.path().join("song.lrc"), "just words\nmore words").unwrap();
        let r = e.resolver.resolve(&request(&e), || {});
        assert!(matches!(
            r,
            Resolved::Found(Lyrics::Plain(_), Source::LocalFile)
        ));
    }

    #[test]
    fn embedded_lyrics_are_used() {
        use lofty::config::WriteOptions;
        use lofty::prelude::*;
        use lofty::tag::{ItemKey, Tag, TagType};
        let e = env(false);
        let mut tag = Tag::new(TagType::Id3v2);
        tag.insert_text(ItemKey::UnsyncLyrics, "[00:05.00]inside".to_owned());
        tag.save_to_path(e.tmp.path().join("song.wav"), WriteOptions::default())
            .unwrap();
        let r = e.resolver.resolve(&request(&e), || {});
        assert_eq!(synced_len(&r), Some(1));
        assert!(matches!(r, Resolved::Found(_, Source::Embedded)));
    }

    #[test]
    fn offline_mode_never_touches_the_network() {
        let mut e = env(false);
        e.resolver.base_url = "http://127.0.0.1:1".into();
        let r = e
            .resolver
            .resolve(&request(&e), || panic!("must not search"));
        assert_eq!(r, Resolved::NotFound(None));
    }

    #[test]
    fn online_result_is_cached_and_reused() {
        let mut e = env(true);
        let (url, server) = serve_once(
            "200 OK",
            r#"{"syncedLyrics":"[00:01.00]one\n[00:02.00]two","plainLyrics":"one\ntwo"}"#,
        );
        e.resolver.base_url = url;
        let mut searched = false;
        let r = e.resolver.resolve(&request(&e), || searched = true);
        server.join().unwrap();
        assert!(searched);
        assert_eq!(synced_len(&r), Some(2));
        assert!(matches!(r, Resolved::Found(_, Source::Online)));

        // Second time: served from the cache, server no longer exists.
        e.resolver.base_url = "http://127.0.0.1:1".into();
        let r = e
            .resolver
            .resolve(&request(&e), || panic!("must not search"));
        assert_eq!(synced_len(&r), Some(2));
        assert!(matches!(r, Resolved::Found(_, Source::Cache)));
    }

    #[test]
    fn plain_only_online_result_is_cached_as_plain() {
        let mut e = env(true);
        let (url, server) = serve_once(
            "200 OK",
            r#"{"syncedLyrics":null,"plainLyrics":"la\n\nla"}"#,
        );
        e.resolver.base_url = url;
        let r = e.resolver.resolve(&request(&e), || {});
        server.join().unwrap();
        assert_eq!(
            r,
            Resolved::Found(
                Lyrics::Plain(vec!["la".into(), String::new(), "la".into()]),
                Source::Online
            )
        );
        assert!(matches!(
            e.resolver.cache.read("song.wav"),
            Some(Cached::Plain(_))
        ));
    }

    #[test]
    fn not_found_is_remembered_until_retry() {
        let mut e = env(true);
        let (url, server) = serve_once("404 Not Found", "{}");
        e.resolver.base_url = url;
        let r = e.resolver.resolve(&request(&e), || {});
        server.join().unwrap();
        assert_eq!(r, Resolved::NotFound(None));

        // Remembered: no new request is made.
        e.resolver.base_url = "http://127.0.0.1:1".into();
        let r = e
            .resolver
            .resolve(&request(&e), || panic!("must not search"));
        assert_eq!(r, Resolved::NotFound(None));

        // "Retry" asks again.
        let (url, server) = serve_once("200 OK", r#"{"plainLyrics":"now there"}"#);
        e.resolver.base_url = url;
        let mut req = request(&e);
        req.retry = true;
        let r = e.resolver.resolve(&req, || {});
        server.join().unwrap();
        assert!(matches!(
            r,
            Resolved::Found(Lyrics::Plain(_), Source::Online)
        ));
    }

    #[test]
    fn network_failure_is_reported_and_not_cached() {
        let mut e = env(true);
        let (url, _s) = serve_once("503 Service Unavailable", "down");
        e.resolver.base_url = url;
        let r = e.resolver.resolve(&request(&e), || {});
        assert!(matches!(r, Resolved::NotFound(Some(_))), "{r:?}");
        assert_eq!(e.resolver.cache.read("song.wav"), None);
    }

    #[test]
    fn unknown_artist_is_not_looked_up() {
        let mut e = env(true);
        e.resolver.base_url = "http://127.0.0.1:1".into();
        let mut req = request(&e);
        req.artist = crate::library::UNKNOWN_ARTIST.into();
        let r = e.resolver.resolve(&req, || panic!("must not search"));
        assert_eq!(r, Resolved::NotFound(None));
    }

    #[test]
    fn worker_answers_the_newest_request_and_saves_offsets() {
        let e = env(false);
        fs::write(e.tmp.path().join("song.lrc"), "[00:01.00]hi").unwrap();
        let db_path = e.tmp.path().join("library.db");
        let handle = spawn(db_path.clone(), e.tmp.path().join("cache"), false);

        handle.send(Command::SetOffset {
            rel_path: "song.wav".into(),
            ms: 300,
        });
        handle.send(Command::Load(request(&e)));
        let ev = handle
            .events
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        match ev {
            Event::Found {
                track_id,
                offset_ms,
                source,
                ..
            } => {
                assert_eq!((track_id, offset_ms, source), (1, 300, Source::LocalFile));
            }
            other => panic!("unexpected {other:?}"),
        }
        // The offset is persisted in the database.
        let db = Db::open(&db_path).unwrap();
        assert_eq!(load_offset(&db, "song.wav"), 300);
        assert_eq!(load_offset(&db, "other.mp3"), 0);
    }
}
