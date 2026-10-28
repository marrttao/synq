//! LRC parser: plain (`[mm:ss.xx]text`) and enhanced (`<mm:ss.xx>` per word).

/// One word (or word piece) of an enhanced line. `text` keeps its original
/// whitespace so that concatenating the words restores the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub time_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub time_ms: u64,
    /// Trimmed text of the whole line (empty for instrumental gaps).
    pub text: String,
    /// Word timings; empty for ordinary LRC lines.
    pub words: Vec<Word>,
    /// Closing `<mm:ss.xx>` tag after the last word, if the file has one.
    pub end_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lyrics {
    /// Lines sorted by time.
    Synced(Vec<Line>),
    /// Text without timings.
    Plain(Vec<String>),
}

/// Parse LRC text. Returns `Synced` if any line has a timestamp, otherwise
/// `Plain` with the non-empty lines, or `None` if there is no text at all.
///
/// A `[offset:+N]` header (milliseconds) is applied to the times: following
/// the LRC convention, a positive offset makes lyrics appear *earlier*.
pub fn parse(input: &str) -> Option<Lyrics> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut offset_ms: i64 = 0;
    let mut lines = Vec::new();
    let mut plain = Vec::new();

    for raw in input.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            // Stanza break in plain lyrics (collapsed, never leading).
            if plain.last().is_some_and(|l: &String| !l.is_empty()) {
                plain.push(String::new());
            }
            continue;
        }
        let mut rest = raw;
        let mut times = Vec::new();
        while let Some((time, after)) = take_line_tag(rest) {
            times.push(time);
            rest = after;
        }
        if times.is_empty() {
            match metadata(raw) {
                Some(("offset", value)) => offset_ms = value.trim().parse().unwrap_or(0),
                Some(_) => {}
                None => plain.push(raw.to_owned()),
            }
            continue;
        }
        for time_ms in times {
            let (text, words, end_ms) = parse_words(rest, time_ms);
            lines.push(Line {
                time_ms,
                text,
                words,
                end_ms,
            });
        }
    }

    if lines.is_empty() {
        while plain.last().is_some_and(String::is_empty) {
            plain.pop();
        }
        return if plain.is_empty() {
            None
        } else {
            Some(Lyrics::Plain(plain))
        };
    }
    if offset_ms != 0 {
        let shift = |t: u64| (t as i64 - offset_ms).max(0) as u64;
        for l in &mut lines {
            l.time_ms = shift(l.time_ms);
            l.end_ms = l.end_ms.map(shift);
            for w in &mut l.words {
                w.time_ms = shift(w.time_ms);
            }
        }
    }
    // Stable: lines with equal times keep their file order.
    lines.sort_by_key(|l| l.time_ms);
    Some(Lyrics::Synced(lines))
}

/// `[mm:ss.xx]` at the start of `s` -> (milliseconds, remainder).
fn take_line_tag(s: &str) -> Option<(u64, &str)> {
    let inner = s.strip_prefix('[')?;
    let end = inner.find(']')?;
    let time = parse_time(&inner[..end])?;
    Some((time, &inner[end + 1..]))
}

/// `[key:value]` header such as `[ar:Artist]`.
fn metadata(s: &str) -> Option<(&str, &str)> {
    let inner = s.strip_prefix('[')?.strip_suffix(']')?;
    let (key, value) = inner.split_once(':')?;
    let key = key.trim();
    (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphabetic())).then_some((key, value))
}

/// "mm:ss", "mm:ss.x", "mm:ss.xx", "mm:ss.xxx" (also "mm:ss:xx") -> ms.
fn parse_time(s: &str) -> Option<u64> {
    let (min, rest) = s.split_once(':')?;
    let (sec, frac) = match rest.split_once(['.', ':']) {
        Some((sec, frac)) => (sec, Some(frac)),
        None => (rest, None),
    };
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    if !digits(min) || !digits(sec) || frac.is_some_and(|f| !digits(f)) {
        return None;
    }
    let (min, sec): (u64, u64) = (min.parse().ok()?, sec.parse().ok()?);
    if sec >= 60 {
        return None;
    }
    let millis = match frac {
        None => 0,
        Some(f) => {
            // ".5" = 500 ms, ".05" = 50 ms, ".057" = 57 ms; extra digits are cut.
            let f = &f[..f.len().min(3)];
            f.parse::<u64>().ok()? * 10u64.pow(3 - f.len() as u32)
        }
    };
    Some((min * 60 + sec) * 1000 + millis)
}

