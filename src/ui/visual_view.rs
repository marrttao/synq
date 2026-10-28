//! The picture panel: which GIF belongs to what is playing, the animation
//! clock and the picker dialog (`g`).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Rect, Size};
use ratatui::style::Style;
use ratatui::widgets::{ListState, Paragraph, Wrap};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, StatefulImage};

use crate::visuals::{self, Animation, Clock, Command, Event};

use super::{App, Mode, View};

/// Decoded pictures kept besides the one on screen (e.g. other playlists').
const SPARE: usize = 2;
/// After a stall (suspended laptop, huge frame) do not fast-forward.
const MAX_STEP: Duration = Duration::from_millis(250);

struct Active {
    name: String,
    frames: Vec<StatefulProtocol>,
    delays: Vec<Duration>,
    clock: Clock,
}

impl Active {
    fn new(picker: &Picker, name: String, animation: Animation) -> Self {
        let mut frames = Vec::new();
        let mut delays = Vec::new();
        for f in animation.frames {
            delays.push(f.delay);
            frames.push(picker.new_resize_protocol(f.image));
        }
        Self {
            name,
            frames,
            delays,
            clock: Clock::default(),
        }
    }
}

pub(super) struct VisualState {
    handle: visuals::Handle,
    global: Option<String>,
    per_playlist: HashMap<i64, String>,
    active: Option<Active>,
    spare: Vec<Active>,
    /// Name a load was requested for and has not come back yet.
    requested: Option<String>,
    /// Pictures that failed to load; not retried until chosen again.
    failed: HashSet<String>,
    error: Option<String>,
    last_tick: Instant,
    /// Playlist the current queue was started from.
    pub(super) play_context: Option<i64>,
    /// Only a complete playlist list may be used to forget removed playlists.
    snapshot_seen: bool,
    /// The user chose something already; older saved choices must not win.
    chosen: bool,
}

impl VisualState {
    pub(super) fn new(handle: visuals::Handle) -> Self {
        Self {
            handle,
            global: None,
            per_playlist: HashMap::new(),
            active: None,
            spare: Vec::new(),
            requested: None,
            failed: HashSet::new(),
            error: None,
            last_tick: Instant::now(),
            play_context: None,
            snapshot_seen: false,
            chosen: false,
        }
    }
}

/// Should the animation clock run? It stops together with the music when
/// `pause_with_music` is on.
fn clock_runs(pause_with_music: bool, playing: bool) -> bool {
    !pause_with_music || playing
}

impl App {
    /// The picture for what is playing: the playlist's own, else the global.
    fn desired_gif(&self) -> Option<String> {
        let own = self
            .visual
            .play_context
            .and_then(|id| self.visual.per_playlist.get(&id));
        own.or(self.visual.global.as_ref()).cloned()
    }

    pub(super) fn drain_visual_events(&mut self) {
        while let Ok(ev) = self.visual.handle.events.try_recv() {
            match ev {
                Event::Choices { global, playlists } => {
                    if !self.visual.chosen {
                        self.visual.global = global;
                        self.visual.per_playlist = playlists;
                    }
                    if self.visual.snapshot_seen {
                        self.forget_removed_playlists();
                    }
                }
                Event::Loaded { name, animation } => {
                    if self.visual.requested.as_deref() == Some(name.as_str()) {
                        self.visual.requested = None;
                    }
                    self.visual.error = None;
                    let loaded = Active::new(&self.visual.handle.picker, name.clone(), animation);
                    let wanted = self.desired_gif().as_deref() == Some(name.as_str());
                    if wanted {
                        if let Some(old) = self.visual.active.replace(loaded) {
                            self.keep_spare(old);
                        }
                    } else {
                        self.keep_spare(loaded);
                    }
                }
                Event::Failed { name, message } => {
                    if self.visual.requested.as_deref() == Some(name.as_str()) {
                        self.visual.requested = None;
                    }
                    self.status = format!("Cannot show {name}: {message}");
                    self.visual.error = Some(format!("Cannot show {name}: {message}"));
                    self.visual.failed.insert(name);
                    self.visual.active = None;
                }
            }
        }
    }

    fn keep_spare(&mut self, item: Active) {
        self.visual.spare.retain(|a| a.name != item.name);
        self.visual.spare.push(item);
        if self.visual.spare.len() > SPARE {
            self.visual.spare.remove(0);
        }
    }

    /// Called after every playlist snapshot.
    pub(super) fn playlists_changed(&mut self) {
        self.visual.snapshot_seen = true;
        self.forget_removed_playlists();
    }

