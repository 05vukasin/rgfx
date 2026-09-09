//! `rgfx-core`: the shared vocabulary every other rgfx crate builds against.
//!
//! The one architectural law of rgfx is that **nothing renders directly to the terminal**.
//! Every media source — an image, a decoded video frame, a rasterized 3D scene, a procedural
//! animation — converges onto a [`Framebuffer`]. Only the terminal layer turns a framebuffer
//! into text, via a [`TerminalEncoder`]. This crate defines that framebuffer, the color and
//! geometry primitives, the camera, the error type, and the three composition-seam traits
//! ([`FrameSource`], [`TerminalEncoder`], [`SceneRenderer`]) so that the loader, renderer, and
//! encoder crates can be developed independently and still compose.
//!
//! These types are a frozen contract: downstream crates assume the exact signatures here.
#![warn(missing_docs)]
#![forbid(unsafe_code)]

mod animation;
mod camera;
mod color;
mod error;
mod framebuffer;
mod geometry;
mod terminal_frame;
mod traits;

pub use animation::{Animation, AnimationPlayer, Frame, LoopPolicy, Sprite};
pub use camera::{Camera, Projection};
pub use color::Color;
pub use error::{Error, Result};
pub use framebuffer::Framebuffer;
pub use geometry::{BoundingBox, BoundingSphere, Mesh, Scene, Vertex};
pub use terminal_frame::{Cell, TerminalFrame};
pub use traits::{FrameSource, SceneRenderer, TerminalEncoder};

/// A terminal viewport measured in character cells.
///
/// Cell dimensions are deliberately kept separate from framebuffer pixel dimensions: an encoder
/// maps some number of framebuffer pixels onto each cell (2×4 for Braille, 1×2 for half-blocks,
/// 1×1 for ASCII). Use [`Viewport::render_size`] to compute the framebuffer size an encoder
/// needs for a given subpixel factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    /// Width in terminal columns.
    pub cols: u16,
    /// Height in terminal rows.
    pub rows: u16,
}

impl Viewport {
    /// Creates a viewport of `cols` × `rows` character cells.
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }

    /// The framebuffer pixel size for this viewport given per-cell subpixel factors.
    ///
    /// For Braille use `(2, 4)`, for half-blocks `(1, 2)`, for ASCII `(1, 1)`.
    pub fn render_size(self, subpixel_x: u16, subpixel_y: u16) -> (usize, usize) {
        (
            self.cols as usize * subpixel_x as usize,
            self.rows as usize * subpixel_y as usize,
        )
    }

    /// The aspect ratio (width / height) of the viewport in pixels, given subpixel factors.
    ///
    /// Returns `1.0` for a degenerate zero-height viewport rather than infinity/NaN.
    pub fn aspect(self, subpixel_x: u16, subpixel_y: u16) -> f32 {
        let (w, h) = self.render_size(subpixel_x, subpixel_y);
        if h == 0 { 1.0 } else { w as f32 / h as f32 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_render_size_matches_subpixels() {
        let vp = Viewport::new(100, 40);
        assert_eq!(vp.render_size(2, 4), (200, 160));
        assert_eq!(vp.render_size(1, 1), (100, 40));
    }

    #[test]
    fn viewport_aspect_is_safe_for_zero_height() {
        assert_eq!(Viewport::new(80, 0).aspect(2, 4), 1.0);
        assert!((Viewport::new(200, 100).aspect(1, 1) - 2.0).abs() < f32::EPSILON);
    }
}
