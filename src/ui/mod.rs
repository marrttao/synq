//! Terminal UI: application state, layout and the main loop.

mod theme;

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::config::Config;
use theme::Theme;

/// ~30 frames per second.
const FRAME: Duration = Duration::from_millis(33);

pub struct App {
    theme: Theme,
    should_quit: bool,
    /// One-line message shown in the bottom bar (errors, hints).
    status: String,
}

impl App {
    pub fn new(config: &Config, status: Option<String>) -> Self {
        Self {
            theme: Theme::from_config(&config.theme),
            should_quit: false,
            status: status.unwrap_or_else(|| "q: quit".into()),
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut next_frame = Instant::now();
        while !self.should_quit {
            terminal.draw(|f| self.draw(f))?;
            next_frame += FRAME;
            // Wait for input until the next frame is due; keeps ~30 fps
            // without busy-looping.
            loop {
                let timeout = next_frame.saturating_duration_since(Instant::now());
                if !event::poll(timeout)? {
                    break;
                }
                self.handle_event(event::read()?);
                if self.should_quit {
                    return Ok(());
                }
            }
            // Do not try to catch up after a long stall.
            if next_frame < Instant::now() {
                next_frame = Instant::now();
            }
        }
        Ok(())
    }

    fn handle_event(&mut self, ev: Event) {
        // Windows reports both press and release; only react to presses.
        if let Event::Key(key) = ev
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.should_quit = true;
                }
                _ => {}
            }
        }
    }

    fn draw(&self, f: &mut Frame) {
        let area = f.area();
        f.render_widget(
            Block::new().style(
                Style::new()
                    .bg(self.theme.background)
                    .fg(self.theme.foreground),
            ),
            area,
        );
        let [main, bottom] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(area);

        // Narrow windows collapse side panels.
        let (show_left, show_right) = (area.width >= 70, area.width >= 110);
        let mut constraints = Vec::new();
        if show_left {
            constraints.push(Constraint::Percentage(28));
        }
        constraints.push(Constraint::Min(20));
        if show_right {
            constraints.push(Constraint::Percentage(28));
        }
        let cols = Layout::horizontal(constraints).split(main);
        let mut cols = cols.iter().copied();
        if show_left && let Some(r) = cols.next() {
            self.panel(f, r, "Library");
        }
        if let Some(r) = cols.next() {
            self.panel(f, r, "Lyrics");
        }
        if show_right && let Some(r) = cols.next() {
            self.panel(f, r, "Visual");
        }

        let bar = Paragraph::new(self.status.as_str())
            .style(Style::new().fg(self.theme.dim))
            .block(self.block("synq"));
        f.render_widget(bar, bottom);
    }

    fn panel(&self, f: &mut Frame, area: Rect, title: &str) {
        f.render_widget(self.block(title), area);
    }

    fn block<'a>(&self, title: &'a str) -> Block<'a> {
        Block::new()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::new().fg(self.theme.border))
            .title_style(Style::new().fg(self.theme.accent))
    }
}
