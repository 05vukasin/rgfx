//! Stdin / pipe streaming (task 025).
//!
//! Two entry points, both driven from [`crate::app`]:
//!
//! - [`run_stdin`] backs `rgfx -`: it reads the *whole* of standard input into memory, sniffs the
//!   format from its magic bytes (reusing [`crate::media`]), and — for a still raster image —
//!   decodes it from those bytes and hands it to the still-image viewer. Buffering is required
//!   because stdin is not seekable and the image decoder needs the complete byte stream.
//! - [`run_stream`] backs `--stream`: it reads a minimal, documented framed protocol from stdin
//!   and renders each frame continuously via the diffing [`FrameEngine`], exposed to the rest of
//!   the pipeline as a [`FrameSource`] ([`StdinFrameSource`]).
//!
//! ## `--stream` wire protocol
//!
//! The protocol is deliberately tiny so it is trivial to produce from a shell or a script:
//!
//! ```text
//! <WIDTH>x<HEIGHT>\n         ascii header line, e.g. "320x240\n"
//! <raw RGB24 frame>          WIDTH*HEIGHT*3 bytes, 8-bit R,G,B, row-major, top-to-bottom
//! <raw RGB24 frame>          … one frame directly after another, no delimiter …
//! ```
//!
//! Playback ends cleanly at end-of-input on a frame boundary. A frame that is truncated by a
//! producer closing the pipe mid-frame ends playback cleanly too (a warning is logged); it never
//! hangs or panics. Frames are resampled (nearest-neighbour) to fit the terminal.
//!
//! Everything here honours the one architectural law: this module only *produces* frames into a
//! [`Framebuffer`]; turning that framebuffer into terminal cells is the encoder's job.

use std::io::{self, BufRead, BufReader, IsTerminal, Read};
use std::time::Duration;

use anyhow::{Context, bail};
use rgfx_core::{Color, FrameSource, Framebuffer, TerminalEncoder, Viewport};
use rgfx_image::DecodedImage;
use rgfx_terminal::{
    AsciiEncoder, BlockEncoder, BrailleEncoder, FrameEngine, SUBPIXEL_X, SUBPIXEL_Y,
    detect_color_mode,
};

use crate::cli::Renderer;
use crate::config::Settings;
use crate::media::{self, MediaKind};
use crate::terminal::Session;

/// Bytes per pixel in the raw RGB24 `--stream` payload.
const BYTES_PER_PIXEL: usize = 3;
/// Per-cell subpixel factors for the half-block encoder (1 wide × 2 tall).
const BLOCK_SUBPIXEL_X: u16 = 1;
const BLOCK_SUBPIXEL_Y: u16 = 2;
/// Per-cell subpixel factors for the ASCII encoder (one pixel per cell).
const ASCII_SUBPIXEL_X: u16 = 1;
const ASCII_SUBPIXEL_Y: u16 = 1;

/// Reads all of standard input, sniffs the format, and routes a still image to the viewer.
///
/// Errors cleanly (never panics) when stdin is an interactive TTY (nothing was piped), when it is
/// empty, or when the bytes are not a supported still image.
pub(crate) fn run_stdin(settings: &Settings) -> anyhow::Result<()> {
    let bytes = read_all_stdin()?;
    match media::detect_bytes(&bytes, None) {
        MediaKind::Image => {
            let image =
                DecodedImage::from_bytes(&bytes).context("decoding image read from stdin")?;
            crate::image_viewer::view_decoded(&image, settings)
        }
        MediaKind::Gif => {
            bail!("animated GIF on stdin is not supported yet; pipe a still image or use --stream")
        }
        MediaKind::Video => {
            bail!("video on stdin is not supported yet; pipe a still image or use --stream")
        }
        MediaKind::Mesh(_) => {
            bail!("3D meshes cannot be read from stdin; pass a file path instead")
        }
        MediaKind::Unknown => bail!(
            "could not detect a supported image format on stdin (use --stream for raw RGB frames)"
        ),
    }
}

