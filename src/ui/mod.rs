//! Terminal UI: application state, input handling and the main loop.
//! Rendering lives in `draw.rs`.

mod bigtext;
mod blocks;
mod draw;
mod effects;
mod help;
mod keys_view;
mod layout;
mod layout_view;
mod lyrics_view;
mod menu_view;
mod palette;
mod palette_view;
mod player;
mod prefs;
mod render;
mod spectrum;
#[cfg(test)]
mod stage10_tests;
#[cfg(test)]
mod stage8_tests;
mod theme;
mod tracklist;
mod visual_view;
mod viz_view;
mod whisper_view;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::config::Config;
use crate::db::Track;
use crate::hotkeys::Hotkeys;
use crate::keys::{self, Keymap};
use crate::library::LibraryEvent;
use crate::lyrics;
use crate::playlists::{self, PlaylistInfo, Target};
use crate::transcribe;
use crate::visuals;
pub use bigtext::load_user_fonts;
use effects::Effects;
use layout::Layout;
pub use layout::LayoutSet;
use lyrics_view::LyricsState;
pub use palette::PaletteSet;
use player::Player;
pub use prefs::Prefs;
use theme::Theme;
use visual_view::{VisualFit, VisualMode, VisualState};
use whisper_view::WhisperStatus;

/// ~30 frames per second.
const FRAME: Duration = Duration::from_millis(33);
const PAGE: usize = 10;
const TABS: [&str; 3] = ["Library", "Playlists", "Favorites"];
const MAX_INPUT: usize = 100;
/// Shown in the bottom bar while there is no message.
const HINT: &str = "Space play/pause | n/p track | Left/Right seek | ? help | F10 menu";
/// How long a message stays before the hint comes back.
const STATUS_LIFE: Duration = Duration::from_secs(6);

/// What the left panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Library,
    PlaylistList,
    PlaylistTracks(i64),
    Favorites,
}

impl View {
    /// Each view remembers its own selection.
    fn slot(self) -> usize {
        match self {
            View::Library => 0,
            View::PlaylistList => 1,
            View::PlaylistTracks(_) => 2,
            View::Favorites => 3,
        }
    }
}

/// Modal states; everything but `Normal` captures the keyboard.
enum Mode {
    Normal,
    /// Typing a search filter.
    Search,
    /// Typing a playlist name (`rename` holds the playlist being renamed).
    Prompt {
        rename: Option<i64>,
        text: String,
    },
    /// Waiting for y/n before deleting a playlist.
    Confirm {
        id: i64,
        name: String,
    },
    /// Asking whether the Whisper model may be downloaded.
    ConfirmModel {
        model: String,
        size_mb: u64,
    },
    /// Choosing the picture for the Visual panel (`names` excludes the
    /// leading "none" row). `playlist` is the playlist in context, if any.
    GifPick {
        names: Vec<String>,
        state: ListState,
        playlist: Option<(i64, String)>,
        to_playlist: bool,
    },
    /// The key list; `scroll` is how many lines are off the top.
    Help {
        scroll: u16,
    },
    /// Everything the player can do, in one list (F10).
    Menu {
        state: ListState,
    },
    /// Editing the colors; `typing` holds a hex value being entered.
    PaletteEdit {
        draft: palette::Palette,
        selected: usize,
        typing: Option<String>,
    },
    /// Arranging the panels; `draft` is drawn instead of the saved layout.
    LayoutEdit {
        draft: Layout,
        selected: usize,
        drag: Option<layout_view::Drag>,
        /// The list of removed panels to add back, while it is open.
        adding: Option<ListState>,
    },
    /// Choosing keys; `capturing` waits for the next key press.
    Settings {
        state: ListState,
        capturing: bool,
    },
    /// Choosing the playlist for a track.
    Pick {
        track_id: i64,
        state: ListState,
    },
}

/// Keys that make sense while only lyrics and the picture are on screen.
fn allowed_in_fullscreen(code: KeyCode) -> bool {
    match code {
        KeyCode::Char(c) => "qgtTS?".contains(c),
        KeyCode::Esc | KeyCode::Up | KeyCode::Down => true,
        _ => false,
    }
}

/// Background color as bytes, for drawing transparent pictures over it.
pub fn background_rgb(cfg: &crate::config::ThemeConfig) -> [u8; 3] {
    match Theme::from_config(cfg).background {
        ratatui::style::Color::Rgb(r, g, b) => [r, g, b],
        _ => [0, 0, 0],
    }
}