    /// Ids are reused by SQLite, so a deleted playlist's picture must go.
    fn forget_removed_playlists(&mut self) {
        let gone: Vec<i64> = self
            .visual
            .per_playlist
            .keys()
            .copied()
            .filter(|id| !self.playlist_data.iter().any(|p| p.id == *id))
            .collect();
        for id in gone {
            self.visual.per_playlist.remove(&id);
            self.visual
                .handle
                .send(Command::SetPlaylist { id, name: None });
        }
        if let Some(id) = self.visual.play_context
            && !self.playlist_data.iter().any(|p| p.id == id)
        {
            self.visual.play_context = None;
        }
    }

    /// Every frame: load what is wanted and move the animation on.
    pub(super) fn tick_visuals(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.visual.last_tick).min(MAX_STEP);
        self.visual.last_tick = now;

        match self.desired_gif() {
            None => {
                self.visual.active = None;
                self.visual.error = None;
            }
            Some(name) => {
                if self.visual.active.as_ref().is_none_or(|a| a.name != name) {
                    if let Some(i) = self.visual.spare.iter().position(|a| a.name == name) {
                        let found = self.visual.spare.remove(i);
                        if let Some(old) = self.visual.active.replace(found) {
                            self.keep_spare(old);
                        }
                    } else if self.visual.requested.as_deref() != Some(name.as_str())
                        && !self.visual.failed.contains(&name)
                    {
                        self.visual.requested = Some(name.clone());
                        self.visual.handle.send(Command::Load(name));
                    }
                }
            }
        }

