//! Screen layouts: where the Library, Lyrics and Visual panels sit (in
//! percent of the area above the player bar) and whether each is shown.
//! A few presets ship with the player; the user's own are kept in the
//! `settings` table (`layout:<name>`), so they survive restarts.

use std::path::PathBuf;

use ratatui::layout::Rect;

use crate::db::Db;

/// Smallest size (percent) a panel can be given in the editor.
pub const MIN_PERCENT: i16 = 10;
/// A panel with fewer cells than this is not drawn at all.
const MIN_COLS: u16 = 8;
const MIN_ROWS: u16 = 3;
const PREFIX: &str = "layout:";
const CURRENT_KEY: &str = "layout_current";
const MAX_NAME: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Module {
    Library,
    Lyrics,
    Visual,
    /// The sound spectrum.
    Visualizer,
    /// Fixed bar at the bottom; it can only be shown or hidden.
    Player,
}

impl Module {
    pub const ALL: [Module; 5] = [
        Module::Library,
        Module::Lyrics,
        Module::Visual,
        Module::Visualizer,
        Module::Player,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Module::Library => "Library",
            Module::Lyrics => "Lyrics",
            Module::Visual => "Visual",
            Module::Visualizer => "Sound",
            Module::Player => "Player bar",
        }
    }
}

/// A panel's place, in percent of the area (0..=100).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub visible: bool,
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
}

const fn place(visible: bool, x: u8, y: u8, w: u8, h: u8) -> Place {
    Place {
        visible,
        x,
        y,
        w,
        h,
    }
}

impl Place {
    /// Cells inside `area`, or `None` when hidden or too small to be useful.
    /// Neighbouring panels share edges exactly (no gaps, no overlaps).
    pub fn rect(&self, area: Rect) -> Option<Rect> {
        if !self.visible {
            return None;
        }
        let r = self.raw_rect(area);
        (r.width >= MIN_COLS && r.height >= MIN_ROWS).then_some(r)
    }

