//! Rendering. All state lives in `App` (see `mod.rs`).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, LineGauge, List, ListItem, ListState, Paragraph, Tabs,
};

use crate::audio::queue::Repeat;

use super::tracklist::{self, fmt_duration};
use super::{App, Mode, TABS, View};

impl App {
    pub(super) fn draw(&mut self, f: &mut Frame) {
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
        self.draw_popup(f, area);
    }

    /// Left panel: tabs, an info line and the list.
    fn draw_browser(&mut self, f: &mut Frame, area: Rect) {
        let block = self.block("");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [tabs_area, info_area, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .areas(inner);

        let tabs = Tabs::new(TABS)
            .select(self.tab)
            .style(Style::new().fg(self.theme.dim))
            .highlight_style(
                Style::new()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            );
        f.render_widget(tabs, tabs_area);
        f.render_widget(Paragraph::new(self.info_line()), info_area);

        let view = self.view();
        let slot = view.slot();
        // Take the state out so the list can borrow `self` while rendering.
        let mut state = std::mem::take(&mut self.lists[slot]);
        let highlight = Style::new()
            .bg(self.theme.accent)
            .fg(self.theme.background)
            .add_modifier(Modifier::BOLD);

        if view == View::PlaylistList {
            let playlists = self.view_playlists();
            fix_selection(&mut state, playlists.len());
            if playlists.is_empty() {
                let hint = if self.filter.is_empty() {
                    "No playlists. Press c to create one."
                } else {
                    "Nothing matches."
                };
                self.hint(f, body, hint);
            } else {
                let items: Vec<ListItem> = playlists
                    .iter()
                    .map(|p| {
                        ListItem::new(Line::from(vec![
                            Span::styled(p.name.clone(), Style::new().fg(self.theme.foreground)),
                            Span::styled(
                                format!("  ({})", p.tracks.len()),
                                Style::new().fg(self.theme.dim),
                            ),
                        ]))
                    })
                    .collect();
                f.render_stateful_widget(
                    List::new(items).highlight_style(highlight),
                    body,
                    &mut state,
                );
            }
        } else {
            let tracks = self.view_tracks();
            fix_selection(&mut state, tracks.len());
            if tracks.is_empty() {
                let hint = if !self.filter.is_empty() {
                    "Nothing matches."
                } else {
                    match view {
                        View::Library if self.scanning => "Scanning music/ ...",
                        View::Library => "No tracks yet. Drop audio files into music/",
                        View::Favorites => "No favorites yet. Press f on a track.",
                        _ => "This playlist is empty. Press a on a track to add it.",
                    }
                };
                self.hint(f, body, hint);
            } else {
                let playing = self.player.now_id;
                let items: Vec<ListItem> = tracks
                    .iter()
                    .map(|t| {
                        tracklist::item(
                            t,
                            &self.theme,
                            playing == Some(t.id),
                            self.favorite_set.contains(&t.id),
                        )
                    })
                    .collect();
                f.render_stateful_widget(
                    List::new(items).highlight_style(highlight),
                    body,
                    &mut state,
                );
            }
        }
        self.lists[slot] = state;
    }

    fn hint(&self, f: &mut Frame, area: Rect, text: &str) {
        f.render_widget(
            Paragraph::new(text.to_owned()).style(Style::new().fg(self.theme.dim)),
            area,
        );
    }

    /// Line under the tabs: search box, breadcrumb or key hints.
    fn info_line(&self) -> Line<'static> {
        let dim = Style::new().fg(self.theme.dim);
        let accent = Style::new().fg(self.theme.accent);
        if matches!(self.mode, Mode::Search) {
            return Line::from(vec![
                Span::styled("/ ", accent),
                Span::raw(self.filter.clone()),
                Span::styled("\u{2581}", accent),
            ]);
        }
        let mut spans = Vec::new();
        if let View::PlaylistTracks(id) = self.view() {
            let name = self
                .playlist_data
                .iter()
                .find(|p| p.id == id)
                .map_or("?", |p| p.name.as_str());
            spans.push(Span::styled(format!("\u{2190} {name}  "), accent));
        }
        if !self.filter.is_empty() {
            spans.push(Span::styled(
                format!("filter \"{}\" (Esc clears)", self.filter),
                accent,
            ));
        } else {
            let hints = match self.view() {
                View::Library => "Enter play  f fav  a add  / search",
                View::PlaylistList => "Enter open  c new  e rename  d delete",
                View::PlaylistTracks(_) => "Enter play  d remove  Esc back",
                View::Favorites => "Enter play  d remove  / search",
            };
            spans.push(Span::styled(hints, dim));
        }
        Line::from(spans)
    }

