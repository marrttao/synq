//! Big, smooth text for the "word" lyrics mode: a real font (two are
//! embedded, SIL Open Font License, see assets/fonts; more are read from the
//! `fonts` folder) rasterized to the size of the
//! panel and drawn with quadrant characters (2 x 2 picture pixels per cell).

use std::sync::OnceLock;

use fontdue::{Font, FontSettings};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

const LOBSTER: &[u8] = include_bytes!("../../assets/fonts/Lobster-Regular.ttf");
const OSWALD: &[u8] = include_bytes!("../../assets/fonts/Oswald.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontKind {
    /// A fat script: the "pretty" one.
    Lobster,
    /// A tall clean grotesque.
    Oswald,
    /// A font the user put into the `fonts` folder (index into the list).
    User(usize),
}

struct UserFont {
    /// Lowercase, as saved in the settings.
    name: String,
    label: String,
    font: Font,
}

static USER_FONTS: OnceLock<Vec<UserFont>> = OnceLock::new();

fn user_fonts() -> &'static [UserFont] {
    USER_FONTS.get().map_or(&[], Vec::as_slice)
}

/// A readable name from a file name: "Birds of Paradise (junk) PERSONAL USE
/// ONLY" becomes "Birds of Paradise" (cut at the first odd character).
fn font_label(stem: &str) -> String {
    let clean: String = stem
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .collect();
    let clean = clean.trim().replace('_', " ");
    if clean.is_empty() {
        stem.trim().to_owned()
    } else {
        clean
    }
}

/// Fonts (`.ttf`, `.otf`) found in `dirs`, usable as one-word fonts after the
/// built-in ones. Call once, at the start; later calls change nothing.
pub fn load_user_fonts(dirs: &[std::path::PathBuf]) {
    let _ = USER_FONTS.set(read_fonts(dirs));
}

fn read_fonts(dirs: &[std::path::PathBuf]) -> Vec<UserFont> {
    let mut found: Vec<UserFont> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            if !matches!(ext.as_deref(), Some("ttf" | "otf")) {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("font");
            let label = font_label(stem);
            let name = label.to_lowercase();
            // The built-in ones also lie in assets/fonts; same name twice is skipped.
            if matches!(name.as_str(), "lobster" | "lobster-regular" | "oswald")
                || found.iter().any(|f| f.name == name)
            {
                continue;
            }
            let font = std::fs::read(&path)
                .ok()
                .and_then(|bytes| Font::from_bytes(bytes, FontSettings::default()).ok());
            match font {
                Some(font) => found.push(UserFont { name, label, font }),
                None => tracing::warn!("cannot use the font {}", path.display()),
            }
        }
    }
    found
}

impl FontKind {
    pub fn from_config(name: &str) -> Self {
        let name = name.trim().to_lowercase();
        if name == "oswald" {
            Self::Oswald
        } else if let Some(i) = user_fonts().iter().position(|f| f.name == name) {
            Self::User(i)
        } else {
            Self::Lobster
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Lobster => Self::Oswald,
            Self::Oswald if user_fonts().is_empty() => Self::Lobster,
            Self::Oswald => Self::User(0),
            Self::User(i) if i + 1 < user_fonts().len() => Self::User(i + 1),
            Self::User(_) => Self::Lobster,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Lobster => "lobster",
            Self::Oswald => "oswald",
            Self::User(i) => user_fonts().get(i).map_or("lobster", |f| f.name.as_str()),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lobster => "Lobster",
            Self::Oswald => "Oswald",
            Self::User(i) => user_fonts().get(i).map_or("Lobster", |f| f.label.as_str()),
        }
    }

    fn font(self) -> Option<&'static Font> {
        static LOBSTER_FONT: OnceLock<Option<Font>> = OnceLock::new();
        static OSWALD_FONT: OnceLock<Option<Font>> = OnceLock::new();
        let (cell, bytes) = match self {
            Self::Lobster => (&LOBSTER_FONT, LOBSTER),
            Self::Oswald => (&OSWALD_FONT, OSWALD),
            Self::User(i) => return user_fonts().get(i).map(|f| &f.font),
        };
        cell.get_or_init(|| Font::from_bytes(bytes, FontSettings::default()).ok())
            .as_ref()
    }
}

/// How much of each picture pixel is covered by the text (0-255).
pub struct Coverage {
    pub w: usize,
    pub h: usize,
    data: Vec<u8>,
}

impl Coverage {
    fn at(&self, x: usize, y: usize) -> u8 {
        if x < self.w && y < self.h {
            self.data[y * self.w + x]
        } else {
            0
        }
    }
}

/// The widest and tallest the text may be, as a share of the panel.
const FILL_W: f32 = 0.9;
const FILL_H: f32 = 0.78;

fn width_at(font: &Font, text: &str, size: f32) -> f32 {
    text.chars()
        .map(|c| font.metrics(c, size).advance_width)
        .sum()
}