        let playing = self.player.now_id.is_some() && !self.player.paused;
        if clock_runs(self.visual.handle.pause_with_music, playing)
            && let Some(active) = &mut self.visual.active
        {
            active.clock.advance(dt, &active.delays);
        }
    }

    pub(super) fn draw_visual(&mut self, f: &mut Frame, area: Rect) {
        let title = match &self.visual.active {
            Some(a) => format!("Visual: {}", a.name),
            None => "Visual".to_owned(),
        };
        let block = self.block(&title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        if let Some(active) = &mut self.visual.active
            && !active.frames.is_empty()
        {
            let index = active.clock.index().min(active.frames.len() - 1);
            let proto = &mut active.frames[index];
            let fit = Size::new(inner.width, inner.height);
            let size = proto.size_for(Resize::Scale(None), fit);
            let (w, h) = (size.width.min(inner.width), size.height.min(inner.height));
            let rect = Rect {
                x: inner.x + (inner.width - w) / 2,
                y: inner.y + (inner.height - h) / 2,
                width: w,
                height: h,
            };
            f.render_stateful_widget(
                StatefulImage::<StatefulProtocol>::default().resize(Resize::Scale(None)),
                rect,
                proto,
            );
            return;
        }
        let text = if let Some(e) = &self.visual.error {
            e.clone()
        } else if self.desired_gif().is_some() {
            "Loading...".to_owned()
        } else {
            "Press g to choose a picture from gifs/".to_owned()
        };
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: true })
                .style(Style::new().fg(self.theme.dim)),
            inner,
        );
    }

    // ---- picker (`g`) -----------------------------------------------------

    pub(super) fn start_gif_pick(&mut self) {
        let names = visuals::list_images(&self.visual.handle.dir);
        if names.is_empty() {
            self.status = "gifs/ is empty: drop GIF, PNG or JPG files there".into();
            return;
        }
        let playlist = match self.view() {
            View::PlaylistTracks(id) => self.playlist_data.iter().find(|p| p.id == id),
            View::PlaylistList => self.selected_playlist(),
            _ => None,
        }
        .map(|p| (p.id, p.name.clone()));
        let to_playlist = playlist.is_some();
        let mut state = ListState::default();
        state.select(Some(self.current_choice_row(
            &names,
            playlist.as_ref(),
            to_playlist,
        )));
        self.mode = Mode::GifPick {
            names,
            state,
            playlist,
            to_playlist,
        };
    }

    /// Row of the list that holds the current choice for the target (0 = none).
    pub(super) fn current_choice_row(
        &self,
        names: &[String],
        playlist: Option<&(i64, String)>,
        to_playlist: bool,
    ) -> usize {
        let current = match (to_playlist, playlist) {
            (true, Some((id, _))) => self.visual.per_playlist.get(id),
            _ => self.visual.global.as_ref(),
        };
        current
            .and_then(|c| names.iter().position(|n| n == c))
            .map_or(0, |i| i + 1)
    }

    pub(super) fn key_gif_pick(&mut self, key: KeyEvent) {
        let Mode::GifPick {
            names,
            state,
            playlist,
            to_playlist,
        } = &mut self.mode
        else {
            return;
        };
        let rows = names.len() + 1;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'g') => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') => {
                let next = (state.selected().unwrap_or(0) + 1).min(rows - 1);
                state.select(Some(next));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.select(Some(state.selected().unwrap_or(0).saturating_sub(1)));
            }
            KeyCode::Tab if playlist.is_some() => {
                *to_playlist = !*to_playlist;
                let (names, playlist, to_playlist) =
                    (names.clone(), playlist.clone(), *to_playlist);
                let row = self.current_choice_row(&names, playlist.as_ref(), to_playlist);
                if let Mode::GifPick { state, .. } = &mut self.mode {
                    state.select(Some(row));
                }
            }
            KeyCode::Enter => {
                let chosen = state
                    .selected()
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| names.get(i))
                    .cloned();
                let target = if *to_playlist { playlist.clone() } else { None };
                self.mode = Mode::Normal;
                self.apply_gif_choice(target, chosen);
            }
            _ => {}
        }
    }

    fn apply_gif_choice(&mut self, playlist: Option<(i64, String)>, name: Option<String>) {
        self.visual.chosen = true;
        if let Some(n) = &name {
            self.visual.failed.remove(n);
        }
        let shown = name.clone().unwrap_or_else(|| "none".into());
        match playlist {
            Some((id, pl_name)) => {
                match &name {
                    Some(n) => self.visual.per_playlist.insert(id, n.clone()),
                    None => self.visual.per_playlist.remove(&id),
                };
                self.visual.handle.send(Command::SetPlaylist { id, name });
                self.status = format!("GIF of playlist \"{pl_name}\": {shown}");
            }
            None => {
                self.visual.global.clone_from(&name);
                self.visual.handle.send(Command::SetGlobal(name));
                self.status = format!("Global GIF: {shown}");
            }
        }
    }

    /// Title and rows of the picker dialog.
    pub(super) fn gif_pick_rows(
        visual: &VisualState,
        names: &[String],
        playlist: Option<&(i64, String)>,
        to_playlist: bool,
    ) -> (String, Vec<String>) {
        let target_playlist = if to_playlist { playlist } else { None };
        let title = match (target_playlist, playlist) {
            (Some((_, n)), _) => format!("GIF for playlist \"{n}\" (Tab: global)"),
            (None, Some((_, n))) => format!("Global GIF (Tab: playlist \"{n}\")"),
            (None, None) => "Global GIF (Enter, Esc)".to_owned(),
        };
        let current = match target_playlist {
            Some((id, _)) => visual.per_playlist.get(id),
            None => visual.global.as_ref(),
        };
        let none = if target_playlist.is_some() {
            "(use the global one)"
        } else {
            "(none)"
        };
        let mut rows = vec![none.to_owned()];
        rows.extend(names.iter().map(|n| {
            if current == Some(n) {
                format!("{n}  *")
            } else {
                n.clone()
            }
        }));
        (title, rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_pauses_with_the_music_only_when_configured() {
        assert!(clock_runs(true, true));
        assert!(!clock_runs(true, false));
        assert!(clock_runs(false, false));
        assert!(clock_runs(false, true));
    }

    use crate::paths::Paths;
    use crate::ui::{
        Event, KeyModifiers, lyrics, playlists, test_config, test_visuals, transcribe,
    };
    use std::sync::mpsc;

    fn key(app: &mut App, code: KeyCode) {
        app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn make_app(paths: &Paths) -> App {
        let (_tx, rx) = mpsc::channel();
        App::new(
            &test_config(),
            None,
            rx,
            playlists::spawn(paths),
            lyrics::spawn(paths.db.clone(), paths.lyrics.clone(), false),
            transcribe::spawn(&test_config().whisper, paths),
            test_visuals(paths),
            paths.music.clone(),
        )
    }

    fn pump(app: &mut App, what: &str, cond: impl Fn(&App) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            app.drain_playlist_events();
            app.drain_visual_events();
            app.tick_visuals();
            if cond(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for: {what} (status: {})", app.status);
    }

    fn render(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn setup() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        paths.ensure_dirs().unwrap();
        drop(crate::db::Db::open(&paths.db).unwrap());
        (tmp, paths)
    }

    #[test]
    fn empty_gifs_folder_says_what_to_do() {
        let (_tmp, paths) = setup();
        let mut app = make_app(&paths);
        key(&mut app, KeyCode::Char('g'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status.contains("gifs/"));
    }

    #[test]
    fn choosing_a_gif_shows_it_and_the_clock_follows_the_music() {
        let (_tmp, paths) = setup();
        std::fs::write(
            paths.gifs.join("a.gif"),
            visuals::gif_bytes(8, 8, &[([255, 0, 0], 60), ([0, 0, 255], 60)]),
        )
        .unwrap();
        let mut app = make_app(&paths);

        // The panel invites to choose before anything is picked.
        let text = |b: &ratatui::buffer::Buffer| {
            b.content().iter().map(|c| c.symbol()).collect::<String>()
        };
        assert!(text(&render(&mut app, 120, 30)).contains("Press g"));

        key(&mut app, KeyCode::Char('g'));
        assert!(matches!(app.mode, Mode::GifPick { .. }));
        render(&mut app, 120, 30); // the dialog
        key(&mut app, KeyCode::Down); // row 0 is "(none)"
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.visual.global.as_deref(), Some("a.gif"));

        pump(&mut app, "gif loaded", |a| a.visual.active.is_some());
        let buf = render(&mut app, 120, 30);
        assert!(text(&buf).contains("Visual: a.gif"));
        assert!(text(&buf).contains('\u{2580}'), "half blocks are drawn");

        // Nothing is playing and the animation pauses with the music: still.
        std::thread::sleep(Duration::from_millis(150));
        app.tick_visuals();
        std::thread::sleep(Duration::from_millis(150));
        app.tick_visuals();
        let index = |a: &App| a.visual.active.as_ref().map(|x| x.clock.index());
        assert_eq!(index(&app), Some(0));

        // With the option off it runs by itself.
        app.visual.handle.pause_with_music = false;
        pump(&mut app, "frame change", |a| index(a) == Some(1));

        // Every size renders without panicking.
        for (w, h) in [(120, 30), (80, 20), (110, 5), (10, 4), (1, 1)] {
            render(&mut app, w, h);
        }
    }

    #[test]
    fn a_playlist_has_its_own_gif_and_the_global_is_the_fallback() {
        let (_tmp, paths) = setup();
        let mut app = make_app(&paths);
        app.playlist_data = vec![
            playlists::PlaylistInfo {
                id: 5,
                name: "Mix".into(),
                tracks: vec![],
            },
            playlists::PlaylistInfo {
                id: 6,
                name: "Other".into(),
                tracks: vec![],
            },
        ];
        app.apply_gif_choice(None, Some("global.gif".into()));
        app.apply_gif_choice(Some((5, "Mix".into())), Some("mix.gif".into()));
        assert_eq!(app.desired_gif().as_deref(), Some("global.gif"));
        app.visual.play_context = Some(5);
        assert_eq!(app.desired_gif().as_deref(), Some("mix.gif"));
        app.visual.play_context = Some(6);
        assert_eq!(app.desired_gif().as_deref(), Some("global.gif"));

        // Choosing "none" for the playlist hands it back to the global one.
        app.visual.play_context = Some(5);
        app.apply_gif_choice(Some((5, "Mix".into())), None);
        assert_eq!(app.desired_gif().as_deref(), Some("global.gif"));

        // Deleting a playlist forgets its picture (ids get reused).
        app.apply_gif_choice(Some((5, "Mix".into())), Some("mix.gif".into()));
        app.playlist_data.retain(|p| p.id != 5);
        app.playlists_changed();
        assert!(!app.visual.per_playlist.contains_key(&5));
        assert_eq!(app.visual.play_context, None);
    }

    #[test]
    fn a_broken_picture_is_reported_and_not_retried() {
        let (_tmp, paths) = setup();
        std::fs::write(paths.gifs.join("bad.gif"), b"not a gif").unwrap();
        let mut app = make_app(&paths);
        app.apply_gif_choice(None, Some("bad.gif".into()));
        pump(&mut app, "failure", |a| a.visual.failed.contains("bad.gif"));
        assert!(app.status.contains("bad.gif"));
        assert!(app.visual.active.is_none());
        assert!(app.visual.requested.is_none());
        let buf = render(&mut app, 120, 30);
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Cannot show"));
        // Picking it again retries.
        app.apply_gif_choice(None, Some("bad.gif".into()));
        assert!(!app.visual.failed.contains("bad.gif"));
    }

    #[test]
    fn the_dialog_switches_between_playlist_and_global() {
        let (_tmp, paths) = setup();
        std::fs::write(
            paths.gifs.join("a.gif"),
            visuals::gif_bytes(4, 4, &[([1, 1, 1], 50)]),
        )
        .unwrap();
        let mut app = make_app(&paths);
        app.playlist_data = vec![playlists::PlaylistInfo {
            id: 5,
            name: "Mix".into(),
            tracks: vec![],
        }];
        app.tab = 1;
        app.open_playlist = Some(5);
        key(&mut app, KeyCode::Char('g'));
        let Mode::GifPick { to_playlist, .. } = &app.mode else {
            panic!("dialog expected");
        };
        assert!(*to_playlist, "inside a playlist the dialog targets it");
        key(&mut app, KeyCode::Tab);
        let Mode::GifPick { to_playlist, .. } = &app.mode else {
            panic!("dialog expected");
        };
        assert!(!*to_playlist);
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
    }
}
