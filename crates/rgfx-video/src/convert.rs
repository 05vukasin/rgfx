//! Converting one decoded `rgb24` frame into a reused [`Framebuffer`].
//!
//! This mirrors `rgfx-image`'s still-image blit — resize with a high-quality
//! filter, then centre and letterbox within the viewport's render size — reusing
//! `rgfx-image`'s public [`fit_dimensions`] / aspect math so video and stills
//! frame identically. The `image` crate performs the resample; the input frame
//! is borrowed (not copied) into an `image` view.

use image::imageops::FilterType;
use image::{ImageBuffer, Rgb};
use rgfx_core::{Color, Error, Framebuffer, Result, Viewport};
use rgfx_image::{RenderOptions, ResizeFilter, fit_dimensions};

/// Maps the reused [`ResizeFilter`] onto the `image` crate's filter type.
fn to_filter(filter: ResizeFilter) -> FilterType {
    match filter {
        ResizeFilter::Nearest => FilterType::Nearest,
        ResizeFilter::Triangle => FilterType::Triangle,
        ResizeFilter::CatmullRom => FilterType::CatmullRom,
        ResizeFilter::Lanczos3 => FilterType::Lanczos3,
    }
}

/// Renders one `rgb24` frame (`src_w` × `src_h`, 3 bytes per pixel, row-major)
/// into `target`.
///
/// `target` is resized to `viewport`'s render size for `opts`' subpixel factors,
/// cleared to `opts.background`, and the aspect-corrected frame is resized and
/// centred within it. Reusing `target` across calls keeps the framebuffer
/// allocation out of the per-frame path.
///
/// # Errors
///
/// Returns [`Error::Decode`] if `rgb.len()` does not equal `src_w * src_h * 3`.
/// Never panics.
pub fn render_rgb24_into(
    rgb: &[u8],
    src_w: u32,
    src_h: u32,
    target: &mut Framebuffer,
    viewport: Viewport,
    opts: &RenderOptions,
) -> Result<()> {
    let expected = (src_w as usize)
        .checked_mul(src_h as usize)
        .and_then(|px| px.checked_mul(3))
        .ok_or_else(|| Error::Decode("frame dimensions overflow".into()))?;
    if rgb.len() != expected {
        return Err(Error::Decode(format!(
            "rgb24 frame size mismatch: expected {expected} bytes for {src_w}x{src_h}, got {}",
            rgb.len()
        )));
    }

    let (rw, rh) = viewport.render_size(opts.subpixel_x, opts.subpixel_y);
    target.resize(rw, rh);
    target.clear(opts.background);
    if rw == 0 || rh == 0 || src_w == 0 || src_h == 0 {
        return Ok(());
    }

    let (fw, fh) = fit_dimensions(src_w, src_h, rw, rh, opts.pixel_aspect());
    if fw == 0 || fh == 0 {
        return Ok(());
    }

    // Borrow the frame bytes as an image view (no copy), then resample.
    let src: ImageBuffer<Rgb<u8>, &[u8]> = ImageBuffer::from_raw(src_w, src_h, rgb)
        .ok_or_else(|| Error::Decode("failed to wrap rgb24 frame as an image".into()))?;
    let resized = image::imageops::resize(&src, fw as u32, fh as u32, to_filter(opts.filter));

    let ox = (rw - fw) / 2;
    let oy = (rh - fh) / 2;
    for y in 0..fh {
        for x in 0..fw {
            let Rgb([r, g, b]) = *resized.get_pixel(x as u32, y as u32);
            target.set(ox + x, oy + y, Color::from_u8(r, g, b, 255));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_mismatch_is_error_not_panic() {
        let mut fb = Framebuffer::new(0, 0);
        // Claims 2x2 (12 bytes) but only supplies 3.
        let err = render_rgb24_into(
            &[1, 2, 3],
            2,
            2,
            &mut fb,
            Viewport::new(4, 4),
            &RenderOptions::braille(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Decode(_)));
    }

    #[test]
    fn resizes_target_to_viewport_render_size() {
        // 2x2 solid red frame.
        let rgb = vec![255u8, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0];
        let mut fb = Framebuffer::new(0, 0);
        render_rgb24_into(
            &rgb,
            2,
            2,
            &mut fb,
            Viewport::new(10, 5),
            &RenderOptions::braille(),
        )
        .unwrap();
        // Braille render size = (10*2, 5*4) = (20, 20).
        assert_eq!((fb.width(), fb.height()), (20, 20));
    }

    #[test]
    fn solid_frame_stays_solid_after_conversion() {
        // A 2x2 solid-blue frame into a square viewport with square pixels.
        let rgb = vec![0u8, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 255];
        let mut fb = Framebuffer::new(0, 0);
        render_rgb24_into(
            &rgb,
            2,
            2,
            &mut fb,
            Viewport::new(4, 2),
            &RenderOptions::braille(),
        )
        .unwrap();
        // render size = (8, 8); centre pixel should be opaque blue.
        let c = fb.get(4, 4);
        assert!(c.b > 0.9 && c.r < 0.1 && c.g < 0.1, "{c:?}");
        assert!(c.a > 0.9);
    }

    #[test]
    fn reuses_framebuffer_across_frames_without_regrowing() {
        let rgb = vec![10u8; 2 * 2 * 3];
        let mut fb = Framebuffer::new(0, 0);
        let vp = Viewport::new(8, 4);
        let opts = RenderOptions::braille();
        render_rgb24_into(&rgb, 2, 2, &mut fb, vp, &opts).unwrap();
        let (w0, h0) = (fb.width(), fb.height());
        render_rgb24_into(&rgb, 2, 2, &mut fb, vp, &opts).unwrap();
        assert_eq!((fb.width(), fb.height()), (w0, h0));
    }
}
