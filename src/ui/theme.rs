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
}

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
        })
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
            });
        let pick = |value: &str, fallback: Color| parse_hex(value).unwrap_or(fallback);
        Self {
            background: pick(&cfg.background, base.background),
            foreground: pick(&cfg.foreground, base.foreground),
            dim: pick(&cfg.dim, base.dim),
            accent: pick(&cfg.accent, base.accent),
            border: pick(&cfg.border, base.border),
        }
    }
}

/// Name of the preset after `current` (wraps around; unknown -> the first).
pub fn next_preset(current: &str) -> &'static str {
    let current = current.trim().to_lowercase();
    let i = PRESETS.iter().position(|(n, _)| *n == current);
    PRESETS[i.map_or(0, |i| (i + 1) % PRESETS.len())].0
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
    fn presets_cycle_through_all_and_stay_readable() {
        let mut seen = vec![];
        let mut name = "default";
        for _ in 0..PRESETS.len() {
            seen.push(name);
            let t = Theme::preset(name).unwrap();
            assert_ne!(t.background, t.foreground);
            assert_ne!(t.background, t.accent);
            assert_ne!(t.background, t.dim);
            name = next_preset(name);
        }
        assert_eq!(name, "default", "the cycle closes");
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), PRESETS.len());
        assert_eq!(next_preset("???"), "default");
    }
}