/// `text` as big as the panel (`cols` x `rows` cells) allows, in the middle.
pub fn render(kind: FontKind, text: &str, cols: u16, rows: u16) -> Option<Coverage> {
    let font = kind.font()?;
    let text = text.trim();
    if text.is_empty() || cols == 0 || rows == 0 {
        return None;
    }
    let (w, h) = (usize::from(cols) * 2, usize::from(rows) * 2);
    // A picture pixel is half a cell wide and half a cell tall, and a cell
    // is about twice as tall as wide: one grid row is two font pixels.
    let probe = 100.0;
    let lm = font.horizontal_line_metrics(probe)?;
    let (w100, h100) = (width_at(font, text, probe), lm.ascent - lm.descent);
    if w100 <= 0.0 || h100 <= 0.0 {
        return None;
    }
    let k = (w as f32 * FILL_W / w100).min(h as f32 * 2.0 * FILL_H / h100);
    let size = (probe * k).clamp(4.0, 800.0);

    let lm = font.horizontal_line_metrics(size)?;
    let ascent = lm.ascent.ceil() as i32;
    let descent = (-lm.descent).ceil() as i32;
    let canvas_w = width_at(font, text, size).ceil() as usize + 4;
    let canvas_h = (ascent + descent).max(1) as usize + 4;
    let mut canvas = vec![0u8; canvas_w * canvas_h];
    let baseline = 2 + ascent;
    let mut pen = 2.0f32;
    for c in text.chars() {
        let (m, bitmap) = font.rasterize(c, size);
        let left = pen as i32 + m.xmin;
        let top = baseline - m.ymin - m.height as i32;
        for row in 0..m.height {
            for col in 0..m.width {
                let (x, y) = (left + col as i32, top + row as i32);
                if x >= 0 && y >= 0 && (x as usize) < canvas_w && (y as usize) < canvas_h {
                    let cell = &mut canvas[y as usize * canvas_w + x as usize];
                    *cell = (*cell).max(bitmap[row * m.width + col]);
                }
            }
        }
        pen += m.advance_width;
    }

    // Into the grid: centered, two font rows per grid row.
    // Horizontally by the ink (script letters overhang their advance),
    // vertically by the line, so that the baseline stays put between words.
    let ink = |range: std::ops::Range<usize>| {
        range.filter(|&x| (0..canvas_h).any(|y| canvas[y * canvas_w + x] > 40))
    };
    let (ink_min, ink_max) = (
        ink(0..canvas_w).next().unwrap_or(0) as i32,
        ink(0..canvas_w).next_back().unwrap_or(0) as i32,
    );
    let ox = (w as i32 - (ink_max - ink_min + 1)) / 2 - ink_min;
    let oy = (h as i32 - canvas_h.div_ceil(2) as i32) / 2;
    let mut data = vec![0u8; w * h];
    for gy in 0..h {
        for gx in 0..w {
            let sx = gx as i32 - ox;
            let sy = 2 * (gy as i32 - oy);
            if sx < 0 || sy < 0 || sx as usize >= canvas_w {
                continue;
            }
            let at = |row: i32| {
                if (row as usize) < canvas_h {
                    u32::from(canvas[row as usize * canvas_w + sx as usize])
                } else {
                    0
                }
            };
            data[gy * w + gx] = ((at(sy) + at(sy + 1)) / 2) as u8;
        }
    }
    Some(Coverage { w, h, data })
}

/// Quadrant characters, indexed by which of the four pixels are on
/// (bits: 1 top-left, 2 top-right, 4 bottom-left, 8 bottom-right).
const QUADRANTS: [char; 16] = [
    ' ', '\u{2598}', '\u{259D}', '\u{2580}', '\u{2596}', '\u{258C}', '\u{259E}', '\u{259B}',
    '\u{2597}', '\u{259A}', '\u{2590}', '\u{259C}', '\u{2584}', '\u{2599}', '\u{259F}', '\u{2588}',
];

fn blend(under: Color, over: Color, coverage: f32) -> Color {
    match (under, over) {
        (Color::Rgb(a, b, c), Color::Rgb(x, y, z)) => {
            let m = |u: u8, o: u8| (f32::from(u) + (f32::from(o) - f32::from(u)) * coverage) as u8;
            Color::Rgb(m(a, x), m(b, y), m(c, z))
        }
        _ => over,
    }
}

