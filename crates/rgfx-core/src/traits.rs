//! The three composition-seam traits that let rgfx crates interoperate without knowing about
//! each other's internals.

use crate::{Camera, Framebuffer, Result, Scene, TerminalFrame, Viewport};
use std::time::Duration;

/// Anything that yields frames into a framebuffer: a still image (one frame), an animated GIF or
/// video (many frames), or a 3D scene rendered on demand.
///
/// The caller owns and reuses the target [`Framebuffer`], sizing it to the desired viewport
/// before each call — implementations render into it rather than allocating their own.
pub trait FrameSource {
    /// Renders the next frame into `target`.
    ///
    /// Returns `Ok(true)` if a frame was produced, or `Ok(false)` when the source is exhausted
    /// (for example, the end of a non-looping video).
    fn next_frame(&mut self, target: &mut Framebuffer) -> Result<bool>;

    /// The delay to wait before showing the next frame, for animated sources.
    ///
    /// Returns `None` for static sources or when timing is driven externally.
    fn frame_delay(&self) -> Option<Duration> {
        None
    }
}

/// Turns a [`Framebuffer`] into a grid of terminal cells. Implemented by the Braille, ASCII, and
/// block encoders in `rgfx-terminal`.
///
/// The `viewport` describes the terminal cell grid to target; the encoder expects `frame` to be
/// sized to the matching pixel resolution for its subpixel factor (see [`Viewport::render_size`]).
pub trait TerminalEncoder {
    /// Encodes `frame` into a [`TerminalFrame`] filling the given `viewport`.
    fn encode(&self, frame: &Framebuffer, viewport: Viewport) -> TerminalFrame;
}

/// Rasterizes a 3D [`Scene`] as seen through a [`Camera`] into a framebuffer. Implemented by the
/// software rasterizer in `rgfx-3d`.
pub trait SceneRenderer {
    /// Renders `scene` from `camera`'s viewpoint into `target`.
    ///
    /// The renderer is responsible for clearing/using `target`'s depth buffer as needed.
    fn render(&mut self, scene: &Scene, camera: &Camera, target: &mut Framebuffer) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    /// A trivial source that fills the framebuffer white for a fixed number of frames — proves
    /// the trait is object-safe and usable via reused framebuffers.
    struct SolidSource {
        remaining: usize,
    }

    impl FrameSource for SolidSource {
        fn next_frame(&mut self, target: &mut Framebuffer) -> Result<bool> {
            if self.remaining == 0 {
                return Ok(false);
            }
            self.remaining -= 1;
            target.clear(Color::WHITE);
            Ok(true)
        }

        fn frame_delay(&self) -> Option<Duration> {
            Some(Duration::from_millis(10))
        }
    }

    #[test]
    fn frame_source_drives_a_reused_framebuffer() {
        let mut fb = Framebuffer::new(2, 2);
        let mut src: Box<dyn FrameSource> = Box::new(SolidSource { remaining: 2 });
        assert!(src.next_frame(&mut fb).unwrap());
        assert_eq!(fb.get(0, 0), Color::WHITE);
        assert!(src.next_frame(&mut fb).unwrap());
        assert!(!src.next_frame(&mut fb).unwrap());
        assert_eq!(src.frame_delay(), Some(Duration::from_millis(10)));
    }
}
