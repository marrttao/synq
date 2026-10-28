//! Actions behind the rebindable keys, the refresh command and the
//! settings window (`S`) where keys are chosen and made global.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use crate::hotkeys::{self, HotkeyEvent};
use crate::keys::{Action, Chord};
use crate::playlists;

use super::lyrics_view::LyricsMode;
use super::{App, Mode};

impl App {
    /// Do what a key (local or global) stands for.
    pub(super) fn run_action(&mut self, action: Action) {
        match action {
            Action::PlayPause => {
                if !self.player.toggle_pause() {
                    self.play_selected();
                }
            }
            Action::Next => self.player.next(),
            Action::Prev => self.player.prev(),
            Action::SeekBack => self.seek(false, false),
            Action::SeekForward => self.seek(true, false),
            Action::SeekBackBig => self.seek(false, true),
            Action::SeekForwardBig => self.seek(true, true),
            Action::VolumeUp => self.player.change_volume(true),
            Action::VolumeDown => self.player.change_volume(false),
            Action::Shuffle => self.player.toggle_shuffle(),
            Action::Repeat => self.player.cycle_repeat(),
            Action::Favorite => self.toggle_favorite(),
            Action::Lyrics => self.show_lyrics = !self.show_lyrics,
            Action::LyricsMode => self.toggle_lyrics_mode(),
            Action::WordFont => {
                self.word_font = self.word_font.cycle();
                self.status = format!("One-word font: {}", self.word_font.label());
            }
            Action::LyricsEarlier => self.shift_lyrics(false),
            Action::LyricsLater => self.shift_lyrics(true),
            Action::Fullscreen => self.toggle_fullscreen(),
            Action::PictureMode => self.toggle_picture_mode(),
            Action::PictureZoomIn => self.zoom_picture(10),
            Action::PictureZoomOut => self.zoom_picture(-10),
            Action::PictureFit => {
                self.visual.fit = self.visual.fit.cycle();
                self.status = format!("Picture: {}", self.visual.fit.label());
            }
            Action::Refresh => self.refresh(),
            Action::SoundStyle => {
                self.sound_style = self.sound_style.cycle();
                self.status = format!("Sound: {}", self.sound_style.label());
            }
            Action::NextLayout => self.step_layout(true),
            Action::PrevLayout => self.step_layout(false),
            Action::NextPalette => self.step_theme(true),
            Action::PrevPalette => self.step_theme(false),
        }
    }

    /// Let the "fit" mode cut off more (or less) of the picture, so that the
    /// picture itself gets bigger (or smaller).
    fn zoom_picture(&mut self, by: i16) {
        let crop = (i16::from(self.visual.crop) + by).clamp(0, 90) as u8;
        self.visual.crop = crop;
        self.status = if self.visual.fit == super::visual_view::VisualFit::Fit {
            format!("Picture: up to {crop}% may be cut off")
        } else {
            format!("Picture crop {crop}% (it counts in the fit mode, V)")
        };
    }

    fn toggle_lyrics_mode(&mut self) {
        self.lyrics_mode = self.lyrics_mode.cycle();
        self.status = match self.lyrics_mode {
            LyricsMode::Full => "Lyrics: all lines".into(),
            LyricsMode::Word => format!(
                "Lyrics: one word at a time ({} font)",
                self.word_font.label()
            ),
        };
    }

    fn toggle_picture_mode(&mut self) {
        self.visual.mode = self.visual.mode.cycle();
        self.status = self.visual.mode.label().into();
    }

    /// Look at the disk again: new tracks, playlist folders and pictures.
    pub(super) fn refresh(&mut self) {
        if let Some(tx) = &self.library_refresh {
            // A send error only means the library thread is gone.
            let _ = tx.send(());
        }
        self.playlists.send(playlists::Command::Resync);
        self.visual.rescan();
        self.scanning = true;
        self.status = "Refreshing music, playlists and pictures...".into();
    }

    /// Presses of global keys that arrived from the system.
    pub(super) fn drain_hotkeys(&mut self) {
        let Some(hk) = &self.hotkeys else { return };
        let mut actions = Vec::new();
        while let Ok(ev) = hk.events.try_recv() {
            match ev {
                HotkeyEvent::Pressed(a) => actions.push(a),
                HotkeyEvent::Failed(msg) => self.status = msg,
            }
        }
        for a in actions {
            self.run_action(a);
        }
    }