pub struct App {
    theme: Theme,
    should_quit: bool,
    /// One-line message shown in the bottom bar (errors, hints).
    status: String,
    /// What `status` was at the last frame, and since when: old messages
    /// give way to the key hints.
    status_seen: String,
    status_since: Instant,
    /// Track that was playing at the last frame (the list follows it).
    last_now: Option<i64>,
    /// Set when a real terminal is attached (mouse capture and so on).
    real_terminal: bool,
    library_rx: Receiver<LibraryEvent>,
    tracks: Vec<Track>,
    track_index: HashMap<i64, usize>,
    scanning: bool,
    playlists: playlists::Handle,
    playlist_data: Vec<PlaylistInfo>,
    favorites: Vec<i64>,
    favorite_set: HashSet<i64>,
    tab: usize,
    open_playlist: Option<i64>,
    filter: String,
    mode: Mode,
    lists: [ListState; 4],
    player: Player,
    music_dir: PathBuf,
    lyrics: lyrics::Handle,
    transcribe: transcribe::Handle,
    whisper_enabled: bool,
    whisper: WhisperStatus,
    /// Track to recognize automatically once it has been playing a moment.
    auto_pending: Option<(i64, Instant)>,
    /// Tracks recognition was already started for in this session.
    attempted: HashSet<i64>,
    /// A download question waiting for the current dialog to close.
    model_question: Option<(String, u64)>,
    lyrics_state: LyricsState,
    /// Track the current lyrics (or lookup) belong to.
    lyrics_for: Option<i64>,
    show_lyrics: bool,
    visual: VisualState,
    fx: Effects,
    fullscreen: bool,
    palettes: PaletteSet,
    lyrics_backdrop: u8,
    spectrum: spectrum::Spectrum,
    prefs: prefs::Prefs,
    sound_style: viz_view::SoundStyle,
    lyrics_mode: lyrics_view::LyricsMode,
    word_font: bigtext::FontKind,
    keymap: Keymap,
    layouts: LayoutSet,
    /// Width of the last frame, to freeze the classic layout for editing.
    screen_width: u16,
    /// The area the panels are laid out in, as of the last frame.
    layout_main: ratatui::layout::Rect,
    hotkeys: Option<Hotkeys>,
    /// Makes the library worker rescan `music/` at once.
    library_refresh: Option<Sender<()>>,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: &Config,
        status: Option<String>,
        library_rx: Receiver<LibraryEvent>,
        playlists: playlists::Handle,
        lyrics: lyrics::Handle,
        transcribe: transcribe::Handle,
        visuals: visuals::Handle,
        music_dir: PathBuf,
    ) -> Self {
        Self {
            theme: Theme::from_config(&config.theme),
            should_quit: false,
            status: status.unwrap_or_else(|| HINT.into()),
            status_seen: String::new(),
            status_since: Instant::now(),
            last_now: None,
            real_terminal: false,
            library_rx,
            tracks: Vec::new(),
            track_index: HashMap::new(),
            scanning: true,
            playlists,
            playlist_data: Vec::new(),
            favorites: Vec::new(),
            favorite_set: HashSet::new(),
            tab: 0,
            open_playlist: None,
            filter: String::new(),
            mode: Mode::Normal,
            lists: Default::default(),
            player: Player::new(&config.audio, music_dir.clone()),
            music_dir,
            lyrics,
            whisper_enabled: config.whisper.enabled && transcribe::AVAILABLE,
            transcribe,
            whisper: WhisperStatus::Idle,
            auto_pending: None,
            attempted: HashSet::new(),
            model_question: None,
            lyrics_state: LyricsState::Idle,
            lyrics_for: None,
            show_lyrics: true,
            visual: VisualState::new(
                visuals,
                VisualMode::from_config(&config.visuals.mode),
                VisualFit::from_config(&config.visuals.fit),
                config.visuals.max_crop,
            ),
            fx: Effects::new(config.ui.animations),
            fullscreen: false,
            palettes: PaletteSet::presets_only(&config.theme.preset),
            lyrics_backdrop: config.ui.lyrics_backdrop.min(100),
            spectrum: spectrum::Spectrum::default(),
            prefs: prefs::Prefs::default(),
            sound_style: viz_view::SoundStyle::from_config(&config.ui.sound_style),
            lyrics_mode: lyrics_view::LyricsMode::from_config(&config.lyrics.display),
            word_font: bigtext::FontKind::from_config(&config.lyrics.word_font),
            keymap: Keymap::defaults(),
            layouts: LayoutSet::presets_only(),
            screen_width: 120,
            layout_main: ratatui::layout::Rect::ZERO,
            hotkeys: None,
            library_refresh: None,
        }
    }

    /// The saved colors win over config.toml, once something was chosen.
    pub fn with_palettes(mut self, palettes: PaletteSet) -> Self {
        if palettes.restored {
            self.theme = palettes.current().theme();
        }
        self.palettes = palettes;
        self
    }

    /// Put back the choices of the last run (they win over config.toml).
    pub fn with_prefs(mut self, prefs: prefs::Prefs) -> Self {
        if let Some(v) = prefs.get("picture_mode") {
            self.visual.mode = VisualMode::from_config(v);
        }
        if let Some(v) = prefs.get("picture_fit") {
            self.visual.fit = VisualFit::from_config(v);
        }
        if let Some(v) = prefs.get("picture_crop").and_then(|v| v.parse::<u8>().ok()) {
            self.visual.crop = v.min(90);
        }
        if let Some(v) = prefs.get("lyrics_mode") {
            self.lyrics_mode = lyrics_view::LyricsMode::from_config(v);
        }
        if let Some(v) = prefs.get("word_font") {
            self.word_font = bigtext::FontKind::from_config(v);
        }
        if let Some(v) = prefs.get("sound_style") {
            self.sound_style = viz_view::SoundStyle::from_config(v);
        }
        if let Some(v) = prefs.get("show_lyrics") {
            self.show_lyrics = v != "off";
        }
        self.player.restore(
            prefs.get("volume").and_then(|v| v.parse::<f32>().ok()),
            prefs.get("shuffle").map(|v| v == "on"),
            prefs
                .get("repeat")
                .and_then(crate::audio::queue::Repeat::from_name),
        );
        self.prefs = prefs;
        self
    }

    /// What is worth remembering, as it is now.
    fn pref_snapshot(&self) -> Vec<(&'static str, String)> {
        let on = |b: bool| if b { "on" } else { "off" }.to_owned();
        vec![
            ("picture_mode", self.visual.mode.name().to_owned()),
            ("picture_fit", self.visual.fit.name().to_owned()),
            ("picture_crop", self.visual.crop.to_string()),
            ("lyrics_mode", self.lyrics_mode.name().to_owned()),
            ("word_font", self.word_font.name().to_owned()),
            ("sound_style", self.sound_style.name().to_owned()),
            ("show_lyrics", on(self.show_lyrics)),
            ("volume", format!("{:.2}", self.player.volume)),
            ("shuffle", on(self.player.shuffle)),
            ("repeat", self.player.repeat.name().to_owned()),
        ]
    }

    /// Save what changed since the last time (after every key or global key).
    fn persist_prefs(&mut self) {
        let now = self.pref_snapshot();
        if let Some(problem) = self.prefs.store(&now) {
            self.status = problem;
        }
    }

    pub fn with_layouts(mut self, layouts: LayoutSet) -> Self {
        self.layouts = layouts;
        self
    }

    /// The parts that talk to the system; tests run without them.
    pub fn with_system(
        mut self,
        keymap: Keymap,
        hotkeys: Option<Hotkeys>,
        library_refresh: Sender<()>,
    ) -> Self {
        self.keymap = keymap;
        self.hotkeys = hotkeys;
        self.library_refresh = Some(library_refresh);
        self.real_terminal = true;
        self.sync_hotkeys();
        self
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut next_frame = Instant::now();
        while !self.should_quit {
            self.drain_library_events();
            self.drain_playlist_events();
            self.drain_lyrics_events();
            self.drain_transcribe_events();
            self.drain_visual_events();
            self.drain_hotkeys();
            if let Some(err) = self.player.poll() {
                self.status = err;
            }
            self.follow_playing();
            self.age_status();
            self.sync_lyrics_request();
            self.tick_whisper();
            self.tick_visuals();
            self.tick_spectrum();
            self.tick_effects();
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

    // ---- data from background threads -------------------------------------

    fn drain_library_events(&mut self) {
        loop {
            match self.library_rx.try_recv() {
                Ok(LibraryEvent::Scanning) => self.scanning = true,
                Ok(LibraryEvent::Tracks(mut tracks)) => {
                    self.scanning = false;
                    // Files deleted by hand are simply gone from the list; the
                    // database keeps them in case the file comes back.
                    tracks.retain(|t| !t.missing);
                    self.keep_selection(|app| {
                        app.track_index =
                            tracks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
                        app.tracks = tracks;
                    });
                }
                Ok(LibraryEvent::Notice(msg)) => self.status = msg,
                Ok(LibraryEvent::Error(msg)) => {
                    self.scanning = false;
                    self.status = msg;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn drain_playlist_events(&mut self) {
        while let Ok(ev) = self.playlists.events.try_recv() {
            match ev {
                playlists::Event::Snapshot(s) => self.keep_selection(|app| {
                    app.playlist_data = s.playlists;
                    app.favorite_set = s.favorites.iter().copied().collect();
                    app.favorites = s.favorites;
                    app.playlists_changed();
                    // An opened playlist may have been deleted.
                    if let Some(id) = app.open_playlist
                        && !app.playlist_data.iter().any(|p| p.id == id)
                    {
                        app.open_playlist = None;
                    }
                }),
                playlists::Event::Notice(msg) | playlists::Event::Error(msg) => {
                    self.status = msg;
                }
            }
        }
    }

    // ---- what is visible --------------------------------------------------

    fn view(&self) -> View {
        match (self.tab, self.open_playlist) {
            (0, _) => View::Library,
            (1, None) => View::PlaylistList,
            (1, Some(id)) => View::PlaylistTracks(id),
            _ => View::Favorites,
        }
    }

    fn ids_to_tracks(&self, ids: &[i64]) -> Vec<&Track> {
        ids.iter()
            .filter_map(|id| self.track_index.get(id).map(|&i| &self.tracks[i]))
            .collect()
    }

    /// Tracks of the current view with the search filter applied.
    fn view_tracks(&self) -> Vec<&Track> {
        let base: Vec<&Track> = match self.view() {
            View::Library => self.tracks.iter().collect(),
            View::PlaylistTracks(id) => self
                .playlist_data
                .iter()
                .find(|p| p.id == id)
                .map(|p| self.ids_to_tracks(&p.tracks))
                .unwrap_or_default(),
            View::Favorites => self.ids_to_tracks(&self.favorites),
            View::PlaylistList => Vec::new(),
        };
        if self.filter.is_empty() {
            return base;
        }
        let needle = self.filter.to_lowercase();
        base.into_iter()
            .filter(|t| {
                [&t.artist, &t.title, &t.album]
                    .iter()
                    .any(|s| s.to_lowercase().contains(&needle))
            })
            .collect()
    }

    fn view_playlists(&self) -> Vec<&PlaylistInfo> {
        let needle = self.filter.to_lowercase();
        self.playlist_data
            .iter()
            .filter(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle))
            .collect()
    }

    fn view_len(&self) -> usize {
        match self.view() {
            View::PlaylistList => self.view_playlists().len(),
            _ => self.view_tracks().len(),
        }
    }

    /// Id of the selected item: a track id, or a playlist id in the list view.
    fn selected_id(&self) -> Option<i64> {
        let sel = self.lists[self.view().slot()].selected()?;
        match self.view() {
            View::PlaylistList => self.view_playlists().get(sel).map(|p| p.id),
            _ => self.view_tracks().get(sel).map(|t| t.id),
        }
    }

    fn selected_track(&self) -> Option<&Track> {
        if self.view() == View::PlaylistList {
            return None;
        }
        let sel = self.lists[self.view().slot()].selected()?;
        self.view_tracks().get(sel).copied()
    }

    fn selected_playlist(&self) -> Option<&PlaylistInfo> {
        if self.view() != View::PlaylistList {
            return None;
        }
        let sel = self.lists[self.view().slot()].selected()?;
        self.view_playlists().get(sel).copied()
    }

    /// Run `change`, then keep the selection on the same item if it still
    /// exists (otherwise clamp it).
    fn keep_selection(&mut self, change: impl FnOnce(&mut Self)) {
        let id = self.selected_id();
        let old_index = self.lists[self.view().slot()].selected();
        change(self);
        let view = self.view();
        let len = self.view_len();
        let position = id.and_then(|id| match view {
            View::PlaylistList => self.view_playlists().iter().position(|p| p.id == id),
            _ => self.view_tracks().iter().position(|t| t.id == id),
        });
        let new = position.or(old_index).map(|i| i.min(len.saturating_sub(1)));
        self.lists[view.slot()].select(if len == 0 {
            None
        } else {
            Some(new.unwrap_or(0))
        });
    }

    fn reset_selection(&mut self) {
        let slot = self.view().slot();
        let none = self.view_len() == 0;
        self.lists[slot].select(if none { None } else { Some(0) });
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.view_len();
        if len == 0 {
            return;
        }
        let slot = self.view().slot();
        let cur = self.lists[slot].selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, len as isize - 1);
        self.lists[slot].select(Some(next as usize));
    }

    // ---- input ------------------------------------------------------------

    fn handle_event(&mut self, ev: Event) {
        // Windows reports both press and release; only react to presses.
        let mut key = match ev {
            Event::Key(k) => k,
            Event::Mouse(m) => return self.mouse_event(m),
            _ => return,
        };
        if key.kind != KeyEventKind::Press {
            return;
        }
        // A Russian layout works too: every letter acts as the Latin key
        // in the same place.
        if matches!(self.mode, Mode::Normal)
            && let KeyCode::Char(c) = key.code
        {
            key.code = KeyCode::Char(keys::latin_for(c));
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }
        self.handle_key(key);
        self.persist_prefs();
    }

    fn handle_key(&mut self, key: KeyEvent) {
        match &self.mode {
            Mode::Normal => self.key_normal(key),
            Mode::Search => self.key_search(key),
            Mode::Prompt { .. } => self.key_prompt(key),
            Mode::Confirm { .. } => self.key_confirm(key),
            Mode::Pick { .. } => self.key_pick(key),
            Mode::GifPick { .. } => self.key_gif_pick(key),
            Mode::Help { .. } => self.key_help(key),
            Mode::Menu { .. } => self.key_menu(key),
            Mode::Settings { .. } => self.key_settings(key),
            Mode::LayoutEdit { .. } => self.key_layout_edit(key),
            Mode::PaletteEdit { .. } => self.key_palette_edit(key),
            Mode::ConfirmModel { .. } => self.key_confirm_model(key),
        }
    }

    fn key_normal(&mut self, key: KeyEvent) {
        if let Some(action) = self.keymap.action_for(key) {
            self.run_action(action);
            return;
        }
        if self.fullscreen && !allowed_in_fullscreen(key.code) {
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_lyrics(true)
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_lyrics(false)
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(PAGE as isize),
            KeyCode::PageUp => self.move_selection(-(PAGE as isize)),
            KeyCode::Home => self.lists[self.view().slot()].select_first(),
            KeyCode::End => self.lists[self.view().slot()].select_last(),
            KeyCode::Tab => self.switch_tab((self.tab + 1) % TABS.len()),
            KeyCode::BackTab => self.switch_tab((self.tab + TABS.len() - 1) % TABS.len()),
            KeyCode::Enter => self.activate(),
            KeyCode::Esc if self.fullscreen => self.toggle_fullscreen(),
            KeyCode::Esc => self.go_back(true),
            KeyCode::Backspace => self.go_back(false),
            KeyCode::Char('/') => self.mode = Mode::Search,
            KeyCode::Char('t') => self.recognize_or_retry(),
            KeyCode::Char('a') => self.start_pick(),
            KeyCode::Char('g') => self.start_gif_pick(),
            KeyCode::Char('?') | KeyCode::F(1) => self.mode = Mode::Help { scroll: 0 },
            KeyCode::F(10) => self.open_menu(),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => self.open_menu(),
            KeyCode::Char('S') => self.open_settings(),
            KeyCode::Char('E') => self.open_layout_editor(),
            KeyCode::Char('C') => self.open_palette_editor(),
            KeyCode::Char('c') => self.start_create(),
            KeyCode::Char('e') => self.start_rename(),
            KeyCode::Char('d') | KeyCode::Delete => self.delete_selected(),
            _ => {}
        }
    }

    /// The highlighted row follows what plays, so that "selected" and
    /// "playing" are the same track unless the user moves away.
    fn follow_playing(&mut self) {
        let now = self.player.now_id;
        if now == self.last_now {
            return;
        }
        self.last_now = now;
        let Some(id) = now else { return };
        let position = self.view_tracks().iter().position(|t| t.id == id);
        if let Some(i) = position {
            self.lists[self.view().slot()].select(Some(i));
        }
    }

    fn age_status(&mut self) {
        if self.status != self.status_seen {
            self.status_seen.clone_from(&self.status);
            self.status_since = Instant::now();
        } else if self.status != HINT && self.status_since.elapsed() > STATUS_LIFE {
            self.status = HINT.into();
        }
    }

    fn switch_tab(&mut self, tab: usize) {
        self.tab = tab;
        self.open_playlist = None;
        self.filter.clear();
        self.keep_selection(|_| {});
    }

    /// Esc clears the filter first; Esc/Backspace then leave an opened playlist.
    fn go_back(&mut self, clears_filter: bool) {
        if clears_filter && !self.filter.is_empty() {
            self.filter.clear();
            self.reset_selection();
        } else if self.open_playlist.is_some() {
            self.open_playlist = None;
            self.filter.clear();
            self.keep_selection(|_| {});
        }
    }

    fn activate(&mut self) {
        if let Some(id) = self.selected_playlist().map(|p| p.id) {
            self.open_playlist = Some(id);
            self.filter.clear();
            self.reset_selection();
        } else {
            self.play_selected();
        }
    }

    fn now_playing(&self) -> Option<&Track> {
        let id = self.player.now_id?;
        self.track_index.get(&id).map(|&i| &self.tracks[i])
    }

    fn play_selected(&mut self) {
        let Some(selected) = self.lists[self.view().slot()].selected() else {
            return;
        };
        self.visual.play_context = match self.view() {
            View::PlaylistTracks(id) => Some(id),
            _ => None,
        };
        let result = {
            let tracks = self.view_tracks();
            // `play_from` only needs &self, so the borrowed `tracks` are fine.
            self.player.play_from(&tracks, selected)
        };
        if let Err(msg) = result {
            self.status = msg;
        }
    }

    fn seek(&mut self, forward: bool, big: bool) {
        let duration = self
            .now_playing()
            .map_or(Duration::ZERO, |t| Duration::from_millis(t.duration_ms));
        self.player.seek(forward, big, duration);
    }

    /// Track the favorite/playlist keys act on: the selected one, else the
    /// one that is playing.
    fn target_track_id(&self) -> Option<i64> {
        if self.fullscreen {
            return self.player.now_id; // the list is not on screen
        }
        self.selected_track().map(|t| t.id).or(self.player.now_id)
    }

    fn toggle_favorite(&mut self) {
        let Some(track) = self.target_track_id() else {
            self.status = "Select a track first".into();
            return;
        };
        let target = Target::Favorites;
        let name = self
            .track_index
            .get(&track)
            .map(|&i| format!("{} - {}", self.tracks[i].artist, self.tracks[i].title))
            .unwrap_or_default();
        let remove = self.favorite_set.contains(&track);
        self.status = if remove {
            format!("Removed from favorites: {name}")
        } else {
            format!("Added to favorites: {name}")
        };
        self.playlists.send(if remove {
            playlists::Command::Remove { target, track }
        } else {
            playlists::Command::Add { target, track }
        });
    }

    fn start_pick(&mut self) {
        let Some(track_id) = self.target_track_id() else {
            self.status = "Select a track first".into();
            return;
        };
        if self.playlist_data.is_empty() {
            self.status = "No playlists yet: open the Playlists tab and press c".into();
            return;
        }
        let mut state = ListState::default();
        state.select_first();
        self.mode = Mode::Pick { track_id, state };
    }

    fn start_create(&mut self) {
        if self.view() == View::PlaylistList {
            self.mode = Mode::Prompt {
                rename: None,
                text: String::new(),
            };
        }
    }

    fn start_rename(&mut self) {
        if let Some(p) = self.selected_playlist() {
            self.mode = Mode::Prompt {
                rename: Some(p.id),
                text: p.name.clone(),
            };
        }
    }

    /// `d`: delete a playlist (after confirmation) or remove a track from
    /// the playlist / favorites being shown.
    fn delete_selected(&mut self) {
        match self.view() {
            View::PlaylistList => {
                if let Some(p) = self.selected_playlist() {
                    self.mode = Mode::Confirm {
                        id: p.id,
                        name: p.name.clone(),
                    };
                }
            }
            View::PlaylistTracks(id) => {
                if let Some(track) = self.selected_track().map(|t| t.id) {
                    self.playlists.send(playlists::Command::Remove {
                        target: Target::Playlist(id),
                        track,
                    });
                }
            }
            View::Favorites => {
                if let Some(track) = self.selected_track().map(|t| t.id) {
                    self.playlists.send(playlists::Command::Remove {
                        target: Target::Favorites,
                        track,
                    });
                }
            }
            View::Library => {}
        }
    }

    fn key_search(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.mode = Mode::Normal,
            KeyCode::Esc => {
                self.filter.clear();
                self.mode = Mode::Normal;
                self.reset_selection();
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.reset_selection();
            }
            KeyCode::Char(c) if self.filter.chars().count() < MAX_INPUT => {
                self.filter.push(c);
                self.reset_selection();
            }
            KeyCode::Down => self.move_selection(1),
            KeyCode::Up => self.move_selection(-1),
            _ => {}
        }
    }

    fn key_prompt(&mut self, key: KeyEvent) {
        let Mode::Prompt { rename, text } = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c) if text.chars().count() < MAX_INPUT => text.push(c),
            KeyCode::Enter => {
                let name = text.clone();
                let cmd = match *rename {
                    Some(id) => playlists::Command::Rename { id, name },
                    None => playlists::Command::Create(name),
                };
                self.mode = Mode::Normal;
                self.playlists.send(cmd);
            }
            _ => {}
        }
    }

    fn key_confirm(&mut self, key: KeyEvent) {
        let Mode::Confirm { id, .. } = &self.mode else {
            return;
        };
        let id = *id;
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.playlists.send(playlists::Command::Delete(id));
        }
        self.mode = Mode::Normal;
    }

    fn key_pick(&mut self, key: KeyEvent) {
        let count = self.playlist_data.len();
        let Mode::Pick { track_id, state } = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') => {
                let next = (state.selected().unwrap_or(0) + 1).min(count.saturating_sub(1));
                state.select(Some(next));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.select(Some(state.selected().unwrap_or(0).saturating_sub(1)));
            }
            KeyCode::Enter => {
                let track = *track_id;
                let chosen = state
                    .selected()
                    .and_then(|i| self.playlist_data.get(i))
                    .map(|p| p.id);
                self.mode = Mode::Normal;
                if let Some(id) = chosen {
                    self.playlists.send(playlists::Command::Add {
                        target: Target::Playlist(id),
                        track,
                    });
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, TrackRecord};
    use crate::library::LibraryEvent;
    use crate::paths::Paths;
    use crate::testutil::wav_bytes;
    use std::sync::mpsc;

    fn key(app: &mut App, code: KeyCode) {
        app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            key(app, KeyCode::Char(c));
        }
    }

    /// Draw at several sizes (including tiny ones) to catch layout panics.
    fn render_all_sizes(app: &mut App) {
        for (w, h) in [(120, 30), (80, 20), (50, 12), (10, 4), (1, 1)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| app.draw(f)).unwrap();
        }
    }

    /// Pump background events until `cond` holds (or fail after 5 s).
    fn wait(app: &mut App, what: &str, cond: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            app.drain_library_events();
            app.drain_playlist_events();
            if cond(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for: {what} (status: {})", app.status);
    }

    #[test]
    fn playlist_favorites_and_search_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        paths.ensure_dirs().unwrap();
        let db = Db::open(&paths.db).unwrap();
        for (file, title) in [("alpha.wav", "Alpha"), ("beta.wav", "Beta")] {
            std::fs::write(paths.music.join(file), wav_bytes(100)).unwrap();
            db.upsert_track(&TrackRecord {
                path: file.into(),
                size: 1,
                mtime_ms: 1,
                title: title.into(),
                artist: "Band".into(),
                album: String::new(),
                duration_ms: 100,
            })
            .unwrap();
        }

        let (lib_tx, lib_rx) = mpsc::channel();
        let mut app = App::new(
            &test_config(),
            None,
            lib_rx,
            playlists::spawn(&paths),
            lyrics::spawn(paths.db.clone(), paths.lyrics.clone(), false),
            transcribe::spawn(&test_config().whisper, &paths),
            test_visuals(&paths),
            paths.music.clone(),
        );
        lib_tx
            .send(LibraryEvent::Tracks(db.tracks().unwrap()))
            .unwrap();
        wait(&mut app, "tracks loaded", |a| a.tracks.len() == 2);
        assert_eq!(
            app.selected_track().map(|t| t.title.as_str()),
            Some("Alpha")
        );

        render_all_sizes(&mut app); // library with tracks
        // Search narrows the list and Esc brings everything back.
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "bet");
        render_all_sizes(&mut app); // search box
        assert_eq!(app.view_len(), 1);
        assert_eq!(app.selected_track().map(|t| t.title.as_str()), Some("Beta"));
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.view_len(), 2);

        // Create a playlist on the Playlists tab (typing 'q' must not quit).
        key(&mut app, KeyCode::Tab);
        render_all_sizes(&mut app); // empty playlist list
        key(&mut app, KeyCode::Char('c'));
        type_text(&mut app, "Quiet mix");
        render_all_sizes(&mut app); // prompt popup
        key(&mut app, KeyCode::Enter);
        wait(&mut app, "playlist created", |a| a.playlist_data.len() == 1);
        assert!(!app.should_quit);
        assert!(paths.playlists.join("Quiet mix").is_dir());

        // Favorite the first track from the Library tab.
        key(&mut app, KeyCode::BackTab);
        key(&mut app, KeyCode::Char('*'));
        wait(&mut app, "favorite added", |a| a.favorites.len() == 1);
        assert!(paths.favorites.join("alpha.wav").is_file());

        // Add the second track to the playlist through the picker.
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Char('a'));
        render_all_sizes(&mut app); // picker popup
        key(&mut app, KeyCode::Enter);
        wait(&mut app, "track in playlist", |a| {
            a.playlist_data.first().is_some_and(|p| p.tracks.len() == 1)
        });
        assert!(paths.playlists.join("Quiet mix").join("beta.wav").is_file());

        // Open the playlist, remove the track again.
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.view_len(), 1);
        key(&mut app, KeyCode::Char('d'));
        wait(&mut app, "track removed", |a| {
            a.playlist_data.first().is_some_and(|p| p.tracks.is_empty())
        });
        assert!(!paths.playlists.join("Quiet mix").join("beta.wav").exists());
        assert!(paths.music.join("beta.wav").is_file());

        // Delete the playlist only after confirming.
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('d'));
        render_all_sizes(&mut app); // confirm popup
        key(&mut app, KeyCode::Char('n'));
        assert_eq!(app.playlist_data.len(), 1, "cancelled");
        key(&mut app, KeyCode::Char('d'));
        key(&mut app, KeyCode::Char('y'));
        wait(&mut app, "playlist deleted", |a| a.playlist_data.is_empty());
        assert!(!paths.playlists.join("Quiet mix").exists());
    }
}

