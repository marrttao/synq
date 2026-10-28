//! The menu (F10 or Ctrl+P): every tool and every action in one list, with
//! the key that does it, so that nothing has to be remembered. Also the
//! keys of the scrolling help window.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState};

use crate::keys::Action;

use super::{App, Mode};

#[derive(Clone, Copy)]
enum Entry {
    Settings,
    Layouts,
    Colors,
    Gif,
    Help,
    Quit,
    Do(Action),
}

impl App {
    /// Label, key and what to do, for every row of the menu.
    fn menu_rows(&self) -> Vec<(String, String, Entry)> {
        let mut rows = vec![
            (
                "Choose keys (and make them global)".to_owned(),
                "S".to_owned(),
                Entry::Settings,
            ),
            (
                "Edit the screen layout".to_owned(),
                "E".to_owned(),
                Entry::Layouts,
            ),
            ("Edit the colors".to_owned(), "C".to_owned(), Entry::Colors),
            (
                "Choose the picture / GIF".to_owned(),
                "g".to_owned(),
                Entry::Gif,
            ),
            (
                "Help: all keys".to_owned(),
                "? / F1".to_owned(),
                Entry::Help,
            ),
        ];
        rows.extend(Action::ALL.iter().map(|&a| {
            let b = self.keymap.binding(a);
            let key = if b.global {
                format!("{} (global)", b.chord.display())
            } else {
                b.chord.display()
            };
            (a.label().to_owned(), key, Entry::Do(a))
        }));
        rows.push(("Quit".to_owned(), "q".to_owned(), Entry::Quit));
        rows
    }

    pub(super) fn open_menu(&mut self) {
        let mut state = ListState::default();
        state.select(Some(0));
        self.mode = Mode::Menu { state };
    }

    pub(super) fn key_menu(&mut self, key: KeyEvent) {
        let rows = self.menu_rows();
        let Mode::Menu { state } = &mut self.mode else {
            return;
        };
        let row = state.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::F(10) => self.mode = Mode::Normal,
            KeyCode::Down => state.select(Some((row + 1).min(rows.len() - 1))),
            KeyCode::Up => state.select(Some(row.saturating_sub(1))),
            KeyCode::PageDown => state.select(Some((row + 10).min(rows.len() - 1))),
            KeyCode::PageUp => state.select(Some(row.saturating_sub(10))),
            KeyCode::Home => state.select(Some(0)),
            KeyCode::End => state.select(Some(rows.len() - 1)),
            KeyCode::Enter => {
                let entry = rows.get(row).map(|r| r.2);
                self.mode = Mode::Normal;
                match entry {
                    Some(Entry::Settings) => self.open_settings(),
                    Some(Entry::Layouts) => self.open_layout_editor(),
                    Some(Entry::Colors) => self.open_palette_editor(),
                    Some(Entry::Gif) => self.start_gif_pick(),
                    Some(Entry::Help) => self.mode = Mode::Help { scroll: 0 },
                    Some(Entry::Quit) => self.should_quit = true,
                    Some(Entry::Do(a)) => self.run_action(a),
                    None => {}
                }
            }
            _ => {}
        }
    }

    pub(super) fn draw_menu(&self, f: &mut Frame, screen: Rect, state: &mut ListState) {
        let rows = self.menu_rows();
        let height = (rows.len() as u16 + 4).min(screen.height);
        let width = 60.min(screen.width);
        let area = Rect {
            x: screen.x + (screen.width - width) / 2,
            y: screen.y + (screen.height - height) / 2,
            width,
            height,
        };
        f.render_widget(Clear, area);
        let label_width = usize::from(width).saturating_sub(24).max(10);
        let items: Vec<ListItem> = rows
            .iter()
            .map(|(label, key, _)| {
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{label:<label_width$}")),
                    Span::styled(key.clone(), Style::new().fg(self.theme.dim)),
                ]))
            })
            .collect();
        f.render_stateful_widget(
            List::new(items)
                .block(super::draw::popup_block(
                    &self.theme,
                    "Menu  (Up/Down, Enter: do it, Esc: close)",
                ))
                .highlight_style(
                    Style::new()
                        .bg(self.theme.accent)
                        .fg(self.theme.background)
                        .add_modifier(Modifier::BOLD),
                ),
            area,
            state,
        );
    }

    /// Arrows scroll the help; any other key closes it.
    pub(super) fn key_help(&mut self, key: KeyEvent) {
        let Mode::Help { scroll } = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Down => *scroll = scroll.saturating_add(1),
            KeyCode::Up => *scroll = scroll.saturating_sub(1),
            KeyCode::PageDown => *scroll = scroll.saturating_add(10),
            KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
            KeyCode::Home => *scroll = 0,
            KeyCode::End => *scroll = u16::MAX,
            _ => self.mode = Mode::Normal,
        }
    }
}
