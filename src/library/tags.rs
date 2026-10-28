//! Reading metadata from audio files with lofty.

use std::path::Path;

use lofty::prelude::*;
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
    let tagged = lofty::read_from_path(path)?;
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