enum Seg<'a> {
    Text(&'a str),
    Time(u64),
}

/// Cut a line body into text pieces and `<mm:ss.xx>` tags. A `<` that does
/// not start a valid time tag stays ordinary text.
fn segments(s: &str) -> Vec<Seg<'_>> {
    let mut out = Vec::new();
    let mut text_start = 0;
    let mut search_from = 0;
    while let Some(rel) = s[search_from..].find('<') {
        let open = search_from + rel;
        let tag = s[open + 1..]
            .find('>')
            .and_then(|len| parse_time(&s[open + 1..open + 1 + len]).map(|t| (t, len)));
        match tag {
            Some((time, len)) => {
                if open > text_start {
                    out.push(Seg::Text(&s[text_start..open]));
                }
                out.push(Seg::Time(time));
                text_start = open + len + 2;
                search_from = text_start;
            }
            None => search_from = open + 1,
        }
    }
    if text_start < s.len() {
        out.push(Seg::Text(&s[text_start..]));
    }
    out
}

/// Line body -> (line text, word timings, end time).
fn parse_words(body: &str, line_time: u64) -> (String, Vec<Word>, Option<u64>) {
    let segs = segments(body);
    if !segs.iter().any(|s| matches!(s, Seg::Time(_))) {
        // Ordinary LRC line: no word timings at all.
        return (body.trim().to_owned(), Vec::new(), None);
    }
    let mut text = String::new();
    let mut words = Vec::new();
    let mut pending: Option<u64> = None;
    for seg in &segs {
        match seg {
            Seg::Time(t) => pending = Some(*t),
            Seg::Text(s) => {
                text.push_str(s);
                if s.trim().is_empty() {
                    continue;
                }
                // Text before the first tag starts with the line itself.
                let time_ms = pending.take().unwrap_or(line_time);
                words.push(Word {
                    time_ms,
                    text: (*s).to_owned(),
                });
            }
        }
    }
    // A tag after the last word closes the line.
    let end_ms = match segs.last() {
        Some(Seg::Time(t)) if !words.is_empty() => Some(*t),
        _ => None,
    };
    (text.trim().to_owned(), words, end_ms)
}

