//! The color editor (`C`) and switching palettes (`T`). While editing, the
//! whole player is drawn in the draft's colors.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::palette::{MAX_COLORS, MIN_COLORS, Palette, hex, role};
use super::theme::parse_hex;
use super::{App, Mode};

impl App {
    /// `T`: the next palette (built-in ones, then yours).
    pub(super) fn next_theme(&mut self) {
        match self.palettes.next() {
            Ok(p) => {
                self.theme = p.theme();
                self.status = format!("Colors: {} ({} colors)", p.name, p.colors.len());
            }
            Err(e) => self.status = e,
        }
    }

    pub(super) fn open_palette_editor(&mut self) {
        let current = self.palettes.current();
        let mut draft = current.clone();
        if !current.custom {
            // Built-in sets stay as they are: the edit becomes a new palette.
            draft.name = self.palettes.fresh_name();
            draft.custom = true;
        }
        self.mode = Mode::PaletteEdit {
            draft,
            selected: 0,
            typing: None,
        };
        self.status = "Color editor".into();
    }

    pub(super) fn key_palette_edit(&mut self, key: KeyEvent) {
        let Mode::PaletteEdit {
            draft,
            selected,
            typing,
        } = &mut self.mode
        else {
            return;
        };
        let last = draft.colors.len() - 1;
        *selected = (*selected).min(last);

        if let Some(text) = typing {
            match key.code {
                KeyCode::Esc => *typing = None,
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) if (c.is_ascii_hexdigit() || c == '#') && text.len() < 7 => {
                    text.push(c);
                }
                KeyCode::Enter => match parse_hex(text) {
                    Some(Color::Rgb(r, g, b)) => {
                        draft.colors[*selected] = [r, g, b];
                        *typing = None;
                        self.theme = draft.theme();
                    }
                    _ => self.status = "Type a color as 6 hex digits, e.g. ff79c6".into(),
                },
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.theme = self.palettes.current().theme();
                self.status = "Colors not saved".into();
                return;
            }
            KeyCode::Down | KeyCode::Tab => {
                *selected = (*selected + 1).min(last);
            }
            KeyCode::Up | KeyCode::BackTab => {
                *selected = selected.saturating_sub(1);
            }
            KeyCode::Enter => {
                *typing = Some(
                    hex(draft.colors[*selected])
                        .trim_start_matches('#')
                        .to_owned(),
                );
            }
            KeyCode::Char('+' | '=') => {
                if draft.colors.len() < MAX_COLORS {
                    let from = draft.colors[last];
                    draft.colors.push(from);
                    *selected = last + 1;
                    self.status = format!("{} colors", draft.colors.len());
                } else {
                    self.status = format!("At most {MAX_COLORS} colors");
                }
            }
            KeyCode::Char('-') | KeyCode::Delete => {
                if draft.colors.len() > MIN_COLORS {
                    draft.colors.remove(*selected);
                    self.status = format!("{} colors", draft.colors.len());
                } else {
                    self.status = format!("At least {MIN_COLORS} colors");
                }
            }
            KeyCode::F(2) => return self.finish_palette_edit(false),
            KeyCode::F(3) => return self.finish_palette_edit(true),
            KeyCode::F(4) => {
                let name = draft.name.clone();
                self.mode = Mode::Normal;
                self.status = match self.palettes.delete(&name) {
                    Ok(()) => format!("Deleted palette {name}"),
                    Err(_) if !self.palettes.exists_custom(&name) => "Colors not saved".into(),
                    Err(e) => e,
                };
                self.theme = self.palettes.current().theme();
                return;
            }
            _ => {}
        }
        // Every change is visible at once.
        if let Mode::PaletteEdit { draft, .. } = &self.mode {
            self.theme = draft.theme();
        }
    }

    fn finish_palette_edit(&mut self, as_new: bool) {
        let Mode::PaletteEdit { draft, .. } = &self.mode else {
            return;
        };
        let mut palette: Palette = draft.clone();
        if as_new {
            palette.name = self.palettes.fresh_name();
        }
        let name = palette.name.clone();
        self.mode = Mode::Normal;
        self.status = match self.palettes.save(palette) {
            Ok(()) => format!("Saved palette {name}"),
            Err(e) => e,
        };
        self.theme = self.palettes.current().theme();
    }

    /// A window at the right edge, so that the player stays visible.
    pub(super) fn draw_palette_editor(&self, f: &mut Frame, screen: Rect) {
        let Mode::PaletteEdit {
            draft,
            selected,
            typing,
        } = &self.mode
        else {
            return;
        };
        let width = 40.min(screen.width);
        let height = (draft.colors.len() as u16 + 7).min(screen.height);
        let area = Rect {
            x: screen.x + screen.width - width,
            y: screen.y + (screen.height - height) / 2,
            width,
            height,
        };
        f.render_widget(Clear, area);
        let mut lines: Vec<Line> = draft
            .colors
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let shown = match (typing, i == *selected) {
                    (Some(text), true) => format!("#{}\u{2581}", text.trim_start_matches('#')),
                    _ => hex(*c),
                };
                let row = Style::new().fg(self.theme.foreground);
                let row = if i == *selected {
                    row.fg(self.theme.accent).add_modifier(Modifier::BOLD)
                } else {
                    row
                };
                Line::from(vec![
                    Span::styled(format!("{:<2}{:<12}{shown:<10}", i + 1, role(i)), row),
                    Span::styled("   ", Style::new().bg(Color::Rgb(c[0], c[1], c[2]))),
                ])
            })
            .collect();
        lines.push(Line::default());
        let hint = if typing.is_some() {
            "Type 6 hex digits, Enter: set, Esc: back"
        } else {
            "Enter: type a color  +: add  -: remove  F2: save  F3: save as new  F4: delete  Esc: cancel"
        };
        lines.push(Line::styled(hint, Style::new().fg(self.theme.dim)));
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: true })
                .block(super::draw::popup_block(
                    &self.theme,
                    &format!("Colors \"{}\" ({})", draft.name, draft.colors.len()),
                )),
            area,
        );
    }
}
