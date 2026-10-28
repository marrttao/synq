//! Reading metadata from audio files with lofty.

use std::path::Path;

use lofty::prelude::*;
use lofty::probe::Probe;
use thiserror::Error;

pub const UNKNOWN_ARTIST: &str = "Unknown Artist";

#[derive(Debug, Error)]
pub enum TagError {
    #[error("unsupported or unreadable audio file: {0}")]
    Parse(#[from] lofty::error::FileParseError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TagInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
}

pub fn read_tags(path: &Path) -> Result<TagInfo, TagError> {
    // The kind of file is taken from its content, not its name: downloads
    // are often an MP4/M4A container called ".mp3".
    let tagged = Probe::open(path)?
        .guess_file_type()
        .map_err(lofty::error::FileParseError::from)?
        .read()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let (title, artist, album) = normalize(
        tag.and_then(|t| t.title()).as_deref(),
        tag.and_then(|t| t.artist()).as_deref(),
        tag.and_then(|t| t.album()).as_deref(),
        path,
    );
    Ok(TagInfo {
        title,
        artist,
        album,
        duration_ms: tagged.properties().duration().as_millis() as u64,
    })
}

/// Apply the fallbacks: file name for the title, "Unknown Artist" for the artist.
pub fn normalize(
    title: Option<&str>,
    artist: Option<&str>,
    album: Option<&str>,
    path: &Path,
) -> (String, String, String) {
    fn clean(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    let title = clean(title)
        .map(str::to_owned)
        .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let artist = clean(artist).unwrap_or(UNKNOWN_ARTIST).to_owned();
    let album = clean(album).unwrap_or_default().to_owned();
    (title, artist, album)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_used_when_present() {
        let (t, a, al) = normalize(
            Some(" Song "),
            Some("Band"),
            Some("Album"),
            Path::new("x/file.mp3"),
        );
        assert_eq!(
            (t.as_str(), a.as_str(), al.as_str()),
            ("Song", "Band", "Album")
        );
    }

    #[test]
    fn missing_title_falls_back_to_file_name() {
        let (t, _, _) = normalize(None, None, None, Path::new("dir/My Track.flac"));
        assert_eq!(t, "My Track");
    }

    #[test]
    fn blank_tags_count_as_missing() {
        let (t, a, al) = normalize(Some("  "), Some(""), Some(" "), Path::new("a.wav"));
        assert_eq!(
            (t.as_str(), a.as_str(), al.as_str()),
            ("a", UNKNOWN_ARTIST, "")
        );
    }
}

#[cfg(test)]
mod misnamed {
    use super::*;

    #[test]
    fn a_file_is_recognized_by_content_not_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("really a wav.mp3");
        std::fs::write(&path, crate::testutil::wav_bytes(1500)).unwrap();
        let info = read_tags(&path).unwrap();
        assert_eq!(info.title, "really a wav");
        assert!(
            (1400..=1600).contains(&info.duration_ms),
            "{}",
            info.duration_ms
        );
    }
}
