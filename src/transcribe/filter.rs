//! Cleaning up Whisper output: music makes it hallucinate, so segments that
//! are probably not singing are dropped before the lyrics are saved.

use crate::lyrics::lrc::{Line, Word};

/// Segments Whisper itself considers likely silence/music.
const NO_SPEECH_MAX: f32 = 0.6;
/// The same phrase may appear this many times in a row (choruses repeat);
/// further repeats are the typical "stuck decoder" loop.
const MAX_CONSECUTIVE_REPEATS: usize = 2;
/// A short snippet alone in this much silence is suspicious.
const SILENCE_GAP_MS: u64 = 10_000;
const SHORT_SEGMENT_MS: u64 = 2_000;
const SHORT_SEGMENT_WORDS: usize = 3;
/// Phrases Whisper invents from its training data on silent audio.
const BOILERPLATE: [&str; 5] = [
    "thanks for watching",
    "thank you for watching",
    "subtitles by",
    "amara org",
    "please subscribe",
];

#[derive(Debug, Clone, PartialEq)]
pub struct RawToken {
    pub text: String,
    pub start_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub no_speech_prob: f32,
    pub tokens: Vec<RawToken>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct FilterStats {
    pub no_speech: usize,
    pub noise: usize,
    pub boilerplate: usize,
    pub repeats: usize,
    pub short_in_silence: usize,
}

/// Lower-case letters and digits only, single spaces: for comparing phrases.
fn normalize(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.trim_end().to_owned()
}

/// "[MUSIC]", "(applause)", "*sighs*", "♪♪": annotations, not lyrics.
fn is_noise(text: &str) -> bool {
    let t = text.trim();
    let wrapped = [('[', ']'), ('(', ')'), ('*', '*'), ('{', '}')]
        .iter()
        .any(|&(open, close)| t.starts_with(open) && t.ends_with(close) && t.chars().count() > 1);
    let only_symbols = t
        .chars()
        .all(|c| !c.is_alphanumeric() || c == '\u{266A}' || c == '\u{266B}');
    wrapped || only_symbols
}

/// Drop segments that are most likely not sung words.
///
/// `audio_ms` is the length of the track (used to judge silence after the
/// last segment; pass 0 if unknown).
pub fn filter_segments(segments: Vec<RawSegment>, audio_ms: u64) -> (Vec<RawSegment>, FilterStats) {
    let mut stats = FilterStats::default();
    let mut kept: Vec<RawSegment> = Vec::new();
    let mut last_key = String::new();
    let mut run = 0usize;

    for (i, seg) in segments.iter().enumerate() {
        let text = seg.text.trim();
        if text.is_empty() || is_noise(text) {
            stats.noise += 1;
            continue;
        }
        if seg.no_speech_prob > NO_SPEECH_MAX {
            stats.no_speech += 1;
            continue;
        }
        let key = normalize(text);
        if BOILERPLATE.iter().any(|b| key.starts_with(b)) {
            stats.boilerplate += 1;
            continue;
        }

        // Short snippet isolated by long silence on both sides.
        let words = key.split(' ').filter(|w| !w.is_empty()).count();
        let gap_before = seg
            .start_ms
            .saturating_sub(kept.last().map_or(0, |k| k.end_ms));
        let next_start = segments.get(i + 1).map_or(audio_ms, |n| n.start_ms);
        let gap_after = next_start.saturating_sub(seg.end_ms);
        let is_short = seg.end_ms.saturating_sub(seg.start_ms) <= SHORT_SEGMENT_MS
            && words <= SHORT_SEGMENT_WORDS;
        // With an unknown track length the trailing silence cannot be judged.
        let trailing_known = audio_ms > 0 || i + 1 < segments.len();
        if is_short && trailing_known && gap_before >= SILENCE_GAP_MS && gap_after >= SILENCE_GAP_MS
        {
            stats.short_in_silence += 1;
            continue;
        }

        if key == last_key {
            run += 1;
            if run > MAX_CONSECUTIVE_REPEATS {
                stats.repeats += 1;
                continue;
            }
        } else {
            last_key = key;
            run = 1;
        }
        kept.push(seg.clone());
    }
    (kept, stats)
}

/// Whisper marks control tokens as `[_BEG_]`, `[_TT_150]`, `<|endoftext|>`.
fn is_special(text: &str) -> bool {
    text.starts_with("[_") || text.starts_with("<|")
}

/// Group sub-word tokens into words. A token starting with whitespace starts
/// a new word; the word's time is its first token's. Times are forced to be
/// non-decreasing and to stay inside the segment, because token timestamps
/// are sometimes garbage. Words get a trailing space except the last.
pub fn words_from_tokens(tokens: &[RawToken], seg_start: u64, seg_end: u64) -> Vec<Word> {
    let mut words: Vec<(u64, String)> = Vec::new();
    for t in tokens {
        if is_special(&t.text) || t.text.trim().is_empty() {
            continue;
        }
        let starts_word = t.text.starts_with(char::is_whitespace);
        match words.last_mut() {
            Some((_, text)) if !starts_word => text.push_str(&t.text),
            _ => words.push((t.start_ms, t.text.trim_start().to_owned())),
        }
    }
    let hi = seg_end.max(seg_start);
    let mut floor = seg_start;
    let count = words.len();
    words
        .into_iter()
        .enumerate()
        .map(|(i, (time, text))| {
            let time_ms = time.clamp(floor, hi);
            floor = time_ms;
            let text = text.trim_end();
            Word {
                time_ms,
                text: if i + 1 < count {
                    format!("{text} ")
                } else {
                    text.to_owned()
                },
            }
        })
        .collect()
}

/// Segments -> lyric lines with word timings (for enhanced LRC).
pub fn to_lines(segments: &[RawSegment]) -> Vec<Line> {
    let mut lines: Vec<Line> = segments
        .iter()
        .map(|s| Line {
            time_ms: s.start_ms,
            text: s.text.split_whitespace().collect::<Vec<_>>().join(" "),
            words: words_from_tokens(&s.tokens, s.start_ms, s.end_ms),
            end_ms: Some(s.end_ms.max(s.start_ms)),
        })
        .filter(|l| !l.text.is_empty())
        .collect();
    lines.sort_by_key(|l| l.time_ms);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: u64, end: u64, text: &str) -> RawSegment {
        RawSegment {
            start_ms: start,
            end_ms: end,
            text: text.into(),
            no_speech_prob: 0.0,
            tokens: Vec::new(),
        }
    }

