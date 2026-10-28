//! The "blocks" picture: every terminal cell shows 2 x 2 picture pixels,
//! using the quadrant characters with a foreground and a background color
//! (twice the detail of half blocks). Drawn as ordinary text, so it is
//! smooth and other panels can lie over it.

use image::RgbaImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// One cell: the character and its two colors.
pub type Cell = (char, Color, Color);

/// Quadrant characters for the sets of pixels that get the foreground
/// color (bits: 1 top-left, 2 top-right, 4 bottom-left, 8 bottom-right).
/// Complements are the same split with the colors swapped, so only the
/// splits that do not include the top-left corner are needed... except
/// that the corner alone is also listed to keep the table obvious.
const SPLITS: [(u8, char); 7] = [
    (1, '\u{2598}'),
    (2, '\u{259D}'),
    (4, '\u{2596}'),
    (8, '\u{2597}'),
    (3, '\u{2580}'),
    (5, '\u{258C}'),
    (6, '\u{259E}'),
];

pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
}

/// `img` must be `2 * cols` x `2 * rows` pixels. Transparent pixels take the
/// color `bg`.
pub fn quadrants(img: &RgbaImage, cols: u16, rows: u16, bg: [u8; 3]) -> Grid {
    let pixel = |x: u32, y: u32| -> [i32; 3] {
        let p = img.get_pixel(x.min(img.width() - 1), y.min(img.height() - 1));
        let a = i32::from(p[3]);
        [0, 1, 2].map(|i| (i32::from(p[i]) * a + i32::from(bg[i]) * (255 - a)) / 255)
    };
    let mut cells = Vec::with_capacity(usize::from(cols) * usize::from(rows));
    for row in 0..u32::from(rows) {
        for col in 0..u32::from(cols) {
            let (x, y) = (col * 2, row * 2);
            let px = [
                pixel(x, y),
                pixel(x + 1, y),
                pixel(x, y + 1),
                pixel(x + 1, y + 1),
            ];
            cells.push(best_cell(&px));
        }
    }
    Grid { cols, rows, cells }
}

fn mean(px: &[[i32; 3]; 4], mask: u8, want: bool) -> ([i32; 3], i32) {
    let (mut sum, mut n) = ([0i32; 3], 0);
    for (i, p) in px.iter().enumerate() {
        if (mask >> i & 1 == 1) == want {
            for c in 0..3 {
                sum[c] += p[c];
            }
            n += 1;
        }
    }
    if n == 0 {
        return ([0; 3], 0);
    }
    (sum.map(|s| s / n), n)
}

fn error(px: &[[i32; 3]; 4], mask: u8, fg: [i32; 3], bg: [i32; 3]) -> i32 {
    px.iter()
        .enumerate()
        .map(|(i, p)| {
            let m = if mask >> i & 1 == 1 { fg } else { bg };
            (0..3).map(|c| (p[c] - m[c]).pow(2)).sum::<i32>()
        })
        .sum()
}

fn rgb(c: [i32; 3]) -> Color {
    Color::Rgb(
        c[0].clamp(0, 255) as u8,
        c[1].clamp(0, 255) as u8,
        c[2].clamp(0, 255) as u8,
    )
}

/// The character and colors that reproduce four pixels best.
fn best_cell(px: &[[i32; 3]; 4]) -> Cell {
    let (flat, _) = mean(px, 0b1111, true);
    let mut best = (error(px, 0b1111, flat, flat), '\u{2588}', flat, flat);
    for (mask, ch) in SPLITS {
        let (fg, _) = mean(px, mask, true);
        let (bg, _) = mean(px, mask, false);
        let err = error(px, mask, fg, bg);
        if err < best.0 {
            best = (err, ch, fg, bg);
        }
    }
    (best.1, rgb(best.2), rgb(best.3))
}

/// Put the grid into the buffer, at the top left of `area`.
pub fn paint(buf: &mut Buffer, area: Rect, grid: &Grid) {
    for row in 0..grid.rows.min(area.height) {
        for col in 0..grid.cols.min(area.width) {
            let Some(&(ch, fg, bg)) = grid
                .cells
                .get(usize::from(row) * usize::from(grid.cols) + usize::from(col))
            else {
                continue;
            };
            let cell = &mut buf[(area.x + col, area.y + row)];
            cell.set_char(ch).set_fg(fg).set_bg(bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn grid_of(pixels: [[u8; 3]; 4]) -> Grid {
        let mut img = RgbaImage::new(2, 2);
        for (i, p) in pixels.iter().enumerate() {
            img.put_pixel(
                (i % 2) as u32,
                (i / 2) as u32,
                Rgba([p[0], p[1], p[2], 255]),
            );
        }
        quadrants(&img, 1, 1, [0, 0, 0])
    }

    #[test]
    fn a_flat_cell_is_a_full_block() {
        let g = grid_of([[10, 20, 30]; 4]);
        assert_eq!(
            g.cells[0],
            ('\u{2588}', Color::Rgb(10, 20, 30), Color::Rgb(10, 20, 30))
        );
    }

    #[test]
    fn a_split_cell_keeps_both_colors() {
        let (r, b) = ([255, 0, 0], [0, 0, 255]);
        // Left half red, right half blue: a half block with two colors.
        let (ch, fg, bg) = grid_of([r, b, r, b]).cells[0];
        assert_eq!(ch, '\u{258C}');
        assert_eq!((fg, bg), (Color::Rgb(255, 0, 0), Color::Rgb(0, 0, 255)));
        // One odd corner is shown as a quadrant, not averaged away.
        let (ch, fg, bg) = grid_of([b, r, r, r]).cells[0];
        assert_eq!(ch, '\u{2598}');
        assert_eq!((fg, bg), (Color::Rgb(0, 0, 255), Color::Rgb(255, 0, 0)));
        // Top and bottom.
        let (ch, ..) = grid_of([r, r, b, b]).cells[0];
        assert_eq!(ch, '\u{2580}');
    }

    #[test]
    fn transparent_pixels_take_the_background() {
        let mut img = RgbaImage::new(2, 2);
        for p in img.pixels_mut() {
            *p = Rgba([255, 255, 255, 0]);
        }
        let g = quadrants(&img, 1, 1, [7, 8, 9]);
        assert_eq!(g.cells[0].1, Color::Rgb(7, 8, 9));
    }

    #[test]
    fn painting_stays_inside_the_area() {
        let g = grid_of([[1, 2, 3]; 4]);
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 5));
        // The area is smaller than the grid, or even empty: nothing breaks.
        paint(&mut buf, Rect::new(2, 2, 0, 0), &g);
        paint(&mut buf, Rect::new(4, 4, 1, 1), &g);
        assert_eq!(buf[(4, 4)].symbol(), "\u{2588}");
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }
}
