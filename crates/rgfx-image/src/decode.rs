//! PNG/JPEG decoding into an in-memory RGBA image and the framebuffer bridge.

use crate::render::{RenderOptions, render_into};
use image::{ImageError, ImageFormat, RgbaImage};
use rgfx_core::{Color, Error, Framebuffer, Result, Viewport};
use std::path::Path;

/// A decoded still image held as 8-bit RGBA pixels.
///
/// Produced by [`DecodedImage::load`] (from a file) or
/// [`DecodedImage::from_bytes`] (from in-memory bytes), and turned into a
/// [`Framebuffer`] by [`DecodedImage::to_framebuffer`].
#[derive(Clone, Debug)]
pub struct DecodedImage {
    image: RgbaImage,
}

impl DecodedImage {
    /// Loads and decodes a PNG or JPEG image from `path`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the file cannot be read, [`Error::Unsupported`]
    /// if the bytes are a recognized-but-unsupported format (anything other than
    /// PNG/JPEG), and [`Error::Decode`] if the bytes are malformed. Never panics.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    /// Decodes a PNG or JPEG image from in-memory `bytes`.
    ///
    /// The format is detected from the byte content, not a file extension.
    ///
    /// # Errors
    ///
    /// See [`DecodedImage::load`]. Empty or malformed input returns
    /// [`Error::Decode`] rather than panicking.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let format = image::guess_format(bytes).map_err(map_image_error)?;
        match format {
            ImageFormat::Png | ImageFormat::Jpeg => {}
            other => {
                return Err(Error::Unsupported(format!("{other:?}")));
            }
        }
        let dynamic =
            image::load_from_memory_with_format(bytes, format).map_err(map_image_error)?;
        Ok(Self {
            image: dynamic.to_rgba8(),
        })
    }

    /// The decoded width in source pixels.
    pub fn width(&self) -> u32 {
        self.image.width()
    }

    /// The decoded height in source pixels.
    pub fn height(&self) -> u32 {
        self.image.height()
    }

    /// The color of the source pixel at `(x, y)`, or `None` if out of bounds.
    pub fn color_at(&self, x: u32, y: u32) -> Option<Color> {
        if x >= self.image.width() || y >= self.image.height() {
            return None;
        }
        let [r, g, b, a] = self.image.get_pixel(x, y).0;
        Some(Color::from_u8(r, g, b, a))
    }

    /// The raw 8-bit RGBA channels of the source pixel at `(x, y)`.
    pub fn rgba_at(&self, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        if x >= self.image.width() || y >= self.image.height() {
            return None;
        }
        let [r, g, b, a] = self.image.get_pixel(x, y).0;
        Some((r, g, b, a))
    }

    /// Converts the image into a freshly allocated [`Framebuffer`] sized to
    /// `viewport`'s render resolution, aspect-corrected and letterboxed per
    /// `opts`.
    ///
    /// To avoid per-frame allocation, prefer [`DecodedImage::render_into`] with
    /// a reused framebuffer (this is what [`crate::ImageSource`] does).
    pub fn to_framebuffer(&self, viewport: Viewport, opts: &RenderOptions) -> Framebuffer {
        let mut fb = Framebuffer::new(0, 0);
        self.render_into(&mut fb, viewport, opts);
        fb
    }

    /// Renders the image into an existing `target`, resizing and clearing it.
    ///
    /// The framebuffer is resized to `viewport`'s render size for `opts`'
    /// subpixel factors, cleared to `opts.background`, and the aspect-corrected
    /// image is centred within it.
    pub fn render_into(&self, target: &mut Framebuffer, viewport: Viewport, opts: &RenderOptions) {
        render_into(&self.image, target, viewport, opts);
    }
}

/// Maps an [`ImageError`] into the shared rgfx [`Error`].
fn map_image_error(err: ImageError) -> Error {
    let msg = err.to_string();
    match err {
        ImageError::Unsupported(_) => Error::Unsupported(msg),
        _ => Error::Decode(msg),
    }
}