    /// Tell the system which keys are global now.
    pub(super) fn sync_hotkeys(&mut self) {
        if let Some(hk) = &self.hotkeys {
            hk.set(self.keymap.globals());
        }
    }

    // ---- settings window --------------------------------------------------

    pub(super) fn open_settings(&mut self) {
        let mut state = ListState::default();
        state.select(Some(0));
        self.mode = Mode::Settings {
            state,
            capturing: false,
        };
    }

    pub(super) fn key_settings(&mut self, key: KeyEvent) {
        let Mode::Settings { state, capturing } = &mut self.mode else {
            return;
        };
        let row = state.selected().unwrap_or(0).min(Action::ALL.len() - 1);
        let action = Action::ALL[row];

        if *capturing {
            if key.code == KeyCode::Esc {
                *capturing = false;
                return;
            }
            *capturing = false;
            let result = self.keymap.rebind(action, Chord::from_event(key));
            self.finish_change(
                result,
                format!(
                    "{}: {}",
                    action.label(),
                    self.keymap.binding(action).chord.display()
                ),
            );
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'S') => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') => {
                state.select(Some((row + 1).min(Action::ALL.len() - 1)));
            }
            KeyCode::Up | KeyCode::Char('k') => state.select(Some(row.saturating_sub(1))),
            KeyCode::Enter => *capturing = true,
            KeyCode::Char(' ' | 'g') => {
                let on = !self.keymap.binding(action).global;
                let result = self.keymap.set_global(action, on);
                let note = if !hotkeys::supported() {
                    "Global keys work on Windows only".to_owned()
                } else if on {
                    format!("{} now works in any program", action.label())
                } else {
                    format!("{} works only in synq", action.label())
                };
                self.finish_change(result, note);
            }
            KeyCode::Backspace | KeyCode::Delete => {
                let result = self.keymap.reset(action);
                self.finish_change(result, format!("{}: default key", action.label()));
            }
            _ => {}
        }
    }

    fn finish_change(&mut self, result: Result<(), String>, ok_note: String) {
        match result {
            Ok(()) => {
                self.status = ok_note;
                self.sync_hotkeys();
            }
            Err(msg) => self.status = msg,
        }
    }

    pub(super) fn draw_settings(
        &self,
        f: &mut Frame,
        screen: Rect,
        state: &mut ListState,
        capturing: bool,
    ) {
        let height = (Action::ALL.len() as u16 + 6).min(screen.height);
        let width = 64.min(screen.width);
        let area = Rect {
            x: screen.x + (screen.width - width) / 2,
            y: screen.y + (screen.height - height) / 2,
            width,
            height,
        };
        f.render_widget(Clear, area);
        let block = super::draw::popup_block(&self.theme, "Settings: keys");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [head, list, hint] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .areas(inner);

        let dim = Style::new().fg(self.theme.dim);
        f.render_widget(
            Paragraph::new(format!(
                "{:<24}{:<16}{}",
                "Action", "Key", "Works everywhere"
            ))
            .style(dim),
            head,
        );
        let selected = state.selected();
        let items: Vec<ListItem> = Action::ALL
            .iter()
            .enumerate()
            .map(|(i, &a)| {
                let b = self.keymap.binding(a);
                let key = if capturing && selected == Some(i) {
                    "press a key...".to_owned()
                } else {
                    b.chord.display()
                };
                let mark = if b.global { "[x]" } else { "[ ]" };
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{:<24}", a.label())),
                    Span::raw(format!("{key:<16}")),
                    Span::raw(mark),
                ]))
            })
            .collect();
        f.render_stateful_widget(
            List::new(items).highlight_style(
                Style::new()
                    .bg(self.theme.accent)
                    .fg(self.theme.background)
                    .add_modifier(Modifier::BOLD),
            ),
            list,
            state,
        );
        let note = if hotkeys::supported() {
            "Global keys need Ctrl or Alt (or F1-F24)."
        } else {
            "Global keys work on Windows only."
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::from("Enter: new key   Space: works everywhere   Backspace: default"),
                Line::from("Esc: close"),
                Line::from(note),
            ])
            .style(dim),
            hint,
        );
    }
}
