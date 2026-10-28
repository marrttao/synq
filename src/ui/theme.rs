//! Color theme built from the `[theme]` section of the config.

use ratatui::style::Color;

use crate::config::ThemeConfig;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub dim: Color,
    pub accent: Color,
    pub border: Color,
    /// Colors beyond the five basic ones (custom palettes only).
    extras: [Color; MAX_EXTRAS],
    extra_count: usize,
}

/// A palette has 5 basic colors and up to this many more.
pub const MAX_EXTRAS: usize = 7;

/// Built-in color sets: name, then background, foreground, dim, accent, border.
const PRESETS: [(&str, [u32; 5]); 5] = [
    (
        "default",
        [0x14141c, 0xe6e6ef, 0x6c6c80, 0xff79c6, 0x44475a],
    ),
    ("nord", [0x2e3440, 0xeceff4, 0x7b88a1, 0x88c0d0, 0x4c566a]),
    (
        "gruvbox",
        [0x282828, 0xebdbb2, 0x928374, 0xfabd2f, 0x504945],
    ),
    ("light", [0xfafafa, 0x2b2b36, 0x8a8a99, 0xd6336c, 0xc8c8d0]),
    ("mono", [0x000000, 0xe0e0e0, 0x707070, 0xffffff, 0x505050]),
];

impl Theme {
    /// A built-in theme by name (case-insensitive).
    pub fn preset(name: &str) -> Option<Self> {
        let name = name.trim().to_lowercase();
        let (_, c) = PRESETS.iter().find(|(n, _)| *n == name)?;
        let rgb = |v: u32| Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        Some(Self {
            background: rgb(c[0]),
            foreground: rgb(c[1]),
            dim: rgb(c[2]),
            accent: rgb(c[3]),
            border: rgb(c[4]),
            extras: [Color::Reset; MAX_EXTRAS],
            extra_count: 0,
        })
    }

    /// Names and five colors of each built-in set.
    pub fn preset_list() -> Vec<(&'static str, Vec<[u8; 3]>)> {
        PRESETS
            .iter()
            .map(|(n, c)| {
                let rgb = c
                    .iter()
                    .map(|v| [(v >> 16) as u8, (v >> 8) as u8, *v as u8]);
                (*n, rgb.collect())
            })
            .collect()
    }

    /// A theme from any number (at least 2) of colors. In order: background,
    /// text, accent, dim text, border; what is missing is mixed from the
    /// background and text colors. Further colors rotate with the accent
    /// (current lyric line, panel titles).
    pub fn from_colors(colors: &[[u8; 3]]) -> Self {
        let get = |i: usize| colors.get(i).copied();
        let bg = get(0).unwrap_or([0, 0, 0]);
        let fg = get(1).unwrap_or([255, 255, 255]);
        let accent = get(2).unwrap_or(fg);
        let dim = get(3).unwrap_or_else(|| mix(bg, fg, 45));
        let border = get(4).unwrap_or_else(|| mix(bg, fg, 20));
        let c = |v: [u8; 3]| Color::Rgb(v[0], v[1], v[2]);
        let mut extras = [Color::Reset; MAX_EXTRAS];
        let mut extra_count = 0;
        for (slot, v) in extras.iter_mut().zip(colors.iter().skip(5)) {
            *slot = c(*v);
            extra_count += 1;
        }
        Self {
            background: c(bg),
            foreground: c(fg),
            dim: c(dim),
            accent: c(accent),
            border: c(border),
            extras,
            extra_count,
        }
    }

    /// A color along the palette: the accent, then the extra colors (or the
    /// text color when there are none); `t` runs from 0 to 1.
    pub fn gradient(&self, t: f32) -> Color {
        let mut stops = vec![self.accent];
        stops.extend(self.extras.iter().take(self.extra_count));
        if stops.len() == 1 {
            stops.push(self.foreground);
        }
        let at = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
        let i = (at.floor() as usize).min(stops.len() - 1);
        let j = (i + 1).min(stops.len() - 1);
        match (stops[i], stops[j]) {
            (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
                let c = mix([r1, g1, b1], [r2, g2, b2], ((at - i as f32) * 100.0) as u32);
                Color::Rgb(c[0], c[1], c[2])
            }
            (a, _) => a,
        }
    }