    /// Bottom panel: now playing, progress, and the state line.
    fn draw_player_bar(&self, f: &mut Frame, area: Rect) {
        let block = self.block("synq");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [title_row, progress_row, state_row] = Layout::vertical([Constraint::Length(1); 3])
            .flex(Flex::Start)
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

    /// Modal dialogs, drawn over everything else.
    fn draw_popup(&mut self, f: &mut Frame, screen: Rect) {
        match &mut self.mode {
            Mode::Normal | Mode::Search => {}
            Mode::Prompt { rename, text } => {
                let title = if rename.is_some() {
                    "Rename playlist"
                } else {
                    "New playlist"
                };
                let area = centered(screen, 50, 3);
                f.render_widget(Clear, area);
                let line = Line::from(vec![
                    Span::raw(text.clone()),
                    Span::styled("\u{2581}", Style::new().fg(self.theme.accent)),
                ]);
                let block = popup_block(&self.theme, title);
                f.render_widget(Paragraph::new(line).block(block), area);
            }
            Mode::Confirm { name, .. } => {
                let area = centered(screen, 56, 4);
                f.render_widget(Clear, area);
                let text = vec![
                    Line::from(format!("Delete playlist \"{name}\"?")),
                    Line::styled(
                        "Files in music/ are not touched.  y: delete  any other key: cancel",
                        Style::new().fg(self.theme.dim),
                    ),
                ];
                let block = popup_block(&self.theme, "Delete");
                f.render_widget(Paragraph::new(text).block(block), area);
            }
            Mode::Pick { state, .. } => {
                let height = (self.playlist_data.len() as u16 + 2).clamp(3, 14);
                let area = centered(screen, 40, height);
                f.render_widget(Clear, area);
                let items: Vec<ListItem> = self
                    .playlist_data
                    .iter()
                    .map(|p| ListItem::new(p.name.clone()))
                    .collect();
                let list = List::new(items)
                    .block(popup_block(&self.theme, "Add to playlist (Enter, Esc)"))
                    .highlight_style(
                        Style::new()
                            .bg(self.theme.accent)
                            .fg(self.theme.background)
                            .add_modifier(Modifier::BOLD),
                    );
                f.render_stateful_widget(list, area, state);
            }
        }
    }

    pub(super) fn block<'a>(&self, title: &'a str) -> Block<'a> {
        Block::new()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::new().fg(self.theme.border))
            .title_style(Style::new().fg(self.theme.accent))
    }
}

fn popup_block<'a>(theme: &super::Theme, title: &'a str) -> Block<'a> {
    Block::new()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::new().bg(theme.background).fg(theme.foreground))
        .border_style(Style::new().fg(theme.accent))
        .title_style(Style::new().fg(theme.accent))
}

/// A `width` x `height` rectangle centered in `area` (shrunk to fit).
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// Keep a selection inside a list that may have shrunk.
fn fix_selection(state: &mut ListState, len: usize) {
    if len == 0 {
        state.select(None);
    } else {
        let i = state.selected().unwrap_or(0).min(len - 1);
        state.select(Some(i));
    }
}
