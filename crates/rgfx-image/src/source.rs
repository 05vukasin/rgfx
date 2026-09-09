//! A [`FrameSource`] adapter for a single still image.

use crate::{DecodedImage, RenderOptions};
use rgfx_core::{FrameSource, Framebuffer, Result, Viewport};

/// Presents a single [`DecodedImage`] as a one-frame [`FrameSource`].
///
/// The first call to [`FrameSource::next_frame`] renders the image into the
/// target (resizing it to the configured viewport) and returns `Ok(true)`;
/// every subsequent call returns `Ok(false)` because a still image is exhausted
/// after one frame.
#[derive(Clone, Debug)]
pub struct ImageSource {
    image: DecodedImage,
    viewport: Viewport,
    opts: RenderOptions,
    emitted: bool,
}

impl ImageSource {
    /// Creates a still-image source rendered at `viewport` with `opts`.
    pub fn new(image: DecodedImage, viewport: Viewport, opts: RenderOptions) -> Self {
        Self {
            image,
            viewport,
            opts,
            emitted: false,
        }
    }

    /// The viewport this source renders into.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Rewinds the source so the frame can be produced again.
    pub fn reset(&mut self) {
        self.emitted = false;
    }
}

impl FrameSource for ImageSource {
    fn next_frame(&mut self, target: &mut Framebuffer) -> Result<bool> {
        if self.emitted {
            return Ok(false);
        }
        self.image.render_into(target, self.viewport, &self.opts);
        self.emitted = true;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::TEST_RED_1X1_PNG;

    #[test]
    fn yields_exactly_one_frame() {
        let img = DecodedImage::from_bytes(TEST_RED_1X1_PNG).unwrap();
        let mut src = ImageSource::new(img, Viewport::new(10, 5), RenderOptions::braille());
        let mut fb = Framebuffer::new(0, 0);

        assert!(src.next_frame(&mut fb).unwrap());
        // render size = (10*2, 5*4) = (20, 20)
        assert_eq!((fb.width(), fb.height()), (20, 20));
        assert!(!src.next_frame(&mut fb).unwrap());
        assert!(!src.next_frame(&mut fb).unwrap());

        src.reset();
        assert!(src.next_frame(&mut fb).unwrap());
    }

    #[test]
    fn frame_delay_is_none_for_still() {
        let img = DecodedImage::from_bytes(TEST_RED_1X1_PNG).unwrap();
        let src = ImageSource::new(img, Viewport::new(4, 4), RenderOptions::default());
        assert!(src.frame_delay().is_none());
    }
}
