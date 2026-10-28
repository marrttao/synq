//! Refresh, the automatic picture, ASCII mode and the key settings window.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::visual_view::VisualMode;
use super::*;
use crate::keys::{Action, Chord, Keymap};
use crate::paths::Paths;

fn press(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    app.handle_event(Event::Key(KeyEvent::new(code, mods)));
}

fn key(app: &mut App, code: KeyCode) {
    press(app, code, KeyModifiers::NONE);
}

fn setup() -> (tempfile::TempDir, Paths, App, mpsc::Receiver<()>) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::new(tmp.path().to_path_buf());
    paths.ensure_dirs().unwrap();
    drop(crate::db::Db::open(&paths.db).unwrap());
    let (_tx, rx) = mpsc::channel();
    let (refresh_tx, refresh_rx) = mpsc::channel();
    let (keymap, _) = Keymap::load(paths.db.clone());
    let app = App::new(
        &test_config(),
        None,
        rx,
        playlists::spawn(&paths),
        lyrics::spawn(paths.db.clone(), paths.lyrics.clone(), false),
        transcribe::spawn(&test_config().whisper, &paths),
        test_visuals(&paths),
        paths.music.clone(),
    )
    .with_system(keymap, None, refresh_tx);
    (tmp, paths, app, refresh_rx)
}

fn pump(app: &mut App, what: &str, cond: impl Fn(&App) -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        app.drain_visual_events();
        app.tick_visuals();
        if cond(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for: {what}");
}

fn gif() -> Vec<u8> {
    crate::visuals::gif_bytes(8, 8, &[([255, 0, 0], 60), ([0, 0, 255], 60)])
}

fn render(app: &mut App, w: u16, h: u16) {
    let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
    t.draw(|f| app.draw(f)).unwrap();
}

#[test]
fn the_first_picture_shows_by_itself_until_none_is_chosen() {
    let (_tmp, paths, mut app, _r) = setup();
    std::fs::write(paths.gifs.join("a.gif"), gif()).unwrap();
    std::fs::write(paths.gifs.join("b.gif"), gif()).unwrap();
    pump(&mut app, "automatic picture", |a| a.visual.active.is_some());
    assert_eq!(app.desired_gif().as_deref(), Some("a.gif"));

    // "(none)" is a choice too, and it is remembered.
    key(&mut app, KeyCode::Char('g'));
    key(&mut app, KeyCode::Enter); // row 0 is "(none)"
    assert_eq!(app.desired_gif(), None);
    app.tick_visuals();
    assert!(app.visual.active.is_none());
    let db = crate::db::Db::open(&paths.db).unwrap();
    pump(&mut app, "saved", |_| {
        db.setting("gif_global").ok().flatten().as_deref() == Some("-")
    });
}

#[test]
fn refresh_asks_for_a_rescan_and_notices_new_pictures() {
    let (_tmp, paths, mut app, refresh_rx) = setup();
    app.drain_visual_events();
    std::thread::sleep(Duration::from_millis(100));
    app.drain_visual_events();
    assert_eq!(app.desired_gif(), None);
    std::fs::write(paths.gifs.join("new.gif"), gif()).unwrap();
    key(&mut app, KeyCode::Char('R'));
    assert!(
        refresh_rx.try_recv().is_ok(),
        "the library is asked to rescan"
    );
    assert!(app.status.contains("Refreshing"));
    pump(&mut app, "new picture", |a| a.visual.active.is_some());
    assert_eq!(app.desired_gif().as_deref(), Some("new.gif"));
}

#[test]
fn v_switches_between_picture_and_ascii() {
    let (_tmp, _paths, mut app, _r) = setup();
    assert_eq!(app.visual.mode, VisualMode::Picture);
    key(&mut app, KeyCode::Char('v'));
    assert_eq!(app.visual.mode, VisualMode::Ascii);
    key(&mut app, KeyCode::Char('v'));
    assert_eq!(app.visual.mode, VisualMode::Picture);
}

#[test]
fn keys_can_be_changed_and_made_global_in_the_settings_window() {
    let (_tmp, paths, mut app, _r) = setup();
    key(&mut app, KeyCode::Char('S'));
    assert!(matches!(app.mode, Mode::Settings { .. }));
    for (w, h) in [(120, 40), (60, 20), (30, 8), (1, 1)] {
        render(&mut app, w, h);
    }

    // Row 0 is "Pause / play": give it Ctrl+K... which is reserved? No: Ctrl+J.
    key(&mut app, KeyCode::Down); // Next track
    key(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
    assert!(matches!(
        app.mode,
        Mode::Settings {
            capturing: false,
            ..
        }
    ));
    let ctrl_n = Chord::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
    assert_eq!(app.keymap.binding(Action::Next).chord, ctrl_n);

    // A clash is refused and says with what.
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('s')); // Shuffle has it
    assert_eq!(app.keymap.binding(Action::Next).chord, ctrl_n);
    assert!(app.status.contains("Shuffle"), "{}", app.status);

    // Global: allowed for Ctrl+N, refused for a plain key.
    key(&mut app, KeyCode::Char(' '));
    assert!(app.keymap.binding(Action::Next).global);
    key(&mut app, KeyCode::Down); // Previous track: plain 'p'
    key(&mut app, KeyCode::Char(' '));
    assert!(!app.keymap.binding(Action::Prev).global);
    assert!(app.status.contains("Ctrl or Alt"), "{}", app.status);

    // Backspace gives the default back.
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Backspace);
    assert_eq!(
        app.keymap.binding(Action::Next),
        Keymap::defaults().binding(Action::Next)
    );
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::Normal));

    // What was chosen is on disk.
    key(&mut app, KeyCode::Char('S'));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::F(7), KeyModifiers::NONE);
    let (again, _) = Keymap::load(paths.db.clone());
    assert_eq!(again.binding(Action::Next).chord.code, KeyCode::F(7));
}

#[test]
fn a_rebound_key_does_the_action_and_the_old_one_does_not() {
    let (_tmp, _paths, mut app, _r) = setup();
    app.keymap
        .rebind(
            Action::Fullscreen,
            Chord::new(KeyCode::F(11), KeyModifiers::NONE),
        )
        .unwrap();
    key(&mut app, KeyCode::Char('F'));
    assert!(!app.fullscreen);
    key(&mut app, KeyCode::F(11));
    assert!(app.fullscreen);
    // Actions also work while a dialog is open when they come from outside.
    app.mode = Mode::Help;
    app.run_action(Action::Fullscreen);
    assert!(!app.fullscreen);
}
