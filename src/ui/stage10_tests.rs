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

fn setup_with_db() -> (tempfile::TempDir, Paths, App) {
    let (tmp, paths, app, _r) = setup();
    let (layouts, _) = LayoutSet::load(paths.db.clone());
    (tmp, paths, app.with_layouts(layouts))
}

fn screen_text(app: &mut App, w: u16, h: u16) -> String {
    let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
    t.draw(|f| app.draw(f)).unwrap();
    t.backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

#[test]
fn l_cycles_through_the_layouts_and_each_one_draws_at_every_size() {
    let (_tmp, _paths, mut app) = setup_with_db();
    let t = screen_text(&mut app, 120, 30);
    assert!(t.contains("Library") && t.contains("Lyrics") && t.contains("Visual"));

    key(&mut app, KeyCode::Char('L'));
    assert!(app.status.contains("Lyrics"), "{}", app.status);
    let t = screen_text(&mut app, 120, 30);
    assert!(!t.contains("Library"), "the Lyrics layout has no list");
    assert!(t.contains("Visual"));

    key(&mut app, KeyCode::Char('L'));
    assert!(app.status.contains("Cinema"), "{}", app.status);
    for _ in 0..4 {
        for (w, h) in [(120, 40), (80, 24), (50, 12), (20, 6), (10, 4), (1, 1)] {
            screen_text(&mut app, w, h);
        }
        key(&mut app, KeyCode::Char('L'));
    }
}

#[test]
fn the_editor_moves_resizes_hides_and_saves_a_new_layout() {
    let (_tmp, paths, mut app) = setup_with_db();
    key(&mut app, KeyCode::Char('E'));
    assert!(matches!(app.mode, Mode::LayoutEdit { .. }));
    // Library is selected first: shrink it and hide the Visual panel.
    press(&mut app, KeyCode::Left, KeyModifiers::SHIFT);
    press(&mut app, KeyCode::Left, KeyModifiers::SHIFT);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char(' ')); // Visual off
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char(' ')); // player bar off
    let Mode::LayoutEdit { draft, .. } = &app.mode else {
        panic!("still editing")
    };
    assert_eq!(draft.places[0].w, 30);
    assert!(!draft.places[2].visible && !draft.player);
    // The preview shows the draft at once, with the editor hints.
    let t = screen_text(&mut app, 120, 30);
    assert!(t.contains("Layout \"Custom 1\"") && !t.contains("Visual:"));
    for (w, h) in [(80, 24), (30, 8), (1, 1)] {
        screen_text(&mut app, w, h);
    }

    key(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::Normal));
    assert_eq!(app.layouts.current().name, "Custom 1");
    assert!(!app.layouts.current().player);

    // It survives a restart and the built-in ones are untouched.
    let (again, _) = LayoutSet::load(paths.db.clone());
    assert_eq!(again.current().name, "Custom 1");
    assert_eq!(again.current().places[0].w, 30);

    // Editing a saved layout and "save as new" keeps the first one.
    key(&mut app, KeyCode::Char('E'));
    key(&mut app, KeyCode::Char('+'));
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.layouts.current().name, "Custom 2");
    assert!(app.layouts.exists_custom("Custom 1"));

    // Esc discards; D deletes.
    key(&mut app, KeyCode::Char('E'));
    key(&mut app, KeyCode::Char('l'));
    key(&mut app, KeyCode::Esc);
    assert!(app.status.contains("not saved"));
    key(&mut app, KeyCode::Char('E'));
    key(&mut app, KeyCode::Char('D'));
    assert!(!app.layouts.exists_custom("Custom 2"));
    assert_eq!(app.layouts.current().name, "Classic");
}

#[test]
fn editing_a_built_in_layout_never_changes_it() {
    let (_tmp, _paths, mut app) = setup_with_db();
    let before = app.layouts.current().clone();
    key(&mut app, KeyCode::Char('E'));
    key(&mut app, KeyCode::Char('K'));
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.layouts.current(), &before);
}

fn setup_with_palettes() -> (tempfile::TempDir, Paths, App) {
    let (tmp, paths, app, _r) = setup();
    let (palettes, _) = PaletteSet::load(paths.db.clone(), "default");
    (tmp, paths, app.with_palettes(palettes))
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

#[test]
fn t_cycles_the_palettes_and_remembers_the_choice() {
    let (_tmp, paths, mut app) = setup_with_palettes();
    let first = app.theme.background;
    key(&mut app, KeyCode::Char('T'));
    assert!(app.status.contains("nord"), "{}", app.status);
    assert_ne!(app.theme.background, first);
    let (again, _) = PaletteSet::load(paths.db.clone(), "default");
    assert!(again.restored);
    assert_eq!(again.current().name, "nord");
}

#[test]
fn the_color_editor_sets_any_number_of_colors_and_shows_them_live() {
    let (_tmp, paths, mut app) = setup_with_palettes();
    key(&mut app, KeyCode::Char('C'));
    assert!(matches!(app.mode, Mode::PaletteEdit { .. }));
    // Background: type a new value.
    key(&mut app, KeyCode::Enter);
    for _ in 0..6 {
        key(&mut app, KeyCode::Backspace);
    }
    type_text(&mut app, "zz"); // not a hex digit: ignored
    type_text(&mut app, "102030");
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.theme.background,
        ratatui::style::Color::Rgb(0x10, 0x20, 0x30)
    );

    // A wrong value is refused and says so.
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Backspace);
    key(&mut app, KeyCode::Enter);
    assert!(app.status.contains("hex"), "{}", app.status);
    key(&mut app, KeyCode::Esc); // leaves typing, not the editor
    assert!(matches!(app.mode, Mode::PaletteEdit { .. }));

    // Grow to 8 colors; the extras rotate with the accent.
    for _ in 0..3 {
        key(&mut app, KeyCode::Char('+'));
    }
    let Mode::PaletteEdit {
        draft, selected, ..
    } = &app.mode
    else {
        panic!("editing")
    };
    assert_eq!((draft.colors.len(), *selected), (8, 7));
    for (w, h) in [(120, 40), (60, 20), (30, 8), (1, 1)] {
        screen_text(&mut app, w, h);
    }
    let t = screen_text(&mut app, 120, 40);
    assert!(t.contains("Extra 3") && t.contains("#102030"));

    // Shrinking stops at 2; the limit is on the other side too.
    for _ in 0..20 {
        key(&mut app, KeyCode::Char('-'));
    }
    let Mode::PaletteEdit { draft, .. } = &app.mode else {
        panic!("editing")
    };
    assert_eq!(draft.colors.len(), 2);
    for _ in 0..20 {
        key(&mut app, KeyCode::Char('+'));
    }
    let Mode::PaletteEdit { draft, .. } = &app.mode else {
        panic!("editing")
    };
    assert_eq!(draft.colors.len(), 12);

    key(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::Normal));
    assert_eq!(app.palettes.current().name, "Mine 1");
    let (again, _) = PaletteSet::load(paths.db.clone(), "default");
    assert_eq!(again.current().colors.len(), 12);
    assert_eq!(again.current().colors[0], [0x10, 0x20, 0x30]);
}

#[test]
fn leaving_the_color_editor_with_esc_restores_the_colors() {
    let (_tmp, _paths, mut app) = setup_with_palettes();
    let before = app.theme.background;
    key(&mut app, KeyCode::Char('C'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Backspace);
    type_text(&mut app, "f");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.theme.background, before);
    assert!(app.palettes.current().colors.len() == 5 && !app.palettes.current().custom);
}
