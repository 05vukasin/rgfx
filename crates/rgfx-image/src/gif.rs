//! Animated-GIF decoding exposed as a multi-frame [`FrameSource`].
//!
//! [`GifSource`] decodes GIF frames lazily (one per [`FrameSource::next_frame`]
//! call), reusing the still-image [`crate::render`] pipeline to blit each
//! already-composited frame into the caller's reused [`Framebuffer`]. Frame
//! timing is reported through [`FrameSource::frame_delay`], and disposal
//! (including restore-to-background) is composited by the underlying decoder so
//! every frame this source produces is a full canvas.

use crate::decode::map_image_error;
use crate::render::{RenderOptions, render_into};
use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, Frames};
use rgfx_core::{FrameSource, Framebuffer, Result, Viewport};
use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

/// What an animated source does when it reaches its last frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GifLoop {
    /// Play the frames once, then report exhaustion (`Ok(false)`).
    #[default]
    Once,
    /// Restart from the first frame indefinitely; never reports exhaustion for a
    /// GIF that has at least one frame.
    Infinite,
}

/// Presents an animated GIF as a multi-frame [`FrameSource`].
///
/// Each [`FrameSource::next_frame`] call decodes and composites the next GIF
/// frame (honouring disposal) and blits it into the caller-provided
/// [`Framebuffer`], resized to the configured [`Viewport`]. When the frames run
/// out the source either stops or loops per its [`GifLoop`] configuration.
///
/// The raw GIF bytes are retained so the source can be rewound ([`GifSource::reset`])
/// or looped without the caller re-supplying them.
pub struct GifSource {
    bytes: Vec<u8>,
    viewport: Viewport,
    opts: RenderOptions,
    loop_mode: GifLoop,
    frames: Frames<'static>,
    current_delay: Option<Duration>,
    exhausted: bool,
    /// Guards against spinning on a GIF that decodes to zero frames: set when the
    /// frame iterator is (re)created and cleared whenever a frame is produced.
    reset_since_frame: bool,
}

impl GifSource {
    /// Loads an animated GIF from `path`, rendered at `viewport` with `opts`.
    ///
    /// # Errors
    ///
    /// Returns [`rgfx_core::Error::Io`] if the file cannot be read,
    /// [`rgfx_core::Error::Decode`] if the bytes are not a decodable GIF, and
    /// [`rgfx_core::Error::Unsupported`] for a recognised-but-unsupported format.
    /// Never panics.
    pub fn load(
        path: impl AsRef<Path>,
        viewport: Viewport,
        opts: RenderOptions,
        loop_mode: GifLoop,
    ) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(bytes, viewport, opts, loop_mode)
    }

    /// Creates an animated-GIF source from in-memory `bytes`.
    ///
    /// The bytes are validated as a decodable GIF up front (the header is
    /// parsed) but individual frames are decoded lazily on demand.
    ///
    /// # Errors
    ///
    /// Returns [`rgfx_core::Error::Decode`] if the bytes are not a decodable
    /// GIF. Malformed input errors rather than panicking.
    pub fn from_bytes(
        bytes: impl Into<Vec<u8>>,
        viewport: Viewport,
        opts: RenderOptions,
        loop_mode: GifLoop,
    ) -> Result<Self> {
        let bytes = bytes.into();
        let frames = decode_frames(&bytes)?;
        Ok(Self {
            bytes,
            viewport,
            opts,
            loop_mode,
            frames,
            current_delay: None,
            exhausted: false,
            reset_since_frame: true,
        })
    }

    /// The viewport this source renders into.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// The configured loop behaviour.
    pub fn loop_mode(&self) -> GifLoop {
        self.loop_mode
    }

    /// Rewinds the source to the first frame.
    ///
    /// # Errors
    ///
    /// Returns [`rgfx_core::Error::Decode`] if the retained bytes fail to
    /// re-decode (they were validated at construction, so this is unexpected).
    pub fn reset(&mut self) -> Result<()> {
        self.frames = decode_frames(&self.bytes)?;
        self.current_delay = None;
        self.exhausted = false;
        self.reset_since_frame = true;
        Ok(())
    }
}

