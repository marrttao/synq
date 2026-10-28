//! Rebindable keys: the actions, the key (chord) of each, and whether the
//! key also works in every other program ("global"). Choices are kept in
//! the `settings` table, one row per action.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::db::Db;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    PlayPause,
    Next,
    Prev,
    SeekBack,
    SeekForward,
    SeekBackBig,
    SeekForwardBig,
    VolumeUp,
    VolumeDown,
    Shuffle,
    Repeat,
    Favorite,
    Lyrics,
    LyricsMode,
    WordFont,
    LyricsEarlier,
    LyricsLater,
    Fullscreen,
    PictureMode,
    PictureFit,
    PictureZoomIn,
    PictureZoomOut,
    Refresh,
    SoundStyle,
    NextLayout,
    PrevLayout,
    NextPalette,
    PrevPalette,
}

impl Action {
    pub const ALL: [Action; 28] = [
        Action::PlayPause,
        Action::Next,
        Action::Prev,
        Action::SeekBack,
        Action::SeekForward,
        Action::SeekBackBig,
        Action::SeekForwardBig,
        Action::VolumeUp,
        Action::VolumeDown,
        Action::Shuffle,
        Action::Repeat,
        Action::Favorite,
        Action::Lyrics,
        Action::LyricsMode,
        Action::WordFont,
        Action::LyricsEarlier,
        Action::LyricsLater,
        Action::Fullscreen,
        Action::PictureMode,
        Action::PictureFit,
        Action::PictureZoomIn,
        Action::PictureZoomOut,
        Action::SoundStyle,
        Action::NextLayout,
        Action::PrevLayout,
        Action::NextPalette,
        Action::PrevPalette,
        Action::Refresh,
    ];

    /// Stable name used in the database.
    pub fn id(self) -> &'static str {
        match self {
            Action::PlayPause => "play_pause",
            Action::Next => "next",
            Action::Prev => "prev",
            Action::SeekBack => "seek_back",
            Action::SeekForward => "seek_forward",
            Action::SeekBackBig => "seek_back_big",
            Action::SeekForwardBig => "seek_forward_big",
            Action::VolumeUp => "volume_up",
            Action::VolumeDown => "volume_down",
            Action::Shuffle => "shuffle",
            Action::Repeat => "repeat",
            Action::Favorite => "favorite",
            Action::Lyrics => "lyrics",
            Action::LyricsMode => "lyrics_mode",
            Action::WordFont => "word_font",
            Action::LyricsEarlier => "lyrics_earlier",
            Action::LyricsLater => "lyrics_later",
            Action::Fullscreen => "fullscreen",
            Action::PictureMode => "picture_mode",
            Action::PictureFit => "picture_fit",
            Action::PictureZoomIn => "picture_zoom_in",
            Action::PictureZoomOut => "picture_zoom_out",
            Action::Refresh => "refresh",
            Action::SoundStyle => "sound_style",
            Action::NextLayout => "next_layout",
            Action::PrevLayout => "prev_layout",
            Action::NextPalette => "next_palette",
            Action::PrevPalette => "prev_palette",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::PlayPause => "Pause / play",
            Action::Next => "Next track",
            Action::Prev => "Previous track",
            Action::SeekBack => "Seek back",
            Action::SeekForward => "Seek forward",
            Action::SeekBackBig => "Seek back (big)",
            Action::SeekForwardBig => "Seek forward (big)",
            Action::VolumeUp => "Volume up",
            Action::VolumeDown => "Volume down",
            Action::Shuffle => "Shuffle on / off",
            Action::Repeat => "Repeat mode",
            Action::Favorite => "Favorite on / off",
            Action::Lyrics => "Show / hide lyrics",
            Action::LyricsMode => "Lyrics: all lines / one word",
            Action::WordFont => "One-word font: Lobster / Oswald",
            Action::LyricsEarlier => "Lyrics earlier 0.1 s",
            Action::LyricsLater => "Lyrics later 0.1 s",
            Action::Fullscreen => "Full screen",
            Action::PictureMode => "Picture / ASCII",
            Action::PictureFit => "Picture: fit / fill / stretch",
            Action::PictureZoomIn => "Picture bigger (crop more)",
            Action::PictureZoomOut => "Picture smaller (crop less)",
            Action::Refresh => "Refresh everything",
            Action::SoundStyle => "Sound: bars / mirror",
            Action::NextLayout => "Next layout",
            Action::PrevLayout => "Previous layout",
            Action::NextPalette => "Next colors",
            Action::PrevPalette => "Previous colors",
        }
    }

    fn default_chord(self) -> Chord {
        let plain = |code| Chord::new(code, KeyModifiers::NONE);
        match self {
            Action::PlayPause => plain(KeyCode::Char(' ')),
            Action::Next => plain(KeyCode::Char('n')),
            Action::Prev => plain(KeyCode::Char('p')),
            Action::SeekBack => plain(KeyCode::Left),
            Action::SeekForward => plain(KeyCode::Right),
            Action::SeekBackBig => Chord::new(KeyCode::Left, KeyModifiers::SHIFT),
            Action::SeekForwardBig => Chord::new(KeyCode::Right, KeyModifiers::SHIFT),
            Action::VolumeUp => plain(KeyCode::Char('+')),
            Action::VolumeDown => plain(KeyCode::Char('-')),
            Action::Shuffle => plain(KeyCode::Char('s')),
            Action::Repeat => plain(KeyCode::Char('r')),
            Action::Favorite => plain(KeyCode::Char('*')),
            Action::Lyrics => plain(KeyCode::Char('l')),
            Action::LyricsMode => plain(KeyCode::Char('w')),
            Action::WordFont => plain(KeyCode::Char('W')),
            Action::LyricsEarlier => plain(KeyCode::Char('[')),
            Action::LyricsLater => plain(KeyCode::Char(']')),
            Action::Fullscreen => plain(KeyCode::Char('f')),
            Action::PictureMode => plain(KeyCode::Char('v')),
            Action::PictureFit => plain(KeyCode::Char('V')),
            Action::PictureZoomIn => plain(KeyCode::Char('z')),
            Action::PictureZoomOut => plain(KeyCode::Char('Z')),
            Action::Refresh => plain(KeyCode::F(5)),
            Action::SoundStyle => plain(KeyCode::Char('x')),
            Action::NextLayout => plain(KeyCode::Char('}')),
            Action::PrevLayout => plain(KeyCode::Char('{')),
            Action::NextPalette => plain(KeyCode::Char(')')),
            Action::PrevPalette => plain(KeyCode::Char('(')),
        }
    }
}

