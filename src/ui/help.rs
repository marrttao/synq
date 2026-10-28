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
    ("F10 / Ctrl+P", "menu: everything in one list"),
    ("S", "settings: choose keys, make them work everywhere"),
    ("E", "layout editor: move, resize, hide panels"),
    ("C", "color editor: any number of colors"),
    ("a", "add to a playlist"),
    ("/", "search (Enter keeps the filter, Esc clears it)"),
    ("g", "choose the GIF (Tab: playlist or global)"),
    ("t", "recognize lyrics again (Whisper)"),
    ("Tab", "Library / Playlists / Favorites"),
    ("Enter", "play, or open a playlist"),
    ("c / e / d", "playlists: create / rename / delete"),
    ("Esc", "back, clear the filter, leave full screen"),
    ("Ctrl+\u{2191} / \u{2193}", "scroll lyrics without timing"),
    ("? / F1", "this help (arrows scroll)"),
    ("q", "quit"),
];

impl App {
    pub(super) fn draw_help(&self, f: &mut Frame, screen: Rect, scroll: u16) {
        let width = 66.min(screen.width);
        let height = ((HELP.len() + Action::ALL.len()) as u16 + 6).min(screen.height);
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
        let text = Style::new().fg(self.theme.foreground);
        let mut lines: Vec<Line> = Vec::new();
        // What the player does comes first: these keys can be changed (S).
        for a in Action::ALL {
            let b = self.keymap.binding(a);
            let shown = if b.global {
                format!("{} (global)", b.chord.display())
            } else {
                b.chord.display()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{shown:<16}"), key),
                Span::styled(a.label(), text),
            ]));
        }
        lines.push(Line::default());
        lines.extend(HELP.iter().map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!("{k:<16}"), key),
                Span::styled(*d, text),
            ])
        }));
        // Never scroll past the end.
        let visible = height.saturating_sub(4);
        let max = (lines.len() as u16).saturating_sub(visible);
        let scroll = scroll.min(max);
        let inner =
            super::draw::popup_block(&self.theme, "Keys  (arrows scroll, any other key closes)");
        let body = inner.inner(area);
        f.render_widget(inner, area);
        let [list, foot] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Min(1),
            ratatui::layout::Constraint::Length(1),
        ])
        .areas(body);
        f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), list);
        f.render_widget(
            Paragraph::new(
                "Keys in the first block can be changed in S. Russian layout works too.",
            )
            .style(Style::new().fg(self.theme.dim)),
            foot,
        );
    }
}
