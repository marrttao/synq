//! The sound visualizer panel: spectrum bars that move with the music, in
//! two styles. The bars always fill the panel's width evenly.

use ratatui::Frame;
use ratatui::layout::Rect;

use super::spectrum::BANDS;
use super::{App, Mode};

/// Eighths of a cell, from empty to full.
const EIGHTHS: [char; 9] = [
    ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
    '\u{2588}',
];
/// The thin mark above a bar that shows the recent peak.
const PEAK_MARK: char = '\u{2594}';
const FULL: char = '\u{2588}';
const LOWER_HALF: char = '\u{2584}';
const UPPER_HALF: char = '\u{2580}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SoundStyle {
    /// Bars growing from the bottom, with peak marks.
    Bars,
    /// Bars growing both ways from the middle line.
    Mirror,
}

impl SoundStyle {
    pub(super) fn from_config(name: &str) -> Self {
        if name.trim().eq_ignore_ascii_case("mirror") {
            Self::Mirror
        } else {
            Self::Bars
        }
    }

    pub(super) fn cycle(self) -> Self {
        match self {
            Self::Bars => Self::Mirror,
            Self::Mirror => Self::Bars,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Bars => "bars",
            Self::Mirror => "mirror",
        }
    }
}

/// Where each bar goes: `(x offset, width)`, filling `width` columns evenly
/// with one-column gaps (none when it is too tight for them).
fn bar_columns(width: u16) -> Vec<(u16, u16)> {
    if width == 0 {
        return Vec::new();
    }
    // About two columns per bar and one for the gap.
    let count = usize::from((width + 1) / 3).clamp(1, BANDS);
    let count = if width < 8 { usize::from(width) } else { count };
    let gap = u16::from(count > 1 && width >= 8);
    let total = width - gap * (count as u16 - 1);
    let (base, extra) = (total / count as u16, usize::from(total % count as u16));
    let mut x = 0u16;
    (0..count)
        .map(|i| {
            // Spread the leftover columns evenly instead of piling them up.
            let w = base + u16::from((i + 1) * extra / count > i * extra / count);
            let at = (x, w);
            x += w + gap;
            at
        })
        .collect()
}

/// The height (0 to 1) of bar `i` of `count`, from the bands it stands for.
fn bar_level(values: &[f32; BANDS], i: usize, count: usize) -> f32 {
    let from = i * BANDS / count;
    let to = ((i + 1) * BANDS / count).clamp(from + 1, BANDS);
    let slice = &values[from..to];
    let max = slice.iter().copied().fold(0.0f32, f32::max);
    let mean = slice.iter().sum::<f32>() / slice.len() as f32;
    // The mean keeps wide bars from being all spike.
    0.6 * max + 0.4 * mean
}