/// A key with Ctrl / Alt / Shift. Shift on a character is already part of
/// the character, so it is dropped (`F` and `Shift+f` are the same).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

const MOD_MASK: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SHIFT);

impl Chord {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let mut mods = mods & MOD_MASK;
        if matches!(code, KeyCode::Char(_)) {
            mods.remove(KeyModifiers::SHIFT);
        }
        let code = match code {
            KeyCode::BackTab => KeyCode::Tab,
            c => c,
        };
        Self { code, mods }
    }

    pub fn from_event(key: KeyEvent) -> Self {
        let mut chord = Self::new(key.code, key.modifiers);
        if key.code == KeyCode::BackTab {
            chord.mods.insert(KeyModifiers::SHIFT);
        }
        chord
    }

    /// Keys that would take over typing in every program.
    pub fn is_safe_global(&self) -> bool {
        self.mods
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || matches!(self.code, KeyCode::F(_))
    }

    /// `ctrl+alt+n`, `f5`, `space`: what is stored and what can be typed
    /// into the database by hand.
    pub fn to_text(self) -> String {
        let mut out = String::new();
        for (flag, name) in [
            (KeyModifiers::CONTROL, "ctrl+"),
            (KeyModifiers::ALT, "alt+"),
            (KeyModifiers::SHIFT, "shift+"),
        ] {
            if self.mods.contains(flag) {
                out.push_str(name);
            }
        }
        out.push_str(&match self.code {
            KeyCode::Char(' ') => "space".to_owned(),
            KeyCode::Char('+') => "plus".to_owned(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::F(n) => format!("f{n}"),
            KeyCode::Enter => "enter".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Delete => "delete".into(),
            KeyCode::Insert => "insert".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pageup".into(),
            KeyCode::PageDown => "pagedown".into(),
            KeyCode::Up => "up".into(),
            KeyCode::Down => "down".into(),
            KeyCode::Left => "left".into(),
            KeyCode::Right => "right".into(),
            _ => "?".into(),
        });
        out
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut mods = KeyModifiers::NONE;
        let mut parts: Vec<&str> = text.split('+').collect();
        let key = parts.pop()?;
        for m in parts {
            mods |= match m {
                "ctrl" => KeyModifiers::CONTROL,
                "alt" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                _ => return None,
            };
        }
        let code = match key {
            "space" => KeyCode::Char(' '),
            "plus" => KeyCode::Char('+'),
            "enter" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "delete" => KeyCode::Delete,
            "insert" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            k if k.starts_with('f') && k.len() > 1 => {
                let n: u8 = k[1..].parse().ok()?;
                if !(1..=24).contains(&n) {
                    return None;
                }
                KeyCode::F(n)
            }
            k => {
                let mut chars = k.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                KeyCode::Char(c)
            }
        };
        Some(Self::new(code, mods))
    }

    /// For the screen: `Ctrl+Alt+N`, `Space`, `F5`.
    pub fn display(&self) -> String {
        let text = self.to_text();
        let mut out = String::new();
        for part in text.split('+').filter(|p| !p.is_empty()) {
            if !out.is_empty() {
                out.push('+');
            }
            let mut chars = part.chars();
            match (chars.next(), part.chars().count()) {
                (Some(c), n) if n > 1 => {
                    out.extend(c.to_uppercase());
                    out.push_str(chars.as_str());
                }
                (Some(c), _) => out.push(c.to_ascii_uppercase()),
                _ => {}
            }
        }
        if self.code == KeyCode::Char('+') && out.ends_with("Plus") {
            out.truncate(out.len() - 4);
            out.push('+');
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub chord: Chord,
    /// Also react when another program has the focus.
    pub global: bool,
}

const PREFIX: &str = "bind:";

pub struct Keymap {
    binds: Vec<(Action, Binding)>,
    db_path: Option<PathBuf>,
}

impl Keymap {
    pub fn defaults() -> Self {
        Self {
            binds: Action::ALL
                .iter()
                .map(|&a| {
                    (
                        a,
                        Binding {
                            chord: a.default_chord(),
                            global: false,
                        },
                    )
                })
                .collect(),
            db_path: None,
        }
    }

    /// Defaults with the saved choices on top. Unreadable rows are skipped.
    pub fn load(db_path: PathBuf) -> (Self, Option<String>) {
        let mut map = Self::defaults();
        let mut problem = None;
        match Db::open(&db_path).and_then(|db| db.settings_with_prefix(PREFIX)) {
            Ok(rows) => {
                for (key, value) in rows {
                    let Some(action) = key
                        .strip_prefix(PREFIX)
                        .and_then(|id| Action::ALL.iter().copied().find(|a| a.id() == id))
                    else {
                        continue;
                    };
                    let (chord_text, global) = match value.split_once(';') {
                        Some((c, g)) => (c, g == "1"),
                        None => (value.as_str(), false),
                    };
                    let Some(chord) = Chord::parse(chord_text) else {
                        tracing::warn!("ignoring saved key for {}: {value}", action.id());
                        continue;
                    };
                    let global = global && chord.is_safe_global();
                    // A saved key that clashes with another action is dropped.
                    if map.owner_of(chord).is_some_and(|o| o != action) {
                        continue;
                    }
                    map.set_binding(action, Binding { chord, global });
                }
            }
            Err(e) => {
                tracing::error!("keys: {e}");
                problem = Some(format!("keys: {e}"));
            }
        }
        map.db_path = Some(db_path);
        (map, problem)
    }

    pub fn binding(&self, action: Action) -> Binding {
        self.binds.iter().find(|(a, _)| *a == action).map_or_else(
            || Binding {
                chord: action.default_chord(),
                global: false,
            },
            |(_, b)| *b,
        )
    }

    fn set_binding(&mut self, action: Action, binding: Binding) {
        if let Some(slot) = self.binds.iter_mut().find(|(a, _)| *a == action) {
            slot.1 = binding;
        }
    }

    fn owner_of(&self, chord: Chord) -> Option<Action> {
        self.binds
            .iter()
            .find(|(_, b)| b.chord == chord)
            .map(|(a, _)| *a)
    }

    /// The action a pressed key triggers inside the player.
    pub fn action_for(&self, key: KeyEvent) -> Option<Action> {
        self.owner_of(Chord::from_event(key))
    }

    /// Keys of the global actions, for registering with the system.
    pub fn globals(&self) -> Vec<(Action, Chord)> {
        self.binds
            .iter()
            .filter(|(_, b)| b.global)
            .map(|(a, b)| (*a, b.chord))
            .collect()
    }

    /// Give `action` a new key. Fails with a message for the status line.
    pub fn rebind(&mut self, action: Action, chord: Chord) -> Result<(), String> {
        if reserved(chord) {
            return Err(format!("{} is reserved", chord.display()));
        }
        if let Some(other) = self.owner_of(chord)
            && other != action
        {
            return Err(format!(
                "{} is already used for \"{}\"",
                chord.display(),
                other.label()
            ));
        }
        let mut b = self.binding(action);
        b.chord = chord;
        // A global key must stay harmless for other programs.
        b.global = b.global && chord.is_safe_global();
        self.set_binding(action, b);
        self.save(action)
    }

    pub fn set_global(&mut self, action: Action, on: bool) -> Result<(), String> {
        let mut b = self.binding(action);
        if on && !b.chord.is_safe_global() {
            return Err(format!(
                "{} needs Ctrl or Alt (or F1-F24) to work everywhere: a plain key would be stolen from every program",
                b.chord.display()
            ));
        }
        b.global = on;
        self.set_binding(action, b);
        self.save(action)
    }

    pub fn reset(&mut self, action: Action) -> Result<(), String> {
        let chord = action.default_chord();
        if let Some(other) = self.owner_of(chord)
            && other != action
        {
            return Err(format!(
                "{} is used for \"{}\" now",
                chord.display(),
                other.label()
            ));
        }
        self.set_binding(
            action,
            Binding {
                chord,
                global: false,
            },
        );
        self.save(action)
    }

    fn save(&self, action: Action) -> Result<(), String> {
        let Some(path) = &self.db_path else {
            return Ok(());
        };
        let b = self.binding(action);
        let value = format!("{};{}", b.chord.to_text(), u8::from(b.global));
        Db::open(path)
            .and_then(|db| db.set_setting(&format!("{PREFIX}{}", action.id()), &value))
            .map_err(|e| {
                tracing::error!("cannot save key: {e}");
                format!("cannot save the key: {e}")
            })
    }
}

/// The Latin key at the same place of the keyboard for a Cyrillic letter, so
/// that every key works with a Russian layout too.
pub fn latin_for(c: char) -> char {
    RU.chars()
        .position(|r| r == c)
        .and_then(|i| EN.chars().nth(i))
        .unwrap_or(c)
}

const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбюЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ";
const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>";

/// Keys that the interface itself needs.
fn reserved(chord: Chord) -> bool {
    let plain = chord.mods.is_empty();
    match chord.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace => true,
        KeyCode::Char('c') if chord.mods == KeyModifiers::CONTROL => true,
        KeyCode::Char(c) if plain => "qjk/acedgtSEC?".contains(c),
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::Delete
            if plain =>
        {
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl(c: char) -> Chord {
        Chord::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn russian_letters_map_to_the_same_keys() {
        assert_eq!(latin_for('ы'), 's');
        assert_eq!(latin_for('т'), 'n');
        assert_eq!(latin_for('з'), 'p');
        assert_eq!(latin_for('Х'), '{');
        assert_eq!(latin_for('Ъ'), '}');
        assert_eq!(latin_for('а'), 'f');
        assert_eq!(latin_for('x'), 'x');
        assert_eq!(latin_for(' '), ' ');
        assert_eq!(RU.chars().count(), EN.chars().count());
    }

    #[test]
    fn chords_round_trip_through_text() {
        for chord in [
            Chord::new(KeyCode::Char(' '), KeyModifiers::NONE),
            Chord::new(KeyCode::Char('+'), KeyModifiers::NONE),
            Chord::new(KeyCode::Char('F'), KeyModifiers::NONE),
            Chord::new(KeyCode::F(5), KeyModifiers::NONE),
            Chord::new(KeyCode::Left, KeyModifiers::CONTROL | KeyModifiers::ALT),
            ctrl('n'),
        ] {
            assert_eq!(Chord::parse(&chord.to_text()), Some(chord), "{chord:?}");
        }
        assert!(Chord::parse("").is_none());
        assert!(Chord::parse("hyper+x").is_none());
        assert!(Chord::parse("f99").is_none());
        assert!(Chord::parse("ab").is_none());
    }

    #[test]
    fn shift_on_a_character_is_not_a_difference() {
        let a = Chord::from_event(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
        let b = Chord::from_event(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::NONE));
        assert_eq!(a, b);
        assert_eq!(a.display(), "F");
        assert_eq!(ctrl('n').display(), "Ctrl+N");
        assert_eq!(
            Chord::new(KeyCode::Char(' '), KeyModifiers::NONE).display(),
            "Space"
        );
        assert_eq!(
            Chord::new(KeyCode::Char('+'), KeyModifiers::NONE).display(),
            "+"
        );
    }

    #[test]
    fn defaults_have_no_clashes_and_match_the_old_keys() {
        let map = Keymap::defaults();
        for (i, (_, a)) in map.binds.iter().enumerate() {
            for (_, b) in &map.binds[i + 1..] {
                assert_ne!(a.chord, b.chord);
            }
            assert!(!reserved(a.chord), "{:?}", a.chord);
        }
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(map.action_for(key('n')), Some(Action::Next));
        assert_eq!(map.action_for(key(' ')), Some(Action::PlayPause));
        assert_eq!(map.action_for(key('y')), None);
    }

    #[test]
    fn rebinding_checks_clashes_reserved_keys_and_global_safety() {
        let mut map = Keymap::defaults();
        assert!(
            map.rebind(
                Action::Next,
                Chord::new(KeyCode::Char('p'), KeyModifiers::NONE)
            )
            .is_err()
        );
        assert!(
            map.rebind(
                Action::Next,
                Chord::new(KeyCode::Char('q'), KeyModifiers::NONE)
            )
            .is_err()
        );
        assert!(map.rebind(Action::Next, ctrl('c')).is_err());
        assert!(map.rebind(Action::Next, ctrl('n')).is_ok());
        assert_eq!(map.binding(Action::Next).chord, ctrl('n'));
        // A plain key may not become global.
        assert!(map.set_global(Action::Prev, true).is_err());
        assert!(map.set_global(Action::Next, true).is_ok());
        assert_eq!(map.globals(), vec![(Action::Next, ctrl('n'))]);
        // Moving a global key to a plain one turns the global flag off.
        map.rebind(
            Action::Next,
            Chord::new(KeyCode::Char('m'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(map.globals().is_empty());
    }

    #[test]
    fn choices_survive_a_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("t.db");
        drop(Db::open(&db).unwrap());
        let (mut map, problem) = Keymap::load(db.clone());
        assert!(problem.is_none());
        map.rebind(Action::Next, ctrl('n')).unwrap();
        map.set_global(Action::Next, true).unwrap();
        map.rebind(
            Action::Refresh,
            Chord::new(KeyCode::F(5), KeyModifiers::NONE),
        )
        .unwrap();
        let (again, _) = Keymap::load(db);
        assert_eq!(
            again.binding(Action::Next),
            Binding {
                chord: ctrl('n'),
                global: true
            }
        );
        assert_eq!(again.binding(Action::Refresh).chord.code, KeyCode::F(5));
        assert_eq!(
            again.binding(Action::Prev),
            Keymap::defaults().binding(Action::Prev)
        );
    }

    #[test]
    fn a_damaged_row_falls_back_to_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("t.db");
        let d = Db::open(&db).unwrap();
        d.set_setting("bind:next", "nonsense+key").unwrap();
        d.set_setting("bind:prev", "n;0").unwrap(); // clashes with Next's default
        d.set_setting("bind:unknown", "x;1").unwrap();
        drop(d);
        let (map, _) = Keymap::load(db);
        assert_eq!(
            map.binding(Action::Next),
            Keymap::defaults().binding(Action::Next)
        );
        assert_eq!(
            map.binding(Action::Prev),
            Keymap::defaults().binding(Action::Prev)
        );
    }
}
