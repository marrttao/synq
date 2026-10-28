//! Lyrics panel: state, events from the lyrics thread, key actions, drawing.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::lyrics::lrc::{self, current_line};
use crate::lyrics::{self, Lyrics, Source};

use super::{App, bigtext};

/// Offset step for `[` and `]`.
const OFFSET_STEP_MS: i64 = 100;

pub(super) enum LyricsState {
    /// Nothing is playing.
    Idle,
    Searching,
    NotFound {
        /// Why the lookup failed (e.g. offline), if it did.
        problem: Option<String>,
    },
    Ready {
        lyrics: Lyrics,
        source: Source,
        /// Positive = lyrics shown later.
        offset_ms: i64,
        /// First visible line of plain lyrics.
        scroll: usize,
    },
}

/// A dark backdrop for text lying over a picture: whatever is already in
/// `area` is mixed with the background color, `percent` of the way (100
/// hides it completely, 0 leaves it alone).
/// How the lyrics are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LyricsMode {
    /// The lines around the current one, the current one highlighted.
    Full,
    /// Only the word that is being sung, in big letters.
    Word,
}

impl LyricsMode {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Word => "word",
        }
    }

    pub(super) fn from_config(name: &str) -> Self {
        if name.trim().eq_ignore_ascii_case("word") {
            Self::Word
        } else {
            Self::Full
        }
    }

    pub(super) fn cycle(self) -> Self {
        match self {
            Self::Full => Self::Word,
            Self::Word => Self::Full,
        }
    }
}

fn darken_behind(buf: &mut ratatui::buffer::Buffer, area: Rect, bg: Color, percent: u8) {
    let Color::Rgb(br, bgc, bb) = bg else { return };
    let mix = |c: Color| match c {
        Color::Rgb(r, g, b) => {
            let m = |x: u8, y: u8| {
                ((u32::from(x) * (100 - u32::from(percent)) + u32::from(y) * u32::from(percent))
                    / 100) as u8
            };
            Color::Rgb(m(r, br), m(g, bgc), m(b, bb))
        }
        _ => bg,
    };
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if percent >= 100 {
                cell.set_char(' ');
            }
            let (fg, cbg) = (mix(cell.fg), mix(cell.bg));
            cell.set_fg(fg).set_bg(cbg);
        }
    }
}

impl App {
    /// Ask for lyrics when the playing track changes.
    pub(super) fn sync_lyrics_request(&mut self) {
        if self.player.now_id == self.lyrics_for {
            return;
        }
        let Some(id) = self.player.now_id else {
            self.lyrics_for = None;
            self.lyrics_state = LyricsState::Idle;
            return;
        };
        // The library may not have loaded yet; try again next frame.
        if self.request_lyrics(id, false) {
            self.lyrics_for = Some(id);
        }
    }

    /// Send a lookup for track `id`; false if the track is unknown.
    pub(super) fn request_lyrics(&mut self, id: i64, retry: bool) -> bool {
        let Some(&index) = self.track_index.get(&id) else {
            return false;
        };
        let t = &self.tracks[index];
        let request = lyrics::Request {
            track_id: t.id,
            rel_path: t.path.clone(),
            audio_path: self.music_dir.join(&t.path),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration_secs: t.duration_ms / 1000,
            retry,
        };
        self.lyrics.send(lyrics::Command::Load(request));
        self.lyrics_state = LyricsState::Searching;
        true
    }

    pub(super) fn drain_lyrics_events(&mut self) {
        while let Ok(ev) = self.lyrics.events.try_recv() {
            match ev {
                lyrics::Event::Searching => {}
                lyrics::Event::Found {
                    track_id,
                    lyrics,
                    source,
                    offset_ms,
                } if Some(track_id) == self.lyrics_for => {
                    self.lyrics_state = LyricsState::Ready {
                        lyrics,
                        source,
                        offset_ms,
                        scroll: 0,
                    };
                }
                lyrics::Event::NotFound {
                    track_id,
                    problem,
                    no_speech,
                } if Some(track_id) == self.lyrics_for => {
                    if let Some(p) = &problem {
                        self.status = format!("Lyrics lookup failed: {p}");
                    }
                    self.lyrics_state = LyricsState::NotFound { problem };
                    // Nothing anywhere: let Whisper try (unless it already did).
                    if !no_speech {
                        self.schedule_auto_transcription(track_id);
                    }
                }
                // Answer for a track that is no longer playing.
                _ => {}
            }
        }
    }

