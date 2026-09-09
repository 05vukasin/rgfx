//! The still-image viewer: decode → framebuffer → encode → present.
//!
//! This is the first real [`MediaViewer`] (task 021). It composes the sibling crates without
//! adding any rendering logic of its own, honouring the one architectural law:
//!
//! 1. [`rgfx_image::DecodedImage`] decodes the file and, through
//!    [`RenderOptions`], writes an aspect-corrected, pre-processed (tone + dither) image into a
//!    reused [`Framebuffer`];
//! 2. a grayscale [`TerminalEncoder`] from `rgfx-terminal` (Braille / ASCII / half-blocks) turns
//!    that framebuffer into a [`TerminalFrame`];
//! 3. the frame is either written to the terminal (interactive, event-driven, re-rendering on
//!    resize) or, with `--output`, serialized to a file via [`TerminalFrame::to_text`].
//!
//! Colour (`--color`) is intentionally not wired here: ANSI colour lands in a later task. The
//! renderer resolution already picks the half-block encoder for `--color` (its per-subpixel
//! colour is the future seam), but every encoder used today is grayscale.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use rgfx_core::{Framebuffer, TerminalEncoder, TerminalFrame, Viewport};
use rgfx_image::{BayerSize, DecodedImage, Dither, Preprocess, RenderOptions, Tone};
use rgfx_terminal::{AsciiEncoder, BlockEncoder, BrailleEncoder, terminal_size};

use crate::cli::{DitherMode, Renderer};
use crate::config::Settings;
use crate::dispatch::{MediaViewer, ViewRequest};
use crate::media::Input;
use crate::terminal::{Session, Signal};

/// How long the interactive loop blocks on input between checks. A still image never redraws on
/// its own, so this only bounds shutdown latency; resize events wake the loop immediately.
const POLL_TIMEOUT: Duration = Duration::from_millis(250);

/// The still-image viewer. Stateless; all inputs arrive via the [`ViewRequest`].
#[derive(Debug, Default)]
pub struct ImageViewer;

impl MediaViewer for ImageViewer {
    fn name(&self) -> &'static str {
        "image"
    }

    fn view(&mut self, request: &ViewRequest<'_>) -> anyhow::Result<()> {
        let path = match request.input {
            Input::File(p) => p.as_path(),
            Input::Stdin => {
                // The `-` path is handled earlier by `crate::stream`, which buffers stdin and
                // calls `view_decoded` directly; the viewer trait only ever sees a file here.
                anyhow::bail!("reading images from stdin goes through the pipe path (`rgfx -`)")
            }
        };
        let image = DecodedImage::load(path)
            .with_context(|| format!("loading image {}", path.display()))?;

        view_decoded(&image, request.settings)
    }
}

/// Renders an already-decoded image (e.g. buffered from stdin) either to `--output` or the live
/// terminal. This is the shared core of the still-image path, independent of where the bytes came
/// from.
pub(crate) fn view_decoded(image: &DecodedImage, settings: &Settings) -> anyhow::Result<()> {
    match &settings.output {
        Some(out) => write_output(image, settings, out),
        None if settings.interactive => run_interactive(image, settings),
        None => run_inline(image, settings),
    }
}

/// Renders a decoded image to encoded text at the non-interactive size (no terminal is entered).
///
/// Used by the `--output` sink and by tests. The size follows `--width`, falling back to a
/// default column count.
pub(crate) fn render_to_text(image: &DecodedImage, settings: &Settings) -> String {
    let mut fb = Framebuffer::new(0, 0);
    render_frame(image, settings, None, &mut fb).to_text()
}

/// The concrete grayscale encoder selected from the [`Renderer`] setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolvedRenderer {
    Braille,
    Ascii,
    Blocks,
}

impl ResolvedRenderer {
    /// Resolves `auto` to a concrete encoder: half-blocks when colour is requested (the future
    /// colour seam), otherwise the higher-resolution Braille encoder.
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

