//! On-disk cache of downloaded (or, later, recognized) lyrics in `data/lyrics/`.
//!
//! Per track there is at most one of:
//! - `<name>.lrc`  synchronized lyrics (maybe enhanced)
//! - `<name>.txt`  plain lyrics
//! - `<name>.none` a lookup that found nothing (so it is not repeated)
//! - `<name>.nospeech` speech recognition found no vocals

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cached {
    Synced(String),
    Plain(String),
    NotFound,
    /// Speech recognition ran and found nothing to transcribe.
    NoSpeech,
}

pub struct Cache {
    dir: PathBuf,
}

const EXTENSIONS: [&str; 4] = ["lrc", "txt", "none", "nospeech"];

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn read(&self, rel_path: &str) -> Option<Cached> {
        let base = self.base(rel_path);
        let read = |ext: &str| fs::read_to_string(base.with_extension(ext)).ok();
        if let Some(text) = read("lrc") {
            Some(Cached::Synced(text))
        } else if let Some(text) = read("txt") {
            Some(Cached::Plain(text))
        } else if base.with_extension("nospeech").is_file() {
            Some(Cached::NoSpeech)
        } else if base.with_extension("none").is_file() {
            Some(Cached::NotFound)
        } else {
            None
        }
    }

    pub fn write_synced(&self, rel_path: &str, text: &str) -> io::Result<()> {
        self.write(rel_path, "lrc", text)
    }

    pub fn write_plain(&self, rel_path: &str, text: &str) -> io::Result<()> {
        self.write(rel_path, "txt", text)
    }

    pub fn write_not_found(&self, rel_path: &str) -> io::Result<()> {
        self.write(rel_path, "none", "")
    }

    #[cfg_attr(not(feature = "whisper"), allow(dead_code))]
    pub fn write_no_speech(&self, rel_path: &str) -> io::Result<()> {
        self.write(rel_path, "nospeech", "")
    }

    /// Drop every cached result for the track (used by "retry").
    pub fn forget(&self, rel_path: &str) {
        let base = self.base(rel_path);
        for ext in EXTENSIONS {
            // Missing files are the normal case.
            let _ = fs::remove_file(base.with_extension(ext));
        }
    }

    /// Store one result; any older entry of another kind is replaced.
    fn write(&self, rel_path: &str, ext: &str, text: &str) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        self.forget(rel_path);
        let target = self.base(rel_path).with_extension(ext);
        // Write-then-rename so a crash never leaves a half-written file.
        let tmp = target.with_extension(format!("{ext}.tmp"));
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &target)
    }

    /// File name without extension: readable stem + hash of the full path.
    fn base(&self, rel_path: &str) -> PathBuf {
        let stem: String = Path::new(rel_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .take(40)
            .collect();
        self.dir.join(format!(
            "{}-{:016x}",
            stem.trim(),
            fnv1a(rel_path.as_bytes())
        ))
    }
}

/// FNV-1a: stable across runs and platforms (unlike `DefaultHasher`).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> (tempfile::TempDir, Cache) {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path().join("lyrics"));
        (tmp, cache)
    }

    #[test]
    fn empty_cache_has_nothing() {
        let (_t, c) = cache();
        assert_eq!(c.read("a.mp3"), None);
    }

    #[test]
    fn stores_each_kind() {
        let (_t, c) = cache();
        c.write_synced("a.mp3", "[00:01.00]hi").unwrap();
        assert_eq!(c.read("a.mp3"), Some(Cached::Synced("[00:01.00]hi".into())));
        c.write_plain("a.mp3", "hi").unwrap();
        assert_eq!(c.read("a.mp3"), Some(Cached::Plain("hi".into())));
        c.write_not_found("a.mp3").unwrap();
        assert_eq!(c.read("a.mp3"), Some(Cached::NotFound));
    }

    #[test]
    fn newer_result_replaces_older_kinds() {
        let (_t, c) = cache();
        c.write_not_found("a.mp3").unwrap();
        c.write_synced("a.mp3", "x").unwrap();
        assert_eq!(c.read("a.mp3"), Some(Cached::Synced("x".into())));
    }

    #[test]
    fn forget_clears_everything() {
        let (_t, c) = cache();
        c.write_not_found("a.mp3").unwrap();
        c.forget("a.mp3");
        assert_eq!(c.read("a.mp3"), None);
        c.forget("never-cached.mp3"); // harmless
    }

    #[test]
    fn same_file_name_in_different_folders_does_not_collide() {
        let (_t, c) = cache();
        c.write_plain("one/song.mp3", "first").unwrap();
        c.write_plain("two/song.mp3", "second").unwrap();
        assert_eq!(c.read("one/song.mp3"), Some(Cached::Plain("first".into())));
        assert_eq!(c.read("two/song.mp3"), Some(Cached::Plain("second".into())));
    }

    #[test]
    fn awkward_names_are_safe_file_names() {
        let (t, c) = cache();
        let rel = "Dir/AC:DC - Back in Black? <live>|\"x\".mp3";
        c.write_plain(rel, "t").unwrap();
        assert_eq!(c.read(rel), Some(Cached::Plain("t".into())));
        let name = fs::read_dir(t.path().join("lyrics"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name()
            .to_string_lossy()
            .into_owned();
        assert!(!name.contains(['<', '>', ':', '"', '?', '|']), "{name}");
    }

    #[test]
    fn hash_is_stable() {
        // Pinned so the cache stays valid across releases.
        assert_eq!(fnv1a(b"abc"), 0xe71f_a219_0541_574b);
    }
}