    /// `t`: look again, ignoring "not found" answers cached earlier.
    pub(super) fn retry_lyrics(&mut self) {
        let id = self.player.now_id;
        if id.is_some_and(|id| self.request_lyrics(id, true)) {
            self.status = "Searching for lyrics again...".into();
        } else {
            self.status = "Nothing is playing".into();
        }
    }

    /// `[` / `]`: shift timing of synchronized lyrics and remember it.
    pub(super) fn shift_lyrics(&mut self, later: bool) {
        let LyricsState::Ready {
            lyrics: Lyrics::Synced(_),
            offset_ms,
            ..
        } = &mut self.lyrics_state
        else {
            self.status = "Timing can only be shifted for synchronized lyrics".into();
            return;
        };
        *offset_ms += if later {
            OFFSET_STEP_MS
        } else {
            -OFFSET_STEP_MS
        };
        let ms = *offset_ms;
        self.status = format!("Lyrics offset {}", format_offset(ms));
        if let Some(rel_path) = self.now_playing().map(|t| t.path.clone()) {
            self.lyrics
                .send(lyrics::Command::SetOffset { rel_path, ms });
        }
    }

    /// Ctrl+Up / Ctrl+Down: scroll plain (untimed) lyrics.
    pub(super) fn scroll_lyrics(&mut self, down: bool) {
        if let LyricsState::Ready {
            lyrics: Lyrics::Plain(lines),
            scroll,
            ..
        } = &mut self.lyrics_state
        {
            *scroll = if down {
                (*scroll + 1).min(lines.len().saturating_sub(1))
            } else {
                scroll.saturating_sub(1)
            };
        }
    }

    /// Index of the line being sung now (synchronized lyrics only).
    pub(super) fn current_line_index(&self) -> Option<usize> {
        let LyricsState::Ready {
            lyrics: Lyrics::Synced(lines),
            offset_ms,
            ..
        } = &self.lyrics_state
        else {
            return None;
        };
        current_line(lines, self.position_ms(), *offset_ms)
    }

    fn position_ms(&self) -> u64 {
        let total = self
            .now_playing()
            .map_or(Duration::ZERO, |t| Duration::from_millis(t.duration_ms));
        self.player.current_position(total).as_millis() as u64
    }

