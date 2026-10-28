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
    key(&mut app, KeyCode::F(5));
    assert!(
        refresh_rx.try_recv().is_ok(),
        "the library is asked to rescan"
    );
    assert!(app.status.contains("Refreshing"));
    pump(&mut app, "new picture", |a| a.visual.active.is_some());
    assert_eq!(app.desired_gif().as_deref(), Some("new.gif"));
}

#[test]
fn v_cycles_picture_blocks_and_ascii() {
    let (_tmp, _paths, mut app, _r) = setup();
    assert_eq!(app.visual.mode, VisualMode::Picture);
    key(&mut app, KeyCode::Char('v'));
    assert_eq!(app.visual.mode, VisualMode::Blocks);
    key(&mut app, KeyCode::Char('v'));
    assert_eq!(app.visual.mode, VisualMode::Ascii);
    key(&mut app, KeyCode::Char('v'));
    assert_eq!(app.visual.mode, VisualMode::Picture);
}

#[test]
fn a_pixel_picture_under_another_panel_is_drawn_as_blocks() {
    // Pixel graphics would cover the text lying over them.
    assert_eq!(
        VisualMode::Picture.effective(true, true),
        VisualMode::Blocks
    );
    assert_eq!(
        VisualMode::Picture.effective(false, true),
        VisualMode::Picture
    );
    assert_eq!(
        VisualMode::Picture.effective(true, false),
        VisualMode::Picture
    );
    assert_eq!(VisualMode::Ascii.effective(true, true), VisualMode::Ascii);
    assert_eq!(VisualMode::from_config("blocks"), VisualMode::Blocks);
    assert_eq!(VisualMode::from_config("ASCII"), VisualMode::Ascii);
    assert_eq!(VisualMode::from_config("x"), VisualMode::Picture);
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
    key(&mut app, KeyCode::Char('f'));
    assert!(!app.fullscreen);
    key(&mut app, KeyCode::F(11));
    assert!(app.fullscreen);
    // Actions also work while a dialog is open when they come from outside.
    app.mode = Mode::Help { scroll: 0 };
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

    key(&mut app, KeyCode::Char('}'));
    assert!(app.status.contains("Lyrics"), "{}", app.status);
    let t = screen_text(&mut app, 120, 30);
    assert!(!t.contains("Library"), "the Lyrics layout has no list");
    assert!(t.contains("Visual"));

    key(&mut app, KeyCode::Char('}'));
    assert!(app.status.contains("Cinema"), "{}", app.status);
    for _ in 0..4 {
        for (w, h) in [(120, 40), (80, 24), (50, 12), (20, 6), (10, 4), (1, 1)] {
            screen_text(&mut app, w, h);
        }
        key(&mut app, KeyCode::Char('}'));
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
    assert_eq!(draft.places[0].w, 24);
    assert!(!draft.places[2].visible && !draft.player);
    // The preview shows the draft at once, with the editor hints.
    let t = screen_text(&mut app, 120, 30);
    assert!(t.contains("Layout \"Custom 1\"") && !t.contains("Visual:"));
    for (w, h) in [(80, 24), (30, 8), (1, 1)] {
        screen_text(&mut app, w, h);
    }

    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Normal));
    assert_eq!(app.layouts.current().name, "Custom 1");
    assert!(!app.layouts.current().player);

    // It survives a restart and the built-in ones are untouched.
    let (again, _) = LayoutSet::load(paths.db.clone());
    assert_eq!(again.current().name, "Custom 1");
    assert_eq!(again.current().places[0].w, 24);

    // Editing a saved layout and "save as new" keeps the first one.
    key(&mut app, KeyCode::Char('E'));
    key(&mut app, KeyCode::Char('+'));
    key(&mut app, KeyCode::F(3));
    assert_eq!(app.layouts.current().name, "Custom 2");
    assert!(app.layouts.exists_custom("Custom 1"));

    // Esc discards; D deletes.
    key(&mut app, KeyCode::Char('E'));
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Esc);
    assert!(app.status.contains("not saved"));
    key(&mut app, KeyCode::Char('E'));
    key(&mut app, KeyCode::F(4));
    assert!(!app.layouts.exists_custom("Custom 2"));
    assert_eq!(app.layouts.current().name, "Classic");
}