/// Buffers the entire standard-input stream, rejecting a TTY or empty input up front.
fn read_all_stdin() -> anyhow::Result<Vec<u8>> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        bail!("no input on stdin: pipe data in, e.g. `cat image.png | rgfx -`");
    }
    let mut buf = Vec::new();
    stdin
        .lock()
        .read_to_end(&mut buf)
        .context("reading stdin")?;
    if buf.is_empty() {
        bail!("stdin was empty: nothing to render");
    }
    Ok(buf)
}

/// Reads the framed `--stream` protocol from stdin and renders it until end-of-input.
///
/// See the [module documentation](self) for the wire format. Errors cleanly when stdin is a TTY
/// or the header is malformed; a truncated final frame ends playback without error.
pub(crate) fn run_stream(settings: &Settings) -> anyhow::Result<()> {
    if io::stdin().is_terminal() {
        bail!("--stream expects frame data piped on stdin; a TTY has nothing to read");
    }
    let source = StdinFrameSource::new(io::stdin().lock(), settings.fps)
        .context("initializing --stream input")?;
    present_stream(source, settings)
}

/// The concrete encoder chosen for a stream, together with its subpixel factors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamEncoder {
    Braille,
    Ascii,
    Blocks,
}

impl StreamEncoder {
    /// Resolves the CLI [`Renderer`] into a concrete encoder, mirroring the still-image viewer:
    /// half-blocks when colour is requested, otherwise the higher-resolution Braille encoder.
    fn resolve(renderer: Renderer, color: bool) -> Self {
        match renderer {
            Renderer::Braille => Self::Braille,
            Renderer::Ascii => Self::Ascii,
            Renderer::Blocks => Self::Blocks,
            Renderer::Auto => {
                if color {
                    Self::Blocks
                } else {
                    Self::Braille
                }
            }
        }
    }

    /// The `(subpixel_x, subpixel_y)` factors used to size the framebuffer for this encoder.
    fn subpixels(self) -> (u16, u16) {
        match self {
            Self::Braille => (SUBPIXEL_X, SUBPIXEL_Y),
            Self::Ascii => (ASCII_SUBPIXEL_X, ASCII_SUBPIXEL_Y),
            Self::Blocks => (BLOCK_SUBPIXEL_X, BLOCK_SUBPIXEL_Y),
        }
    }

    /// A boxed encoder instance. Encoders are stateless and cheap to construct.
    fn encoder(self) -> Box<dyn TerminalEncoder> {
        match self {
            Self::Braille => Box::new(BrailleEncoder::new()),
            Self::Ascii => Box::new(AsciiEncoder::new()),
            Self::Blocks => Box::new(BlockEncoder::new()),
        }
    }
}

/// Drives a [`StdinFrameSource`] through an interactive [`Session`], presenting each frame with a
/// reused [`FrameEngine`] and framebuffer (no per-frame allocation). The session restores the
/// terminal on every exit path.
fn present_stream<R: Read>(
    mut source: StdinFrameSource<R>,
    settings: &Settings,
) -> anyhow::Result<()> {
    let mut session = Session::open()?;
    let mut engine = FrameEngine::new(detect_color_mode());
    let mut fb = Framebuffer::new(0, 0);

    let resolved = StreamEncoder::resolve(settings.renderer, settings.color);
    let (sx, sy) = resolved.subpixels();
    let encoder = resolved.encoder();
    let (src_w, src_h) = source.dimensions();

    loop {
        let bounds = session.viewport()?;
        let viewport = fit_viewport(src_w, src_h, bounds, settings.width, sx, sy);
        let (pw, ph) = viewport.render_size(sx, sy);
        fb.resize(pw, ph);
        fb.clear(Color::BLACK);

        if !source.next_frame(&mut fb)? {
            break;
        }
        let frame = encoder.encode(&fb, viewport);
        session.render_frame(&mut engine, &frame)?;
        if let Some(delay) = source.frame_delay() {
            std::thread::sleep(delay);
        }
    }
    Ok(())
}

