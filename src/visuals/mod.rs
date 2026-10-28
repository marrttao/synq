//! Pictures and GIFs for the "Visual" panel: finding them in `gifs/`,
//! decoding in the background, remembering the choices, and the frame clock.
//! Drawing is in `ui/visual_view.rs`.

mod decode;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use ratatui_image::picker::{Picker, ProtocolType};

use crate::db::Db;
use crate::paths::Paths;
pub use decode::{Animation, decode};

const EXTENSIONS: [&str; 4] = ["gif", "png", "jpg", "jpeg"];
const GLOBAL_KEY: &str = "gif_global";
const PLAYLIST_PREFIX: &str = "gif_playlist:";

/// Picture file names in `dir`, sorted. Hidden files are skipped.
pub fn list_images(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && is_image_name(n))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

fn is_image_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// A stored name must be a plain file name: settings are editable text.
fn safe_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".."
}

/// Pick the image protocol. Must run after the terminal is in raw mode on
/// the alternate screen, because the terminal is queried for its abilities.
/// `background` is what transparent pixels are drawn over.
pub fn detect_picker(protocol: &str, background: [u8; 3]) -> Picker {
    let mut picker = Picker::from_query_stdio().unwrap_or_else(|e| {
        tracing::info!("terminal query failed ({e}); using half blocks");
        Picker::halfblocks()
    });
    let forced = match protocol.trim().to_lowercase().as_str() {
        "" | "auto" => None,
        "kitty" => Some(ProtocolType::Kitty),
        "sixel" => Some(ProtocolType::Sixel),
        "iterm2" | "iterm" => Some(ProtocolType::Iterm2),
        "halfblocks" | "halfblock" => Some(ProtocolType::Halfblocks),
        other => {
            tracing::warn!("unknown visuals.protocol \"{other}\", using auto");
            None
        }
    };
    if let Some(p) = forced {
        picker.set_protocol_type(p);
    }
    picker.set_background_color(Some([background[0], background[1], background[2], 255]));
    tracing::info!("image protocol: {:?}", picker.protocol_type());
    picker
}

/// Position in an animation. Time only moves when `advance` is given some,
/// so pausing is simply not calling it.
#[derive(Debug, Default, Clone)]
pub struct Clock {
    index: usize,
    into_frame: Duration,
}

impl Clock {
    pub fn index(&self) -> usize {
        self.index
    }

    pub fn advance(&mut self, dt: Duration, delays: &[Duration]) {
        if delays.len() < 2 {
            self.index = 0;
            return;
        }
        self.index = self.index.min(delays.len() - 1);
        self.into_frame += dt;
        // Delays are never zero (see `normalize_delay`), so this ends.
        while self.into_frame >= delays[self.index] {
            self.into_frame -= delays[self.index];
            self.index = (self.index + 1) % delays.len();
        }
    }
}

pub enum Command {
    Load(String),
    SetGlobal(Option<String>),
    SetPlaylist { id: i64, name: Option<String> },
}

pub enum Event {
    /// Saved choices, sent once at start.
    Choices {
        global: Option<String>,
        playlists: HashMap<i64, String>,
    },
    Loaded {
        name: String,
        animation: Animation,
    },
    Failed {
        name: String,
        message: String,
    },
}

pub struct Handle {
    tx: Sender<Command>,
    pub events: Receiver<Event>,
    pub picker: Picker,
    pub dir: PathBuf,
    pub pause_with_music: bool,
}

impl Handle {
    pub fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_err() {
            tracing::error!("visuals thread is gone");
        }
    }
}

pub fn spawn(paths: &Paths, picker: Picker, pause_with_music: bool) -> Handle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (ev_tx, ev_rx) = mpsc::channel();
    let (db_path, dir) = (paths.db.clone(), paths.gifs.clone());
    let worker_dir = dir.clone();
    let spawned = thread::Builder::new()
        .name("visuals".into())
        .spawn(move || run(&db_path, &worker_dir, &cmd_rx, &ev_tx));
    if let Err(e) = spawned {
        tracing::error!("cannot start visuals thread: {e}");
    }
    Handle {
        tx: cmd_tx,
        events: ev_rx,
        picker,
        dir,
        pause_with_music,
    }
}

fn run(db_path: &Path, dir: &Path, commands: &Receiver<Command>, events: &Sender<Event>) {
    let db = match Db::open(db_path) {
        Ok(db) => db,
        Err(e) => {
            tracing::error!("visuals: {e}");
            return;
        }
    };
    if events.send(read_choices(&db)).is_err() {
        return;
    }
    while let Ok(first) = commands.recv() {
        // Only the newest load matters; settings are applied in order.
        let mut load = None;
        for cmd in std::iter::once(first).chain(commands.try_iter()) {
            match cmd {
                Command::Load(name) => load = Some(name),
                Command::SetGlobal(name) => save(&db, GLOBAL_KEY, name.as_deref()),
                Command::SetPlaylist { id, name } => {
                    save(&db, &format!("{PLAYLIST_PREFIX}{id}"), name.as_deref());
                }
            }
        }
        let Some(name) = load else { continue };
        let event = if !safe_name(&name) {
            Event::Failed {
                message: "invalid file name".into(),
                name,
            }
        } else {
            match decode(&dir.join(&name)) {
                Ok(animation) => Event::Loaded { name, animation },
                Err(e) => {
                    tracing::warn!("{name}: {e}");
                    Event::Failed {
                        message: e.to_string(),
                        name,
                    }
                }
            }
        };
        if events.send(event).is_err() {
            return; // UI is gone
        }
    }
}