    /// Like `rect` but also for hidden or tiny panels (the editor draws a
    /// ghost there).
    pub fn raw_rect(&self, area: Rect) -> Rect {
        let edge = |start: u16, len: u16, from: u8, size: u8| {
            let a = u32::from(len) * u32::from(from) / 100;
            let b = u32::from(len) * (u32::from(from) + u32::from(size)) / 100;
            (start + a as u16, (b - a) as u16)
        };
        let (x, width) = edge(area.x, area.width, self.x, self.w);
        let (y, height) = edge(area.y, area.height, self.y, self.h);
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    pub fn moved(&mut self, dx: i16, dy: i16) {
        self.x = (i16::from(self.x) + dx).clamp(0, 100 - i16::from(self.w)) as u8;
        self.y = (i16::from(self.y) + dy).clamp(0, 100 - i16::from(self.h)) as u8;
    }

    pub fn resized(&mut self, dw: i16, dh: i16) {
        self.w = (i16::from(self.w) + dw).clamp(MIN_PERCENT, 100 - i16::from(self.x)) as u8;
        self.h = (i16::from(self.h) + dh).clamp(MIN_PERCENT, 100 - i16::from(self.y)) as u8;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub name: String,
    /// Made by the user (kept in the database); presets are not.
    pub custom: bool,
    /// The classic arrangement folds panels away in narrow windows.
    pub responsive: bool,
    /// Library, Lyrics, Visual, Sound (the spectrum).
    pub places: [Place; 4],
    pub player: bool,
}

impl Layout {
    fn preset(name: &str, responsive: bool, places: [Place; 4]) -> Self {
        Self {
            name: name.to_owned(),
            custom: false,
            responsive,
            places,
            player: true,
        }
    }

    pub fn place(&self, m: Module) -> Option<&Place> {
        self.places.get(m as usize)
    }

    pub fn place_mut(&mut self, m: Module) -> Option<&mut Place> {
        self.places.get_mut(m as usize)
    }

    /// Places for a window `width` columns wide.
    pub fn effective(&self, width: u16) -> [Place; 4] {
        if !self.responsive {
            return self.places;
        }
        let [mut lib, mut lyr, mut vis, mut viz] = self.places;
        // The spectrum sits in the same column as the lyrics.
        if width < 110 {
            vis.visible = false;
            lyr.w += vis.w;
            viz.w = lyr.w;
        }
        if width < 70 {
            lib.visible = false;
            lyr.x = lib.x;
            lyr.w += lib.w;
            viz.x = lyr.x;
            viz.w = lyr.w;
        }
        [lib, lyr, vis, viz]
    }

    /// A fixed copy of what is on screen now, ready for editing.
    pub fn frozen(&self, width: u16) -> Self {
        Self {
            places: self.effective(width),
            responsive: false,
            ..self.clone()
        }
    }

    pub fn to_text(&self) -> String {
        let p = |tag: &str, p: &Place| {
            format!(
                "{tag}={},{},{},{},{}",
                u8::from(p.visible),
                p.x,
                p.y,
                p.w,
                p.h
            )
        };
        format!(
            "v1;player={};{};{};{};{}",
            u8::from(self.player),
            p("lib", &self.places[0]),
            p("lyr", &self.places[1]),
            p("vis", &self.places[2]),
            p("viz", &self.places[3]),
        )
    }

    pub fn parse(name: &str, text: &str) -> Option<Self> {
        let mut player = true;
        let mut places = [None; 4];
        for part in text.split(';') {
            let Some((key, value)) = part.split_once('=') else {
                continue; // the version tag
            };
            match key {
                "player" => player = value == "1",
                "lib" | "lyr" | "vis" | "viz" => {
                    let n: Vec<u8> = value
                        .split(',')
                        .map(|v| v.trim().parse().ok())
                        .collect::<Option<_>>()?;
                    let [v, x, y, w, h] = n[..] else { return None };
                    if w < 1
                        || h < 1
                        || u16::from(x) + u16::from(w) > 100
                        || u16::from(y) + u16::from(h) > 100
                    {
                        return None;
                    }
                    let slot = ["lib", "lyr", "vis", "viz"]
                        .iter()
                        .position(|k| *k == key)?;
                    places[slot] = Some(place(v == 1, x, y, w, h));
                }
                _ => {}
            }
        }
        // Layouts saved before the spectrum existed do not show it.
        let viz = places[3].unwrap_or(place(false, 0, 78, 100, 22));
        let [Some(a), Some(b), Some(c), _] = places else {
            return None;
        };
        Some(Self {
            name: name.to_owned(),
            custom: true,
            responsive: false,
            places: [a, b, c, viz],
            player,
        })
    }
}

pub(super) fn presets() -> Vec<Layout> {
    vec![
        // What the player always looked like.
        Layout::preset(
            "Classic",
            true,
            [
                place(true, 0, 0, 34, 100),
                place(true, 34, 0, 42, 78),
                place(true, 76, 0, 24, 100),
                place(true, 34, 78, 42, 22),
            ],
        ),
        // Words and a big picture; no list.
        Layout::preset(
            "Lyrics",
            false,
            [
                place(false, 0, 0, 25, 100),
                place(true, 0, 0, 60, 78),
                place(true, 60, 0, 40, 100),
                place(true, 0, 78, 60, 22),
            ],
        ),
        // The picture on top, lyrics under it.
        Layout::preset(
            "Cinema",
            false,
            [
                place(false, 0, 0, 25, 100),
                place(true, 0, 62, 62, 38),
                place(true, 0, 0, 100, 62),
                place(true, 62, 62, 38, 38),
            ],
        ),
    ]
}

/// All layouts and the one in use.
pub struct LayoutSet {
    layouts: Vec<Layout>,
    current: usize,
    db_path: Option<PathBuf>,
}

impl LayoutSet {
    pub fn presets_only() -> Self {
        Self {
            layouts: presets(),
            current: 0,
            db_path: None,
        }
    }

    pub fn load(db_path: PathBuf) -> (Self, Option<String>) {
        let mut set = Self::presets_only();
        let mut problem = None;
        let read = Db::open(&db_path)
            .and_then(|db| Ok((db.settings_with_prefix(PREFIX)?, db.setting(CURRENT_KEY)?)));
        match read {
            Ok((rows, current)) => {
                let mut rows = rows;
                rows.sort();
                for (key, value) in rows {
                    let Some(name) = key.strip_prefix(PREFIX) else {
                        continue;
                    };
                    if value.is_empty() || set.index_of(name).is_some() {
                        continue; // deleted, or clashes with a preset
                    }
                    match Layout::parse(name, &value) {
                        Some(l) => set.layouts.push(l),
                        None => tracing::warn!("ignoring damaged layout \"{name}\""),
                    }
                }
                if let Some(i) = current.and_then(|c| set.index_of(&c)) {
                    set.current = i;
                }
            }
            Err(e) => {
                tracing::error!("layouts: {e}");
                problem = Some(format!("layouts: {e}"));
            }
        }
        set.db_path = Some(db_path);
        (set, problem)
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        self.layouts.iter().position(|l| l.name == name)
    }

    pub fn current(&self) -> &Layout {
        &self.layouts[self.current.min(self.layouts.len() - 1)]
    }

    pub fn exists_custom(&self, name: &str) -> bool {
        self.layouts.iter().any(|l| l.custom && l.name == name)
    }

    /// Switch to the next or previous layout; returns its name.
    pub fn step(&mut self, forward: bool) -> Result<String, String> {
        let n = self.layouts.len();
        self.current = (self.current + if forward { 1 } else { n - 1 }) % n;
        let name = self.current().name.clone();
        self.remember_current().map(|()| name)
    }

    /// "Custom 1", "Custom 2"... the first one that is free.
    pub fn fresh_name(&self) -> String {
        (1..)
            .map(|n| format!("Custom {n}"))
            .find(|n| self.index_of(n).is_none())
            .unwrap_or_else(|| "Custom".to_owned())
    }

    /// Keep `layout` (replacing a custom one of the same name) and use it.
    pub fn save(&mut self, mut layout: Layout) -> Result<(), String> {
        layout.name.truncate(MAX_NAME);
        layout.custom = true;
        layout.responsive = false;
        match self.index_of(&layout.name) {
            Some(i) if self.layouts[i].custom => {
                self.current = i;
                self.layouts[i] = layout.clone();
            }
            Some(_) => return Err(format!("\"{}\" is a built-in layout", layout.name)),
            None => {
                self.layouts.push(layout.clone());
                self.current = self.layouts.len() - 1;
            }
        }
        self.write(&format!("{PREFIX}{}", layout.name), &layout.to_text())?;
        self.remember_current()
    }

    /// Forget the custom layout `name`; the classic one is shown instead.
    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        let Some(i) = self.index_of(name).filter(|&i| self.layouts[i].custom) else {
            return Err("Only your own layouts can be deleted".into());
        };
        self.layouts.remove(i);
        self.current = 0;
        self.write(&format!("{PREFIX}{name}"), "")?;
        self.remember_current()
    }

    fn remember_current(&self) -> Result<(), String> {
        self.write(CURRENT_KEY, &self.current().name)
    }

    fn write(&self, key: &str, value: &str) -> Result<(), String> {
        let Some(path) = &self.db_path else {
            return Ok(());
        };
        Db::open(path)
            .and_then(|db| db.set_setting(key, value))
            .map_err(|e| {
                tracing::error!("cannot save layout: {e}");
                format!("cannot save the layout: {e}")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 30,
    };

    #[test]
    fn neighbours_share_edges_exactly() {
        for width in [110, 111, 120, 157, 300] {
            let area = Rect { width, ..AREA };
            let l = &presets()[0];
            let [a, b, c, _] = l.effective(width).map(|p| p.raw_rect(area));
            assert_eq!(a.x + a.width, b.x, "{width}");
            assert_eq!(b.x + b.width, c.x, "{width}");
            assert_eq!(c.x + c.width, width, "{width}");
        }
    }

    #[test]
    fn classic_folds_panels_away_in_narrow_windows() {
        let l = &presets()[0];
        let vis = |w| l.effective(w).map(|p| p.visible);
        assert_eq!(vis(120), [true, true, true, true]);
        assert_eq!(vis(90), [true, true, false, true]);
        assert_eq!(vis(50), [false, true, false, true]);
        let [_, lyr, _, viz] = l.effective(50);
        assert_eq!((lyr.x, lyr.w), (0, 100));
        assert_eq!(
            (viz.x, viz.w),
            (0, 100),
            "the spectrum stays under the lyrics"
        );
        // The others do not move by themselves.
        assert_eq!(presets()[1].effective(50), presets()[1].places);
    }

    #[test]
    fn old_saved_layouts_load_without_the_spectrum() {
        let old = "v1;player=1;lib=1,0,0,34,100;lyr=1,34,0,42,100;vis=1,76,0,24,100";
        let l = Layout::parse("Old", old).unwrap();
        assert!(!l.places[3].visible, "it was not there, so it stays off");
        assert_eq!(Layout::parse("Old", &l.to_text()).unwrap().places, l.places);
    }

    #[test]
    fn tiny_or_hidden_panels_are_not_drawn() {
        let p = place(true, 0, 0, 100, 100);
        assert!(p.rect(AREA).is_some());
        assert!(p.rect(Rect { width: 7, ..AREA }).is_none());
        assert!(p.rect(Rect { height: 2, ..AREA }).is_none());
        assert!(place(false, 0, 0, 100, 100).rect(AREA).is_none());
        assert_eq!(p.rect(Rect::ZERO), None);
    }

    #[test]
    fn moving_and_resizing_stay_inside_the_screen() {
        let mut p = place(true, 50, 50, 40, 40);
        p.moved(100, 100);
        assert_eq!((p.x, p.y), (60, 60));
        p.moved(-100, -100);
        assert_eq!((p.x, p.y), (0, 0));
        p.resized(500, 500);
        assert_eq!((p.w, p.h), (100, 100));
        p.resized(-500, -500);
        assert_eq!((p.w, p.h), (10, 10));
        p.moved(95, 0);
        assert_eq!(p.x, 90);
        p.resized(50, 0);
        assert_eq!(p.w, 10, "cannot grow past the edge");
    }

    #[test]
    fn layouts_round_trip_through_text() {
        let mut l = presets()[1].clone();
        l.player = false;
        let back = Layout::parse("Lyrics", &l.to_text()).unwrap();
        assert_eq!(back.places, l.places);
        assert!(!back.player);
        for bad in [
            "",
            "v1",
            "lib=1,0,0,50",
            "lib=1,0,0,0,50;lyr=1,0,0,1,1;vis=1,0,0,1,1",
            "lib=1,60,0,50,50;lyr=1,0,0,1,1;vis=1,0,0,1,1",
            "lib=x",
        ] {
            assert!(Layout::parse("x", bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn own_layouts_are_saved_switched_and_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("t.db");
        drop(Db::open(&db).unwrap());
        let (mut set, problem) = LayoutSet::load(db.clone());
        assert!(problem.is_none());
        assert_eq!(set.current().name, "Classic");

        let mut mine = set.current().frozen(120);
        mine.name = set.fresh_name();
        assert_eq!(mine.name, "Custom 1");
        mine.places[1].resized(-20, 0);
        set.save(mine.clone()).unwrap();
        assert_eq!(set.fresh_name(), "Custom 2");
        assert!(set.exists_custom("Custom 1"));

        // It comes back after a restart, as the current one.
        let (again, _) = LayoutSet::load(db.clone());
        assert_eq!(again.current().name, "Custom 1");
        assert_eq!(again.current().places, mine.places);

        // Presets cannot be overwritten; cycling visits everything.
        let mut fake = mine.clone();
        fake.name = "Classic".into();
        assert!(set.save(fake).is_err());
        let seen: Vec<String> = (0..5).map(|_| set.step(true).unwrap()).collect();
        assert_eq!(seen, ["Classic", "Lyrics", "Cinema", "Custom 1", "Classic"]);
        assert!(set.delete("Classic").is_err());
        set.delete("Custom 1").unwrap();
        assert_eq!(set.current().name, "Classic");
        let (after, _) = LayoutSet::load(db);
        assert!(!after.exists_custom("Custom 1"));
    }
}
