//! `rgfx-image`: still-image → [`rgfx_core::Framebuffer`] pipeline for PNG and JPEG.
//!
//! This crate decodes a PNG or JPEG file into an in-memory RGBA image
//! ([`DecodedImage`]), corrects its aspect ratio for a terminal's non-square
//! pixels, resizes it with a high-quality filter, and writes it into an
//! [`rgfx_core::Framebuffer`]. A single decoded image can also be exposed as a
//! one-frame [`rgfx_core::FrameSource`] via [`ImageSource`].
//!
//! It never touches the terminal or any encoder: everything converges on the
//! framebuffer, per the rgfx architectural law. The image-quality stage —
//! dithering ([`Dither`]) and tone controls ([`Tone`]), configured via
//! [`Preprocess`] — runs here as an in-place framebuffer transform. Additional
//! formats (WebP/BMP/GIF) live in sibling crates.
//!
//! # Aspect-ratio correction
//!
//! Terminal cells are not square (typically about 1:2 width:height), and an
//! encoder packs several framebuffer pixels into each cell (2×4 for Braille,
//! 1×2 for half-blocks). A single framebuffer pixel therefore has a physical
//! aspect ratio that is neither 1:1 nor the cell's. [`pixel_aspect`] exposes
//! that correction factor so the CLI can pass renderer-specific ratios, and
//! [`fit_dimensions`] uses it to letterbox the image without distortion.
//!
//! ```
//! use rgfx_core::Viewport;
//! use rgfx_image::{DecodedImage, RenderOptions};
//!
//! // A 1×1 red PNG, decoded from embedded bytes.
//! # let png: &[u8] = rgfx_image::doctest_png();
//! let img = DecodedImage::from_bytes(png).unwrap();
//! let fb = img.to_framebuffer(Viewport::new(40, 20), &RenderOptions::braille());
//! assert_eq!(fb.width(), 80); // 40 cols * 2 subpixels
//! assert_eq!(fb.height(), 80); // 20 rows * 4 subpixels
//! ```
#![warn(missing_docs)]
#![forbid(unsafe_code)]

mod aspect;
mod decode;
mod preprocess;
mod render;
mod source;

pub use aspect::{fit_dimensions, pixel_aspect};
pub use decode::DecodedImage;
pub use preprocess::{BayerSize, Dither, Preprocess, Tone};
pub use render::{RenderOptions, ResizeFilter};
pub use source::ImageSource;

// Re-export the shared error vocabulary so downstream users can name the exact
// types this crate returns without a separate `rgfx_core` dependency.
pub use rgfx_core::{Error, Result};

/// A tiny 1×1 opaque-red PNG, used by the crate-level doctest. Not part of the
/// stable API.
#[doc(hidden)]
pub fn doctest_png() -> &'static [u8] {
    decode::TEST_RED_1X1_PNG
}