    pub(super) fn draw_lyrics(&self, f: &mut Frame, area: Rect) {
        let title = match (&self.lyrics_state, self.show_lyrics) {
            (
                LyricsState::Ready {
                    source,
                    offset_ms,
                    lyrics,
                    ..
                },
                true,
            ) => match lyrics {
                Lyrics::Synced(_) if *offset_ms != 0 => {
                    format!(
                        "Lyrics | {} | {}",
                        source.label(),
                        format_offset(*offset_ms)
                    )
                }
                Lyrics::Synced(_) => format!("Lyrics | {}", source.label()),
                Lyrics::Plain(_) => {
                    format!("Lyrics | {} | not synced, Ctrl+Up/Down", source.label())
                }
            },
            _ => "Lyrics".to_owned(),
        };
        darken_behind(
            f.buffer_mut(),
            area,
            self.theme.background,
            self.lyrics_backdrop,
        );
        let block = self.block_n(&title, 2);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.fx.lyrics_inner.set(inner);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let dim = Style::new().fg(self.theme.dim);
        let message = |text: &str| {
            Paragraph::new(vec![Line::styled(text.to_owned(), dim)]).alignment(Alignment::Center)
        };
        // Put a short message in the vertical middle.
        let middle = Rect {
            y: inner.y + inner.height / 2,
            height: 1,
            ..inner
        };

        if !self.show_lyrics {
            f.render_widget(message("Lyrics hidden (press l)"), middle);
            return;
        }
        match &self.lyrics_state {
            LyricsState::Idle => f.render_widget(message("Nothing playing"), middle),
            LyricsState::Searching => {
                f.render_widget(message("Looking for lyrics..."), middle);
            }
            LyricsState::NotFound { problem } => {
                let mut lines = Vec::new();
                if let Some(text) = self.whisper_panel_text() {
                    lines.push(Line::styled(text, dim));
                } else {
                    lines.push(Line::styled("No lyrics found", dim));
                    if let Some(p) = problem {
                        lines.push(Line::styled(format!("({p})"), dim));
                    }
                    lines.push(Line::styled(
                        if self.whisper_enabled {
                            "Press t to recognize them with Whisper"
                        } else {
                            "Press t to search again"
                        },
                        dim,
                    ));
                }
                let area = Rect {
                    y: inner.y + inner.height.saturating_sub(lines.len() as u16) / 2,
                    height: (lines.len() as u16).min(inner.height),
                    ..inner
                };
                f.render_widget(
                    Paragraph::new(lines)
                        .alignment(Alignment::Center)
                        .wrap(Wrap { trim: true }),
                    area,
                );
            }
            LyricsState::Ready {
                lyrics: Lyrics::Plain(lines),
                scroll,
                ..
            } => {
                let text: Vec<Line> = lines
                    .iter()
                    .skip(*scroll)
                    .map(|l| Line::styled(l.clone(), Style::new().fg(self.theme.foreground)))
                    .collect();
                f.render_widget(
                    Paragraph::new(text)
                        .alignment(Alignment::Center)
                        .wrap(Wrap { trim: false }),
                    inner,
                );
            }
            LyricsState::Ready {
                lyrics: Lyrics::Synced(lines),
                offset_ms,
                ..
            } if self.lyrics_mode == LyricsMode::Word => {
                let word = lrc::current_word(lines, self.position_ms(), *offset_ms);
                match word {
                    Some(w) => {
                        let color = self.theme.rotating(w.line * 3 + w.index);
                        if let Some(cov) =
                            bigtext::render(self.word_font, &w.text, inner.width, inner.height)
                        {
                            bigtext::paint(
                                f.buffer_mut(),
                                inner,
                                &cov,
                                color,
                                self.theme.background,
                            );
                        }
                    }
                    // Between lines, or in an instrumental part.
                    None => f.render_widget(message("\u{266A}"), middle),
                }
            }
            LyricsState::Ready {
                lyrics: Lyrics::Synced(lines),
                offset_ms,
                ..
            } => {
                let pos_ms = self.position_ms();
                let current = current_line(lines, pos_ms, *offset_ms);
                let width = inner.width as usize;
                let rows: Vec<Line> = window(current, lines.len(), inner.height as usize)
                    .into_iter()
                    .map(|slot| match slot {
                        None => Line::raw(""),
                        Some((i, true)) if !lines[i].words.is_empty() => {
                            let pos = pos_ms as i64 - *offset_ms;
                            let sung = Style::new()
                                .fg(self.theme.rotating(i))
                                .add_modifier(Modifier::BOLD);
                            let waiting = Style::new().fg(self.theme.foreground);
                            Line::from(karaoke(&lines[i].words, pos, width, sung, waiting))
                        }
                        Some((i, is_current)) => {
                            let text = if lines[i].text.is_empty() {
                                if is_current {
                                    "\u{266A}".to_owned()
                                } else {
                                    String::new()
                                }
                            } else {
                                fit(&lines[i].text, width)
                            };
                            let style = if is_current {
                                Style::new()
                                    .fg(self.theme.rotating(i))
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                dim
                            };
                            Line::from(Span::styled(text, style))
                        }
                    })
                    .collect();
                f.render_widget(Paragraph::new(rows).alignment(Alignment::Center), inner);
            }
        }
    }
}

/// The current line split by words: words already reached are `sung`, the
/// rest `waiting`. Cut with an ellipsis if wider than `width` columns.
fn karaoke(
    words: &[lrc::Word],
    pos_ms: i64,
    width: usize,
    sung: Style,
    waiting: Style,
) -> Vec<Span<'static>> {
    let columns = |s: &str| Span::raw(s.to_owned()).width();
    let total: usize = words.iter().map(|w| columns(&w.text)).sum();
    // Leave room for the ellipsis when the line has to be shortened.
    let limit = if total > width {
        width.saturating_sub(1)
    } else {
        width
    };
    let mut spans = Vec::new();
    let mut used = 0;
    for w in words {
        let style = if w.time_ms as i64 <= pos_ms {
            sung
        } else {
            waiting
        };
        let mut text = String::new();
        for c in w.text.chars() {
            let cw = columns(c.encode_utf8(&mut [0; 4]));
            if used + cw > limit {
                break;
            }
            used += cw;
            text.push(c);
        }
        let cut = text.chars().count() < w.text.chars().count();
        if !text.is_empty() {
            spans.push(Span::styled(text, style));
        }
        if cut {
            break;
        }
    }
    if total > width {
        spans.push(Span::styled("\u{2026}", waiting));
    }
    spans
}