/// Index of the line being sung at `pos_ms` (the last line that has
/// started), with `offset_ms` > 0 delaying the lyrics.
pub fn current_line(lines: &[Line], pos_ms: u64, offset_ms: i64) -> Option<usize> {
    let pos = pos_ms as i64 - offset_ms;
    if pos < 0 {
        return None;
    }
    lines
        .partition_point(|l| l.time_ms as i64 <= pos)
        .checked_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synced(input: &str) -> Vec<Line> {
        match parse(input) {
            Some(Lyrics::Synced(l)) => l,
            other => panic!("expected synced lyrics, got {other:?}"),
        }
    }

    #[test]
    fn parses_basic_lines() {
        let l = synced("[00:12.00]Hello\n[00:15.50]World\n");
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].time_ms, l[0].text.as_str()), (12_000, "Hello"));
        assert_eq!((l[1].time_ms, l[1].text.as_str()), (15_500, "World"));
        assert!(l[0].words.is_empty());
    }

    #[test]
    fn fraction_lengths() {
        assert_eq!(parse_time("00:01"), Some(1_000));
        assert_eq!(parse_time("00:01.5"), Some(1_500));
        assert_eq!(parse_time("00:01.05"), Some(1_050));
        assert_eq!(parse_time("00:01.057"), Some(1_057));
        assert_eq!(parse_time("00:01.0579"), Some(1_057));
        assert_eq!(parse_time("01:01:50"), Some(61_500)); // colon as separator
        assert_eq!(parse_time("100:00.00"), Some(6_000_000));
    }

    #[test]
    fn rejects_bad_times() {
        for bad in ["", "ab:cd", "00:61", "00", "00:01.", "-1:00", "00:0x"] {
            assert_eq!(parse_time(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn multiple_timestamps_repeat_the_line_and_sort() {
        let l = synced("[00:30.00]Later\n[00:10.00][00:50.00]Chorus\n");
        let got: Vec<_> = l.iter().map(|l| (l.time_ms, l.text.as_str())).collect();
        assert_eq!(
            got,
            [(10_000, "Chorus"), (30_000, "Later"), (50_000, "Chorus")]
        );
    }

    #[test]
    fn metadata_is_skipped_and_empty_lines_are_kept() {
        let l = synced("[ar:Someone]\n[ti:Song]\n[by:me]\n[00:01.00]\n[00:02.00]Text\n");
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].text, "");
        assert_eq!(l[1].text, "Text");
    }

    #[test]
    fn offset_header_shifts_times_earlier_when_positive() {
        let l = synced("[offset:+500]\n[00:01.00]A\n[00:00.20]B\n");
        assert_eq!(l[0].time_ms, 0); // 200 - 500, clamped at zero
        assert_eq!(l[1].time_ms, 500);
        let l = synced("[offset:-1000]\n[00:01.00]A\n");
        assert_eq!(l[0].time_ms, 2_000);
    }

    #[test]
    fn handles_bom_crlf_and_whitespace() {
        let l = synced("\u{feff}[00:01.00]  One  \r\n\r\n[00:02.00]Two\r\n");
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].text, "One");
    }

    #[test]
    fn text_without_timestamps_is_plain() {
        let p = parse("[ar:X]\nFirst line\n\nSecond line\n");
        assert_eq!(
            p,
            Some(Lyrics::Plain(vec![
                "First line".into(),
                String::new(),
                "Second line".into()
            ]))
        );
        // Leading, trailing and repeated blank lines are dropped/collapsed.
        assert_eq!(
            parse("\n\nA\n\n\n\nB\n\n"),
            Some(Lyrics::Plain(vec!["A".into(), String::new(), "B".into()]))
        );
    }

    #[test]
    fn empty_or_metadata_only_input_is_none() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("  \n\n"), None);
        assert_eq!(parse("[ar:X]\n[ti:Y]\n"), None);
    }

    #[test]
    fn brackets_inside_text_are_kept() {
        let l = synced("[00:01.00]Hello [world] <3\n");
        assert_eq!(l[0].text, "Hello [world] <3");
        assert!(l[0].words.len() <= 1);
    }

    #[test]
    fn enhanced_lines_get_word_timings() {
        let l = synced("[00:12.00]<00:12.00>Hello <00:12.50>big <00:13.00>world <00:13.60>\n");
        let line = &l[0];
        assert_eq!(line.text, "Hello big world");
        let words: Vec<_> = line
            .words
            .iter()
            .map(|w| (w.time_ms, w.text.as_str()))
            .collect();
        assert_eq!(
            words,
            [(12_000, "Hello "), (12_500, "big "), (13_000, "world ")]
        );
        assert_eq!(line.end_ms, Some(13_600));
        let joined: String = line.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(joined.trim(), line.text);
    }

    #[test]
    fn enhanced_line_without_closing_tag_has_no_end() {
        let l = synced("[00:01.00]<00:01.00>One <00:01.40>two\n");
        assert_eq!(l[0].words.len(), 2);
        assert_eq!(l[0].end_ms, None);
    }

    #[test]
    fn text_before_the_first_word_tag_starts_at_the_line_time() {
        let l = synced("[00:05.00]Intro <00:06.00>word\n");
        let w: Vec<_> = l[0].words.iter().map(|w| w.time_ms).collect();
        assert_eq!(w, [5_000, 6_000]);
        assert_eq!(l[0].text, "Intro word");
    }

    #[test]
    fn word_offsets_follow_the_offset_header() {
        let l = synced("[offset:500]\n[00:02.00]<00:02.00>a <00:03.00>b <00:04.00>\n");
        assert_eq!(l[0].time_ms, 1_500);
        assert_eq!(l[0].words[1].time_ms, 2_500);
        assert_eq!(l[0].end_ms, Some(3_500));
    }

    #[test]
    fn current_line_follows_position_and_offset() {
        let l = synced("[00:10.00]A\n[00:20.00]B\n[00:30.00]C\n");
        assert_eq!(current_line(&l, 5_000, 0), None);
        assert_eq!(current_line(&l, 10_000, 0), Some(0));
        assert_eq!(current_line(&l, 19_999, 0), Some(0));
        assert_eq!(current_line(&l, 20_000, 0), Some(1));
        assert_eq!(current_line(&l, 999_999, 0), Some(2));
        // A positive offset delays the lyrics.
        assert_eq!(current_line(&l, 20_000, 1_000), Some(0));
        assert_eq!(current_line(&l, 21_000, 1_000), Some(1));
        // A negative one brings them forward.
        assert_eq!(current_line(&l, 9_000, -1_000), Some(0));
        assert_eq!(current_line(&[], 1_000, 0), None);
    }
}
