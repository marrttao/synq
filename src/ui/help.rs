//! The `?` screen: every key in one list.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::App;

/// Keys and what they do; also the single source for tests.
pub(super) const HELP: &[(&str, &str)] = &[
    ("Space", "pause / play (starts the selected track)"),
    ("n / p", "next / previous track"),
    ("\u{2190} / \u{2192}", "seek back / forward"),
    ("+ / -", "volume"),
    ("s", "shuffle on / off"),
    ("r", "repeat: off / all / one"),
    ("f", "add to / remove from favorites"),
    ("a", "add to a playlist"),
    ("/", "search (Enter keeps the filter, Esc clears it)"),
    ("l", "show / hide lyrics"),
    ("g", "choose the GIF (Tab: playlist or global)"),
    ("F", "full screen: lyrics and picture only"),
    ("[ / ]", "lyrics earlier / later by 0.1 s"),
    ("t", "recognize lyrics again (Whisper)"),
    ("T", "next color theme"),
    ("Tab", "Library / Playlists / Favorites"),
    ("Enter", "play, or open a playlist"),
    ("c / e / d", "playlists: create / rename / delete"),
    ("Esc", "back, clear the filter, leave full screen"),
    ("Ctrl+\u{2191} / \u{2193}", "scroll lyrics without timing"),
    ("?", "this help"),
    ("q", "quit"),
];

impl App {
    pub(super) fn draw_help(&self, f: &mut Frame, screen: Rect) {
        let width = 62.min(screen.width);
        let height = (HELP.len() as u16 + 4).min(screen.height);
        let area = Rect {
            x: screen.x + (screen.width - width) / 2,
            y: screen.y + (screen.height - height) / 2,
            width,
            height,
        };
        f.render_widget(Clear, area);
        let key = Style::new()
            .fg(self.theme.accent)
            .add_modifier(Modifier::BOLD);
        let mut lines: Vec<Line> = HELP
            .iter()
            .map(|(k, d)| {
                Line::from(vec![
                    Span::styled(format!("{k:<14}"), key),
                    Span::styled(*d, Style::new().fg(self.theme.foreground)),
                ])
            })
            .collect();
        lines.push(Line::default());
        lines.push(Line::styled(
            "Any key closes this window",
            Style::new().fg(self.theme.dim),
        ));
        f.render_widget(
            Paragraph::new(lines).block(super::draw::popup_block(&self.theme, "Keys")),
            area,
        );
    }
}