/// Chooses the render viewport for a source frame, preserving its aspect ratio within the
/// terminal bounds (or a `--width` override).
fn fit_viewport(
    src_w: u32,
    src_h: u32,
    bounds: Viewport,
    width_override: Option<u32>,
    sx: u16,
    sy: u16,
) -> Viewport {
    let max_cols = width_override
        .map(|w| w.clamp(1, u16::MAX as u32) as u16)
        .unwrap_or(bounds.cols)
        .max(1);
    let max_rows = bounds.rows.max(1);

    let budget_w = max_cols as f32 * sx as f32;
    let budget_h = max_rows as f32 * sy as f32;
    let sw = src_w.max(1) as f32;
    let sh = src_h.max(1) as f32;

    let scale = (budget_w / sw).min(budget_h / sh);
    let target_w = (sw * scale).round().max(1.0);
    let target_h = (sh * scale).round().max(1.0);

    let cols = ((target_w / sx as f32).round() as u16).clamp(1, max_cols);
    let rows = ((target_h / sy as f32).round() as u16).clamp(1, max_rows);
    Viewport::new(cols, rows)
}

/// The outcome of attempting to read one raw frame from the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameStatus {
    /// A full frame was read.
    Complete,
    /// Clean end-of-input on a frame boundary (zero bytes available).
    Eof,
    /// The stream closed part-way through a frame; carries the number of bytes read.
    Truncated(usize),
}

