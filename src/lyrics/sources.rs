//! Lyrics that live next to the track or inside its tags.

use std::fs;
use std::path::Path;

use lofty::prelude::*;
use lofty::tag::ItemKey;

/// Text of `<track name>.lrc` next to the audio file. Files in old
/// encodings are read lossily rather than rejected.
pub fn local_lrc(audio: &Path) -> Option<String> {
    ["lrc", "LRC"].iter().find_map(|ext| {
        let bytes = fs::read(audio.with_extension(ext)).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    })
}

/// Lyrics embedded in the tags: ID3 USLT, Vorbis/FLAC LYRICS, MP4 ©lyr.
/// (lofty does not expose ID3 SYLT frames, so those are not read.)
pub fn embedded(audio: &Path) -> Option<String> {
    let tagged = lofty::read_from_path(audio).ok()?;
    tagged.tags().iter().find_map(|tag| {
        [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
            .iter()
            .filter_map(|key| tag.get_string(*key))
            .map(str::trim)
            .find(|s| !s.is_empty())
            .map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::wav_bytes;

    #[test]
    fn finds_lrc_next_to_the_track() {
        let tmp = tempfile::tempdir().unwrap();
        let audio = tmp.path().join("song.mp3");
        assert_eq!(local_lrc(&audio), None);
        fs::write(tmp.path().join("song.lrc"), "[00:01.00]hi").unwrap();
        assert_eq!(local_lrc(&audio).as_deref(), Some("[00:01.00]hi"));
    }

    #[test]
    fn non_utf8_lrc_is_still_read() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.lrc"), b"[00:01.00]caf\xe9").unwrap();
        let text = local_lrc(&tmp.path().join("a.flac")).unwrap();
        assert!(text.starts_with("[00:01.00]caf"));
    }

    #[test]
    fn untagged_or_unreadable_files_have_no_embedded_lyrics() {
        let tmp = tempfile::tempdir().unwrap();
        let wav = tmp.path().join("a.wav");
        fs::write(&wav, wav_bytes(100)).unwrap();
        assert_eq!(embedded(&wav), None);
        assert_eq!(embedded(&tmp.path().join("missing.mp3")), None);
    }

    #[test]
    fn reads_lyrics_written_into_tags() {
        use lofty::config::WriteOptions;
        use lofty::tag::{Tag, TagType};
        let tmp = tempfile::tempdir().unwrap();
        let wav = tmp.path().join("a.wav");
        fs::write(&wav, wav_bytes(100)).unwrap();

        let mut tag = Tag::new(TagType::Id3v2);
        tag.insert_text(ItemKey::UnsyncLyrics, "[00:01.00]embedded".to_owned());
        tag.save_to_path(&wav, WriteOptions::default()).unwrap();

        assert_eq!(embedded(&wav).as_deref(), Some("[00:01.00]embedded"));
    }
}
