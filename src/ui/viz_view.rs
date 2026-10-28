//! The sound visualizer panel: spectrum bars that move with the music.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use super::spectrum::BANDS;
use super::{App, Mode};

/// Eighths of a cell, from empty to full.
const EIGHTHS: [char; 9] = [
    ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
    '\u{2588}',
];

impl App {
    /// Update the bars once per frame (only when a panel shows them).
    pub(super) fn tick_spectrum(&mut self) {
        let wanted = match &self.mode {
            Mode::LayoutEdit { .. } => false,
            _ => self.layouts.current().places[3].visible,
        };
        let playing = self.player.now_id.is_some() && !self.player.paused;
        if wanted || self.spectrum.bands.iter().any(|&b| b > 0.0) {
            self.spectrum.update(self.player.scope(), playing && wanted);
        }
    }

    pub(super) fn draw_visualizer(&self, f: &mut Frame, area: Rect) {
        let block = self.block_n("Sound", 4);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        // Bars two columns wide with a gap, one column wide when it is tight.
        let (bar_w, gap) = if inner.width >= 3 * 12 {
            (2u16, 1u16)
        } else {
            (1, 0)
        };
        let count = usize::from(((inner.width + gap) / (bar_w + gap)).clamp(1, 128));
        let used = count as u16 * (bar_w + gap) - gap;
        let left = inner.x + (inner.width - used.min(inner.width)) / 2;
        let rows = usize::from(inner.height);
        for i in 0..count {
            // The bands this bar stands for: the loudest of them.
            let (from, to) = (
                i * BANDS / count,
                ((i + 1) * BANDS / count).max(i * BANDS / count + 1),
            );
            let level = self.spectrum.bands[from..to.min(BANDS)]
                .iter()
                .copied()
                .fold(0.0f32, f32::max);
            let eighths = (level * rows as f32 * 8.0).round() as usize;
            let color = self
                .theme
                .gradient(i as f32 / count.max(2).saturating_sub(1) as f32);
            let x = left + i as u16 * (bar_w + gap);
            for row in 0..rows {
                // `row` counts from the bottom.
                let fill = eighths.saturating_sub(row * 8).min(8);
                if fill == 0 {
                    continue;
                }
                let y = inner.y + inner.height - 1 - row as u16;
                let ch = EIGHTHS[fill].to_string().repeat(usize::from(bar_w));
                f.render_widget(
                    Paragraph::new(ch).style(Style::new().fg(color)),
                    Rect {
                        x,
                        y,
                        width: bar_w.min(inner.x + inner.width - x),
                        height: 1,
                    },
                );
            }
        }
    }
}
