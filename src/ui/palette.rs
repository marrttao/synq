//! Color palettes: the built-in sets and the user's own, each a list of
//! N colors (2 to 12). The chosen one is remembered; custom ones are kept
//! in the `settings` table as `palette:<name>`.

use std::path::PathBuf;

use ratatui::style::Color;

use super::theme::{MAX_EXTRAS, Theme, parse_hex};
use crate::db::Db;

pub const MIN_COLORS: usize = 2;
pub const MAX_COLORS: usize = 5 + MAX_EXTRAS;
const PREFIX: &str = "palette:";
const CURRENT_KEY: &str = "palette_current";
const MAX_NAME: usize = 30;

/// What each position is used for.
pub fn role(i: usize) -> String {
    match i {
        0 => "Background".into(),
        1 => "Text".into(),
        2 => "Accent".into(),
        3 => "Dim text".into(),
        4 => "Border".into(),
        n => format!("Extra {}", n - 4),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub name: String,
    pub custom: bool,
    pub colors: Vec<[u8; 3]>,
}

impl Palette {
    pub fn theme(&self) -> Theme {
        Theme::from_colors(&self.colors)
    }

    pub fn to_text(&self) -> String {
        self.colors
            .iter()
            .map(|c| hex(*c))
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn parse(name: &str, text: &str) -> Option<Self> {
        let colors: Vec<[u8; 3]> = text
            .split(',')
            .map(|c| match parse_hex(c)? {
                Color::Rgb(r, g, b) => Some([r, g, b]),
                _ => None,
            })
            .collect::<Option<_>>()?;
        (MIN_COLORS..=MAX_COLORS)
            .contains(&colors.len())
            .then(|| Self {
                name: name.to_owned(),
                custom: true,
                colors,
            })
    }
}

pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub struct PaletteSet {
    items: Vec<Palette>,
    current: usize,
    db_path: Option<PathBuf>,
    /// A choice from an earlier run was found (it wins over config.toml).
    pub restored: bool,
}

impl PaletteSet {
    /// Built-in sets only; `preset` (from config.toml) is the current one.
    pub fn presets_only(preset: &str) -> Self {
        let items: Vec<Palette> = Theme::preset_list()
            .into_iter()
            .map(|(name, colors)| Palette {
                name: name.to_owned(),
                custom: false,
                colors,
            })
            .collect();
        let current = items
            .iter()
            .position(|p| p.name == preset.trim().to_lowercase())
            .unwrap_or(0);
        Self {
            items,
            current,
            db_path: None,
            restored: false,
        }
    }

    pub fn load(db_path: PathBuf, preset: &str) -> (Self, Option<String>) {
        let mut set = Self::presets_only(preset);
        let mut problem = None;
        let read = Db::open(&db_path)
            .and_then(|db| Ok((db.settings_with_prefix(PREFIX)?, db.setting(CURRENT_KEY)?)));
        match read {
            Ok((mut rows, current)) => {
                rows.sort();
                for (key, value) in rows {
                    let Some(name) = key.strip_prefix(PREFIX) else {
                        continue;
                    };
                    if value.is_empty() || set.index_of(name).is_some() {
                        continue;
                    }
                    match Palette::parse(name, &value) {
                        Some(p) => set.items.push(p),
                        None => tracing::warn!("ignoring damaged palette \"{name}\""),
                    }
                }
                if let Some(i) = current.and_then(|c| set.index_of(&c)) {
                    set.current = i;
                    set.restored = true;
                }
            }
            Err(e) => {
                tracing::error!("palettes: {e}");
                problem = Some(format!("palettes: {e}"));
            }
        }
        set.db_path = Some(db_path);
        (set, problem)
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        self.items.iter().position(|p| p.name == name)
    }

    pub fn current(&self) -> &Palette {
        &self.items[self.current.min(self.items.len() - 1)]
    }

    pub fn exists_custom(&self, name: &str) -> bool {
        self.items.iter().any(|p| p.custom && p.name == name)
    }

    /// Switch to the next palette; returns it.
    pub fn next(&mut self) -> Result<Palette, String> {
        self.current = (self.current + 1) % self.items.len();
        let now = self.current().clone();
        self.remember().map(|()| now)
    }

    pub fn fresh_name(&self) -> String {
        (1..)
            .map(|n| format!("Mine {n}"))
            .find(|n| self.index_of(n).is_none())
            .unwrap_or_else(|| "Mine".to_owned())
    }

    pub fn save(&mut self, mut palette: Palette) -> Result<(), String> {
        palette.name.truncate(MAX_NAME);
        palette.custom = true;
        match self.index_of(&palette.name) {
            Some(i) if self.items[i].custom => {
                self.current = i;
                self.items[i] = palette.clone();
            }
            Some(_) => return Err(format!("\"{}\" is a built-in palette", palette.name)),
            None => {
                self.items.push(palette.clone());
                self.current = self.items.len() - 1;
            }
        }
        self.write(&format!("{PREFIX}{}", palette.name), &palette.to_text())?;
        self.remember()
    }

    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        let Some(i) = self.index_of(name).filter(|&i| self.items[i].custom) else {
            return Err("Only your own palettes can be deleted".into());
        };
        self.items.remove(i);
        self.current = 0;
        self.write(&format!("{PREFIX}{name}"), "")?;
        self.remember()
    }

    fn remember(&self) -> Result<(), String> {
        self.write(CURRENT_KEY, &self.current().name)
    }

    fn write(&self, key: &str, value: &str) -> Result<(), String> {
        let Some(path) = &self.db_path else {
            return Ok(());
        };
        Db::open(path)
            .and_then(|db| db.set_setting(key, value))
            .map_err(|e| {
                tracing::error!("cannot save palette: {e}");
                format!("cannot save the palette: {e}")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_round_trip_and_reject_bad_text() {
        let p = Palette::parse("x", "#000000,#ffffff,#ff0080").unwrap();
        assert_eq!(p.colors.len(), 3);
        assert_eq!(Palette::parse("x", &p.to_text()), Some(p));
        for bad in ["", "#000000", "#000000,zzz", "#000000,#ffffff,"] {
            assert!(Palette::parse("x", bad).is_none(), "{bad}");
        }
        let too_many = vec!["#000000"; MAX_COLORS + 1].join(",");
        assert!(Palette::parse("x", &too_many).is_none());
        assert_eq!(role(2), "Accent");
        assert_eq!(role(7), "Extra 3");
    }

    #[test]
    fn the_config_preset_starts_as_current_and_own_palettes_persist() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("t.db");
        drop(Db::open(&db).unwrap());
        let (mut set, problem) = PaletteSet::load(db.clone(), "nord");
        assert!(problem.is_none() && !set.restored);
        assert_eq!(set.current().name, "nord");

        let mut mine = set.current().clone();
        mine.name = set.fresh_name();
        mine.colors.push([1, 2, 3]);
        mine.colors.push([4, 5, 6]);
        set.save(mine.clone()).unwrap();
        assert_eq!(mine.colors.len(), 7);

        let (again, _) = PaletteSet::load(db.clone(), "default");
        assert!(again.restored);
        assert_eq!(again.current().colors, mine.colors);
        assert!(again.current().custom);

        // Built-ins are protected; cycling wraps; delete goes back.
        let mut fake = mine.clone();
        fake.name = "nord".into();
        assert!(set.save(fake).is_err());
        assert!(set.delete("nord").is_err());
        set.delete("Mine 1").unwrap();
        assert_eq!(set.current().name, "default");
        let first = set.next().unwrap();
        assert_eq!(first.name, "nord");
        let (after, _) = PaletteSet::load(db, "default");
        assert!(!after.exists_custom("Mine 1"));
    }
}