impl App {
    /// Update the bars once per frame (only when a panel shows them).
    pub(super) fn tick_spectrum(&mut self) {
        let wanted = match &self.mode {
            Mode::LayoutEdit { .. } => false,
            _ => self.layouts.current().places[3].visible,
        };
        let playing = self.player.now_id.is_some() && !self.player.paused;
        let moving = self.spectrum.bands.iter().any(|&b| b > 0.0)
            || self.spectrum.peaks.iter().any(|&p| p > 0.0);
        if wanted || moving {
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
        let columns = bar_columns(inner.width);
        let count = columns.len();
        let rows = usize::from(inner.height);
        let buf = f.buffer_mut();
        let mut put = |x: u16, y: u16, ch: char, color| {
            let cell = &mut buf[(inner.x + x, inner.y + y)];
            cell.set_char(ch).set_fg(color);
        };
        for (i, &(x0, w)) in columns.iter().enumerate() {
            let level = bar_level(&self.spectrum.bands, i, count);
            let peak = bar_level(&self.spectrum.peaks, i, count);
            let across = i as f32 / count.saturating_sub(1).max(1) as f32;
            // Color follows both the place along the spectrum and the height.
            let color_at = |height: f32| self.theme.gradient(0.55 * across + 0.45 * height);
            match self.sound_style {
                SoundStyle::Bars => {
                    let eighths = (level * rows as f32 * 8.0).round() as usize;
                    for row in 0..rows {
                        // `row` counts from the bottom.
                        let fill = eighths.saturating_sub(row * 8).min(8);
                        if fill == 0 {
                            continue;
                        }
                        let y = (rows - 1 - row) as u16;
                        let color = color_at(row as f32 / rows as f32);
                        for dx in 0..w {
                            put(x0 + dx, y, EIGHTHS[fill], color);
                        }
                    }
                    // The peak mark sits just above the bar.
                    let mark_row = ((peak * rows as f32 * 8.0).round() as usize / 8).min(rows - 1);
                    if peak > 0.02 && mark_row > eighths / 8 {
                        let y = (rows - 1 - mark_row) as u16;
                        for dx in 0..w {
                            put(x0 + dx, y, PEAK_MARK, color_at(peak));
                        }
                    }
                }
                SoundStyle::Mirror => {
                    // Half-cell steps; each side of the middle gets half the rows.
                    let side = rows / 2;
                    let halves = (level * rows as f32).round() as usize;
                    for k in 0..side {
                        let covered = halves.saturating_sub(k * 2).min(2);
                        if covered == 0 {
                            continue;
                        }
                        let color = color_at(k as f32 / side.max(1) as f32);
                        let (up_y, down_y) = ((side - 1 - k) as u16, (rows - side + k) as u16);
                        for dx in 0..w {
                            put(
                                x0 + dx,
                                up_y,
                                if covered == 2 { FULL } else { LOWER_HALF },
                                color,
                            );
                            put(
                                x0 + dx,
                                down_y,
                                if covered == 2 { FULL } else { UPPER_HALF },
                                color,
                            );
                        }
                    }
                    // With an odd number of rows the middle row is a thin base.
                    if rows % 2 == 1 && level > 0.02 {
                        for dx in 0..w {
                            put(x0 + dx, side as u16, FULL, color_at(0.0));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_fill_the_width_exactly_and_evenly() {
        for width in [1u16, 2, 5, 8, 9, 20, 33, 50, 100, 157, 400] {
            let cols = bar_columns(width);
            assert!(!cols.is_empty(), "{width}");
            // Inside the panel, in order, without overlapping.
            let mut end = 0;
            for &(x, w) in &cols {
                assert!(w >= 1 && x >= end, "{width}: {cols:?}");
                end = x + w;
            }
            assert!(end <= width, "{width}: {cols:?}");
            // Nothing is left over at the right edge.
            assert_eq!(end, width, "{width}: {cols:?}");
            // Widths differ by at most one column.
            let widths: Vec<u16> = cols.iter().map(|c| c.1).collect();
            let (lo, hi) = (widths.iter().min().unwrap(), widths.iter().max().unwrap());
            assert!(hi - lo <= 1, "{width}: {widths:?}");
        }
        assert!(bar_columns(0).is_empty());
    }

    #[test]
    fn a_bar_stands_for_its_bands() {
        let mut values = [0.0f32; BANDS];
        values[0] = 1.0;
        assert!(bar_level(&values, 0, BANDS) > 0.9);
        assert_eq!(bar_level(&values, 1, BANDS), 0.0);
        // Fewer bars than bands: a bar covers several, the loudest leads.
        assert!(bar_level(&values, 0, 16) > 0.5);
        assert_eq!(bar_level(&[0.5; BANDS], 3, 20), 0.5);
    }

    #[test]
    fn style_names_come_from_config() {
        assert_eq!(SoundStyle::from_config("mirror"), SoundStyle::Mirror);
        assert_eq!(SoundStyle::from_config("???"), SoundStyle::Bars);
        assert_eq!(SoundStyle::Bars.cycle().cycle(), SoundStyle::Bars);
    }
}