/// A tiny 1×1 opaque-red PNG used by the crate doctest and tests.
pub(crate) const TEST_RED_1X1_PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 248, 207, 192, 240, 31, 0,
    5, 0, 1, 255, 86, 199, 47, 13, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4×4 PNG whose pixel `(x, y)` is `(min(x*80,255), min(y*80,255), 40, 255)`.
    const TEST_4X4_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 4, 0, 0, 0, 4, 8, 6,
        0, 0, 0, 169, 241, 158, 126, 0, 0, 0, 44, 73, 68, 65, 84, 120, 218, 21, 200, 49, 1, 0, 0,
        8, 2, 65, 226, 24, 135, 136, 68, 164, 129, 190, 195, 45, 39, 105, 214, 8, 10, 201, 4, 130,
        250, 35, 4, 130, 230, 163, 4, 130, 118, 246, 0, 227, 233, 33, 113, 203, 159, 183, 128, 0,
        0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    /// A 2×2 solid opaque-blue PNG.
    const TEST_BLUE_2X2_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6,
        0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 16, 73, 68, 65, 84, 120, 218, 99, 96, 96, 248, 255, 31,
        130, 161, 12, 0, 63, 210, 7, 249, 92, 19, 224, 66, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
        130,
    ];

    #[test]
    fn decodes_1x1_red_png() {
        let img = DecodedImage::from_bytes(TEST_RED_1X1_PNG).unwrap();
        assert_eq!((img.width(), img.height()), (1, 1));
        assert_eq!(img.rgba_at(0, 0), Some((255, 0, 0, 255)));
    }

    #[test]
    fn decodes_4x4_png_to_known_pixels() {
        let img = DecodedImage::from_bytes(TEST_4X4_PNG).unwrap();
        assert_eq!((img.width(), img.height()), (4, 4));
        for y in 0..4u32 {
            for x in 0..4u32 {
                let want = (
                    (x * 80).min(255) as u8,
                    (y * 80).min(255) as u8,
                    40u8,
                    255u8,
                );
                assert_eq!(img.rgba_at(x, y), Some(want), "pixel ({x},{y})");
            }
        }
        assert_eq!(img.color_at(4, 0), None);
    }

    #[test]
    fn solid_image_survives_resize_into_framebuffer() {
        let img = DecodedImage::from_bytes(TEST_BLUE_2X2_PNG).unwrap();
        // Square viewport with square (braille) pixels => a 2x2 square fills it fully.
        let opts = RenderOptions::braille();
        let fb = img.to_framebuffer(Viewport::new(4, 2), &opts);
        // render size = (4*2, 2*4) = (8, 8)
        assert_eq!((fb.width(), fb.height()), (8, 8));
        // A solid blue image resized stays solid blue everywhere.
        let center = fb.get(4, 4);
        assert!(
            center.b > 0.9 && center.r < 0.1 && center.g < 0.1,
            "{center:?}"
        );
        assert!(center.a > 0.9);
    }

    #[test]
    fn empty_bytes_error_not_panic() {
        // Empty input is an unrecognizable format: must be an Err, never a panic.
        assert!(DecodedImage::from_bytes(&[]).is_err());
    }

    #[test]
    fn corrupt_png_errors() {
        // Valid PNG signature, garbage body.
        let mut bytes = TEST_RED_1X1_PNG[..16].to_vec();
        bytes.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02, 0x03]);
        assert!(DecodedImage::from_bytes(&bytes).is_err());
    }

    #[test]
    fn unsupported_format_is_rejected() {
        // A GIF header ("GIF89a") is a recognized-but-unsupported format here.
        let gif = b"GIF89a\x01\x00\x01\x00\x00\x00\x00;";
        assert!(matches!(
            DecodedImage::from_bytes(gif),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn missing_file_is_io_error() {
        let err = DecodedImage::load("/definitely/not/a/real/file.png").unwrap_err();
        assert!(matches!(err, Error::Io(_)));
    }
}