/// Which line goes on which row, with the current line in the middle row.
/// `None` marks an empty row. Before the first line starts (`current` is
/// `None`) the middle row is empty and the first lines wait below it.
fn window(current: Option<usize>, len: usize, rows: usize) -> Vec<Option<(usize, bool)>> {
    let mid = rows / 2;
    let cur = current.map_or(-1, |c| c as isize);
    (0..rows)
        .map(|row| {
            let i = cur + row as isize - mid as isize;
            (i >= 0 && (i as usize) < len).then_some((i as usize, i == cur))
        })
        .collect()
}

/// Shorten `text` to `width` terminal columns, ending with an ellipsis.
fn fit(text: &str, width: usize) -> String {
    let columns = |s: &str| Span::raw(s.to_owned()).width();
    if columns(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    for c in text.chars() {
        let mut next = out.clone();
        next.push(c);
        if columns(&next) + 1 > width {
            break;
        }
        out = next;
    }
    out.push('\u{2026}');
    out
}

/// "+0.3 s" / "-1.2 s" / "0.0 s".
fn format_offset(ms: i64) -> String {
    let sign = if ms > 0 { "+" } else { "" };
    format!("{sign}{:.1} s", ms as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_line_sits_in_the_middle_row() {
        let w = window(Some(5), 10, 5);
        assert_eq!(
            w,
            [
                Some((3, false)),
                Some((4, false)),
                Some((5, true)),
                Some((6, false)),
                Some((7, false))
            ]
        );
    }

    #[test]
    fn window_is_padded_at_both_ends() {
        let w = window(Some(0), 2, 5);
        assert_eq!(w, [None, None, Some((0, true)), Some((1, false)), None]);
        let w = window(Some(1), 2, 5);
        assert_eq!(w, [None, Some((0, false)), Some((1, true)), None, None]);
    }

    #[test]
    fn before_the_first_line_upcoming_lines_wait_below() {
        let w = window(None, 3, 5);
        assert_eq!(w, [None, None, None, Some((0, false)), Some((1, false))]);
    }

    #[test]
    fn window_handles_degenerate_sizes() {
        assert!(window(Some(0), 3, 0).is_empty());
        assert_eq!(window(Some(0), 0, 3), [None, None, None]);
        assert_eq!(window(Some(0), 1, 1), [Some((0, true))]);
    }

    #[test]
    fn long_lines_are_cut_with_an_ellipsis() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("0123456789", 10), "0123456789");
        assert_eq!(fit("0123456789ab", 10), "012345678\u{2026}");
        assert_eq!(fit("anything", 0), "\u{2026}");
        // Wide characters count as two columns.
        assert_eq!(
            fit("\u{3042}\u{3044}\u{3046}\u{3048}", 5),
            "\u{3042}\u{3044}\u{2026}"
        );
    }

    #[test]
    fn offsets_are_formatted_with_a_sign() {
        assert_eq!(format_offset(300), "+0.3 s");
        assert_eq!(format_offset(-1200), "-1.2 s");
        assert_eq!(format_offset(0), "0.0 s");
    }

    #[test]
    fn karaoke_colors_words_as_they_are_reached() {
        let w = |t: u64, s: &str| lrc::Word {
            time_ms: t,
            text: s.into(),
        };
        let words = [w(1000, "one "), w(2000, "two "), w(3000, "three")];
        let sung = Style::new().fg(ratatui::style::Color::Red);
        let waiting = Style::new().fg(ratatui::style::Color::Blue);
        let spans = karaoke(&words, 2500, 80, sung, waiting);
        let styles: Vec<_> = spans.iter().map(|s| s.style).collect();
        assert_eq!(styles, [sung, sung, waiting]);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "one two three");
        assert!(
            karaoke(&words, 0, 80, sung, waiting)
                .iter()
                .all(|s| s.style == waiting)
        );
        assert!(
            karaoke(&words, 9999, 80, sung, waiting)
                .iter()
                .all(|s| s.style == sung)
        );
    }

    #[test]
    fn karaoke_line_is_shortened_to_the_width() {
        let w = |t: u64, s: &str| lrc::Word {
            time_ms: t,
            text: s.into(),
        };
        let words = [w(0, "hello "), w(10, "wonderful "), w(20, "world")];
        let st = Style::new();
        for width in [1, 5, 8, 12, 25, 80] {
            let spans = karaoke(&words, 15, width, st, st);
            let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                Span::raw(text.clone()).width() <= width,
                "{width}: {text:?}"
            );
        }
        let text: String = karaoke(&words, 15, 12, st, st)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(text, "hello wonde\u{2026}");
    }
}