fn save(db: &Db, key: &str, value: Option<&str>) {
    // An empty value means "no choice"; the row is simply overwritten.
    if let Err(e) = db.set_setting(key, value.unwrap_or("")) {
        tracing::error!("cannot save {key}: {e}");
    }
}

fn read_choices(db: &Db) -> Event {
    let global = db
        .setting(GLOBAL_KEY)
        .ok()
        .flatten()
        .filter(|n| !n.is_empty());
    let playlists = db
        .settings_with_prefix(PLAYLIST_PREFIX)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .filter_map(|(k, v)| Some((k.strip_prefix(PLAYLIST_PREFIX)?.parse().ok()?, v)))
        .collect();
    Event::Choices { global, playlists }
}

#[cfg(test)]
pub(crate) use decode::tests::gif_bytes;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn ms(v: &[u64]) -> Vec<Duration> {
        v.iter().map(|&m| Duration::from_millis(m)).collect()
    }

    #[test]
    fn clock_follows_each_frames_own_delay_and_wraps() {
        let delays = ms(&[100, 300, 40]);
        let mut c = Clock::default();
        c.advance(Duration::from_millis(99), &delays);
        assert_eq!(c.index(), 0);
        c.advance(Duration::from_millis(1), &delays);
        assert_eq!(c.index(), 1);
        c.advance(Duration::from_millis(299), &delays);
        assert_eq!(c.index(), 1);
        c.advance(Duration::from_millis(1), &delays);
        assert_eq!(c.index(), 2);
        c.advance(Duration::from_millis(40), &delays);
        assert_eq!(c.index(), 0);
        // A long stall skips ahead instead of looping forever.
        c.advance(Duration::from_secs(1), &delays);
        assert!(c.index() < 3);
    }

    #[test]
    fn clock_without_time_does_not_move() {
        let delays = ms(&[100, 100]);
        let mut c = Clock::default();
        c.advance(Duration::ZERO, &delays);
        c.advance(Duration::ZERO, &delays);
        assert_eq!(c.index(), 0);
    }

    #[test]
    fn single_frames_never_advance() {
        let mut c = Clock::default();
        c.advance(Duration::from_secs(5), &ms(&[100]));
        assert_eq!(c.index(), 0);
        c.advance(Duration::from_secs(5), &[]);
        assert_eq!(c.index(), 0);
    }

    #[test]
    fn listing_keeps_only_pictures_sorted_without_case() {
        let dir = tempfile::tempdir().unwrap();
        for n in [
            "b.GIF",
            "a.png",
            "C.jpeg",
            "d.jpg",
            "notes.txt",
            ".hidden.gif",
        ] {
            std::fs::write(dir.path().join(n), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("sub.gif")).unwrap();
        assert_eq!(
            list_images(dir.path()),
            ["a.png", "b.GIF", "C.jpeg", "d.jpg"]
        );
        assert!(list_images(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn stored_names_cannot_leave_the_folder() {
        assert!(safe_name("cat.gif"));
        assert!(!safe_name("../secret.gif"));
        assert!(!safe_name("a/b.gif"));
        assert!(!safe_name("a\\b.gif"));
        assert!(!safe_name(".."));
        assert!(!safe_name(""));
    }

    fn wait_event(h: &Handle) -> Event {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(ev) = h.events.try_recv() {
                return ev;
            }
            assert!(Instant::now() < end, "no event from the visuals thread");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn choices_survive_a_restart_and_loading_works() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        paths.ensure_dirs().unwrap();
        drop(Db::open(&paths.db).unwrap()); // creates the schema
        std::fs::write(
            paths.gifs.join("a.gif"),
            gif_bytes(6, 6, &[([1, 2, 3], 100), ([4, 5, 6], 100)]),
        )
        .unwrap();
        std::fs::write(paths.gifs.join("bad.gif"), b"nope").unwrap();

        let h = spawn(&paths, Picker::halfblocks(), true);
        assert!(matches!(
            wait_event(&h),
            Event::Choices { global: None, playlists } if playlists.is_empty()
        ));
        h.send(Command::SetGlobal(Some("a.gif".into())));
        h.send(Command::SetPlaylist {
            id: 7,
            name: Some("a.gif".into()),
        });
        h.send(Command::SetPlaylist {
            id: 8,
            name: Some("b.gif".into()),
        });
        h.send(Command::SetPlaylist { id: 8, name: None });
        h.send(Command::Load("a.gif".into()));
        match wait_event(&h) {
            Event::Loaded { name, animation } => {
                assert_eq!(name, "a.gif");
                assert_eq!(animation.frames.len(), 2);
            }
            _ => panic!("expected Loaded"),
        }
        h.send(Command::Load("bad.gif".into()));
        assert!(matches!(wait_event(&h), Event::Failed { name, .. } if name == "bad.gif"));
        h.send(Command::Load("../x.gif".into()));
        assert!(matches!(wait_event(&h), Event::Failed { .. }));
        drop(h);

        // Settings are written before the next command is read, but the
        // thread may still be finishing: retry for a moment.
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            let h = spawn(&paths, Picker::halfblocks(), true);
            if let Event::Choices { global, playlists } = wait_event(&h)
                && global.as_deref() == Some("a.gif")
                && playlists.get(&7).map(String::as_str) == Some("a.gif")
                && !playlists.contains_key(&8)
            {
                break;
            }
            assert!(Instant::now() < end, "choices were not saved");
        }
    }
}
