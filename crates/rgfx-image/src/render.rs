//! Resize options and the blit that writes a decoded image into a framebuffer.

use crate::aspect::{fit_dimensions, pixel_aspect};
use crate::preprocess::Preprocess;
use image::RgbaImage;
use image::imageops::FilterType;
use rgfx_core::{Color, Framebuffer, Viewport};

/// The resampling filter used when resizing an image to the render resolution.
///
/// Prefer the high-quality options for stills; `Nearest`/`Triangle` are cheaper
/// for large downscales where quality matters less.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ResizeFilter {
    /// Nearest-neighbour: fastest, blocky.
    Nearest,
    /// Linear (triangle) interpolation: fast, decent quality.
    Triangle,
    /// Catmull–Rom cubic: good quality.
    CatmullRom,
    /// Lanczos with a window of 3: highest quality, the default for stills.
    #[default]
    Lanczos3,
}

impl ResizeFilter {
    fn to_image(self) -> FilterType {
        match self {
            ResizeFilter::Nearest => FilterType::Nearest,
            ResizeFilter::Triangle => FilterType::Triangle,
            ResizeFilter::CatmullRom => FilterType::CatmullRom,
            ResizeFilter::Lanczos3 => FilterType::Lanczos3,
        }
    }
}

/// How to turn a [`crate::DecodedImage`] into a [`Framebuffer`] for a viewport.
///
/// The subpixel factors and `cell_aspect` together determine the framebuffer
/// pixel shape (see [`pixel_aspect`]); the image is letterboxed within the
/// viewport's render size and centred, with borders filled by `background`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    /// Framebuffer pixels per cell horizontally (2 for Braille, 1 otherwise).
    pub subpixel_x: u16,
    /// Framebuffer pixels per cell vertically (4 for Braille, 2 for half-blocks).
    pub subpixel_y: u16,
    /// Physical width-to-height ratio of one terminal cell (~0.5 is typical).
    pub cell_aspect: f32,
    /// The resampling filter used for the resize.
    pub filter: ResizeFilter,
    /// The letterbox/background fill for pixels outside the fitted image.
    pub background: Color,
    /// The image-quality stage (tone + dithering) applied to the framebuffer
    /// after the blit. Defaults to an identity transform.
    pub preprocess: Preprocess,
}

impl RenderOptions {
    /// Options for a Braille encoder: 2×4 subpixels per cell.
    pub fn braille() -> Self {
        Self {
            subpixel_x: 2,
            subpixel_y: 4,
            cell_aspect: 0.5,
            filter: ResizeFilter::Lanczos3,
            background: Color::TRANSPARENT,
            preprocess: Preprocess::IDENTITY,
        }
    }

    /// Options for a half-block encoder: 1×2 subpixels per cell.
    pub fn half_blocks() -> Self {
        Self {
            subpixel_x: 1,
            subpixel_y: 2,
            cell_aspect: 0.5,
            filter: ResizeFilter::Lanczos3,
            background: Color::TRANSPARENT,
            preprocess: Preprocess::IDENTITY,
        }
    }

    /// Options for an ASCII encoder: 1×1 subpixels per cell.
    pub fn ascii() -> Self {
        Self {
            subpixel_x: 1,
            subpixel_y: 1,
            cell_aspect: 0.5,
            filter: ResizeFilter::Lanczos3,
            background: Color::TRANSPARENT,
            preprocess: Preprocess::IDENTITY,
        }
    }

    /// The framebuffer-pixel aspect correction factor for these options.
    ///
    /// This is the value the CLI passes through so renderer-specific ratios are
    /// honoured. See [`pixel_aspect`].
    pub fn pixel_aspect(&self) -> f32 {
        pixel_aspect(self.cell_aspect, self.subpixel_x, self.subpixel_y)
    }
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self::braille()
    }
}

/// Resizes `image` and blits it, centred and aspect-corrected, into `target`.
///
/// `target` is resized to the viewport's render size, cleared to
/// `opts.background`, and then the fitted image is written into it. Reusing an
/// existing `Framebuffer` keeps this allocation-light on the caller's side.
pub(crate) fn render_into(
    image: &RgbaImage,
    target: &mut Framebuffer,
    viewport: Viewport,
    opts: &RenderOptions,
) {
    let (rw, rh) = viewport.render_size(opts.subpixel_x, opts.subpixel_y);
    target.resize(rw, rh);
    target.clear(opts.background);
    if rw == 0 || rh == 0 {
        return;
    }

    let (fw, fh) = fit_dimensions(image.width(), image.height(), rw, rh, opts.pixel_aspect());
    if fw == 0 || fh == 0 {
        return;
    }

    let resized = image::imageops::resize(image, fw as u32, fh as u32, opts.filter.to_image());

    let ox = (rw - fw) / 2;
    let oy = (rh - fh) / 2;
    for y in 0..fh {
        for x in 0..fw {
            let px = resized.get_pixel(x as u32, y as u32);
            let [r, g, b, a] = px.0;
            target.set(ox + x, oy + y, Color::from_u8(r, g, b, a));
        }
    }

    // The image-quality stage runs in place on the finished framebuffer.
    opts.preprocess.apply(target);
}
