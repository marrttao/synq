//! The layout editor (`E`) and switching between layouts.
//!
//! While editing, the screen shows the panels as labelled boxes (not their
//! contents), so it is obvious which is which. They can be dragged with the
//! mouse or moved with the keyboard.

use ratatui::Frame;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use super::layout::{Layout, Module, Place, presets};
use super::{App, Mode};

/// Percent per key press.
const STEP: i16 = 5;
/// How close to the bottom-right corner a click grabs the resize handle.
const CORNER_COLS: u16 = 3;
const CORNER_ROWS: u16 = 2;

/// Library, Lyrics, Visual from bottom to top as drawn on the screen:
/// the picture is at the bottom, so lyrics can be placed over it.
const Z_ORDER: [usize; 3] = [2, 0, 1];

/// A mouse drag in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Drag {
    module: usize,
    resize: bool,
    from: (u16, u16),
    orig: Place,
}

impl App {
    /// The layout to draw: the draft while editing, else the chosen one.
    pub(super) fn active_layout(&self) -> Layout {
        match &self.mode {
            Mode::LayoutEdit { draft, .. } => draft.clone(),
            _ => self.layouts.current().clone(),
        }
    }

    pub(super) fn step_layout(&mut self, forward: bool) {
        match self.layouts.step(forward) {
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
        self.mode = Mode::LayoutEdit {
            draft,
            selected: 0,
            drag: None,
        };
        self.set_mouse(true);
        self.status = "Layout editor".into();
    }

    fn close_layout_editor(&mut self) {
        self.mode = Mode::Normal;
        self.set_mouse(false);
    }

    /// The mouse is only captured while editing, so that text can be
    /// selected in the terminal the rest of the time.
    fn set_mouse(&self, on: bool) {
        use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
        if !self.real_terminal {
            return;
        }
        let result = if on {
            ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture)
        } else {
            ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture)
        };
        if let Err(e) = result {
            tracing::warn!("mouse capture: {e}");
        }
    }

    pub(super) fn key_layout_edit(&mut self, key: KeyEvent) {
        let width = self.screen_width;
        let Mode::LayoutEdit {
            draft, selected, ..
        } = &mut self.mode
        else {
            return;
        };
        let module = Module::ALL[*selected % Module::ALL.len()];
        // Shift or Ctrl with an arrow resizes instead of moving.
        let resize = key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL);
        let direction = match key.code {
            KeyCode::Left => Some((-STEP, 0)),
            KeyCode::Right => Some((STEP, 0)),
            KeyCode::Up => Some((0, -STEP)),
            KeyCode::Down => Some((0, STEP)),
            _ => None,
        };
        if let Some((dx, dy)) = direction {
            match draft.place_mut(module) {
                Some(p) if resize => p.resized(dx, dy),
                Some(p) => p.moved(dx, dy),
                None => self.status = "The player bar stays at the bottom (Space: hide)".into(),
            }
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.close_layout_editor();
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
            // 1, 2, 3...: start again from one of the built-in layouts.
            KeyCode::Char(c @ '1'..='9') => {
                let n = usize::from(c as u8 - b'1');
                if let Some(preset) = presets().get(n) {
                    let fixed = preset.frozen(width);
                    draft.places = fixed.places;
                    draft.player = fixed.player;
                    self.status = format!("Started from {}", preset.name);
                }
            }
            KeyCode::Enter | KeyCode::F(2) => self.finish_layout_edit(false),
            KeyCode::F(3) => self.finish_layout_edit(true),
            KeyCode::F(4) => {
                let name = draft.name.clone();
                self.close_layout_editor();
                self.status = match self.layouts.delete(&name) {
                    Ok(()) => format!("Deleted layout {name}"),
                    Err(_) if !self.layouts.exists_custom(&name) => "Layout not saved".into(),
                    Err(e) => e,
                };
            }
            _ => {}
        }
    }

    /// Drag a panel to move it, drag its bottom-right corner to resize it.
    pub(super) fn mouse_event(&mut self, m: MouseEvent) {
        let main = self.layout_main;
        let Mode::LayoutEdit {
            draft,
            selected,
            drag,
        } = &mut self.mode
        else {
            return;
        };
        let (col, row) = (m.column, m.row);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Top first: the order they are drawn in, backwards.
                let hit = Z_ORDER.iter().rev().copied().find(|&i| {
                    draft.places[i].visible && inside(draft.places[i].raw_rect(main), col, row)
                });
                match hit {
                    Some(i) => {
                        let r = draft.places[i].raw_rect(main);
                        let corner = col + CORNER_COLS >= r.x + r.width
                            && row + CORNER_ROWS >= r.y + r.height;
                        *selected = i;
                        *drag = Some(Drag {
                            module: i,
                            resize: corner,
                            from: (col, row),
                            orig: draft.places[i],
                        });
                    }
                    None if row >= main.y + main.height => *selected = Module::Player as usize,
                    None => {}
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(d) = *drag else { return };
                let to_percent = |cells: i32, total: u16| {
                    if total == 0 {
                        0
                    } else {
                        (cells * 100 / i32::from(total)) as i16
                    }
                };
                let dx = to_percent(i32::from(col) - i32::from(d.from.0), main.width);
                let dy = to_percent(i32::from(row) - i32::from(d.from.1), main.height);
                let mut p = d.orig;
                if d.resize {
                    p.resized(dx, dy);
                } else {
                    p.moved(dx, dy);
                }
                if let Some(slot) = draft.places.get_mut(d.module) {
                    *slot = p;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => *drag = None,
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
        self.close_layout_editor();
        self.status = match self.layouts.save(layout) {
            Ok(()) => format!("Saved layout {name}"),
            Err(e) => e,
        };
    }

    /// The whole editor screen: labelled boxes and the help.
    pub(super) fn draw_layout_editor(&self, f: &mut Frame, screen: Rect, main: Rect) {
        let Mode::LayoutEdit {
            draft, selected, ..
        } = &self.mode
        else {
            return;
        };
        let theme = &self.theme;
        let dim = Style::new().fg(theme.dim);
        // The selected panel is drawn last, so that it is never covered.
        let mut order: Vec<usize> = Z_ORDER.to_vec();
        order.sort_by_key(|&i| i == *selected);
        for i in order {
            let place = draft.places[i];
            let module = Module::ALL[i];
            let rect = place.raw_rect(main);
            if rect.width < 2 || rect.height < 2 {
                continue;
            }
            let is_selected = *selected == i;
            if !place.visible && !is_selected {
                continue;
            }
            f.render_widget(Clear, rect);
            let (title, body) = if place.visible {
                (
                    format!(" {} {}x{}% ", module.label(), place.w, place.h),
                    String::from("drag me / arrows"),
                )
            } else {
                (
                    format!(" {} (hidden) ", module.label()),
                    String::from("Space: show"),
                )
            };
            let mut block = Block::new()
                .borders(Borders::ALL)
                .title(title)
                .style(Style::new().bg(theme.background));
            block = if is_selected {
                block
                    .border_type(BorderType::Thick)
                    .border_style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD))
                    .title_style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD))
            } else {
                block
                    .border_style(Style::new().fg(theme.border))
                    .title_style(Style::new().fg(theme.foreground))
            };
            let inner = block.inner(rect);
            f.render_widget(block, rect);
            if inner.height > 0 {
                let mid = Rect {
                    y: inner.y + inner.height / 2,
                    height: 1,
                    ..inner
                };
                f.render_widget(
                    Paragraph::new(vec![Line::from(module.label().to_uppercase())])
                        .alignment(Alignment::Center)
                        .style(
                            Style::new()
                                .fg(theme.foreground)
                                .add_modifier(Modifier::BOLD),
                        ),
                    mid,
                );
                if inner.height > 2 {
                    let sub = Rect {
                        y: mid.y + 1,
                        ..mid
                    };
                    f.render_widget(
                        Paragraph::new(body).alignment(Alignment::Center).style(dim),
                        sub,
                    );
                }
            }
            if is_selected && place.visible && rect.width > 4 && rect.height > 2 {
                // The resize handle.
                f.render_widget(
                    Paragraph::new("\u{25E2}").style(Style::new().fg(theme.accent)),
                    Rect {
                        x: rect.x + rect.width - 2,
                        y: rect.y + rect.height - 1,
                        width: 1,
                        height: 1,
                    },
                );
            }
        }

        let height = if draft.player { 5 } else { 4 }.min(screen.height);
        let area = Rect {
            y: screen.y + screen.height - height,
            height,
            ..screen
        };
        f.render_widget(Clear, area);
        let module = Module::ALL[*selected % Module::ALL.len()];
        let now = match draft.place(module) {
            Some(p) => format!(
                "{} {}x{}% at {},{}{}",
                module.label(),
                p.w,
                p.h,
                p.x,
                p.y,
                if p.visible { "" } else { " (hidden)" }
            ),
            None => format!(
                "Player bar ({})",
                if draft.player { "shown" } else { "hidden" }
            ),
        };
        let text = vec![
            Line::from(format!(
                "Selected: {now}.  Drag a panel with the mouse; drag its corner to resize."
            )),
            Line::from(
                "Tab: next panel | arrows: move | Shift+arrows: resize | +/-: scale | Space: hide/show | 1-3: start from a preset",
            ),
            Line::from("Enter: save | F3: save as new | F4: delete | Esc: cancel"),
        ];
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: true })
                .style(Style::new().fg(theme.foreground))
                .block(super::draw::popup_block(
                    theme,
                    &format!("Layout \"{}\"", draft.name),
                )),
            area,
        );
    }
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}
