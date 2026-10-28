//! Stage 8: karaoke on screen, full screen, themes, help, animations.

use std::sync::mpsc;

use ratatui::buffer::Buffer;

use super::lyrics_view::LyricsState;
use super::*;
use crate::lyrics::lrc::{Line, Word};
use crate::lyrics::{Lyrics, Source};
use crate::paths::Paths;

fn key(app: &mut App, code: KeyCode) {
    app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn make_app(animations: bool) -> (tempfile::TempDir, App) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::new(tmp.path().to_path_buf());
    paths.ensure_dirs().unwrap();
    drop(crate::db::Db::open(&paths.db).unwrap());
    let mut config = test_config();
    config.ui.animations = animations;
    let (_tx, rx) = mpsc::channel();
    let app = App::new(
        &config,
        None,
        rx,
        playlists::spawn(&paths),
        lyrics::spawn(paths.db.clone(), paths.lyrics.clone(), false),
        transcribe::spawn(&config.whisper, &paths),
        test_visuals(&paths),
        paths.music.clone(),
    );
    (tmp, app)
}

fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal.backend().buffer().clone()
}

fn text(b: &Buffer) -> String {
    b.content().iter().map(|c| c.symbol()).collect()
}

fn word(t: u64, s: &str) -> Word {
    Word {
        time_ms: t,
        text: s.into(),
    }
}

/// Pretend track 1 is paused at the start with these lyrics loaded.
fn with_lyrics(app: &mut App, lines: Vec<Line>) {
    app.player.now_id = Some(1);
    app.player.paused = true;
    app.lyrics_for = Some(1);
    app.lyrics_state = LyricsState::Ready {
        lyrics: Lyrics::Synced(lines),
        source: Source::LocalFile,
        offset_ms: 0,
        scroll: 0,
    };
}

fn line(time_ms: u64, text: &str, words: Vec<Word>) -> Line {
    Line {
        time_ms,
        text: text.into(),
        words,
        end_ms: None,
    }
}

#[test]
fn help_lists_every_key_of_the_spec_and_any_key_closes_it() {
    for k in ["a", "/", "g", "t", "S", "q", "? / F1"] {
        assert!(
            help::HELP.iter().any(|(key, _)| *key == k),
            "the help misses {k}"
        );
    }
    let (_tmp, mut app) = make_app(false);
    key(&mut app, KeyCode::Char('?'));
    assert!(matches!(app.mode, Mode::Help { .. }));
    assert!(text(&render(&mut app, 120, 40)).contains("Full screen"));
    for (w, h) in [(80, 20), (30, 8), (10, 4), (1, 1)] {
        render(&mut app, w, h);
    }
    key(&mut app, KeyCode::Char('x'));
    assert!(matches!(app.mode, Mode::Normal));
    assert!(!app.should_quit);
}

#[test]
fn fullscreen_shows_only_lyrics_and_picture_and_ignores_the_list_keys() {
    let (_tmp, mut app) = make_app(false);
    assert!(text(&render(&mut app, 120, 30)).contains("Library"));
    key(&mut app, KeyCode::Char('f'));
    assert!(app.fullscreen);
    for (w, h) in [(120, 30), (80, 24), (50, 12), (10, 4), (1, 1)] {
        let t = text(&render(&mut app, w, h));
        assert!(!t.contains("Library"), "{w}x{h}");
    }
    let t = text(&render(&mut app, 120, 30));
    assert!(t.contains("Lyrics") && t.contains("Visual"));
    // Keys that would act on the hidden list do nothing.
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::Normal));
    assert_eq!(app.tab, 0);
    key(&mut app, KeyCode::Esc);
    assert!(!app.fullscreen);
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Char('f'));
    assert!(!app.fullscreen);
}

#[test]
fn theme_key_cycles_through_the_presets() {
    let (_tmp, mut app) = make_app(false);
    let first = app.theme.background;
    let mut seen = vec![first];
    for _ in 0..4 {
        key(&mut app, KeyCode::Char(')'));
        seen.push(app.theme.background);
    }
    seen.sort_by_key(|c| format!("{c:?}"));
    seen.dedup();
    assert_eq!(seen.len(), 5, "five different backgrounds");
    key(&mut app, KeyCode::Char(')'));
    assert_eq!(app.theme.background, first, "the cycle closes");
    assert!(app.status.starts_with("Colors:"));
    render(&mut app, 120, 30);
}

#[test]
fn the_current_word_is_highlighted_karaoke_style() {
    let (_tmp, mut app) = make_app(false);
    // At position 0 the first word is reached, the second is not.
    with_lyrics(
        &mut app,
        vec![
            line(0, "one two", vec![word(0, "one "), word(60_000, "two")]),
            line(90_000, "next", vec![]),
        ],
    );
    let buf = render(&mut app, 120, 30);
    let find = |needle: char| {
        buf.content()
            .iter()
            .position(|c| c.symbol() == needle.to_string() && c.fg != app.theme.dim)
            .map(|i| buf.content()[i].fg)
    };
    let sung = find('o').unwrap();
    let waiting = find('w').unwrap();
    assert_eq!(sung, app.theme.accent);
    assert_eq!(waiting, app.theme.foreground);
}

#[test]
fn animations_start_on_a_new_line_and_can_be_turned_off() {
    for (enabled, expect) in [(true, true), (false, false)] {
        let (_tmp, mut app) = make_app(enabled);
        with_lyrics(&mut app, vec![line(0, "hello", vec![])]);
        render(&mut app, 120, 30); // lets the panel report its size
        app.tick_effects();
        assert_eq!(app.fx.running(), expect, "animations = {enabled}");
        // Frames with running effects still draw.
        render(&mut app, 120, 30);
        render(&mut app, 10, 4);
    }
}

#[test]
fn the_word_mode_draws_only_the_sung_word_in_big_letters() {
    let (_tmp, mut app) = make_app(false);
    with_lyrics(
        &mut app,
        vec![
            line(0, "one two", vec![word(0, "one "), word(60_000, "two")]),
            line(90_000, "next line", vec![]),
        ],
    );
    key(&mut app, KeyCode::Char('w'));
    assert_eq!(app.lyrics_mode, lyrics_view::LyricsMode::Word);
    assert!(app.status.contains("one word"), "{}", app.status);
    let buf = render(&mut app, 120, 40);
    let t = text(&buf);
    // The text itself is not printed as words: only big quadrant characters.
    assert!(!t.contains("one two") && !t.contains("next line"));
    let blocks = buf
        .content()
        .iter()
        .filter(|c| ('\u{2580}'..='\u{259F}').contains(&c.symbol().chars().next().unwrap_or(' ')))
        .count();
    assert!(blocks > 40, "big letters are drawn: {blocks}");
    // The other font, and every size.
    key(&mut app, KeyCode::Char('W'));
    assert!(app.status.contains("Oswald"), "{}", app.status);
    for (w, h) in [(120, 40), (60, 14), (30, 8), (10, 4), (1, 1)] {
        render(&mut app, w, h);
    }
    // Back to all lines.
    key(&mut app, KeyCode::Char('w'));
    assert!(text(&render(&mut app, 120, 40)).contains("next line"));
}