impl FrameSource for GifSource {
    fn next_frame(&mut self, target: &mut Framebuffer) -> Result<bool> {
        if self.exhausted {
            return Ok(false);
        }
        loop {
            match self.frames.next() {
                Some(Ok(frame)) => {
                    self.current_delay = Some(Duration::from(frame.delay()));
                    let buffer = frame.into_buffer();
                    render_into(&buffer, target, self.viewport, &self.opts);
                    self.reset_since_frame = false;
                    return Ok(true);
                }
                Some(Err(err)) => {
                    self.exhausted = true;
                    self.current_delay = None;
                    return Err(map_image_error(err));
                }
                None => {
                    // End of the frame stream. Loop by re-decoding, unless we just
                    // reset and still got nothing (an empty GIF), which would spin.
                    if self.loop_mode == GifLoop::Infinite && !self.reset_since_frame {
                        self.frames = decode_frames(&self.bytes)?;
                        self.reset_since_frame = true;
                        continue;
                    }
                    self.exhausted = true;
                    self.current_delay = None;
                    return Ok(false);
                }
            }
        }
    }

    fn frame_delay(&self) -> Option<Duration> {
        self.current_delay
    }
}

/// Builds a lazy, owned frame iterator over `bytes`, validating the GIF header.
///
/// The decoder owns a copy of `bytes` (via an in-memory cursor), so the returned
/// iterator is `'static` and can be stored.
fn decode_frames(bytes: &[u8]) -> Result<Frames<'static>> {
    let decoder = GifDecoder::new(Cursor::new(bytes.to_vec())).map_err(map_image_error)?;
    Ok(decoder.into_frames())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rgfx_core::Color;

    /// A 3-frame 2×2 GIF over palette {black, red, green, blue}, generated once:
    /// - frame 0: full 2×2 red, disposal = restore-to-background, delay 100 ms;
    /// - frame 1: 1×1 green at (0,0), disposal = keep, delay 200 ms;
    /// - frame 2: 1×1 blue at (1,1), disposal = keep, delay 300 ms.
    ///
    /// Restore-to-background clears to transparent, so after frame 0 the canvas
    /// is wiped; kept frames then accumulate on top.
    const TEST_GIF_3FRAME: &[u8] = &[
        71, 73, 70, 56, 57, 97, 2, 0, 2, 0, 145, 0, 0, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255,
        33, 255, 11, 78, 69, 84, 83, 67, 65, 80, 69, 50, 46, 48, 3, 1, 0, 0, 0, 33, 249, 4, 8, 10,
        0, 0, 0, 44, 0, 0, 0, 0, 2, 0, 2, 0, 0, 2, 2, 140, 83, 0, 33, 249, 4, 4, 20, 0, 0, 0, 44,
        0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 84, 1, 0, 33, 249, 4, 4, 30, 0, 0, 0, 44, 1, 0, 1, 0, 1,
        0, 1, 0, 0, 2, 2, 92, 1, 0, 59,
    ];

    /// Render options that map one source pixel to one framebuffer pixel with no
    /// resampling, so composited frame pixels can be asserted exactly.
    fn exact_opts() -> RenderOptions {
        RenderOptions {
            subpixel_x: 1,
            subpixel_y: 1,
            cell_aspect: 1.0,
            filter: crate::ResizeFilter::Nearest,
            background: Color::TRANSPARENT,
            preprocess: crate::Preprocess::IDENTITY,
        }
    }

    fn approx(c: Color, r: f32, g: f32, b: f32, a: f32) -> bool {
        (c.r - r).abs() < 1e-3
            && (c.g - g).abs() < 1e-3
            && (c.b - b).abs() < 1e-3
            && (c.a - a).abs() < 1e-3
    }

    #[test]
    fn three_frames_in_order_with_delays() {
        let mut src = GifSource::from_bytes(
            TEST_GIF_3FRAME,
            Viewport::new(2, 2),
            exact_opts(),
            GifLoop::Once,
        )
        .unwrap();
        let mut fb = Framebuffer::new(0, 0);

        assert!(src.next_frame(&mut fb).unwrap());
        assert_eq!(src.frame_delay(), Some(Duration::from_millis(100)));

        assert!(src.next_frame(&mut fb).unwrap());
        assert_eq!(src.frame_delay(), Some(Duration::from_millis(200)));

        assert!(src.next_frame(&mut fb).unwrap());
        assert_eq!(src.frame_delay(), Some(Duration::from_millis(300)));

        // Exactly three frames, then exhausted (Once).
        assert!(!src.next_frame(&mut fb).unwrap());
        assert!(!src.next_frame(&mut fb).unwrap());
    }

    #[test]
    fn disposal_composites_across_frames() {
        let mut src = GifSource::from_bytes(
            TEST_GIF_3FRAME,
            Viewport::new(2, 2),
            exact_opts(),
            GifLoop::Once,
        )
        .unwrap();
        let mut fb = Framebuffer::new(0, 0);

        // Frame 0: full red.
        src.next_frame(&mut fb).unwrap();
        assert_eq!((fb.width(), fb.height()), (2, 2));
        for y in 0..2 {
            for x in 0..2 {
                assert!(approx(fb.get(x, y), 1.0, 0.0, 0.0, 1.0), "f0 ({x},{y})");
            }
        }

        // Frame 1: background restored (transparent) then green at (0,0).
        src.next_frame(&mut fb).unwrap();
        assert!(approx(fb.get(0, 0), 0.0, 1.0, 0.0, 1.0), "f1 (0,0) green");
        assert!(approx(fb.get(1, 0), 0.0, 0.0, 0.0, 0.0), "f1 (1,0) clear");
        assert!(approx(fb.get(0, 1), 0.0, 0.0, 0.0, 0.0), "f1 (0,1) clear");
        assert!(approx(fb.get(1, 1), 0.0, 0.0, 0.0, 0.0), "f1 (1,1) clear");

        // Frame 2: green kept at (0,0), blue added at (1,1).
        src.next_frame(&mut fb).unwrap();
        assert!(
            approx(fb.get(0, 0), 0.0, 1.0, 0.0, 1.0),
            "f2 (0,0) green kept"
        );
        assert!(approx(fb.get(1, 1), 0.0, 0.0, 1.0, 1.0), "f2 (1,1) blue");
        assert!(approx(fb.get(1, 0), 0.0, 0.0, 0.0, 0.0), "f2 (1,0) clear");
        assert!(approx(fb.get(0, 1), 0.0, 0.0, 0.0, 0.0), "f2 (0,1) clear");
    }

    #[test]
    fn infinite_loop_restarts_after_last_frame() {
        let mut src = GifSource::from_bytes(
            TEST_GIF_3FRAME,
            Viewport::new(2, 2),
            exact_opts(),
            GifLoop::Infinite,
        )
        .unwrap();
        let mut fb = Framebuffer::new(0, 0);

        // Three frames, then it wraps to frame 0 (red) instead of exhausting.
        for _ in 0..3 {
            assert!(src.next_frame(&mut fb).unwrap());
        }
        assert!(src.next_frame(&mut fb).unwrap());
        assert_eq!(src.frame_delay(), Some(Duration::from_millis(100)));
        for y in 0..2 {
            for x in 0..2 {
                assert!(
                    approx(fb.get(x, y), 1.0, 0.0, 0.0, 1.0),
                    "wrapped f0 ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn reset_replays_from_first_frame() {
        let mut src = GifSource::from_bytes(
            TEST_GIF_3FRAME,
            Viewport::new(2, 2),
            exact_opts(),
            GifLoop::Once,
        )
        .unwrap();
        let mut fb = Framebuffer::new(0, 0);
        while src.next_frame(&mut fb).unwrap() {}
        src.reset().unwrap();
        assert!(src.next_frame(&mut fb).unwrap());
        assert_eq!(src.frame_delay(), Some(Duration::from_millis(100)));
    }

    #[test]
    fn frame_delay_is_none_before_first_frame() {
        let src = GifSource::from_bytes(
            TEST_GIF_3FRAME,
            Viewport::new(2, 2),
            exact_opts(),
            GifLoop::Once,
        )
        .unwrap();
        assert!(src.frame_delay().is_none());
    }

    #[test]
    fn malformed_gif_errors_not_panics() {
        // Valid GIF89a signature, truncated garbage body.
        let bytes = b"GIF89a\x02\x00\x02\x00\x00\xDE\xAD";
        assert!(
            GifSource::from_bytes(&bytes[..], Viewport::new(2, 2), exact_opts(), GifLoop::Once)
                .is_err()
        );
    }

    #[test]
    fn non_gif_bytes_error() {
        assert!(
            GifSource::from_bytes(&[][..], Viewport::new(2, 2), exact_opts(), GifLoop::Once)
                .is_err()
        );
    }
}