#[cfg(test)]
mod lyrics_tests {
    use super::*;
    use crate::db::{Db, TrackRecord};
    use crate::lyrics::Lyrics;
    use crate::paths::Paths;
    use crate::testutil::wav_bytes;
    use std::sync::mpsc;

    fn key(app: &mut App, code: KeyCode) {
        app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn wait(app: &mut App, what: &str, cond: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            app.drain_library_events();
            app.drain_lyrics_events();
            app.sync_lyrics_request();
            if cond(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for: {what}");
    }

    fn render(app: &mut App, w: u16, h: u16) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
    }

    #[test]
    fn lyrics_follow_the_playing_track_and_can_be_shifted() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        paths.ensure_dirs().unwrap();
        let db = Db::open(&paths.db).unwrap();
        std::fs::write(paths.music.join("song.wav"), wav_bytes(100)).unwrap();
        std::fs::write(
            paths.music.join("song.lrc"),
            "[00:01.00]first\n[00:02.00]second\n",
        )
        .unwrap();
        std::fs::write(paths.music.join("quiet.wav"), wav_bytes(100)).unwrap();
        for file in ["song.wav", "quiet.wav"] {
            db.upsert_track(&TrackRecord {
                path: file.into(),
                size: 1,
                mtime_ms: 1,
                title: file.into(),
                artist: "Band".into(),
                album: String::new(),
                duration_ms: 100,
            })
            .unwrap();
        }

        let (lib_tx, lib_rx) = mpsc::channel();
        let mut app = App::new(
            &test_config(),
            None,
            lib_rx,
            playlists::spawn(&paths),
            lyrics::spawn(paths.db.clone(), paths.lyrics.clone(), false),
            transcribe::spawn(&test_config().whisper, &paths),
            test_visuals(&paths),
            paths.music.clone(),
        );
        lib_tx
            .send(LibraryEvent::Tracks(db.tracks().unwrap()))
            .unwrap();
        wait(&mut app, "tracks", |a| a.tracks.len() == 2);
        render(&mut app, 120, 30); // Idle

        // "Play" the track that has a .lrc file (no audio device needed).
        let song_id = app.tracks.iter().find(|t| t.path == "song.wav").unwrap().id;
        app.player.now_id = Some(song_id);
        render(&mut app, 120, 30); // Searching
        wait(&mut app, "lyrics loaded", |a| {
            matches!(a.lyrics_state, LyricsState::Ready { .. })
        });
        render(&mut app, 120, 30);
        render(&mut app, 30, 6);

        // Shifting changes the offset and is remembered by the worker.
        key(&mut app, KeyCode::Char(']'));
        key(&mut app, KeyCode::Char(']'));
        key(&mut app, KeyCode::Char('['));
        match &app.lyrics_state {
            LyricsState::Ready {
                offset_ms, lyrics, ..
            } => {
                assert_eq!(*offset_ms, 100);
                assert!(matches!(lyrics, Lyrics::Synced(l) if l.len() == 2));
            }
            _ => panic!("lyrics not ready"),
        }

        // A track without any lyrics: not found, retry does not crash.
        let quiet_id = app
            .tracks
            .iter()
            .find(|t| t.path == "quiet.wav")
            .unwrap()
            .id;
        app.player.now_id = Some(quiet_id);
        wait(&mut app, "not found", |a| {
            matches!(a.lyrics_state, LyricsState::NotFound { .. })
        });
        render(&mut app, 120, 30);
        key(&mut app, KeyCode::Char('t'));
        wait(&mut app, "not found again", |a| {
            matches!(a.lyrics_state, LyricsState::NotFound { .. })
        });
        key(&mut app, KeyCode::Char('['));
        assert!(app.status.contains("synchronized"), "{}", app.status);

        // The offset of the first track survives switching back to it.
        app.player.now_id = Some(song_id);
        wait(&mut app, "lyrics reloaded", |a| {
            matches!(a.lyrics_state, LyricsState::Ready { .. })
        });
        match &app.lyrics_state {
            LyricsState::Ready { offset_ms, .. } => assert_eq!(*offset_ms, 100),
            _ => panic!("lyrics not ready"),
        }

        // Hiding lyrics still renders; stopping resets the panel.
        key(&mut app, KeyCode::Char('l'));
        render(&mut app, 120, 30);
        app.player.now_id = None;
        app.sync_lyrics_request();
        assert!(matches!(app.lyrics_state, LyricsState::Idle));
    }
}