/// Reads exactly one `frame_bytes`-sized frame into `buf`, distinguishing EOF from truncation.
fn read_frame<R: Read>(
    reader: &mut R,
    frame_bytes: usize,
    buf: &mut Vec<u8>,
) -> io::Result<FrameStatus> {
    buf.resize(frame_bytes, 0);
    let mut filled = 0;
    while filled < frame_bytes {
        let n = reader.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    if filled == 0 {
        Ok(FrameStatus::Eof)
    } else if filled == frame_bytes {
        Ok(FrameStatus::Complete)
    } else {
        Ok(FrameStatus::Truncated(filled))
    }
}

/// A [`FrameSource`] that decodes the `--stream` wire protocol from any byte reader.
///
/// Construction reads and validates the `WxH` header; each [`FrameSource::next_frame`] reads one
/// raw RGB24 frame and resamples it into the caller-sized target framebuffer.
pub(crate) struct StdinFrameSource<R: Read> {
    reader: BufReader<R>,
    width: u32,
    height: u32,
    frame_bytes: usize,
    scratch: Vec<u8>,
    fps: u32,
    frames_read: u64,
}

impl<R: Read> StdinFrameSource<R> {
    /// Wraps `reader`, parsing the leading `WxH` header line. `fps` (0 = unpaced) drives
    /// [`FrameSource::frame_delay`].
    fn new(reader: R, fps: u32) -> anyhow::Result<Self> {
        let mut reader = BufReader::new(reader);
        let (width, height) = parse_header(&mut reader)?;
        let frame_bytes = width as usize * height as usize * BYTES_PER_PIXEL;
        Ok(Self {
            reader,
            width,
            height,
            frame_bytes,
            scratch: Vec::with_capacity(frame_bytes),
            fps,
            frames_read: 0,
        })
    }

    /// The declared frame dimensions `(width, height)` in pixels.
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// How many complete frames have been produced so far.
    #[cfg(test)]
    fn frames_read(&self) -> u64 {
        self.frames_read
    }
}

impl<R: Read> FrameSource for StdinFrameSource<R> {
    fn next_frame(&mut self, target: &mut Framebuffer) -> rgfx_core::Result<bool> {
        match read_frame(&mut self.reader, self.frame_bytes, &mut self.scratch)? {
            FrameStatus::Complete => {
                blit_rgb24(&self.scratch, self.width, self.height, target);
                self.frames_read += 1;
                Ok(true)
            }
            FrameStatus::Eof => Ok(false),
            FrameStatus::Truncated(n) => {
                tracing::warn!(
                    read = n,
                    expected = self.frame_bytes,
                    "stdin stream ended mid-frame; stopping playback"
                );
                Ok(false)
            }
        }
    }

    fn frame_delay(&self) -> Option<Duration> {
        (self.fps > 0).then(|| Duration::from_secs_f32(1.0 / self.fps as f32))
    }
}

/// Parses the `WIDTHxHEIGHT` header line from the front of the stream.
fn parse_header<R: BufRead>(reader: &mut R) -> anyhow::Result<(u32, u32)> {
    let mut line = String::new();
    if reader
        .read_line(&mut line)
        .context("reading stream header")?
        == 0
    {
        bail!("empty --stream input: expected a `WxH` header line (e.g. `320x240`)");
    }
    let line = line.trim();
    let (w, h) = line
        .split_once(['x', 'X'])
        .with_context(|| format!("malformed --stream header {line:?}: expected `WxH`"))?;
    let width: u32 = w
        .trim()
        .parse()
        .with_context(|| format!("invalid stream width {w:?}"))?;
    let height: u32 = h
        .trim()
        .parse()
        .with_context(|| format!("invalid stream height {h:?}"))?;
    if width == 0 || height == 0 {
        bail!("stream dimensions must be non-zero (got {width}x{height})");
    }
    Ok((width, height))
}

/// Resamples a raw RGB24 frame into `target` with nearest-neighbour sampling.
fn blit_rgb24(src: &[u8], src_w: u32, src_h: u32, target: &mut Framebuffer) {
    let (tw, th) = (target.width(), target.height());
    if tw == 0 || th == 0 || src_w == 0 || src_h == 0 {
        return;
    }
    let (sw, sh) = (src_w as usize, src_h as usize);
    for ty in 0..th {
        let sy = (ty * sh / th).min(sh - 1);
        for tx in 0..tw {
            let sx = (tx * sw / tw).min(sw - 1);
            let idx = (sy * sw + sx) * BYTES_PER_PIXEL;
            let color = match src.get(idx..idx + BYTES_PER_PIXEL) {
                Some([r, g, b]) => Color::from_u8(*r, *g, *b, 255),
                _ => Color::BLACK,
            };
            target.set(tx, ty, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{DitherMode, RenderOpts};
    use crate::config::Config;
    use std::io::Cursor;

    fn settings(opts: RenderOpts) -> Settings {
        Settings::resolve(&Config::default(), &opts)
    }

    /// A framed stream: `WxH` header followed by `frames` frames each filled with `fill`.
    fn stream_bytes(w: u32, h: u32, frames: usize, fill: u8) -> Vec<u8> {
        let mut data = format!("{w}x{h}\n").into_bytes();
        let frame_len = (w * h) as usize * BYTES_PER_PIXEL;
        for _ in 0..frames {
            data.extend(std::iter::repeat_n(fill, frame_len));
        }
        data
    }

    #[test]
    fn stdin_image_is_sniffed_and_rendered() {
        // The `-` path: sniff magic bytes → decode from bytes → encode to text (snapshot).
        let png = rgfx_image::doctest_png();
        assert_eq!(media::detect_bytes(png, None), MediaKind::Image);

        let image = DecodedImage::from_bytes(png).expect("valid embedded PNG");
        let s = settings(RenderOpts {
            renderer: Some(Renderer::Ascii),
            width: Some(4),
            dither: Some(DitherMode::None),
            ..RenderOpts::default()
        });
        // 1x1 red at 4 cols ASCII => 4x2 cells, every cell '-' (luma 0.299 → ramp '-').
        assert_eq!(
            crate::image_viewer::render_to_text(&image, &s),
            "----\n----"
        );
    }

    #[test]
    fn non_image_stdin_bytes_are_classified_not_decoded() {
        // Garbage bytes sniff as Unknown; the decoder rejects them without panicking.
        assert_eq!(
            media::detect_bytes(b"not an image", None),
            MediaKind::Unknown
        );
        assert!(DecodedImage::from_bytes(b"not an image").is_err());
    }

    #[test]
    fn stream_parses_header_and_counts_frames() {
        let data = stream_bytes(2, 2, 3, 0);
        let mut src = StdinFrameSource::new(Cursor::new(data), 30).unwrap();
        assert_eq!(src.dimensions(), (2, 2));

        let mut fb = Framebuffer::new(4, 4);
        let mut count = 0;
        while src.next_frame(&mut fb).unwrap() {
            count += 1;
        }
        assert_eq!(count, 3);
        assert_eq!(src.frames_read(), 3);
        // Exhausted source keeps reporting no more frames.
        assert!(!src.next_frame(&mut fb).unwrap());
    }

    #[test]
    fn read_frame_distinguishes_eof_partial_and_complete() {
        let mut buf = Vec::new();
        let mut empty = Cursor::new(Vec::<u8>::new());
        assert_eq!(
            read_frame(&mut empty, 12, &mut buf).unwrap(),
            FrameStatus::Eof
        );

        let mut partial = Cursor::new(vec![0u8; 5]);
        assert_eq!(
            read_frame(&mut partial, 12, &mut buf).unwrap(),
            FrameStatus::Truncated(5)
        );

        let mut full = Cursor::new(vec![0u8; 12]);
        assert_eq!(
            read_frame(&mut full, 12, &mut buf).unwrap(),
            FrameStatus::Complete
        );
    }

    #[test]
    fn truncated_final_frame_ends_playback_cleanly() {
        let mut data = stream_bytes(2, 2, 1, 0); // one full 12-byte frame
        data.extend(vec![0u8; 5]); // partial frame: producer closed mid-frame
        let mut src = StdinFrameSource::new(Cursor::new(data), 0).unwrap();
        let mut fb = Framebuffer::new(2, 2);

        assert!(src.next_frame(&mut fb).unwrap(), "first full frame renders");
        assert!(
            !src.next_frame(&mut fb).unwrap(),
            "truncated frame stops cleanly (no panic, no error)"
        );
        assert_eq!(src.frames_read(), 1);
    }

    #[test]
    fn malformed_or_empty_header_is_rejected() {
        assert!(StdinFrameSource::new(Cursor::new(b"notaheader\n".to_vec()), 30).is_err());
        assert!(StdinFrameSource::new(Cursor::new(Vec::new()), 30).is_err());
        assert!(StdinFrameSource::new(Cursor::new(b"0x0\n".to_vec()), 30).is_err());
        assert!(StdinFrameSource::new(Cursor::new(b"12xabc\n".to_vec()), 30).is_err());
    }

    #[test]
    fn frame_delay_follows_fps() {
        let paced = StdinFrameSource::new(Cursor::new(b"1x1\n".to_vec()), 30).unwrap();
        assert_eq!(
            paced.frame_delay(),
            Some(Duration::from_secs_f32(1.0 / 30.0))
        );

        let unpaced = StdinFrameSource::new(Cursor::new(b"1x1\n".to_vec()), 0).unwrap();
        assert!(unpaced.frame_delay().is_none());
    }

    #[test]
    fn frame_pixels_are_blitted_into_the_target() {
        let mut data = b"1x1\n".to_vec();
        data.extend_from_slice(&[10, 20, 30]); // a single known pixel
        let mut src = StdinFrameSource::new(Cursor::new(data), 0).unwrap();

        let mut fb = Framebuffer::new(2, 2);
        assert!(src.next_frame(&mut fb).unwrap());
        // Nearest-neighbour upscales the 1x1 source: every target pixel is that colour.
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(fb.get(x, y), Color::from_u8(10, 20, 30, 255));
            }
        }
    }

    #[test]
    fn fit_viewport_preserves_aspect_within_bounds() {
        // A 2:1 source in an 80x24 (ascii, 1x1) terminal is height-limited: it fills the 24 rows
        // and the aspect fixes the width at 48 cols, never exceeding the bounds on either axis.
        let vp = fit_viewport(200, 100, Viewport::new(80, 24), None, 1, 1);
        assert!(vp.cols <= 80 && vp.rows <= 24);
        assert_eq!((vp.cols, vp.rows), (48, 24));
        // A --width override caps columns.
        let vp = fit_viewport(200, 100, Viewport::new(80, 24), Some(40), 1, 1);
        assert!(vp.cols <= 40);
    }
}
