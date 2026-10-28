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

impl Theme {
    pub fn from_config(cfg: &ThemeConfig) -> Self {
        let d = ThemeConfig::default();
        let pick = |value: &str, fallback: &str| {
            parse_hex(value)
                .or_else(|| parse_hex(fallback))
                .unwrap_or(Color::Reset)
        };
        Self {
            background: pick(&cfg.background, &d.background),
            foreground: pick(&cfg.foreground, &d.foreground),
            dim: pick(&cfg.dim, &d.dim),
            accent: pick(&cfg.accent, &d.accent),
            border: pick(&cfg.border, &d.border),
        }
    }
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
        assert_eq!(
            Some(theme.accent),
            parse_hex(&ThemeConfig::default().accent)
        );
    }
}
