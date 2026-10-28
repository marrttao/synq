//! The picture panel: which GIF belongs to what is playing, the animation
//! clock and the picker dialog (`g`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::DynamicImage;
use image::imageops::FilterType;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListState, Paragraph, Wrap};
use ratatui_image::picker::ProtocolType;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, StatefulImage};

use crate::visuals::{self, Animation, Clock, Command, Event};

use super::blocks;
use super::render::{self, Kind, Renderer};
use super::{App, Mode, View};

/// Decoded pictures kept besides the one on screen (e.g. other playlists').
const SPARE: usize = 2;
/// After a stall (suspended laptop, huge frame) do not fast-forward.
const MAX_STEP: Duration = Duration::from_millis(250);

/// How the picture is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VisualMode {
    /// Real pixels where the terminal can (sixel, kitty, iTerm2). Sharp,
    /// but heavy for the terminal, and text cannot be drawn over it.
    Picture,
    /// Half-block characters: smooth, and other panels can lie over it.
    Blocks,
    /// Characters picked by brightness, tinted with the pixel color.
    Ascii,
}

/// How the picture is fitted to its panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VisualFit {
    /// Covers the whole panel; what does not fit its proportions is cut off.
    Fill,
    /// The whole picture with its proportions; the free space is filled with
    /// a blurred copy of it. Nothing is cut off.
    Fit,
    /// The whole picture stretched over the whole panel.
    Stretch,
}

impl VisualFit {
    pub(super) fn from_config(name: &str) -> Self {
        match name.trim().to_lowercase().as_str() {
            "fill" => Self::Fill,
            "stretch" => Self::Stretch,
            _ => Self::Fit,
        }
    }

    pub(super) fn cycle(self) -> Self {
        match self {
            Self::Fill => Self::Fit,
            Self::Fit => Self::Stretch,
            Self::Stretch => Self::Fill,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Fill => "fill (cropped)",
            Self::Fit => "fit (whole picture)",
            Self::Stretch => "stretch",
        }
    }
}

impl VisualMode {
    pub(super) fn from_config(name: &str) -> Self {
        match name.trim().to_lowercase().as_str() {
            "ascii" => Self::Ascii,
            "blocks" | "halfblocks" => Self::Blocks,
            _ => Self::Picture,
        }
    }

    pub(super) fn cycle(self) -> Self {
        match self {
            Self::Picture => Self::Blocks,
            Self::Blocks => Self::Ascii,
            Self::Ascii => Self::Picture,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Picture => "Picture mode (pixels)",
            Self::Blocks => "Blocks mode (smooth)",
            Self::Ascii => "ASCII mode",
        }
    }

    /// Pixel graphics are drawn after all text, so a panel lying over the
    /// picture would be hidden by it; and without a graphics protocol the
    /// terminal would only get the coarse half blocks. Both are the job of
    /// the (better) blocks mode.
    pub(super) fn effective(self, overlapped: bool, graphics: bool) -> Self {
        if self == Self::Picture && (overlapped || !graphics) {
            Self::Blocks
        } else {
            self
        }
    }
}

/// Brightness ramp for the ASCII mode, dark to light.
const RAMP: &[u8] = br#" .'`^",:;Il!i><~+_-?][}{1)(|\/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$"#;

/// A frame made for the panel, ready to show.
enum Cached {
    Blocks(blocks::Grid),
    Ascii(Vec<Line<'static>>),
    Pixels(StatefulProtocol),
}

pub(super) struct Active {
    name: String,
    /// Frames as stored; the panel versions are made from them in the
    /// background (see `render.rs`).
    images: Arc<Vec<DynamicImage>>,
    delays: Vec<Duration>,
    clock: Clock,
    /// What the frames below were requested for, and which batch of jobs
    /// they belong to.
    key: Option<render::Key>,
    generation: u64,
    cache: Vec<Option<Cached>>,
    pending: Vec<bool>,
}

impl Active {
    fn new(name: String, animation: Animation) -> Self {
        let mut images = Vec::new();
        let mut delays = Vec::new();
        for f in animation.frames {
            delays.push(f.delay);
            images.push(f.image);
        }
        let n = images.len();
        Self {
            name,
            images: Arc::new(images),
            delays,
            clock: Clock::default(),
            key: None,
            generation: 0,
            cache: (0..n).map(|_| None).collect(),
            pending: vec![false; n],
        }
    }

