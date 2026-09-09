//! Integer line rasterization into a [`Framebuffer`].
//!
//! [`draw_line`] plots the classic integer Bresenham pixel set for a segment and clips it to the
//! framebuffer viewport: pixels outside `0..width` × `0..height` are simply not written, so an
//! off-screen or partially off-screen segment never writes out of bounds and never panics. The
//! in-bounds portion of a line is exactly the pixel set Bresenham would produce for the full
//! segment — clipping only removes pixels, it never shifts them.

use rgfx_core::{Color, Framebuffer};

/// Draws the line segment from `(x0, y0)` to `(x1, y1)` into `fb` using integer Bresenham,
/// setting each covered in-bounds pixel to `color`.
///
/// Coordinates are signed pixel positions and may lie outside the framebuffer: the segment is
/// clipped to the viewport by skipping out-of-bounds pixels, so no write ever lands out of bounds
/// and the call never panics. Depth is ignored — every plotted pixel is written unconditionally
/// (last write wins), which is the intended behavior for a flat wireframe overlay.
///
/// The set of pixels written is exactly the Bresenham rasterization of the segment intersected
/// with the viewport rectangle.
pub fn draw_line(fb: &mut Framebuffer, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
    let w = fb.width() as i32;
    let h = fb.height() as i32;
    if w == 0 || h == 0 {
        return;
    }

    // Trivial reject: if both endpoints lie strictly beyond the same edge of the viewport, the
    // whole segment is off-screen and contributes no in-bounds pixels. This keeps the stepping
    // loop from iterating over long fully-off-screen runs while never dropping a visible pixel.
    if (x0 < 0 && x1 < 0) || (x0 >= w && x1 >= w) || (y0 < 0 && y1 < 0) || (y0 >= h && y1 >= h) {
        return;
    }

    let mut x = x0;
    let mut y = y0;
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        if x >= 0 && x < w && y >= 0 && y < h {
            fb.set(x as usize, y as usize, color);
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collects the set of pixels with non-zero alpha, as `(x, y)` pairs.
    fn drawn(fb: &Framebuffer) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for y in 0..fb.height() {
            for x in 0..fb.width() {
                if fb.get(x, y).a > 0.0 {
                    out.push((x, y));
                }
            }
        }
        out.sort_unstable();
        out
    }

    fn draw(fb: &mut Framebuffer, a: (i32, i32), b: (i32, i32)) {
        draw_line(fb, a.0, a.1, b.0, b.1, Color::WHITE);
    }

    #[test]
    fn horizontal_line_exact_pixels() {
        let mut fb = Framebuffer::new(8, 8);
        draw(&mut fb, (1, 3), (5, 3));
        assert_eq!(drawn(&fb), vec![(1, 3), (2, 3), (3, 3), (4, 3), (5, 3)]);
    }

    #[test]
    fn horizontal_line_is_direction_agnostic() {
        let mut a = Framebuffer::new(8, 8);
        let mut b = Framebuffer::new(8, 8);
        draw(&mut a, (1, 3), (5, 3));
        draw(&mut b, (5, 3), (1, 3));
        assert_eq!(
            drawn(&a),
            drawn(&b),
            "endpoint order must not change the pixels"
        );
    }

    #[test]
    fn vertical_line_exact_pixels() {
        let mut fb = Framebuffer::new(8, 8);
        draw(&mut fb, (2, 1), (2, 4));
        assert_eq!(drawn(&fb), vec![(2, 1), (2, 2), (2, 3), (2, 4)]);
    }

    #[test]
    fn diagonal_line_exact_pixels() {
        let mut fb = Framebuffer::new(8, 8);
        draw(&mut fb, (0, 0), (4, 4));
        assert_eq!(
            drawn(&fb),
            vec![(0, 0), (1, 1), (2, 2), (3, 3), (4, 4)],
            "a 45-degree line steps one pixel diagonally each iteration"
        );
    }

    #[test]
    fn steep_line_exact_pixels() {
        // Slope steeper than 45 degrees: Y advances faster than X, so several pixels share an X.
        let mut fb = Framebuffer::new(8, 8);
        draw(&mut fb, (0, 0), (2, 5));
        assert_eq!(
            drawn(&fb),
            vec![(0, 0), (0, 1), (1, 2), (1, 3), (2, 4), (2, 5)]
        );
    }

    #[test]
    fn single_point_line_plots_one_pixel() {
        let mut fb = Framebuffer::new(8, 8);
        draw(&mut fb, (3, 3), (3, 3));
        assert_eq!(drawn(&fb), vec![(3, 3)]);
    }

    #[test]
    fn partially_offscreen_line_clips_without_oob() {
        // Left endpoint is off-screen; only the in-bounds pixels are written.
        let mut fb = Framebuffer::new(5, 5);
        draw(&mut fb, (-3, 2), (2, 2));
        assert_eq!(drawn(&fb), vec![(0, 2), (1, 2), (2, 2)]);
    }

    #[test]
    fn vertical_line_clips_at_top_and_bottom() {
        let mut fb = Framebuffer::new(5, 5);
        draw(&mut fb, (2, -3), (2, 7));
        assert_eq!(drawn(&fb), vec![(2, 0), (2, 1), (2, 2), (2, 3), (2, 4)]);
    }

    #[test]
    fn fully_offscreen_line_draws_nothing() {
        let mut fb = Framebuffer::new(5, 5);
        draw(&mut fb, (-10, -10), (-4, -1));
        assert!(drawn(&fb).is_empty());
        draw(&mut fb, (10, 0), (20, 4));
        assert!(drawn(&fb).is_empty());
    }

    #[test]
    fn extreme_coordinates_do_not_panic() {
        // A segment that crosses the whole small viewport with large endpoints must terminate and
        // stay in bounds.
        let mut fb = Framebuffer::new(4, 4);
        draw(&mut fb, (-1000, -1000), (1000, 1000));
        // The main diagonal is covered; every written pixel is in bounds (guaranteed by `drawn`).
        assert!(fb.get(0, 0).a > 0.0);
        assert!(fb.get(3, 3).a > 0.0);
    }

    #[test]
    fn zero_sized_framebuffer_is_a_noop() {
        let mut fb = Framebuffer::new(0, 0);
        draw(&mut fb, (0, 0), (5, 5)); // must not panic
        assert!(fb.is_empty());
    }
}