    fn texts(segs: &[RawSegment]) -> Vec<&str> {
        segs.iter().map(|s| s.text.as_str()).collect()
    }

    fn run(segs: Vec<RawSegment>) -> (Vec<RawSegment>, FilterStats) {
        filter_segments(segs, 0)
    }

    #[test]
    fn ordinary_lyrics_pass_untouched() {
        let input = vec![
            seg(1_000, 4_000, " Is this the real life?"),
            seg(4_000, 7_000, " Is this just fantasy?"),
        ];
        let (out, stats) = run(input.clone());
        assert_eq!(out, input);
        assert_eq!(stats, FilterStats::default());
    }

    #[test]
    fn segments_with_high_no_speech_probability_are_dropped() {
        let mut quiet = seg(0, 3_000, "mumbling");
        quiet.no_speech_prob = 0.9;
        let mut borderline = seg(3_000, 6_000, "kept");
        borderline.no_speech_prob = 0.6;
        let (out, stats) = run(vec![quiet, borderline]);
        assert_eq!(texts(&out), ["kept"]);
        assert_eq!(stats.no_speech, 1);
    }

    #[test]
    fn music_annotations_and_symbols_are_dropped() {
        let (out, stats) = run(vec![
            seg(0, 2_000, " [MUSIC]"),
            seg(2_000, 4_000, "(applause)"),
            seg(4_000, 6_000, "\u{266A}\u{266A}"),
            seg(6_000, 8_000, "..."),
            seg(8_000, 10_000, "*sighs*"),
            seg(10_000, 12_000, "real words"),
        ]);
        assert_eq!(texts(&out), ["real words"]);
        assert_eq!(stats.noise, 5);
    }

    #[test]
    fn endless_repeats_are_cut_but_a_doubled_line_stays() {
        let mut input = Vec::new();
        for i in 0..6 {
            input.push(seg(i * 2_000, i * 2_000 + 1_800, " I love you"));
        }
        let (out, stats) = run(input);
        assert_eq!(out.len(), MAX_CONSECUTIVE_REPEATS);
        assert_eq!(stats.repeats, 4);
    }