    /// The frame to show: the current one if it is ready, else the latest
    /// earlier one (the picture then just holds still for a moment).
    fn frame_to_show(&self, index: usize) -> Option<usize> {
        let n = self.cache.len();
        (0..n)
            .map(|back| (index + n - back) % n)
            .find(|&i| self.cache[i].is_some())
    }

    /// Frames that pixel pictures may keep: they are big.
    fn keep_limit(key: &render::Key, frames: usize) -> usize {
        match key.kind {
            Kind::Pixels { width, height } => {
                let bytes = (width as usize * height as usize * 4).max(1);
                ((160usize << 20) / bytes).clamp(6, frames.max(6))
            }
            _ => frames,
        }
    }

    /// Drop the frames furthest behind the current one when too many are kept.
    fn evict(&mut self, index: usize) {
        let Some(key) = self.key else { return };
        let n = self.cache.len();
        let limit = Self::keep_limit(&key, n);
        let mut kept: Vec<usize> = (0..n).filter(|&i| self.cache[i].is_some()).collect();
        // Furthest behind first, in playing order.
        kept.sort_by_key(|&i| std::cmp::Reverse((index + n - i) % n));
        let extra = kept.len().saturating_sub(limit);
        for &i in kept.iter().take(extra) {
            self.cache[i] = None;
        }
    }
}

/// The picture made exactly `w` x `h` pixels, the way `fit` says. `crop` is
/// how much of the picture (percent, of the side that overflows) the "fit"
/// mode may cut off to make the picture bigger; the rest of the panel is
/// filled with a blurred copy.
pub(super) fn scaled(
    image: &DynamicImage,
    w: u32,
    h: u32,
    fit: VisualFit,
    crop: u8,
) -> DynamicImage {
    let (w, h) = (w.max(1), h.max(1));
    let filter = FilterType::CatmullRom;
    match fit {
        VisualFit::Fill => image.resize_to_fill(w, h, filter),
        VisualFit::Stretch => image.resize_exact(w, h, filter),
        VisualFit::Fit => {
            let (iw, ih) = (image.width() as f32, image.height() as f32);
            // The scale at which the whole picture fits, and the one at
            // which it covers the panel.
            let whole = (w as f32 / iw).min(h as f32 / ih);
            let cover = (w as f32 / iw).max(h as f32 / ih);
            let keep = 1.0 - f32::from(crop.min(90)) / 100.0;
            let scale = cover.min(whole / keep);
            // The part of the picture that is on the screen, centered.
            let (vis_w, vis_h) = ((w as f32 / scale).min(iw), (h as f32 / scale).min(ih));
            let window = image.crop_imm(
                ((iw - vis_w) / 2.0) as u32,
                ((ih - vis_h) / 2.0) as u32,
                vis_w.round().max(1.0) as u32,
                vis_h.round().max(1.0) as u32,
            );
            let near = |size: f32, limit: u32| {
                let n = size.round().max(1.0) as u32;
                // A line of backdrop one pixel wide would only look like a flaw.
                if limit.saturating_sub(n) <= 1 {
                    limit
                } else {
                    n.min(limit)
                }
            };
            let (out_w, out_h) = (near(vis_w * scale, w), near(vis_h * scale, h));
            let part = window.resize_exact(out_w, out_h, filter).to_rgba8();
            if part.width() == w && part.height() == h {
                return DynamicImage::ImageRgba8(part);
            }
            // The free space is filled with a blurred, darker copy of the
            // picture, as video players do.
            let backdrop = image
                .resize_to_fill((w / 16).max(1), (h / 16).max(1), FilterType::Triangle)
                .resize_exact(w, h, FilterType::Triangle)
                .brighten(-45);
            let mut canvas = backdrop.to_rgba8();
            let x = i64::from((w - part.width()) / 2);
            let y = i64::from((h - part.height()) / 2);
            image::imageops::overlay(&mut canvas, &part, x, y);
            DynamicImage::ImageRgba8(canvas)
        }
    }
}

/// One text line per panel row, `cols` characters wide. The character
/// follows the brightness (stretched to the picture's own range), its color
/// is the pixel's and the cell background is a dark shade of it.
pub(super) fn ascii_lines(
    image: &DynamicImage,
    cols: u16,
    rows: u16,
    fit: VisualFit,
    crop: u8,
) -> Vec<Line<'static>> {
    // A character cell is about twice as tall as it is wide.
    let img = scaled(image, u32::from(cols), u32::from(rows) * 2, fit, crop).to_rgba8();
    let luma =
        |c: [u8; 3]| (u32::from(c[0]) * 299 + u32::from(c[1]) * 587 + u32::from(c[2]) * 114) / 1000;
    let cell = |col: u32, row: u32| -> ([u8; 3], u32) {
        let (mut sum, mut alpha) = ([0u32; 3], 0u32);
        for y in [row * 2, row * 2 + 1] {
            let p = img.get_pixel(col, y.min(img.height() - 1));
            let a = u32::from(p[3]);
            for i in 0..3 {
                sum[i] += u32::from(p[i]) * a / 255;
            }
            alpha += a;
        }
        (sum.map(|c| (c / 2) as u8), alpha / 2)
    };
    let (mut lo, mut hi) = (255u32, 0u32);
    for row in 0..u32::from(rows) {
        for col in 0..u32::from(cols) {
            let (c, alpha) = cell(col, row);
            if alpha >= 128 {
                let l = luma(c);
                lo = lo.min(l);
                hi = hi.max(l);
            }
        }
    }
    (0..u32::from(rows))
        .map(|row| {
            let spans: Vec<Span<'static>> = (0..u32::from(cols))
                .map(|col| {
                    let (c, alpha) = cell(col, row);
                    let idx = if alpha < 40 {
                        0
                    } else {
                        // Stretch to the range of this picture, then a gamma that
                        // gives the dark half more characters.
                        let l = luma(c);
                        let t = if hi > lo + 8 {
                            (l.saturating_sub(lo)) as f32 / (hi - lo) as f32
                        } else {
                            l as f32 / 255.0
                        };
                        let t = t.clamp(0.0, 1.0).powf(0.8);
                        (t * (RAMP.len() - 1) as f32).round() as usize
                    };
                    // Very dark pixels would be invisible; lift the color a bit.
                    let fg = Color::Rgb(c[0].max(48), c[1].max(48), c[2].max(48));
                    let bg = Color::Rgb(c[0] / 5, c[1] / 5, c[2] / 5);
                    Span::styled(
                        char::from(RAMP[idx]).to_string(),
                        Style::new().fg(fg).bg(bg),
                    )
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

pub(super) struct VisualState {
    handle: visuals::Handle,
    global: Option<String>,
    per_playlist: HashMap<i64, String>,
    pub(super) active: Option<Active>,
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
    /// First picture in `gifs/`: shown while nothing was chosen at all.
    auto: Option<String>,
    pub(super) mode: VisualMode,
    pub(super) fit: VisualFit,
    /// How much the "fit" mode may cut off to make the picture bigger (percent).
    pub(super) crop: u8,
    renderer: Renderer,
}

/// Stored as the global choice when the user picked "(none)", so that the
/// automatic first picture does not come back.
const NO_PICTURE: &str = "-";

impl VisualState {
    pub(super) fn new(handle: visuals::Handle, mode: VisualMode, fit: VisualFit, crop: u8) -> Self {
        Self {
            mode,
            fit,
            crop: crop.min(90),
            renderer: Renderer::spawn(&handle.picker),
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
            auto: None,
        }
    }

    /// Look at `gifs/` again: new files appear, failed ones may be retried
    /// and everything on screen is decoded afresh.
    pub(super) fn rescan(&mut self) {
        self.auto = visuals::list_images(&self.handle.dir).into_iter().next();
        self.failed.clear();
        self.error = None;
        self.requested = None;
        self.active = None;
        self.spare.clear();
    }
}

/// Should the animation clock run? It stops together with the music when
/// `pause_with_music` is on.
fn clock_runs(pause_with_music: bool, playing: bool) -> bool {
    !pause_with_music || playing
}

impl App {
    /// The picture for what is playing: the playlist's own, else the global.
    pub(super) fn desired_gif(&self) -> Option<String> {
        let own = self
            .visual
            .play_context
            .and_then(|id| self.visual.per_playlist.get(&id));
        if own.is_some() {
            return own.cloned();
        }
        match self.visual.global.as_deref() {
            Some(NO_PICTURE) => None,
            Some(name) => Some(name.to_owned()),
            None => self.visual.auto.clone(),
        }
    }

    pub(super) fn drain_visual_events(&mut self) {
        while let Ok(ev) = self.visual.handle.events.try_recv() {
            match ev {
                Event::Choices { global, playlists } => {
                    self.visual.auto = visuals::list_images(&self.visual.handle.dir)
                        .into_iter()
                        .next();
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
                    let loaded = Active::new(name.clone(), animation);
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
        self.take_rendered_frames();

        match self.desired_gif() {
            None => {
                self.visual.active = None;
                self.visual.error = None;
            }
            Some(name) => {
                if self.visual.active.as_ref().is_none_or(|a| a.name != name) {
                    if let Some(i) = self.visual.spare.iter().position(|a| a.name == name) {
                        let mut found = self.visual.spare.remove(i);
                        found.key = None;
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

    /// Frames the background threads have finished since the last time.
    fn take_rendered_frames(&mut self) {
        // A few per screen update: handing over a pixel frame is not free.
        for _ in 0..8 {
            let Ok(done) = self.visual.renderer.done.try_recv() else {
                return;
            };
            let Some(active) = &mut self.visual.active else {
                continue;
            };
            if active.generation != done.id || done.index >= active.cache.len() {
                continue; // made for a size or picture that is gone
            }
            active.pending[done.index] = false;
            active.cache[done.index] = Some(match done.output {
                render::Output::Blocks(grid) => Cached::Blocks(grid),
                render::Output::Ascii(lines) => Cached::Ascii(lines),
                render::Output::Pixels(proto) => Cached::Pixels(proto),
            });
            let index = active.clock.index();
            active.evict(index);
        }
    }

    /// Is any frame of the current picture ready to show? (For tests.)
    #[cfg(test)]
    pub(super) fn picture_ready(&self) -> bool {
        self.visual
            .active
            .as_ref()
            .is_some_and(|a| a.cache.iter().any(Option::is_some))
    }

    /// `overlapped`: another panel lies over this one.
    pub(super) fn draw_visual(&mut self, f: &mut Frame, area: Rect, overlapped: bool) {
        let title = match &self.visual.active {
            Some(a) => format!("Visual: {}", a.name),
            None => "Visual".to_owned(),
        };
        let block = self.block_n(&title, 3);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let graphics = self.visual.handle.picker.protocol_type() != ProtocolType::Halfblocks;
        let mode = self.visual.mode.effective(overlapped, graphics);
        let (fit, crop) = (self.visual.fit, self.visual.crop);
        let font = self.visual.handle.picker.font_size();
        let background = match self.theme.background {
            Color::Rgb(r, g, b) => [r, g, b],
            _ => [0, 0, 0],
        };
        let VisualState {
            active, renderer, ..
        } = &mut self.visual;
        if let Some(active) = active
            && !active.images.is_empty()
        {
            let n = active.images.len();
            let index = active.clock.index().min(n - 1);
            let kind = match mode {
                VisualMode::Ascii => Kind::Ascii,
                VisualMode::Blocks => Kind::Blocks,
                VisualMode::Picture => Kind::Pixels {
                    width: u32::from(inner.width) * u32::from(font.width),
                    height: u32::from(inner.height) * u32::from(font.height),
                },
            };
            let key = render::Key {
                cols: inner.width,
                rows: inner.height,
                fit,
                crop,
                kind,
            };
            if active.key != Some(key) {
                // Another size or style: everything made so far is stale.
                active.key = Some(key);
                active.generation = renderer.new_generation();
                active.cache.iter_mut().for_each(|c| *c = None);
                active.pending.iter_mut().for_each(|p| *p = false);
            }
            // Ask for this frame and the next few; pixel frames are heavy,
            // so only a few are made ahead.
            let ahead = if matches!(kind, Kind::Pixels { .. }) {
                3
            } else {
                24
            }
            .min(n);
            for k in 0..ahead {
                let i = (index + k) % n;
                if active.cache[i].is_none() && !active.pending[i] {
                    active.pending[i] = true;
                    renderer.request(render::Job {
                        id: active.generation,
                        index: i,
                        images: Arc::clone(&active.images),
                        key,
                        background,
                    });
                }
            }
            if let Some(show) = active.frame_to_show(index) {
                match &mut active.cache[show] {
                    Some(Cached::Ascii(lines)) => {
                        f.render_widget(Paragraph::new(lines.clone()), inner);
                    }
                    Some(Cached::Blocks(grid)) => blocks::paint(f.buffer_mut(), inner, grid),
                    Some(Cached::Pixels(proto)) => f.render_stateful_widget(
                        StatefulImage::<StatefulProtocol>::default().resize(Resize::Fit(None)),
                        inner,
                        proto,
                    ),
                    None => {}
                }
            }
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
                // "(none)" is remembered, otherwise the first picture returns.
                let stored = name.clone().unwrap_or_else(|| NO_PICTURE.to_owned());
                self.visual.global = Some(stored.clone());
                self.visual.handle.send(Command::SetGlobal(Some(stored)));
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

    /// Frames are made in the background: draw until one is ready.
    fn settled(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        render(app, w, h); // asks for the frames
        let end = Instant::now() + Duration::from_secs(10);
        while !app.picture_ready() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
            app.tick_visuals();
        }
        assert!(app.picture_ready(), "a frame was made");
        render(app, w, h)
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
        // With "fill" the picture covers the whole panel, not a part of it.
        app.visual.fit = VisualFit::Fill;
        let buf = settled(&mut app, 120, 30);
        let red = ratatui::style::Color::Rgb(255, 0, 0);
        let painted = buf
            .content()
            .iter()
            .filter(|c| c.fg == red || c.bg == red)
            .count();
        // 120 columns: the panel is 24% wide, 30 rows minus the player bar.
        assert!(painted > 25 * 20, "the picture fills its panel: {painted}");

        // ASCII mode draws characters instead.
        app.visual.mode = VisualMode::Ascii;
        let buf = settled(&mut app, 120, 30);
        let tinted = buf
            .content()
            .iter()
            .filter(|c| c.fg == ratatui::style::Color::Rgb(255, 48, 48))
            .count();
        assert!(tinted > 25 * 20, "ASCII fills the panel too: {tinted}");
        app.visual.mode = VisualMode::Picture;

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

#[cfg(test)]
mod fit_tests {
    use super::*;

    fn wide() -> DynamicImage {
        DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            40,
            20,
            image::Rgba([200, 10, 10, 255]),
        ))
    }

    #[test]
    fn every_fit_gives_exactly_the_panel_size() {
        for fit in [VisualFit::Fill, VisualFit::Fit, VisualFit::Stretch] {
            for (w, h) in [(100, 100), (10, 80), (300, 20), (1, 1)] {
                let img = scaled(&wide(), w, h, fit, 0);
                assert_eq!((img.width(), img.height()), (w, h), "{fit:?}");
            }
        }
    }

    #[test]
    fn every_fit_covers_the_whole_panel_and_fit_keeps_the_whole_picture() {
        let opaque = |img: &DynamicImage| img.to_rgba8().pixels().all(|p| p[3] == 255);
        assert!(opaque(&scaled(&wide(), 60, 60, VisualFit::Fill, 0)));
        assert!(opaque(&scaled(&wide(), 60, 60, VisualFit::Stretch, 0)));
        assert!(opaque(&scaled(&wide(), 60, 60, VisualFit::Fit, 0)));
        let fit = scaled(&wide(), 60, 60, VisualFit::Fit, 0).to_rgba8();
        // The picture is as wide as the panel, all of it, in the middle...
        assert_eq!(fit.get_pixel(30, 30).0, [200, 10, 10, 255]);
        assert_eq!(fit.get_pixel(0, 30).0, [200, 10, 10, 255]);
        // ...and the free space above and below is a darker blurred copy.
        let band = fit.get_pixel(30, 2);
        assert_eq!(band[3], 255);
        assert!(band[0] < 200 && band[0] > 100, "{band:?}");
        // A picture of the panel's own proportions has no bands at all.
        let exact = scaled(&wide(), 80, 40, VisualFit::Fit, 0).to_rgba8();
        assert_eq!(exact.get_pixel(0, 0).0, [200, 10, 10, 255]);
    }

    #[test]
    fn a_crop_allowance_makes_the_picture_bigger_without_empty_edges() {
        // 40x20 into 60x60: whole picture 60x30; with 40% allowed it is
        // scaled 1.5 / 0.6 = 2.5 times (100x50), so 24 of 40 columns show.
        let none = scaled(&wide(), 60, 60, VisualFit::Fit, 0).to_rgba8();
        let some = scaled(&wide(), 60, 60, VisualFit::Fit, 40).to_rgba8();
        let height = |img: &image::RgbaImage| {
            (0..60)
                .filter(|&y| img.get_pixel(30, y).0 == [200, 10, 10, 255])
                .count()
        };
        assert_eq!(height(&none), 30);
        assert_eq!(height(&some), 50);
        // Enough allowance to cover the panel: no backdrop at all.
        let full = scaled(&wide(), 60, 60, VisualFit::Fit, 90).to_rgba8();
        assert_eq!(height(&full), 60);
        // The size is always exactly the panel's.
        for crop in [0, 25, 40, 90] {
            let img = scaled(&wide(), 77, 31, VisualFit::Fit, crop);
            assert_eq!((img.width(), img.height()), (77, 31));
        }
    }

    #[test]
    fn the_fit_names_come_from_config_and_cycle() {
        assert_eq!(VisualFit::from_config("fit"), VisualFit::Fit);
        assert_eq!(VisualFit::from_config("Stretch"), VisualFit::Stretch);
        assert_eq!(VisualFit::from_config("???"), VisualFit::Fit);
        assert_eq!(VisualFit::from_config("fill"), VisualFit::Fill);
        assert_eq!(VisualFit::Fill.cycle().cycle().cycle(), VisualFit::Fill);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    /// Not a test: `cargo test --release bench_scaled -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_scaled() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(360, 640, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x * y) % 256) as u8, 255])
        }));
        for (name, w, h) in [
            ("blocks 240x100 cells", 480, 200),
            ("pixels 1500x900", 1500, 900),
        ] {
            for fit in [VisualFit::Fit, VisualFit::Fill] {
                let n = 10;
                let t = Instant::now();
                for _ in 0..n {
                    std::hint::black_box(scaled(&img, w, h, fit, 35));
                }
                eprintln!(
                    "{name:<22} {fit:?}: {:>7.2} ms per frame",
                    t.elapsed().as_secs_f64() * 1000.0 / n as f64
                );
            }
        }
    }
}
