//! The `?` screen: every key in one list.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::App;
use crate::keys::Action;

/// Keys and what they do; also the single source for tests.
pub(super) const HELP: &[(&str, &str)] = &[
    ("S", "settings: choose keys, make them work everywhere"),
    ("E", "layout editor: move, resize, hide panels"),
    ("a", "add to a playlist"),
    ("/", "search (Enter keeps the filter, Esc clears it)"),
    ("g", "choose the GIF (Tab: playlist or global)"),
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
        let width = 64.min(screen.width);
        let height = ((HELP.len() + Action::ALL.len()) as u16 + 5).min(screen.height);
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
        for a in Action::ALL {
            let b = self.keymap.binding(a);
            let shown = if b.global {
                format!("{} (global)", b.chord.display())
            } else {
                b.chord.display()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{shown:<14}"), key),
                Span::styled(a.label(), Style::new().fg(self.theme.foreground)),
            ]));
        }
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
