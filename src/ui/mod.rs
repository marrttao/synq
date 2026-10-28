//! Terminal UI: application state, layout and the main loop.

mod theme;
mod tracklist;

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListState, Paragraph, Tabs};
use ratatui::{DefaultTerminal, Frame};

use crate::config::Config;
use crate::db::Track;
use crate::library::LibraryEvent;
use theme::Theme;

/// ~30 frames per second.
const FRAME: Duration = Duration::from_millis(33);
const PAGE: usize = 10;
const TABS: [&str; 3] = ["Library", "Playlists", "Favorites"];

pub struct App {
    theme: Theme,
    should_quit: bool,
    /// One-line message shown in the bottom bar (errors, hints).
    status: String,
    library_rx: Receiver<LibraryEvent>,
    tracks: Vec<Track>,
    list_state: ListState,
    scanning: bool,
    tab: usize,
}

impl App {
    pub fn new(
        config: &Config,
        status: Option<String>,
        library_rx: Receiver<LibraryEvent>,
    ) -> Self {
        Self {
            theme: Theme::from_config(&config.theme),
            should_quit: false,
            status: status.unwrap_or_else(|| "q: quit".into()),
            library_rx,
            tracks: Vec::new(),
            list_state: ListState::default(),
            scanning: true,
            tab: 0,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut next_frame = Instant::now();
        while !self.should_quit {
            self.drain_library_events();
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

    fn drain_library_events(&mut self) {
        loop {
            match self.library_rx.try_recv() {
                Ok(LibraryEvent::Scanning) => self.scanning = true,
                Ok(LibraryEvent::Tracks(tracks)) => {
                    self.scanning = false;
                    self.set_tracks(tracks);
                }
                Ok(LibraryEvent::Error(msg)) => {
                    self.scanning = false;
                    self.status = msg;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    /// Replace the list, keeping the selection on the same track if it survives.
    fn set_tracks(&mut self, tracks: Vec<Track>) {
        let selected_id = self
            .list_state
            .selected()
            .and_then(|i| self.tracks.get(i))
            .map(|t| t.id);
        let new_index = selected_id
            .and_then(|id| tracks.iter().position(|t| t.id == id))
            .or_else(|| self.list_state.selected())
            .map(|i| i.min(tracks.len().saturating_sub(1)));
        self.tracks = tracks;
        self.list_state.select(if self.tracks.is_empty() {
            None
        } else {
            Some(new_index.unwrap_or(0))
        });
    }

    fn move_selection(&mut self, delta: isize) {
        if self.tracks.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, self.tracks.len() as isize - 1);
        self.list_state.select(Some(next as usize));
    }

    fn handle_event(&mut self, ev: Event) {
        // Windows reports both press and release; only react to presses.
        let Event::Key(key) = ev else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(PAGE as isize),
            KeyCode::PageUp => self.move_selection(-(PAGE as isize)),
            KeyCode::Home => self.list_state.select_first(),
            KeyCode::End => self.list_state.select_last(),
            KeyCode::Tab => self.tab = (self.tab + 1) % TABS.len(),
            KeyCode::BackTab => self.tab = (self.tab + TABS.len() - 1) % TABS.len(),
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame) {
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
            constraints.push(Constraint::Percentage(34));
        }
        constraints.push(Constraint::Min(20));
        if show_right {
            constraints.push(Constraint::Percentage(24));
        }
        let cols = Layout::horizontal(constraints).split(main);
        let mut cols = cols.iter().copied();
        if show_left && let Some(r) = cols.next() {
            self.draw_browser(f, r);
        }
        if let Some(r) = cols.next() {
            f.render_widget(self.block("Lyrics"), r);
        }
        if show_right && let Some(r) = cols.next() {
            f.render_widget(self.block("Visual"), r);
        }

        let scanning = if self.scanning { " | scanning..." } else { "" };
        let text = format!("{} tracks{scanning} | {}", self.tracks.len(), self.status);
        let bar = Paragraph::new(text)
            .style(Style::new().fg(self.theme.dim))
            .block(self.block("synq"));
        f.render_widget(bar, bottom);
    }

    /// Left panel: tabs plus the track list.
    fn draw_browser(&mut self, f: &mut Frame, area: Rect) {
        let block = self.block("");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [tabs_area, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);

        let tabs = Tabs::new(TABS)
            .select(self.tab)
            .style(Style::new().fg(self.theme.dim))
            .highlight_style(
                Style::new()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            );
        f.render_widget(tabs, tabs_area);

        if self.tab != 0 {
            f.render_widget(
                Paragraph::new("Coming in a later stage").style(Style::new().fg(self.theme.dim)),
                body,
            );
        } else if self.tracks.is_empty() {
            let hint = if self.scanning {
                "Scanning music/ ..."
            } else {
                "No tracks yet. Drop audio files into music/"
            };
            f.render_widget(
                Paragraph::new(hint).style(Style::new().fg(self.theme.dim)),
                body,
            );
        } else {
            let items = self.tracks.iter().map(|t| tracklist::item(t, &self.theme));
            let list = List::new(items).highlight_style(
                Style::new()
                    .bg(self.theme.accent)
                    .fg(self.theme.background)
                    .add_modifier(Modifier::BOLD),
            );
            f.render_stateful_widget(list, body, &mut self.list_state);
        }
    }

    fn block<'a>(&self, title: &'a str) -> Block<'a> {
        Block::new()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::new().fg(self.theme.border))
            .title_style(Style::new().fg(self.theme.accent))
    }
}
