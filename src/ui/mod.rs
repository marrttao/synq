//! Terminal UI: application state, layout and the main loop.

mod player;
mod theme;
mod tracklist;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, LineGauge, List, ListState, Paragraph, Tabs};
use ratatui::{DefaultTerminal, Frame};

use crate::audio::queue::Repeat;
use crate::config::Config;
use crate::db::Track;
use crate::library::LibraryEvent;
use player::Player;
use theme::Theme;
use tracklist::fmt_duration;

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
    player: Player,
}

impl App {
    pub fn new(
        config: &Config,
        status: Option<String>,
        library_rx: Receiver<LibraryEvent>,
        music_dir: PathBuf,
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
            player: Player::new(&config.audio, music_dir),
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut next_frame = Instant::now();
        while !self.should_quit {
            self.drain_library_events();
            if let Some(err) = self.player.poll() {
                self.status = err;
            }
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
            KeyCode::Enter => self.play_selected(),
            KeyCode::Char(' ') => {
                if !self.player.toggle_pause() {
                    self.play_selected();
                }
            }
            KeyCode::Char('n') => self.player.next(),
            KeyCode::Char('p') => self.player.prev(),
            KeyCode::Right => self.seek(true),
            KeyCode::Left => self.seek(false),
            KeyCode::Char('+' | '=') => self.player.change_volume(true),
            KeyCode::Char('-') => self.player.change_volume(false),
            KeyCode::Char('s') => self.player.toggle_shuffle(),
            KeyCode::Char('r') => self.player.cycle_repeat(),
            _ => {}
        }
    }

    fn now_playing(&self) -> Option<&Track> {
        let id = self.player.now_id?;
        self.tracks.iter().find(|t| t.id == id)
    }

    fn play_selected(&mut self) {
        if self.tab != 0 {
            return;
        }
        let Some(selected) = self.list_state.selected() else {
            return;
        };
        if let Err(msg) = self.player.play_from(&self.tracks, selected) {
            self.status = msg;
        }
    }

    fn seek(&mut self, forward: bool) {
        let duration = self
            .now_playing()
            .map_or(Duration::ZERO, |t| Duration::from_millis(t.duration_ms));
        self.player.seek(forward, duration);
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
            Layout::vertical([Constraint::Min(3), Constraint::Length(5)]).areas(area);

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

        self.draw_player_bar(f, bottom);
    }

    /// Bottom panel: now playing, progress, and the state line.
    fn draw_player_bar(&self, f: &mut Frame, area: Rect) {
        let block = self.block("synq");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [title_row, progress_row, state_row] = Layout::vertical([Constraint::Length(1); 3])
            .flex(ratatui::layout::Flex::Start)
            .areas(inner);

        let now = self.now_playing();
        let (icon, title) = match now {
            Some(t) => (
                if self.player.paused {
                    "\u{23F8}"
                } else {
                    "\u{25B6}"
                },
                format!("{} - {}", t.artist, t.title),
            ),
            None => (
                "\u{25A0}",
                "Nothing playing (Enter: play selected)".to_owned(),
            ),
        };
        f.render_widget(
            Paragraph::new(format!("{icon} {title}")).style(Style::new().fg(self.theme.accent)),
            title_row,
        );

        let total_ms = now.map_or(0, |t| t.duration_ms);
        let pos_ms = self.player.position.as_millis() as u64;
        let ratio = if total_ms == 0 {
            0.0
        } else {
            (pos_ms as f64 / total_ms as f64).clamp(0.0, 1.0)
        };
        let label = format!("{} / {}", fmt_duration(pos_ms), fmt_duration(total_ms));
        f.render_widget(
            LineGauge::default()
                .ratio(ratio)
                .label(Line::from(label))
                .filled_style(Style::new().fg(self.theme.accent))
                .unfilled_style(Style::new().fg(self.theme.border))
                .style(Style::new().fg(self.theme.foreground)),
            progress_row,
        );

        let repeat = match self.player.repeat {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        };
        let shuffle = if self.player.shuffle { "on" } else { "off" };
        let scanning = if self.scanning { " | scanning..." } else { "" };
        let text = format!(
            "vol {}% | shuffle {shuffle} | repeat {repeat} | {} tracks{scanning} | {}",
            (self.player.volume * 100.0).round() as u32,
            self.tracks.len(),
            self.status
        );
        f.render_widget(
            Paragraph::new(text).style(Style::new().fg(self.theme.dim)),
            state_row,
        );
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
            let playing = self.player.now_id;
            let items = self
                .tracks
                .iter()
                .map(|t| tracklist::item(t, &self.theme, playing == Some(t.id)));
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