    #[test]
    fn repeats_are_compared_ignoring_case_and_punctuation() {
        let (out, _) = run(vec![
            seg(0, 1_000, "Yeah!"),
            seg(1_000, 2_000, "yeah"),
            seg(2_000, 3_000, "YEAH?"),
            seg(3_000, 4_000, "Yeah."),
        ]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn a_phrase_may_come_back_after_other_lines() {
        let (out, _) = run(vec![
            seg(0, 2_000, "Chorus line"),
            seg(2_000, 4_000, "Chorus line"),
            seg(4_000, 6_000, "Verse line"),
            seg(6_000, 8_000, "Chorus line"),
            seg(8_000, 10_000, "Chorus line"),
        ]);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn short_snippet_alone_in_long_silence_is_dropped() {
        let (out, stats) = run(vec![
            seg(1_000, 4_000, "First line"),
            // 30 s of silence, a lonely 1 s "Thank you.", 30 s of silence.
            seg(34_000, 35_000, "Thank you."),
            seg(65_000, 68_000, "Second line"),
        ]);
        assert_eq!(texts(&out), ["First line", "Second line"]);
        assert_eq!(stats.short_in_silence, 1);
    }

    #[test]
    fn short_line_after_an_instrumental_break_is_kept_if_singing_follows() {
        let (out, _) = run(vec![
            seg(1_000, 4_000, "First line"),
            seg(34_000, 35_000, "Hey!"),
            seg(35_500, 38_000, "Back to singing"),
        ]);
        assert_eq!(texts(&out), ["First line", "Hey!", "Back to singing"]);
    }

    #[test]
    fn long_segments_in_silence_are_not_treated_as_short() {
        let (out, _) = run(vec![seg(40_000, 46_000, "a long and real sentence here")]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn trailing_silence_is_judged_by_the_track_length() {
        let lone = seg(30_000, 31_000, "Thanks.");
        let (out, _) = filter_segments(vec![lone.clone()], 120_000);
        assert!(out.is_empty());
        // Unknown length: we cannot tell, so keep it.
        let (out, _) = filter_segments(vec![lone], 0);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn known_boilerplate_is_dropped() {
        let (out, stats) = run(vec![
            seg(0, 3_000, "Thanks for watching!"),
            seg(3_000, 6_000, "Subtitles by the Amara.org community"),
            seg(6_000, 9_000, "I was watching you"),
        ]);
        assert_eq!(texts(&out), ["I was watching you"]);
        assert_eq!(stats.boilerplate, 2);
    }

    #[test]
    fn empty_input_and_blank_text() {
        let (out, _) = run(Vec::new());
        assert!(out.is_empty());
        let (out, _) = run(vec![seg(0, 1_000, "   ")]);
        assert!(out.is_empty());
    }

    fn tok(text: &str, start: u64) -> RawToken {
        RawToken {
            text: text.into(),
            start_ms: start,
        }
    }

    #[test]
    fn tokens_are_grouped_into_words() {
        let tokens = vec![
            tok("[_BEG_]", 1_000),
            tok(" Hel", 1_000),
            tok("lo", 1_200),
            tok(" world", 1_500),
            tok("!", 1_800),
            tok("<|endoftext|>", 2_000),
        ];
        let words = words_from_tokens(&tokens, 1_000, 2_000);
        let got: Vec<_> = words.iter().map(|w| (w.time_ms, w.text.as_str())).collect();
        assert_eq!(got, [(1_000, "Hello "), (1_500, "world!")]);
    }

    #[test]
    fn word_times_are_clamped_and_monotonic() {
        let tokens = vec![
            tok(" a", 500), // before the segment
            tok(" b", 1_400),
            tok(" c", 1_200), // goes backwards
            tok(" d", 9_000), // after the segment
        ];
        let words = words_from_tokens(&tokens, 1_000, 2_000);
        let times: Vec<_> = words.iter().map(|w| w.time_ms).collect();
        assert_eq!(times, [1_000, 1_400, 1_400, 2_000]);
    }

    #[test]
    fn lines_carry_words_and_end_times_and_are_sorted() {
        let mut a = seg(5_000, 7_000, " later  line");
        a.tokens = vec![tok(" later", 5_000), tok(" line", 5_600)];
        let b = seg(1_000, 2_000, " early");
        let lines = to_lines(&[a, b]);
        assert_eq!(lines[0].text, "early");
        assert!(lines[0].words.is_empty());
        assert_eq!(lines[1].text, "later line");
        assert_eq!(lines[1].words.len(), 2);
        assert_eq!(lines[1].end_ms, Some(7_000));
    }

    #[test]
    fn output_survives_a_round_trip_through_lrc() {
        use crate::lyrics::lrc::{Lyrics, format_lrc, parse};
        let mut s = seg(12_000, 14_000, " Hello big world");
        s.tokens = vec![
            tok(" Hello", 12_000),
            tok(" big", 12_500),
            tok(" world", 13_000),
        ];
        let lines = to_lines(&[s]);
        let text = format_lrc(&lines);
        assert_eq!(
            text,
            "[00:12.00]<00:12.00>Hello <00:12.50>big <00:13.00>world<00:14.00>\n"
        );
        assert_eq!(parse(&text), Some(Lyrics::Synced(lines)));
    }
}
