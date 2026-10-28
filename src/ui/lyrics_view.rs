//! Lyrics panel: state, events from the lyrics thread, key actions, drawing.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::lyrics::lrc::current_line;
use crate::lyrics::{self, Lyrics, Source};

use super::App;

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
    fn request_lyrics(&mut self, id: i64, retry: bool) -> bool {
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
                    track_id, problem, ..
                } if Some(track_id) == self.lyrics_for => {
                    if let Some(p) = &problem {
                        self.status = format!("Lyrics lookup failed: {p}");
                    }
                    self.lyrics_state = LyricsState::NotFound { problem };
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
        let block = self.block(&title);
        let inner = block.inner(area);
        f.render_widget(block, area);
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
                let mut lines = vec![Line::styled("No lyrics found", dim)];
                if let Some(p) = problem {
                    lines.push(Line::styled(format!("({p})"), dim));
                }
                lines.push(Line::styled("Press t to search again", dim));
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
            } => {
                let total = self
                    .now_playing()
                    .map_or(Duration::ZERO, |t| Duration::from_millis(t.duration_ms));
                let pos_ms = self.player.current_position(total).as_millis() as u64;
                let current = current_line(lines, pos_ms, *offset_ms);
                let width = inner.width as usize;
                let rows: Vec<Line> = window(current, lines.len(), inner.height as usize)
                    .into_iter()
                    .map(|slot| match slot {
                        None => Line::raw(""),
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
                                    .fg(self.theme.accent)
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
}