    /// The i-th color of the rotation: the accent, then the extra colors.
    pub fn rotating(&self, i: usize) -> Color {
        match i % (self.extra_count + 1) {
            0 => self.accent,
            k => self.extras[k - 1],
        }
    }

    /// The preset named in the config, with the individual colors on top.
    /// Anything unknown or unparsable falls back to the preset.
    pub fn from_config(cfg: &ThemeConfig) -> Self {
        let base = Self::preset(&cfg.preset)
            .or_else(|| Self::preset("default"))
            .unwrap_or(Self {
                background: Color::Reset,
                foreground: Color::Reset,
                dim: Color::Reset,
                accent: Color::Reset,
                border: Color::Reset,
                extras: [Color::Reset; MAX_EXTRAS],
                extra_count: 0,
            });
        let pick = |value: &str, fallback: Color| parse_hex(value).unwrap_or(fallback);
        Self {
            background: pick(&cfg.background, base.background),
            foreground: pick(&cfg.foreground, base.foreground),
            dim: pick(&cfg.dim, base.dim),
            accent: pick(&cfg.accent, base.accent),
            border: pick(&cfg.border, base.border),
            ..base
        }
    }
}

/// `a` towards `b`; `percent` 0 is `a`, 100 is `b`.
fn mix(a: [u8; 3], b: [u8; 3], percent: u32) -> [u8; 3] {
    let m = |x: u8, y: u8| ((u32::from(x) * (100 - percent) + u32::from(y) * percent) / 100) as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

/// Parse "#rrggbb" (the leading '#' is optional).
pub fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_number_of_colors_makes_a_theme() {
        let c = |v: u8| [v, v, v];
        let two = Theme::from_colors(&[c(0), c(200)]);
        assert_eq!(two.background, Color::Rgb(0, 0, 0));
        assert_eq!(two.accent, two.foreground);
        assert_ne!(two.dim, two.background);
        assert_ne!(two.border, two.background);
        assert_eq!(two.rotating(5), two.accent, "nothing to rotate with");

        let many: Vec<[u8; 3]> = (0..12).map(|i| c(i * 20)).collect();
        let t = Theme::from_colors(&many);
        assert_eq!(t.dim, Color::Rgb(60, 60, 60));
        assert_eq!(t.border, Color::Rgb(80, 80, 80));
        // Accent, then the seven extras, then round again.
        assert_eq!(t.rotating(0), t.accent);
        assert_eq!(t.rotating(1), Color::Rgb(100, 100, 100));
        assert_eq!(t.rotating(7), Color::Rgb(220, 220, 220));
        assert_eq!(t.rotating(8), t.accent);
        assert!(Theme::from_colors(&[]).background == Color::Rgb(0, 0, 0));
    }

    #[test]
    fn parses_hex() {
        assert_eq!(parse_hex("#ff0080"), Some(Color::Rgb(255, 0, 128)));
        assert_eq!(parse_hex("00ff00"), Some(Color::Rgb(0, 255, 0)));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("zzzzzz"), None);
        assert_eq!(parse_hex("#ééé"), None);
    }

    #[test]
    fn bad_value_falls_back_to_default() {
        let cfg = ThemeConfig {
            accent: "nope".into(),
            ..ThemeConfig::default()
        };
        let theme = Theme::from_config(&cfg);
        assert_eq!(theme.accent, Theme::preset("default").unwrap().accent);
    }

    #[test]
    fn colors_override_the_preset() {
        let cfg = ThemeConfig {
            preset: "nord".into(),
            accent: "#010203".into(),
            ..ThemeConfig::default()
        };
        let theme = Theme::from_config(&cfg);
        assert_eq!(theme.accent, Color::Rgb(1, 2, 3));
        assert_eq!(theme.background, Theme::preset("nord").unwrap().background);
    }

    #[test]
    fn unknown_preset_means_default() {
        let cfg = ThemeConfig {
            preset: "nope".into(),
            ..ThemeConfig::default()
        };
        assert_eq!(
            Theme::from_config(&cfg).background,
            Theme::preset("default").unwrap().background
        );
    }

    #[test]
    fn presets_stay_readable() {
        for (name, _) in Theme::preset_list() {
            let t = Theme::preset(name).unwrap();
            assert_ne!(t.background, t.foreground);
            assert_ne!(t.background, t.accent);
            assert_ne!(t.background, t.dim);
        }
    }
}
