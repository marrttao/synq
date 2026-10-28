//! The layout editor (`E`) and switching between layouts.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::layout::{Layout, Module};
use super::{App, Mode};

/// Percent per key press.
const STEP: i16 = 2;

impl App {
    /// The layout to draw: the draft while editing, else the chosen one.
    pub(super) fn active_layout(&self) -> Layout {
        match &self.mode {
            Mode::LayoutEdit { draft, .. } => draft.clone(),
            _ => self.layouts.current().clone(),
        }
    }

    pub(super) fn next_layout(&mut self) {
        match self.layouts.next() {
            Ok(name) => self.status = format!("Layout: {name}"),
            Err(e) => self.status = e,
        }
    }

    pub(super) fn open_layout_editor(&mut self) {
        let current = self.layouts.current();
        let mut draft = current.frozen(self.screen_width);
        if !current.custom {
            // Presets stay as they are: the edit becomes a new layout.
            draft.name = self.layouts.fresh_name();
            draft.custom = true;
        }
        self.mode = Mode::LayoutEdit { draft, selected: 0 };
        self.status = "Layout editor".into();
    }

    pub(super) fn key_layout_edit(&mut self, key: KeyEvent) {
        let Mode::LayoutEdit { draft, selected } = &mut self.mode else {
            return;
        };
        let module = Module::ALL[*selected % Module::ALL.len()];
        // Only arrows (and function keys below) are used, so that nothing
        // here can be mistaken for the player's own keys. Shift or Ctrl
        // with an arrow resizes instead of moving.
        let resize = key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL);
        let direction = match key.code {
            KeyCode::Left => Some((-STEP, 0, resize)),
            KeyCode::Right => Some((STEP, 0, resize)),
            KeyCode::Up => Some((0, -STEP, resize)),
            KeyCode::Down => Some((0, STEP, resize)),
            _ => None,
        };
        if let Some((dx, dy, resize)) = direction {
            match draft.place_mut(module) {
                Some(p) if resize => p.resized(dx, dy),
                Some(p) => p.moved(dx, dy),
                None => self.status = "The player bar stays at the bottom (Space: hide)".into(),
            }
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status = "Layout not saved".into();
            }
            KeyCode::Tab => *selected = (*selected + 1) % Module::ALL.len(),
            KeyCode::BackTab => {
                *selected = (*selected + Module::ALL.len() - 1) % Module::ALL.len();
            }
            KeyCode::Char('+' | '=') => {
                if let Some(p) = draft.place_mut(module) {
                    p.resized(STEP, STEP);
                }
            }
            KeyCode::Char('-') => {
                if let Some(p) = draft.place_mut(module) {
                    p.resized(-STEP, -STEP);
                }
            }
            KeyCode::Char(' ') => match draft.place_mut(module) {
                Some(p) => p.visible = !p.visible,
                None => draft.player = !draft.player,
            },
            KeyCode::Enter | KeyCode::F(2) => self.finish_layout_edit(false),
            KeyCode::F(3) => self.finish_layout_edit(true),
            KeyCode::F(4) => {
                let name = draft.name.clone();
                self.mode = Mode::Normal;
                self.status = match self.layouts.delete(&name) {
                    Ok(()) => format!("Deleted layout {name}"),
                    Err(_) if !self.layouts.exists_custom(&name) => "Layout not saved".into(),
                    Err(e) => e,
                };
            }
            _ => {}
        }
    }

    /// Save the draft; `as_new` keeps the old layout and adds another.
    fn finish_layout_edit(&mut self, as_new: bool) {
        let Mode::LayoutEdit { draft, .. } = &self.mode else {
            return;
        };
        let mut layout = draft.clone();
        if as_new {
            layout.name = self.layouts.fresh_name();
        }
        let name = layout.name.clone();
        self.mode = Mode::Normal;
        self.status = match self.layouts.save(layout) {
            Ok(()) => format!("Saved layout {name}"),
            Err(e) => e,
        };
    }

    /// Frame around the selected panel and the key hints.
    pub(super) fn draw_layout_editor(&self, f: &mut Frame, screen: Rect, main: Rect) {
        let Mode::LayoutEdit { draft, selected } = &self.mode else {
            return;
        };
        let module = Module::ALL[*selected % Module::ALL.len()];
        let accent = Style::new().fg(self.theme.accent);
        if let Some(place) = draft.place(module) {
            let rect = place.raw_rect(main);
            if rect.width >= 2 && rect.height >= 2 {
                let shown = place.visible && place.rect(main).is_some();
                if !shown {
                    // A hidden panel is shown as an empty frame to put back.
                    f.render_widget(Clear, rect);
                }
                let title = if shown {
                    format!("{} {}x{}%", module.label(), place.w, place.h)
                } else {
                    format!("{} (hidden: Space)", module.label())
                };
                f.render_widget(
                    Block::new()
                        .borders(Borders::ALL)
                        .border_style(accent.add_modifier(Modifier::BOLD))
                        .title(title)
                        .title_style(accent),
                    rect,
                );
            }
        }
        let height = 5.min(screen.height);
        let area = Rect {
            y: screen.y + screen.height - height,
            height,
            ..screen
        };
        f.render_widget(Clear, area);
        let player = if draft.player { "shown" } else { "hidden" };
        let text = vec![
            Line::from(format!(
                "Tab: panel ({})   arrows: move   Shift+arrows: resize   +/-: scale   Space: show/hide",
                module.label()
            )),
            Line::from(format!(
                "Enter: save   F3: save as new   F4: delete   Esc: cancel   (player bar {player})"
            )),
        ];
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: true })
                .style(Style::new().fg(self.theme.foreground))
                .block(super::draw::popup_block(
                    &self.theme,
                    &format!("Layout \"{}\"", draft.name),
                )),
            area,
        );
    }
}