/// Draw the text over whatever is in `area` (backgrounds stay; only the
/// letters are added), in `color`.
pub fn paint(buf: &mut Buffer, area: Rect, cov: &Coverage, color: Color, backing: Color) {
    let area = area.intersection(buf.area);
    for cy in 0..area.height {
        for cx in 0..area.width {
            let (px, py) = (usize::from(cx) * 2, usize::from(cy) * 2);
            let c = [
                cov.at(px, py),
                cov.at(px + 1, py),
                cov.at(px, py + 1),
                cov.at(px + 1, py + 1),
            ];
            let peak = c.into_iter().max().unwrap_or(0);
            if peak < 24 {
                continue;
            }
            // Pixels that are mostly covered are on; a faint edge still
            // shows as its strongest pixel, in a softer color.
            let mut mask = 0usize;
            for (i, v) in c.into_iter().enumerate() {
                if v >= 110 {
                    mask |= 1 << i;
                }
            }
            if mask == 0 {
                let i = c.iter().position(|&v| v == peak).unwrap_or(0);
                mask = 1 << i;
            }
            let on: Vec<u8> = c
                .into_iter()
                .enumerate()
                .filter(|(i, _)| mask >> i & 1 == 1)
                .map(|(_, v)| v)
                .collect();
            let mean = on.iter().map(|&v| u32::from(v)).sum::<u32>() as f32
                / (on.len().max(1) as f32 * 255.0);
            let cell = &mut buf[(area.x + cx, area.y + cy)];
            let under = if matches!(cell.bg, Color::Rgb(..)) {
                cell.bg
            } else {
                backing
            };
            cell.set_char(QUADRANTS[mask])
                .set_fg(blend(under, color, mean.clamp(0.0, 1.0)));
            if !matches!(cell.bg, Color::Rgb(..)) {
                cell.set_bg(backing);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(cov: &Coverage) -> usize {
        cov.data.iter().filter(|&&v| v > 100).count()
    }

    #[test]
    fn text_is_rendered_in_the_middle_and_fills_the_panel() {
        for kind in [FontKind::Lobster, FontKind::Oswald] {
            let cov = render(kind, "Hello", 24, 20).unwrap();
            assert_eq!((cov.w, cov.h), (48, 40));
            assert!(lit(&cov) > 150, "{kind:?}");
            // Nothing touches the edges; it uses most of the width.
            let columns: Vec<usize> = (0..cov.w)
                .filter(|&x| (0..cov.h).any(|y| cov.at(x, y) > 100))
                .collect();
            let (first, last) = (columns[0], columns[columns.len() - 1]);
            assert!(first >= 1 && last < cov.w - 1);
            assert!(last - first > cov.w / 2, "{kind:?}: {first}..{last}");
        }
    }

    #[test]
    fn a_longer_word_gets_a_smaller_font_and_cyrillic_works() {
        let short = render(FontKind::Lobster, "I", 60, 12).unwrap();
        let long = render(FontKind::Lobster, "unbelievable", 60, 12).unwrap();
        let height = |c: &Coverage| {
            (0..c.h)
                .filter(|&y| (0..c.w).any(|x| c.at(x, y) > 100))
                .count()
        };
        assert!(height(&long) < height(&short));
        for kind in [FontKind::Lobster, FontKind::Oswald] {
            let ru = render(kind, "\u{43c}\u{438}\u{440}", 40, 10).unwrap();
            assert!(lit(&ru) > 50, "{kind:?} has Cyrillic");
        }
    }

    #[test]
    fn nothing_to_draw_and_tiny_panels_do_not_break() {
        assert!(render(FontKind::Lobster, "  ", 10, 5).is_none());
        assert!(render(FontKind::Lobster, "a", 0, 5).is_none());
        for (c, r) in [(1, 1), (2, 1), (5, 2), (200, 60)] {
            let cov = render(FontKind::Oswald, "Wow!", c, r);
            let mut buf = Buffer::empty(Rect::new(0, 0, c, r));
            if let Some(cov) = cov {
                paint(
                    &mut buf,
                    Rect::new(0, 0, c, r),
                    &cov,
                    Color::Red,
                    Color::Black,
                );
            }
        }
    }

    #[test]
    fn painting_adds_letters_and_keeps_the_background() {
        let cov = render(FontKind::Oswald, "O", 30, 10).unwrap();
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        for cell in buf.content.iter_mut() {
            cell.set_bg(Color::Rgb(10, 20, 30));
        }
        paint(
            &mut buf,
            Rect::new(0, 0, 30, 10),
            &cov,
            Color::Rgb(255, 255, 255),
            Color::Black,
        );
        let drawn = buf.content.iter().filter(|c| c.symbol() != " ").count();
        assert!(drawn > 20);
        assert!(buf.content.iter().all(|c| c.bg == Color::Rgb(10, 20, 30)));
        assert!(
            buf.content
                .iter()
                .any(|c| c.fg == Color::Rgb(255, 255, 255))
        );
    }

    #[test]
    fn fonts_from_a_folder_are_named_and_skip_the_built_in_ones() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cool Font \u{2555} PERSONAL.ttf"), OSWALD).unwrap();
        std::fs::write(tmp.path().join("Oswald.ttf"), OSWALD).unwrap();
        std::fs::write(tmp.path().join("broken.ttf"), b"nope").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), b"x").unwrap();
        let found = read_fonts(&[tmp.path().to_path_buf(), tmp.path().join("missing")]);
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["cool font"]);
        assert_eq!(found[0].label, "Cool Font");
    }

    #[test]
    fn font_names_come_from_config() {
        assert_eq!(FontKind::from_config("oswald"), FontKind::Oswald);
        assert_eq!(FontKind::from_config("???"), FontKind::Lobster);
        assert_eq!(FontKind::Lobster.cycle().cycle(), FontKind::Lobster);
    }
}
