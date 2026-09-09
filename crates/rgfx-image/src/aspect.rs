//! Pure aspect-ratio math for placing a square-pixel image onto a terminal's
//! non-square framebuffer pixels.
//!
//! These functions contain no I/O and no `image` dependency, so they are cheap
//! to unit-test in isolation.

/// The physical aspect ratio (width / height) of a single framebuffer pixel.
///
/// This is the correction factor the resize step needs. Terminal cells are not
/// square: `cell_aspect` is the physical width-to-height ratio of one cell
/// (about `0.5` for the common ~1:2 cell). An encoder subdivides each cell into
/// `subpixel_x` × `subpixel_y` framebuffer pixels, so a single framebuffer
/// pixel is `cell_width / subpixel_x` wide and `cell_height / subpixel_y` tall,
/// giving a physical aspect of `cell_aspect * (subpixel_y / subpixel_x)`.
///
/// For the common cases this yields:
/// - Braille (`2×4`) with `cell_aspect = 0.5` → `1.0` (square pixels);
/// - half-blocks (`1×2`) with `cell_aspect = 0.5` → `1.0`;
/// - ASCII (`1×1`) with `cell_aspect = 0.5` → `0.5` (pixels twice as tall as wide).
///
/// Returns `1.0` for degenerate zero subpixel factors rather than a NaN/∞.
pub fn pixel_aspect(cell_aspect: f32, subpixel_x: u16, subpixel_y: u16) -> f32 {
    if subpixel_x == 0 || subpixel_y == 0 || !cell_aspect.is_finite() || cell_aspect <= 0.0 {
        return 1.0;
    }
    cell_aspect * (subpixel_y as f32 / subpixel_x as f32)
}

/// Fits a `img_w` × `img_h` source image (square source pixels) into a
/// `max_w` × `max_h` framebuffer region, preserving the image's real-world
/// aspect ratio given the per-pixel correction `pixel_aspect`.
///
/// Returns the framebuffer-pixel dimensions `(width, height)` the image should
/// be resized to. The result never exceeds the region and each axis is at least
/// `1` (unless the region or source is empty, in which case `(0, 0)`).
///
/// The physical on-screen size of the fitted region is
/// `width * pixel_aspect : height`, so the constraint solved here is
/// `width / height = (img_w / img_h) / pixel_aspect`.
pub fn fit_dimensions(
    img_w: u32,
    img_h: u32,
    max_w: usize,
    max_h: usize,
    pixel_aspect: f32,
) -> (usize, usize) {
    if img_w == 0 || img_h == 0 || max_w == 0 || max_h == 0 {
        return (0, 0);
    }
    let pa = if pixel_aspect.is_finite() && pixel_aspect > 0.0 {
        pixel_aspect as f64
    } else {
        1.0
    };

    // Target width / height ratio in framebuffer pixels.
    let ratio = (img_w as f64 / img_h as f64) / pa;

    let max_w_f = max_w as f64;
    let max_h_f = max_h as f64;

    // Try filling the full width first; fall back to filling the full height.
    let mut w = max_w_f;
    let mut h = (max_w_f / ratio).round();
    if h > max_h_f {
        h = max_h_f;
        w = (max_h_f * ratio).round();
    }

    let w = (w.round() as usize).clamp(1, max_w);
    let h = (h.round() as usize).clamp(1, max_h);
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braille_pixels_are_square() {
        assert!((pixel_aspect(0.5, 2, 4) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn half_block_pixels_are_square() {
        assert!((pixel_aspect(0.5, 1, 2) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ascii_pixels_are_tall() {
        // 1x1 subpixels over a 1:2 cell => pixel is twice as tall as wide.
        assert!((pixel_aspect(0.5, 1, 1) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn degenerate_inputs_fall_back_to_unity() {
        assert_eq!(pixel_aspect(0.5, 0, 4), 1.0);
        assert_eq!(pixel_aspect(0.0, 2, 4), 1.0);
        assert_eq!(pixel_aspect(f32::NAN, 2, 4), 1.0);
    }

    #[test]
    fn square_image_square_pixels_fills_square_region() {
        // 100x100 image into 80x80 region, square pixels => 80x80.
        assert_eq!(fit_dimensions(100, 100, 80, 80, 1.0), (80, 80));
    }

    #[test]
    fn wide_image_is_width_limited() {
        // 200x100 (2:1) into 80x80, square pixels => width 80, height 40.
        assert_eq!(fit_dimensions(200, 100, 80, 80, 1.0), (80, 40));
    }

    #[test]
    fn tall_image_is_height_limited() {
        // 100x200 (1:2) into 80x80, square pixels => height 80, width 40.
        assert_eq!(fit_dimensions(100, 200, 80, 80, 1.0), (40, 80));
    }

    #[test]
    fn ascii_correction_stretches_height() {
        // Square image, ASCII pixels (aspect 0.5, tall). Target ratio
        // w/h = 1 / 0.5 = 2, so a full-width 80 wants height 40.
        assert_eq!(fit_dimensions(100, 100, 80, 80, 0.5), (80, 40));
    }

    #[test]
    fn aspect_preserved_within_one_pixel() {
        // A 640x480 (4:3) photo into a 120x120 braille framebuffer (square px).
        let (w, h) = fit_dimensions(640, 480, 120, 120, 1.0);
        let want = 640.0f64 / 480.0;
        let got = w as f64 / h as f64;
        // Rounding to integer pixels keeps the displayed ratio within ~1px.
        assert!((got - want).abs() < (1.0 / h as f64) + 1e-9, "{w}x{h}");
    }

    #[test]
    fn empty_region_or_image_is_zero() {
        assert_eq!(fit_dimensions(0, 10, 80, 80, 1.0), (0, 0));
        assert_eq!(fit_dimensions(10, 10, 0, 80, 1.0), (0, 0));
    }
}
