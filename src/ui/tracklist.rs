//! Track list formatting helpers.

use std::time::Duration;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListItem;

use crate::db::Track;

use super::theme::Theme;

/// "m:ss", or "h:mm:ss" for long tracks.
pub fn fmt_duration(ms: u64) -> String {
    let secs = Duration::from_millis(ms).as_secs();
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// One row: "Artist - Title      m:ss". Missing files are grey and struck out;
/// the track being played gets a marker and the accent color.
pub fn item<'a>(track: &'a Track, theme: &Theme, playing: bool, favorite: bool) -> ListItem<'a> {
    let style = if track.missing {
        Style::new()
            .fg(theme.dim)
            .add_modifier(Modifier::CROSSED_OUT)
    } else if playing {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.foreground)
    };
    let marker = if playing { "\u{25B6}" } else { " " };
    let star = if favorite { "\u{2605}" } else { " " };
    let label = format!("{marker}{star} {} - {}", track.artist, track.title);
    let time = fmt_duration(track.duration_ms);
    ListItem::new(Line::from(vec![
        Span::styled(label, style),
        Span::styled(format!("  {time}"), Style::new().fg(theme.dim)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(fmt_duration(0), "0:00");
        assert_eq!(fmt_duration(65_000), "1:05");
        assert_eq!(fmt_duration(3_725_000), "1:02:05");
    }
}