    /// The base render options (subpixel factors, cell aspect, resize filter) for this encoder.
    fn render_options(self) -> RenderOptions {
        match self {
            Self::Braille => RenderOptions::braille(),
            Self::Ascii => RenderOptions::ascii(),
            Self::Blocks => RenderOptions::half_blocks(),
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

/// Maps the CLI [`DitherMode`] onto the image crate's [`Dither`], resolving `auto` per renderer.
///
/// Dithering only helps the 1-bit Braille path; for the grayscale ramp encoders (ASCII / blocks)
/// `auto` means "no dithering" so their tonal ramp is preserved. An explicit choice is always
/// honoured.
fn resolve_dither(mode: DitherMode, resolved: ResolvedRenderer) -> Dither {
    match mode {
        DitherMode::Auto => match resolved {
            ResolvedRenderer::Braille => Dither::FloydSteinberg,
            ResolvedRenderer::Ascii | ResolvedRenderer::Blocks => Dither::None,
        },
        DitherMode::None => Dither::None,
        DitherMode::Threshold => Dither::ThresholdOnly,
        DitherMode::Floyd => Dither::FloydSteinberg,
        DitherMode::Atkinson => Dither::Atkinson,
        DitherMode::Bayer => Dither::Bayer(BayerSize::Four),
    }
}

/// Builds the image-quality stage (tone + dithering) from the resolved settings.
fn build_preprocess(settings: &Settings, resolved: ResolvedRenderer) -> Preprocess {
    Preprocess {
        tone: Tone {
            gamma: settings.gamma,
            contrast: settings.contrast,
            brightness: 0.0,
            sharpen: None,
        },
        dither: resolve_dither(settings.dither, resolved),
        threshold: settings.threshold,
    }
}

/// The number of terminal columns per row that preserves the image's real-world aspect ratio.
///
/// Cells are physically `cell_aspect` wide per unit tall, so an undistorted image satisfies
/// `(cols * cell_aspect) / rows == img_w / img_h`; this returns `cols / rows`. Note it depends
/// only on the cell aspect, not the encoder's subpixel factors: those set framebuffer pixel
/// resolution, not the cell grid shape.
fn cols_per_row(img_w: u32, img_h: u32, cell_aspect: f32) -> f32 {
    let image_aspect = img_w.max(1) as f32 / img_h.max(1) as f32;
    let ca = if cell_aspect.is_finite() && cell_aspect > 0.0 {
        cell_aspect
    } else {
        0.5
    };
    (image_aspect / ca).max(f32::MIN_POSITIVE)
}

/// The render viewport for a fixed target width, deriving rows from the image aspect.
fn viewport_for_width(img_w: u32, img_h: u32, cols: u16, cell_aspect: f32) -> Viewport {
    let cols = cols.max(1);
    let k = cols_per_row(img_w, img_h, cell_aspect);
    let rows = (cols as f32 / k).round().clamp(1.0, u16::MAX as f32) as u16;
    Viewport::new(cols, rows)
}

/// The largest render viewport that fits within `bounds`, preserving the image aspect.
fn fit_within(img_w: u32, img_h: u32, bounds: Viewport, cell_aspect: f32) -> Viewport {
    let k = cols_per_row(img_w, img_h, cell_aspect);
    let max_cols = bounds.cols.max(1);
    let max_rows = bounds.rows.max(1);

    // Try filling the full width, then fall back to filling the full height.
    let mut cols = max_cols;
    let mut rows = (cols as f32 / k).round().clamp(1.0, u16::MAX as f32) as u16;
    if rows > max_rows {
        rows = max_rows;
        cols = (rows as f32 * k).round().clamp(1.0, max_cols as f32) as u16;
    }
    Viewport::new(cols, rows)
}

/// Chooses the render viewport: `--width` fixes the width, otherwise the image is fit within the
/// available `bounds` (the live terminal), falling back to a default width when neither exists.
fn render_viewport(
    image: &DecodedImage,
    settings: &Settings,
    bounds: Option<Viewport>,
    cell_aspect: f32,
) -> Viewport {
    let (w, h) = (image.width(), image.height());
    match settings.width {
        Some(width) => {
            let cols = width.clamp(1, u16::MAX as u32) as u16;
            viewport_for_width(w, h, cols, cell_aspect)
        }
        None => match bounds {
            Some(bounds) => fit_within(w, h, bounds, cell_aspect),
            // No explicit width and no fit-bounds (the `--output`/text path): follow the real
            // terminal width, letting rows follow the image aspect (height is not capped, so a
            // saved dump keeps the whole image). Falls back to 80 cols with no terminal.
            None => viewport_for_width(w, h, terminal_size().cols, cell_aspect),
        },
    }
}

/// Renders the image into `fb` (reused) and encodes it to a [`TerminalFrame`].
///
/// `bounds` is the terminal viewport for interactive fitting, or `None` for non-interactive
/// output (where `--width` or a default drives the size).
fn render_frame(
    image: &DecodedImage,
    settings: &Settings,
    bounds: Option<Viewport>,
    fb: &mut Framebuffer,
) -> TerminalFrame {
    let resolved = ResolvedRenderer::resolve(settings.renderer, settings.color);
    let mut opts = resolved.render_options();
    opts.preprocess = build_preprocess(settings, resolved);

    let viewport = render_viewport(image, settings, bounds, opts.cell_aspect);
    image.render_into(fb, viewport, &opts);
    resolved.encoder().encode(fb, viewport)
}

/// Writes the encoded image to `out` as text (`--output`). Non-interactive: no terminal is entered.
fn write_output(image: &DecodedImage, settings: &Settings, out: &Path) -> anyhow::Result<()> {
    std::fs::write(out, render_to_text(image, settings))
        .with_context(|| format!("writing output to {}", out.display()))?;
    tracing::info!(path = %out.display(), "wrote encoded image");
    Ok(())
}

/// The fit-bounds for inline rendering: the terminal minus one row of headroom.
///
/// Reserving a row means that after the image is printed, the shell prompt lands on the spare
/// line instead of forcing the terminal to scroll — which would clip the top of the image (the
/// "misaligned by N rows" bug). Never returns a zero dimension.
fn inline_bounds(term: Viewport) -> Viewport {
    Viewport::new(term.cols.max(1), term.rows.saturating_sub(1).max(1))
}

/// Prints the image inline at the current terminal size and returns to the shell.
///
/// This is the default for a still image: no raw mode, no alternate screen, no `q`. The render is
/// fit within `(cols, rows - 1)` — one row of headroom so the shell prompt that appears after the
/// image does not push the terminal to scroll and clip the top row. An explicit `--width` still
/// overrides (rows then follow the image aspect and the output may exceed the viewport height,
/// which is the caller's choice). Output goes to stdout as a single write with a trailing newline.
fn run_inline(image: &DecodedImage, settings: &Settings) -> anyhow::Result<()> {
    let bounds = inline_bounds(terminal_size());
    let mut fb = Framebuffer::new(0, 0);
    let frame = render_frame(image, settings, Some(bounds), &mut fb);
    // Print the whole frame; the trailing newline keeps the returning prompt on its own line.
    println!("{}", frame.to_text());
    Ok(())
}

/// Displays the image in the live terminal, re-rendering on resize and idling otherwise.
///
/// The [`Session`] owns the terminal's raw/alternate-screen state and restores it on every exit
/// path (normal quit, error, or panic), so quitting always leaves the terminal usable.
fn run_interactive(image: &DecodedImage, settings: &Settings) -> anyhow::Result<()> {
    let mut session = Session::open()?;
    // One framebuffer, reused across every re-render (no per-frame allocation).
    let mut fb = Framebuffer::new(0, 0);
    let mut dirty = true;

    loop {
        if dirty {
            let bounds = session.viewport()?;
            let frame = render_frame(image, settings, Some(bounds), &mut fb);
            session.present(&frame.to_text())?;
            dirty = false;
        }
        match session.wait(POLL_TIMEOUT)? {
            Signal::Quit => break,
            Signal::Redraw => dirty = true,
            Signal::Idle => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::RenderOpts;
    use crate::config::Config;

    /// A 1×1 opaque-red PNG borrowed from `rgfx-image`'s test fixtures.
    fn red_pixel() -> DecodedImage {
        DecodedImage::from_bytes(rgfx_image::doctest_png()).expect("valid embedded PNG")
    }

    fn settings(opts: RenderOpts) -> Settings {
        Settings::resolve(&Config::default(), &opts)
    }

    #[test]
    fn cols_per_row_reflects_image_aspect() {
        // Square image, 1:2 cells => 2 cols per row.
        assert!((cols_per_row(100, 100, 0.5) - 2.0).abs() < 1e-6);
        // 2:1 wide image => 4 cols per row.
        assert!((cols_per_row(200, 100, 0.5) - 4.0).abs() < 1e-6);
        // Degenerate cell aspect falls back to 0.5.
        assert!((cols_per_row(100, 100, 0.0) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn viewport_for_width_derives_rows_from_aspect() {
        // Wide 2:1 image at 40 cols => 40/4 = 10 rows.
        assert_eq!(viewport_for_width(200, 100, 40, 0.5), Viewport::new(40, 10));
        // Tall 1:2 image at 40 cols => 40/1 = 40 rows.
        assert_eq!(viewport_for_width(100, 200, 40, 0.5), Viewport::new(40, 40));
        // Rows never collapse below 1.
        assert_eq!(viewport_for_width(1000, 1, 4, 0.5).rows, 1);
    }

    #[test]
    fn a_still_image_is_inline_by_default() {
        // Regression (task 030): the default must be inline print-and-return, not the
        // full-screen interactive viewer. `--interactive` opts back in.
        let s = settings(RenderOpts::default());
        assert!(
            !s.interactive,
            "still images must default to inline, not interactive"
        );
        assert!(s.output.is_none());
    }

    #[test]
    fn inline_bounds_reserves_one_row_of_headroom() {
        // Regression (task 030): inline height leaves a row for the returning prompt so the
        // image never scrolls off the top.
        assert_eq!(
            inline_bounds(Viewport::new(100, 30)),
            Viewport::new(100, 29)
        );
        // Never collapses to zero on a degenerate terminal.
        assert_eq!(inline_bounds(Viewport::new(0, 0)), Viewport::new(1, 1));
        assert_eq!(inline_bounds(Viewport::new(80, 1)), Viewport::new(80, 1));
    }

    #[test]
    fn inline_render_fits_within_the_reserved_bounds() {
        // A 2:1 image (4 cols/row) in a 100×30 terminal: inline bounds 100×29, width-fit → 25
        // rows, which is ≤ 29 so nothing scrolls.
        let img = DecodedImage::from_bytes(rgfx_image::doctest_png()).unwrap();
        let bounds = inline_bounds(Viewport::new(100, 30));
        let vp = render_viewport(&img, &settings(RenderOpts::default()), Some(bounds), 0.5);
        assert!(
            vp.rows <= bounds.rows,
            "inline render must fit within the reserved rows"
        );
        assert!(vp.cols <= bounds.cols);
    }

    #[test]
    fn fit_within_respects_the_terminal_bounds() {
        // Tall image into an 80x24 terminal is height-limited: 24 rows, 24 cols (k=1).
        let vp = fit_within(50, 100, Viewport::new(80, 24), 0.5);
        assert_eq!(vp, Viewport::new(24, 24));
        // Wide image is width-limited and never exceeds the bounds on either axis.
        let vp = fit_within(200, 100, Viewport::new(80, 24), 0.5);
        assert!(vp.cols <= 80 && vp.rows <= 24);
        assert_eq!(vp.cols, 80);
    }

    #[test]
    fn width_setting_scales_output_dimensions() {
        // The same image at two widths produces correspondingly sized frames, for every renderer.
        for renderer in [Renderer::Braille, Renderer::Ascii, Renderer::Blocks] {
            let mut fb = Framebuffer::new(0, 0);

            let narrow = settings(RenderOpts {
                renderer: Some(renderer),
                width: Some(20),
                ..RenderOpts::default()
            });
            let wide = settings(RenderOpts {
                renderer: Some(renderer),
                width: Some(40),
                ..RenderOpts::default()
            });

            let f_narrow = render_frame(&red_pixel(), &narrow, None, &mut fb);
            let f_wide = render_frame(&red_pixel(), &wide, None, &mut fb);

            // 1x1 image, cell aspect 0.5 => 2 cols per row.
            assert_eq!((f_narrow.cols(), f_narrow.rows()), (20, 10), "{renderer:?}");
            assert_eq!((f_wide.cols(), f_wide.rows()), (40, 20), "{renderer:?}");
        }
    }

    #[test]
    fn color_auto_selects_blocks_else_braille() {
        assert_eq!(
            ResolvedRenderer::resolve(Renderer::Auto, false),
            ResolvedRenderer::Braille
        );
        assert_eq!(
            ResolvedRenderer::resolve(Renderer::Auto, true),
            ResolvedRenderer::Blocks
        );
    }

    #[test]
    fn auto_dither_is_floyd_for_braille_only() {
        assert_eq!(
            resolve_dither(DitherMode::Auto, ResolvedRenderer::Braille),
            Dither::FloydSteinberg
        );
        assert_eq!(
            resolve_dither(DitherMode::Auto, ResolvedRenderer::Ascii),
            Dither::None
        );
        // An explicit choice overrides the per-renderer default.
        assert_eq!(
            resolve_dither(DitherMode::Atkinson, ResolvedRenderer::Ascii),
            Dither::Atkinson
        );
    }

    #[test]
    fn ascii_render_to_string_snapshot() {
        // A solid image fills the whole framebuffer, so every cell is identical: a stable snapshot.
        let s = settings(RenderOpts {
            renderer: Some(Renderer::Ascii),
            width: Some(4),
            dither: Some(DitherMode::None),
            ..RenderOpts::default()
        });
        let mut fb = Framebuffer::new(0, 0);
        let frame = render_frame(&red_pixel(), &s, None, &mut fb);
        // Red luma 0.299 maps to ramp index round(0.299*9)=3 => '-'. Frame is 4x2 cells.
        assert_eq!(frame.to_text(), "----\n----");
    }

    #[test]
    fn braille_solid_dark_image_is_all_blank_dots() {
        // Red luma (0.299) is below the 0.5 threshold and, undithered, yields empty Braille cells.
        let s = settings(RenderOpts {
            renderer: Some(Renderer::Braille),
            width: Some(3),
            dither: Some(DitherMode::None),
            ..RenderOpts::default()
        });
        let mut fb = Framebuffer::new(0, 0);
        let frame = render_frame(&red_pixel(), &s, None, &mut fb);
        // 1x1 image at 3 cols, k=2 => rows = round(3/2) = 2.
        assert_eq!((frame.cols(), frame.rows()), (3, 2));
        assert!(
            frame
                .to_text()
                .chars()
                .all(|c| c == '\u{2800}' || c == '\n')
        );
    }

    #[test]
    fn output_flag_writes_expected_bytes() {
        // End-to-end through the real --output path: decode a temp PNG, encode, write, read back.
        let dir = std::env::temp_dir();
        let png = dir.join("rgfx_image_viewer_in.png");
        let out = dir.join("rgfx_image_viewer_out.txt");
        std::fs::write(&png, rgfx_image::doctest_png()).unwrap();

        let s = settings(RenderOpts {
            renderer: Some(Renderer::Ascii),
            width: Some(6),
            dither: Some(DitherMode::None),
            output: Some(out.clone()),
            ..RenderOpts::default()
        });
        let request = ViewRequest {
            input: &Input::File(png.clone()),
            kind: crate::media::MediaKind::Image,
            settings: &s,
        };
        ImageViewer
            .view(&request)
            .expect("output render should succeed");

        let written = std::fs::read_to_string(&out).unwrap();
        std::fs::remove_file(&png).ok();
        std::fs::remove_file(&out).ok();

        // 1x1 red at 6 cols ascii => 6x3 cells, every cell '-'.
        assert_eq!(written, "------\n------\n------");
    }

    #[test]
    fn stdin_input_is_a_clean_error() {
        let s = settings(RenderOpts::default());
        let request = ViewRequest {
            input: &Input::Stdin,
            kind: crate::media::MediaKind::Image,
            settings: &s,
        };
        let err = ImageViewer.view(&request).unwrap_err();
        assert!(err.to_string().contains("stdin"), "got: {err}");
    }

    #[test]
    fn missing_file_errors_without_panicking() {
        let s = settings(RenderOpts::default());
        let request = ViewRequest {
            input: &Input::File("/no/such/rgfx/image.png".into()),
            kind: crate::media::MediaKind::Image,
            settings: &s,
        };
        let err = ImageViewer.view(&request).unwrap_err();
        assert!(err.to_string().contains("loading image"), "got: {err}");
    }
}