#[test]
fn editing_a_built_in_layout_never_changes_it() {
    let (_tmp, _paths, mut app) = setup_with_db();
    let before = app.layouts.current().clone();
    key(&mut app, KeyCode::Char('E'));
    press(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
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
    key(&mut app, KeyCode::Char(')'));
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

    key(&mut app, KeyCode::F(2));
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

#[test]
fn player_keys_do_nothing_inside_the_editors() {
    let (_tmp, paths, app) = setup_with_palettes();
    let (layouts, _) = LayoutSet::load(paths.db.clone());
    let mut app = app.with_layouts(layouts);
    let before = app.layouts.current().clone();
    key(&mut app, KeyCode::Char('E'));
    let Mode::LayoutEdit { draft: start, .. } = &app.mode else {
        panic!("editing")
    };
    let start = start.clone();
    // Letters that are player keys (and used to be editor keys) change nothing.
    for c in "hjklHJKLnsxDvFR".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    let Mode::LayoutEdit { draft, .. } = &app.mode else {
        panic!("still editing")
    };
    assert_eq!(draft, &start);
    assert!(!app.fullscreen);
    // Ctrl+arrow resizes like Shift+arrow.
    press(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
    let Mode::LayoutEdit { draft, .. } = &app.mode else {
        panic!("still editing")
    };
    assert_eq!(draft.places[0].w, start.places[0].w - 5);
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.layouts.current(), &before);

    key(&mut app, KeyCode::Char('C'));
    let colors = app.palettes.current().colors.clone();
    for c in "jknsxzv".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    let Mode::PaletteEdit {
        draft,
        selected,
        typing,
    } = &app.mode
    else {
        panic!("editing colors")
    };
    assert_eq!(draft.colors, colors);
    assert_eq!((*selected, typing.is_none()), (0, true));
}

fn track(id: i64, title: &str) -> crate::db::Track {
    crate::db::Track {
        id,
        path: format!("{title}.mp3"),
        title: title.into(),
        artist: "A".into(),
        album: String::new(),
        duration_ms: 60_000,
        missing: false,
    }
}

fn with_tracks(app: &mut App, n: i64) {
    app.tracks = (1..=n).map(|i| track(i, &format!("t{i}"))).collect();
    app.track_index = app
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id, i))
        .collect();
}

#[test]
fn the_highlight_follows_the_track_that_plays() {
    let (_tmp, _paths, mut app, _r) = setup();
    with_tracks(&mut app, 5);
    app.lists[0].select(Some(0));
    // The player moves on by itself (next track, end of a track...).
    app.player.now_id = Some(4);
    app.follow_playing();
    assert_eq!(app.lists[0].selected(), Some(3));
    // The user moves away; nothing pulls the highlight back until the track changes.
    key(&mut app, KeyCode::Up);
    app.follow_playing();
    assert_eq!(app.lists[0].selected(), Some(2));
    app.player.now_id = Some(2);
    app.follow_playing();
    assert_eq!(app.lists[0].selected(), Some(1));
    // Favorite and "add to playlist" act on that same highlighted track.
    key(&mut app, KeyCode::Char('*'));
    assert!(app.status.contains("t2"), "{}", app.status);
}

#[test]
fn old_messages_give_way_to_the_key_hints() {
    let (_tmp, _paths, mut app, _r) = setup();
    app.age_status();
    assert_eq!(app.status, HINT);
    app.status = "Something happened".into();
    app.age_status();
    assert_eq!(app.status, "Something happened");
    app.status_since = Instant::now() - Duration::from_secs(10);
    app.age_status();
    assert_eq!(app.status, HINT);
}

#[test]
fn a_russian_layout_works_for_the_letter_keys() {
    let (_tmp, _paths, mut app, _r) = setup();
    key(&mut app, KeyCode::Char('а')); // the key of "f": full screen
    assert!(app.fullscreen);
    key(&mut app, KeyCode::Char('а'));
    assert!(!app.fullscreen);
    key(&mut app, KeyCode::Char('Х')); // Shift+"[": previous layout
    assert!(app.status.contains("Cinema"), "{}", app.status);
    key(&mut app, KeyCode::Char('Ъ')); // next layout
    assert!(app.status.contains("Classic"), "{}", app.status);
    // Typing into the search box is not translated.
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Char('ы'));
    assert_eq!(app.filter, "ы");
}

#[test]
fn layouts_and_palettes_have_keys_both_ways() {
    let (_tmp, _paths, mut app) = setup_with_palettes();
    key(&mut app, KeyCode::Char('{'));
    assert!(app.status.contains("Cinema"), "{}", app.status);
    key(&mut app, KeyCode::Char('}'));
    assert!(app.status.contains("Classic"), "{}", app.status);
    let start = app.theme.background;
    key(&mut app, KeyCode::Char(')'));
    let forward = app.theme.background;
    assert_ne!(forward, start);
    key(&mut app, KeyCode::Char('('));
    assert_eq!(app.theme.background, start);
}

