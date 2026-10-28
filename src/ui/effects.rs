//! Animations (tachyonfx): a fade for the line that starts, for a line
//! entering from below, for a freshly loaded lyrics panel and for the
//! switch to full screen. Off completely when `ui.animations` is false.

use std::cell::Cell;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use tachyonfx::{Effect, EffectManager, Interpolation, fx};

use super::App;
use super::lyrics_view::LyricsState;

/// After a stall the effects should not jump to their end at once.
const MAX_STEP: Duration = Duration::from_millis(100);

pub(super) struct Effects {
    manager: EffectManager<u8>,
    enabled: bool,
    last: Instant,
    /// Line that was current when the last effect was started.
    line: Option<usize>,
    /// (track, lyrics ready) when the panel fade was last started.
    panel: (Option<i64>, bool),
    /// Inside of the lyrics panel as of the last frame.
    pub(super) lyrics_inner: Cell<Rect>,
}

impl Effects {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            manager: EffectManager::default(),
            enabled,
            last: Instant::now(),
            line: None,
            panel: (None, false),
            lyrics_inner: Cell::new(Rect::ZERO),
        }
    }

    pub(super) fn add(&mut self, effect: Effect) {
        if self.enabled {
            self.manager.add_effect(effect);
        }
    }

    #[cfg(test)]
    pub(super) fn running(&self) -> bool {
        self.manager.is_running()
    }

    /// Apply running effects to the frame that has just been drawn.
    pub(super) fn process(&mut self, buf: &mut Buffer, area: Rect) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).min(MAX_STEP);
        self.last = now;
        if self.enabled {
            let dt = tachyonfx::Duration::from_millis(dt.as_millis() as u32);
            self.manager.process_effects(dt, buf, area);
        }
    }
}

impl App {
    /// Start effects for what changed since the last frame.
    pub(super) fn tick_effects(&mut self) {
        if !self.fx.enabled {
            return;
        }
        let inner = self.fx.lyrics_inner.get();
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let bg = self.theme.background;
        let ease = Interpolation::QuadOut;

        let ready = matches!(self.lyrics_state, LyricsState::Ready { .. });
        let panel = (self.lyrics_for, ready);
        if panel != self.fx.panel {
            self.fx.panel = panel;
            if ready {
                self.fx
                    .add(fx::fade_from_fg(bg, (450, ease)).with_area(inner));
            }
        }

        let line = self.current_line_index();
        if line != self.fx.line {
            self.fx.line = line;
            if line.is_some() {
                let row = |y: u16| Rect {
                    y,
                    height: 1,
                    ..inner
                };
                // The line that starts glows up from the dim color...
                let mid = inner.y + inner.height / 2;
                self.fx
                    .add(fx::fade_from_fg(self.theme.dim, (350, ease)).with_area(row(mid)));
                // ...and the one that comes in from below fades in, which
                // makes the scroll look smooth.
                if inner.height > 2 {
                    self.fx.add(
                        fx::fade_from_fg(bg, (500, ease))
                            .with_area(row(inner.y + inner.height - 1)),
                    );
                }
            }
        }
    }

    /// `F`: switch full screen on or off.
    pub(super) fn toggle_fullscreen(&mut self) {
        self.fullscreen = !self.fullscreen;
        let bg = self.theme.background;
        self.fx
            .add(fx::fade_from(bg, bg, (350, Interpolation::QuadOut)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_effects_never_run() {
        let red = ratatui::style::Color::Red;
        let mut off = Effects::new(false);
        off.add(fx::fade_from_fg(red, 100));
        assert!(!off.running());
        let mut on = Effects::new(true);
        on.add(fx::fade_from_fg(red, 100));
        assert!(on.running());
    }
}