/// Picture worker for tests: half blocks, no terminal queries.
#[cfg(test)]
fn test_visuals(paths: &crate::paths::Paths) -> visuals::Handle {
    visuals::spawn(paths, ratatui_image::picker::Picker::halfblocks(), true)
}

/// Config for tests: no automatic speech recognition in the background.
#[cfg(test)]
fn test_config() -> Config {
    let mut config = Config::default();
    config.whisper.enabled = false;
    config
}

#[cfg(all(test, feature = "whisper"))]
mod whisper_tests {
    use super::*;
    use crate::db::{Db, TrackRecord};
    use crate::paths::Paths;
    use crate::testutil::wav_bytes;
    use std::sync::mpsc;

    fn key(app: &mut App, code: KeyCode) {
        app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn wait(app: &mut App, what: &str, cond: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            app.drain_library_events();
            app.drain_lyrics_events();
            app.drain_transcribe_events();
            app.sync_lyrics_request();
            app.tick_whisper();
            if cond(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for: {what}");
    }

    fn render(app: &mut App) {
        for (w, h) in [(120, 30), (60, 10)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| app.draw(f)).unwrap();
        }
    }

    #[test]
    fn missing_model_is_only_downloaded_after_asking() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        paths.ensure_dirs().unwrap();
        let db = Db::open(&paths.db).unwrap();
        std::fs::write(paths.music.join("a.wav"), wav_bytes(100)).unwrap();
        db.upsert_track(&TrackRecord {
            path: "a.wav".into(),
            size: 1,
            mtime_ms: 1,
            title: "A".into(),
            artist: "Band".into(),
            album: String::new(),
            duration_ms: 100,
        })
        .unwrap();

        let mut config = Config::default();
        config.whisper.enabled = true;
        config.whisper.model = "tiny".into();
        config.lyrics.online = false;
        let (lib_tx, lib_rx) = mpsc::channel();
        let mut app = App::new(
            &config,
            None,
            lib_rx,
            playlists::spawn(&paths),
            lyrics::spawn(paths.db.clone(), paths.lyrics.clone(), false),
            transcribe::spawn(&config.whisper, &paths),
            test_visuals(&paths),
            paths.music.clone(),
        );
        lib_tx
            .send(LibraryEvent::Tracks(db.tracks().unwrap()))
            .unwrap();
        wait(&mut app, "tracks", |a| a.tracks.len() == 1);

        // "Play" a track that has no lyrics anywhere.
        app.player.now_id = Some(app.tracks[0].id);
        wait(&mut app, "not found", |a| {
            matches!(a.lyrics_state, LyricsState::NotFound { .. })
        });
        assert!(
            app.auto_pending.is_some(),
            "automatic recognition scheduled"
        );
        render(&mut app);

        // The user asks for recognition: the model is missing, so ask first.
        key(&mut app, KeyCode::Char('t'));
        wait(&mut app, "download question", |a| {
            matches!(a.mode, Mode::ConfirmModel { .. })
        });
        match &app.mode {
            Mode::ConfirmModel { model, size_mb } => {
                assert_eq!((model.as_str(), *size_mb), ("tiny", 75));
            }
            _ => unreachable!(),
        }
        render(&mut app); // popup

        // "No": nothing is downloaded, the user is told how to retry.
        key(&mut app, KeyCode::Char('n'));
        assert!(matches!(app.mode, Mode::Normal));
        wait(&mut app, "decline message", |a| {
            a.status.contains("declined")
        });
        assert!(!paths.models.join("ggml-tiny.bin").exists());
        assert!(std::fs::read_dir(&paths.models).unwrap().next().is_none());

        // Pressing t again asks again.
        key(&mut app, KeyCode::Char('t'));
        wait(&mut app, "second question", |a| {
            matches!(a.mode, Mode::ConfirmModel { .. })
        });
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn disabled_whisper_never_asks_and_t_searches_again() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        paths.ensure_dirs().unwrap();
        let mut config = Config::default();
        config.whisper.enabled = false;
        let (_lib_tx, lib_rx) = mpsc::channel();
        let mut app = App::new(
            &config,
            None,
            lib_rx,
            playlists::spawn(&paths),
            lyrics::spawn(paths.db.clone(), paths.lyrics.clone(), false),
            transcribe::spawn(&config.whisper, &paths),
            test_visuals(&paths),
            paths.music.clone(),
        );
        key(&mut app, KeyCode::Char('t'));
        app.tick_whisper();
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.auto_pending.is_none());
        assert!(app.status.contains("disabled") || app.status.contains("Nothing"));
    }
}