#[test]
fn the_menu_lists_everything_and_runs_the_chosen_row() {
    let (_tmp, _paths, mut app, _r) = setup();
    key(&mut app, KeyCode::F(10));
    assert!(matches!(app.mode, Mode::Menu { .. }));
    let t = screen_text(&mut app, 120, 40);
    assert!(t.contains("Edit the screen layout") && t.contains("Pause / play"));
    for (w, h) in [(60, 20), (30, 8), (1, 1)] {
        screen_text(&mut app, w, h);
    }
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter); // "Edit the screen layout"
    assert!(matches!(app.mode, Mode::LayoutEdit { .. }));
    key(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert!(matches!(app.mode, Mode::Menu { .. }));
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::Normal));
    // Any action can be run from the menu too.
    key(&mut app, KeyCode::F(10));
    for _ in 0..5 + Action::ALL
        .iter()
        .position(|a| *a == Action::Fullscreen)
        .unwrap()
    {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Enter);
    assert!(app.fullscreen);
}

#[test]
fn help_scrolls_with_arrows_and_closes_with_any_other_key() {
    let (_tmp, _paths, mut app, _r) = setup();
    key(&mut app, KeyCode::F(1));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    assert!(matches!(app.mode, Mode::Help { scroll: 2 }));
    for (w, h) in [(120, 20), (60, 10), (1, 1)] {
        screen_text(&mut app, w, h);
    }
    key(&mut app, KeyCode::End);
    screen_text(&mut app, 120, 20);
    key(&mut app, KeyCode::Char('x'));
    assert!(matches!(app.mode, Mode::Normal));
}

fn mouse(app: &mut App, kind: ratatui::crossterm::event::MouseEventKind, col: u16, row: u16) {
    app.handle_event(Event::Mouse(ratatui::crossterm::event::MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }));
}

#[test]
fn panels_can_be_dragged_and_resized_with_the_mouse() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind::*};
    let (_tmp, _paths, mut app) = setup_with_db();
    screen_text(&mut app, 120, 30);
    key(&mut app, KeyCode::Char('E'));
    screen_text(&mut app, 120, 30);
    let main = app.layout_main;
    assert_eq!((main.width, main.height), (120, 25));
    let Mode::LayoutEdit { draft, .. } = &app.mode else {
        panic!("editing")
    };
    let lyrics = draft.places[1];
    // Press on the middle of the Lyrics panel, drag 12 columns right.
    mouse(&mut app, Down(MouseButton::Left), 60, 10);
    mouse(&mut app, Drag(MouseButton::Left), 66, 10);
    mouse(&mut app, Up(MouseButton::Left), 66, 10);
    let Mode::LayoutEdit {
        draft, selected, ..
    } = &app.mode
    else {
        panic!("editing")
    };
    assert_eq!(*selected, 1);
    // Lyrics sits against the right-hand panel, so it can only move a little.
    assert_eq!(draft.places[1].w, lyrics.w);
    assert_eq!(draft.places[1].x, lyrics.x + 5);

    // Grab the bottom-right corner of the Library panel and shrink it.
    let lib = draft.places[0];
    let r = lib.raw_rect(main);
    let (cx, cy) = (r.x + r.width - 1, r.y + r.height - 1);
    mouse(&mut app, Down(MouseButton::Left), cx, cy);
    mouse(&mut app, Drag(MouseButton::Left), cx - 12, cy - 5);
    mouse(&mut app, Up(MouseButton::Left), cx - 12, cy - 5);
    let Mode::LayoutEdit {
        draft, selected, ..
    } = &app.mode
    else {
        panic!("editing")
    };
    assert_eq!(*selected, 0);
    assert_eq!(draft.places[0].w, lib.w - 10);
    assert_eq!(draft.places[0].h, lib.h - 20);
    screen_text(&mut app, 120, 30);
    // A mouse event outside the editor does nothing.
    key(&mut app, KeyCode::Esc);
    mouse(&mut app, Down(MouseButton::Left), 5, 5);
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn lyrics_placed_over_the_picture_are_drawn_over_it() {
    use super::layout::{Layout, Place};
    let (_tmp, _paths, mut app) = setup_with_db();
    // Lyrics exactly on top of the Visual panel (and nothing else shown).
    let mut l = app.layouts.current().frozen(120);
    l.name = "Over".into();
    l.places[0].visible = false;
    l.places[1] = Place {
        visible: true,
        x: 40,
        y: 0,
        w: 60,
        h: 100,
    };
    l.places[2] = l.places[1];
    app.layouts.save(l.clone()).unwrap();
    let t = screen_text(&mut app, 120, 30);
    assert!(t.contains("Lyrics"), "the lyrics stay visible");
    assert!(!t.contains("Visual"), "the picture is under them");
    let _: &Layout = &l;
}
