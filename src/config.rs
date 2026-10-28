//! Loading and saving of `config.toml`.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read or write config: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid config.toml: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("cannot serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
    pub theme: ThemeConfig,
    pub audio: AudioConfig,
    pub lyrics: LyricsConfig,
    pub whisper: WhisperConfig,
    pub visuals: VisualsConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Enable fade-in / scroll / highlight animations.
    pub animations: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Built-in color set: default / nord / gruvbox / light / mono.
    pub preset: String,
    /// Optional overrides of the preset, "#rrggbb"; empty = from the preset.
    pub background: String,
    pub foreground: String,
    pub dim: String,
    pub accent: String,
    pub border: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Start-up volume, 0.0 - 1.0.
    pub volume: f32,
    /// Seek step for the arrow keys, in seconds.
    pub seek_step_secs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsConfig {
    /// Look lyrics up on LRCLIB when nothing local is found.
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WhisperConfig {
    /// Transcribe lyrics locally when no other source has them.
    pub enabled: bool,
    /// Model size: tiny / base / small / medium.
    pub model: String,
    /// Language code ("en", "ru", ...) or "auto" to detect it.
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualsConfig {
    /// Image protocol: auto / kitty / sixel / iterm2 / halfblocks.
    pub protocol: String,
    /// How the picture is drawn: "picture" or "ascii" (key `v` switches).
    pub mode: String,
    /// How the picture fits its panel: "fill", "fit" or "stretch" (key V).
    pub fit: String,
    /// Pause GIF animation together with the music.
    pub pause_with_music: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { animations: true }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            preset: "default".into(),
            background: String::new(),
            foreground: String::new(),
            dim: String::new(),
            accent: String::new(),
            border: String::new(),
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            volume: 0.8,
            seek_step_secs: 5,
        }
    }
}

impl Default for LyricsConfig {
    fn default() -> Self {
        Self { online: true }
    }
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "base".into(),
            language: "auto".into(),
        }
    }
}

impl Default for VisualsConfig {
    fn default() -> Self {
        Self {
            protocol: "auto".into(),
            mode: "picture".into(),
            fit: "fill".into(),
            pause_with_music: true,
        }
    }
}

/// Written on first start; kept in sync with `Config::default()` by a test.
pub const DEFAULT_TOML: &str = r##"# synq settings. Delete a line to get its default back.

[ui]
# Fade-in / scroll / highlight animations.
animations = true

[theme]
# Built-in colors: default / nord / gruvbox / light / mono (press T to try them).
preset = "default"
# Any color can be overridden with "#rrggbb":
# background = "#14141c"
# foreground = "#e6e6ef"
# dim = "#6c6c80"
# accent = "#ff79c6"
# border = "#44475a"

[audio]
# Start-up volume, 0.0 - 1.0.
volume = 0.8
# Seek step for the arrow keys, in seconds.
seek_step_secs = 5

[lyrics]
# Look lyrics up on LRCLIB when nothing local is found.
online = true

[whisper]
# Transcribe lyrics locally when no other source has them.
enabled = true
# Model size: tiny / base / small / medium.
model = "base"
# Language code ("en", "ru", ...) or "auto" to detect it.
language = "auto"

[visuals]
# Image protocol: auto / kitty / sixel / iterm2 / halfblocks.
protocol = "auto"
# How the picture is drawn: "picture" or "ascii" (key v switches).
mode = "picture"
# How the picture fits its panel: "fill" (crops), "fit" (whole) or "stretch" (key V).
fit = "fill"
# Pause GIF animation together with the music.
pause_with_music = true
"##;

impl Config {
    /// Load the config; if the file does not exist, create it with defaults.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::write(path, DEFAULT_TOML)?;
                Ok(Self::default())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Save the config (comments of a hand-edited file are not preserved).
    #[allow(dead_code)] // used once settings can be changed from the UI
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_template_matches_defaults() {
        let parsed: Config = toml::from_str(DEFAULT_TOML).unwrap();
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn creates_file_on_first_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let cfg = Config::load_or_create(&path).unwrap();
        assert_eq!(cfg, Config::default());
        assert!(path.is_file());
    }

    #[test]
    fn partial_file_falls_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "[whisper]\nmodel = \"small\"\n").unwrap();
        let cfg = Config::load_or_create(&path).unwrap();
        assert_eq!(cfg.whisper.model, "small");
        assert!(cfg.whisper.enabled);
        assert_eq!(cfg.audio, AudioConfig::default());
    }

    #[test]
    fn invalid_file_is_an_error_not_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "this is = = not toml").unwrap();
        assert!(Config::load_or_create(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "this is = = not toml");
    }

    #[test]
    fn save_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let mut cfg = Config::default();
        cfg.audio.volume = 0.3;
        cfg.save(&path).unwrap();
        assert_eq!(Config::load_or_create(&path).unwrap(), cfg);
    }
}
