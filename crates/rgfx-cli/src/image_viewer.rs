//! The still-image viewer: decode → framebuffer → encode → present.
//!
//! This is the first real [`MediaViewer`] (task 021). It composes the sibling crates without
//! adding any rendering logic of its own, honouring the one architectural law:
//!
//! 1. [`rgfx_image::DecodedImage`] decodes the file and, through
//!    [`RenderOptions`], writes an aspect-corrected, pre-processed (tone + dither) image into a
//!    reused [`Framebuffer`];
//! 2. a [`TerminalEncoder`] from `rgfx-terminal` (Braille / ASCII / half-blocks) turns that
//!    framebuffer into a [`TerminalFrame`], grayscale by default or attaching per-cell colour when
//!    the interactive colour menu is on;
//! 3. the frame is either written to the terminal (interactive, event-driven, re-rendering on
//!    resize) or, with `--output`, serialized to a file via [`TerminalFrame::to_text`].
//!
//! Colour (task 039): the full-screen preview exposes a modal colour menu (`C`) that turns ANSI
//! colour on/off and picks the fidelity (16 / 256 / truecolor, clamped to the terminal's detected
//! capability). The chosen [`ColorMode`] is wired into *both* the encoder options (so the encoder
//! attaches colours) and the [`FrameEngine`]'s serializer (so it quantizes to the right fidelity);
//! changing the mode rebuilds the engine, forcing a full redraw. With colour off the encoder stays
//! grayscale and the output is byte-identical to the pre-039 path. The `--output` path is always
//! grayscale and deterministic.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use rgfx_core::{Framebuffer, TerminalEncoder, TerminalFrame, Viewport};
use rgfx_image::{BayerSize, DecodedImage, Dither, Preprocess, RenderOptions, Tone};
use rgfx_terminal::{
    AsciiEncoder, AsciiOptions, BlockEncoder, BlockOptions, BrailleEncoder, BrailleOptions,
    ColorMode, Event, FrameEngine, KeyCode, KeyEvent, detect_color_mode, terminal_size,
};

use crate::cli::{ColorFidelity, DitherMode, Renderer};
use crate::config::Settings;
use crate::dispatch::{MediaViewer, ViewRequest};
use crate::media::Input;
use crate::terminal::Session;

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
        None if settings.cat => run_inline(image, settings),
        None => run_fullscreen(image, settings),
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
/// What a key press means to the full-screen image preview loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageAction {
    Quit,
    Redraw,
    Ignore,
}

/// Contrast delta applied per `+`/`-` press inside the colour menu (reuses the tone stage).
const CONTRAST_STEP: f32 = 0.1;
/// Contrast is clamped to this range so the tone stage stays well-behaved.
const CONTRAST_RANGE: (f32, f32) = (0.1, 4.0);

/// A monotonic fidelity rank for a [`ColorMode`], so modes can be compared and clamped.
///
/// `None < Ansi16 < Ansi256 < TrueColor`. This is the ordering the colour menu cycles through and
/// the terminal capability clamps against.
fn color_rank(mode: ColorMode) -> u8 {
    match mode {
        ColorMode::None => 0,
        ColorMode::Ansi16 => 1,
        ColorMode::Ansi256 => 2,
        ColorMode::TrueColor => 3,
    }
}

/// Clamps `mode` down to the terminal's detected `capability` so we never emit escapes a terminal
/// cannot render (e.g. truecolor on a 16-color `TERM`).
fn clamp_color_mode(mode: ColorMode, capability: ColorMode) -> ColorMode {
    if color_rank(mode) <= color_rank(capability) {
        mode
    } else {
        capability
    }
}

/// Maps the CLI `--color-mode` seed onto a [`ColorMode`].
fn fidelity_mode(fidelity: ColorFidelity) -> ColorMode {
    match fidelity {
        ColorFidelity::Ansi16 => ColorMode::Ansi16,
        ColorFidelity::Ansi256 => ColorMode::Ansi256,
        ColorFidelity::TrueColor => ColorMode::TrueColor,
    }
}

/// A short label for the status bar: the fidelity name, or `off`.
fn color_mode_name(mode: ColorMode) -> &'static str {
    match mode {
        ColorMode::None => "off",
        ColorMode::Ansi16 => "16",
        ColorMode::Ansi256 => "256",
        ColorMode::TrueColor => "true",
    }
}

/// The interactive state of the full-screen image preview: the active encoder, dithering, and
/// toggles, plus the base tone carried over from the CLI/config and the modal colour menu.
struct ImageState {
    renderer: ResolvedRenderer,
    dither: DitherMode,
    invert: bool,
    /// Whether ANSI colour output is enabled (the on/off now lives inside the colour menu).
    color: bool,
    /// The selected colour fidelity, always clamped to `capability`. Used when `color` is on.
    color_mode: ColorMode,
    /// The terminal's detected colour capability; the fidelity ceiling for `color_mode`.
    capability: ColorMode,
    /// Whether the modal colour menu is open (captures input while true).
    color_menu_open: bool,
    gamma: f32,
    contrast: f32,
    /// The contrast the viewer started with, restored by the colour menu's reset.
    base_contrast: f32,
    threshold: f32,
    show_ui: bool,
}

impl ImageState {
    /// Builds the initial state, seeding colour from the CLI/config and clamping the requested
    /// fidelity to the terminal's detected `capability`.
    fn from_settings(settings: &Settings, capability: ColorMode) -> Self {
        // Seed the fidelity from --color-mode when given, else the terminal's full capability.
        let seed = settings.color_mode.map(fidelity_mode).unwrap_or(capability);
        Self {
            renderer: ResolvedRenderer::resolve(settings.renderer, settings.color),
            dither: settings.dither,
            invert: false,
            color: settings.color,
            color_mode: clamp_color_mode(seed, capability),
            capability,
            color_menu_open: false,
            gamma: settings.gamma,
            contrast: settings.contrast,
            base_contrast: settings.contrast,
            threshold: settings.threshold,
            show_ui: true,
        }
    }

    /// The colour mode the encoder attaches and the [`FrameEngine`] must quantize to: the selected
    /// fidelity when colour is on, otherwise [`ColorMode::None`] (grayscale, no escapes).
    fn effective_mode(&self) -> ColorMode {
        if self.color {
            self.color_mode
        } else {
            ColorMode::None
        }
    }

    /// Advances the fidelity to the next available step (Ansi16 → Ansi256 → TrueColor → …),
    /// wrapping within the range the terminal actually supports. A no-op on a colourless terminal.
    fn cycle_color_mode(&mut self) {
        const ORDER: [ColorMode; 3] = [ColorMode::Ansi16, ColorMode::Ansi256, ColorMode::TrueColor];
        let cap = color_rank(self.capability);
        if cap == 0 {
            return; // The terminal reports no colour support; nothing to cycle.
        }
        let count = (cap as usize).min(ORDER.len());
        let cur = clamp_color_mode(self.color_mode, self.capability);
        let idx = ORDER[..count].iter().position(|&m| m == cur).unwrap_or(0);
        self.color_mode = ORDER[(idx + 1) % count];
    }

    /// Restores the colour-menu-controlled settings to their defaults: colour off, fidelity at the
    /// terminal's full capability, and the tone contrast back to the seeded base.
    fn reset_color(&mut self) {
        self.color = false;
        self.color_mode = self.capability;
        self.contrast = self.base_contrast;
    }

    /// Routes a key while the colour menu is open, mutating colour state. Returns whether anything
    /// changed (so the caller can request a redraw). Menu open/close (`Esc`/`C`) is handled by
    /// [`ImageState::on_key`] before this is reached.
    fn on_color_menu_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('o') | KeyCode::Char('O') | KeyCode::Char(' ') => {
                self.color = !self.color
            }
            KeyCode::Char('m') | KeyCode::Char('M') => self.cycle_color_mode(),
            KeyCode::Char('b') | KeyCode::Char('B') => self.renderer = ResolvedRenderer::Blocks,
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.contrast =
                    (self.contrast + CONTRAST_STEP).clamp(CONTRAST_RANGE.0, CONTRAST_RANGE.1);
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                self.contrast =
                    (self.contrast - CONTRAST_STEP).clamp(CONTRAST_RANGE.0, CONTRAST_RANGE.1);
            }
            KeyCode::Char('r') | KeyCode::Char('R') => self.reset_color(),
            _ => return false,
        }
        true
    }

    fn cycle_renderer(&mut self) {
        self.renderer = match self.renderer {
            ResolvedRenderer::Braille => ResolvedRenderer::Ascii,
            ResolvedRenderer::Ascii => ResolvedRenderer::Blocks,
            ResolvedRenderer::Blocks => ResolvedRenderer::Braille,
        };
    }

    fn cycle_dither(&mut self) {
        self.dither = match self.dither {
            DitherMode::Auto => DitherMode::None,
            DitherMode::None => DitherMode::Floyd,
            DitherMode::Floyd => DitherMode::Atkinson,
            DitherMode::Atkinson => DitherMode::Bayer,
            DitherMode::Bayer => DitherMode::Threshold,
            DitherMode::Threshold => DitherMode::Auto,
        };
    }

    /// Applies a key press, updating the state and reporting what the loop should do.
    ///
    /// The modal colour menu takes priority: `C` opens/closes it, and while it is open the colour
    /// keys drive colour rather than the image controls (Ctrl+C still quits). While it is closed the
    /// `R`/`D`/`I`/`F` image controls behave as before, and `C` opens the menu.
    fn on_key(&mut self, key: KeyEvent) -> ImageAction {
        // Ctrl+C always quits, even out of the modal menu.
        if key.modifiers.ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            return ImageAction::Quit;
        }

        // While the colour menu is open it captures input: Esc/C close it, everything else drives
        // colour (never the image controls or a plain-key quit).
        if self.color_menu_open {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('C') => {
                    self.color_menu_open = false;
                    ImageAction::Redraw
                }
                _ => {
                    if self.on_color_menu_key(key) {
                        ImageAction::Redraw
                    } else {
                        ImageAction::Ignore
                    }
                }
            };
        }

        if crate::terminal::is_quit(key) {
            return ImageAction::Quit;
        }
        match key.code {
            KeyCode::Char('r') | KeyCode::Char('R') => self.cycle_renderer(),
            KeyCode::Char('d') | KeyCode::Char('D') => self.cycle_dither(),
            KeyCode::Char('i') | KeyCode::Char('I') => self.invert = !self.invert,
            KeyCode::Char('c') | KeyCode::Char('C') => self.color_menu_open = true,
            KeyCode::Char('f') | KeyCode::Char('F') => self.show_ui = !self.show_ui,
            _ => return ImageAction::Ignore,
        }
        ImageAction::Redraw
    }

    /// The preprocessing (tone + dithering) for the current state.
    fn preprocess(&self) -> Preprocess {
        Preprocess {
            tone: Tone {
                gamma: self.gamma,
                contrast: self.contrast,
                brightness: 0.0,
                sharpen: None,
            },
            dither: resolve_dither(self.dither, self.renderer),
            threshold: self.threshold,
        }
    }

    /// The encoder for the current state. It attaches colours at the state's [`effective_mode`],
    /// which is [`ColorMode::None`] (grayscale) while colour is off.
    ///
    /// [`effective_mode`]: ImageState::effective_mode
    fn encoder(&self) -> Box<dyn TerminalEncoder> {
        let color = self.effective_mode();
        match self.renderer {
            ResolvedRenderer::Braille => Box::new(BrailleEncoder::with_options(BrailleOptions {
                invert: self.invert,
                gamma: self.gamma,
                contrast: self.contrast,
                color,
                ..BrailleOptions::default()
            })),
            ResolvedRenderer::Ascii => Box::new(AsciiEncoder::with_options(AsciiOptions {
                invert: self.invert,
                gamma: self.gamma,
                color,
                ..AsciiOptions::default()
            })),
            ResolvedRenderer::Blocks => Box::new(BlockEncoder::with_options(BlockOptions {
                invert: self.invert,
                color,
                ..BlockOptions::default()
            })),
        }
    }

    fn renderer_name(&self) -> &'static str {
        match self.renderer {
            ResolvedRenderer::Braille => "braille",
            ResolvedRenderer::Ascii => "ascii",
            ResolvedRenderer::Blocks => "blocks",
        }
    }
}

/// Renders `image` for the current `state` into a `bounds`-sized [`TerminalFrame`], letterboxed
/// aspect-correct by the image pipeline, then overlays the options bar (and, when open, the modal
/// colour menu) over it.
///
/// The encoder attaches colours at `state.effective_mode()`; the caller must present the frame
/// through a [`FrameEngine`] built at that same mode so the quantization matches.
fn render_state(
    image: &DecodedImage,
    state: &ImageState,
    bounds: Viewport,
    fb: &mut Framebuffer,
) -> TerminalFrame {
    let mut opts = state.renderer.render_options();
    opts.preprocess = state.preprocess();
    image.render_into(fb, bounds, &opts);
    let mut frame = state.encoder().encode(fb, bounds);
    if state.show_ui {
        overlay_image_status(&mut frame, image, state);
    }
    // The modal colour menu draws over everything (even with the status bar hidden) since the user
    // explicitly opened it.
    if state.color_menu_open {
        overlay_color_menu(&mut frame, state);
    }
    frame
}

/// The status-bar label for the current colour state: the fidelity name when on, else `off`.
fn color_status(state: &ImageState) -> &'static str {
    if state.color {
        color_mode_name(state.color_mode)
    } else {
        "off"
    }
}

/// Draws the two-line options bar (mode info + key help) across the bottom rows of `frame`.
fn overlay_image_status(frame: &mut TerminalFrame, image: &DecodedImage, state: &ImageState) {
    let status = format!(
        "{}x{} | {} | dither:{} | color:{} | invert:{}",
        image.width(),
        image.height(),
        state.renderer_name(),
        dither_name(state.dither),
        color_status(state),
        if state.invert { "on" } else { "off" },
    );
    let help = "R:renderer  D:dither  I:invert  C:color-menu  F:ui  Q:quit";
    crate::viewer_chrome::overlay_bottom_bar(frame, &status, help);
}

/// Draws the modal colour-menu panel (on/off, fidelity, detected max, contrast, and key help)
/// floating near the top-left of `frame`.
fn overlay_color_menu(frame: &mut TerminalFrame, state: &ImageState) {
    let lines = vec![
        "Colour menu".to_string(),
        format!("colour:   {}", if state.color { "on" } else { "off" }),
        format!("fidelity: {}", color_mode_name(state.color_mode)),
        format!("max:      {}", color_mode_name(state.capability)),
        format!("contrast: {:.2}", state.contrast),
        String::new(),
        "O/Space on/off   M fidelity".to_string(),
        "B blocks   +/- contrast".to_string(),
        "R reset   Esc/C close".to_string(),
    ];
    crate::viewer_chrome::overlay_panel(frame, 1, 1, &lines);
}

/// A short human name for a dither mode, for the options bar.
fn dither_name(d: DitherMode) -> &'static str {
    match d {
        DitherMode::Auto => "auto",
        DitherMode::None => "none",
        DitherMode::Threshold => "threshold",
        DitherMode::Floyd => "floyd",
        DitherMode::Atkinson => "atkinson",
        DitherMode::Bayer => "bayer",
    }
}

/// Opens the full-screen image preview: enters the alternate screen, renders the image
/// fit-to-terminal with a bottom options bar, and re-renders on resize or a control key. The
/// [`Session`] restores the terminal on every exit path.
fn run_fullscreen(image: &DecodedImage, settings: &Settings) -> anyhow::Result<()> {
    let mut session = Session::open()?;
    // The terminal's detected colour capability is the fidelity ceiling the colour menu clamps to.
    let capability = detect_color_mode();
    let mut state = ImageState::from_settings(settings, capability);
    // The engine's serializer mode must track the encoder's colour mode so escapes quantize to the
    // right fidelity. It starts at the state's effective mode and is rebuilt whenever that changes.
    let mut engine_mode = state.effective_mode();
    let mut engine = FrameEngine::new(engine_mode);
    let mut fb = Framebuffer::new(0, 0);
    let mut viewport = session.viewport()?;
    let mut dirty = true;

    loop {
        if dirty {
            // A change in the effective colour mode (on/off or fidelity) means the engine's
            // serializer mode is stale: rebuild it so the new escapes take effect. A fresh engine's
            // first render is a full redraw, which is exactly what we want after a mode switch.
            let desired = state.effective_mode();
            if desired != engine_mode {
                engine = FrameEngine::new(desired);
                engine_mode = desired;
            }
            let frame = render_state(image, &state, viewport, &mut fb);
            session.render_frame(&mut engine, &frame)?;
            dirty = false;
        }
        match session.poll_event(POLL_TIMEOUT)? {
            Some(Event::Resize(cols, rows)) => {
                viewport = Viewport::new(cols, rows);
                dirty = true;
            }
            Some(Event::Key(key)) => match state.on_key(key) {
                ImageAction::Quit => break,
                ImageAction::Redraw => dirty = true,
                ImageAction::Ignore => {}
            },
            _ => {}
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

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: rgfx_terminal::KeyModifiers::NONE,
        }
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
    fn still_image_defaults_to_fullscreen_and_cat_opts_out() {
        // Task 032: the default is now the full-screen preview; `-c`/`--cat` opts into the
        // inline print-and-return behavior (task 030's run_inline).
        let default = settings(RenderOpts::default());
        assert!(!default.cat, "default is full-screen, not cat/inline");
        assert!(default.output.is_none());
        let cat = settings(RenderOpts {
            cat: true,
            ..RenderOpts::default()
        });
        assert!(cat.cat, "--cat selects inline output");
    }

    /// Builds an [`ImageState`] from render options at a fixed truecolor capability, for tests.
    fn state_with(opts: RenderOpts) -> ImageState {
        ImageState::from_settings(&settings(opts), ColorMode::TrueColor)
    }

    #[test]
    fn image_controls_cycle_renderer_dither_and_toggles() {
        let mut st = state_with(RenderOpts::default());
        let start = st.renderer;
        assert_eq!(st.on_key(key(KeyCode::Char('r'))), ImageAction::Redraw);
        assert_ne!(st.renderer, start, "R cycles the renderer");
        assert_eq!(st.on_key(key(KeyCode::Char('i'))), ImageAction::Redraw);
        assert!(st.invert, "I toggles invert");
        let d = st.dither;
        assert_eq!(st.on_key(key(KeyCode::Char('d'))), ImageAction::Redraw);
        assert_ne!(st.dither, d, "D cycles dither");
        assert_eq!(st.on_key(key(KeyCode::Char('f'))), ImageAction::Redraw);
        assert!(!st.show_ui, "F toggles the options bar");
        assert_eq!(st.on_key(key(KeyCode::Char('q'))), ImageAction::Quit);
    }

    #[test]
    fn fullscreen_render_fills_viewport_and_overlays_bar() {
        let img = red_pixel();
        let st = state_with(RenderOpts::default());
        let mut fb = Framebuffer::new(0, 0);
        let frame = render_state(&img, &st, Viewport::new(80, 12), &mut fb);
        assert_eq!(frame.cols(), 80);
        assert_eq!(frame.rows(), 12);
        // The bottom row is the key-help line (fits at 80 cols), not blank braille.
        assert!(frame.to_text().lines().last().unwrap().contains("Q:quit"));
    }

    #[test]
    fn c_opens_and_closes_the_colour_menu_and_routes_keys() {
        let mut st = state_with(RenderOpts::default());
        assert!(!st.color_menu_open);
        // Closed: C opens the menu rather than toggling colour directly.
        assert_eq!(st.on_key(key(KeyCode::Char('c'))), ImageAction::Redraw);
        assert!(st.color_menu_open, "C opens the colour menu");
        assert!(!st.color, "opening the menu does not toggle colour");

        // Open: O toggles colour on; image controls are captured (R does not cycle the renderer).
        let r = st.renderer;
        assert_eq!(st.on_key(key(KeyCode::Char('o'))), ImageAction::Redraw);
        assert!(st.color, "O turns colour on inside the menu");
        assert_eq!(st.on_key(key(KeyCode::Char('r'))), ImageAction::Redraw);
        assert_eq!(
            st.renderer, r,
            "R resets colour, not the renderer, while the menu is open"
        );
        assert!(!st.color, "R inside the menu resets colour to off");

        // B quick-picks the blocks renderer while open.
        assert_eq!(st.on_key(key(KeyCode::Char('b'))), ImageAction::Redraw);
        assert_eq!(
            st.renderer,
            ResolvedRenderer::Blocks,
            "B picks the blocks renderer"
        );

        // Esc closes; afterwards image controls resume.
        assert_eq!(st.on_key(key(KeyCode::Esc)), ImageAction::Redraw);
        assert!(!st.color_menu_open, "Esc closes the menu");
        let before = st.renderer;
        assert_eq!(st.on_key(key(KeyCode::Char('r'))), ImageAction::Redraw);
        assert_ne!(
            st.renderer, before,
            "R cycles the renderer once the menu is closed"
        );
    }

    #[test]
    fn colour_menu_cycles_fidelity_and_clamps_to_capability() {
        // On a truecolor terminal M cycles through all three fidelities and wraps.
        let mut st =
            ImageState::from_settings(&settings(RenderOpts::default()), ColorMode::TrueColor);
        st.color = true;
        st.color_mode = ColorMode::Ansi16;
        st.color_menu_open = true;
        assert_eq!(st.on_key(key(KeyCode::Char('m'))), ImageAction::Redraw);
        assert_eq!(st.color_mode, ColorMode::Ansi256);
        st.on_key(key(KeyCode::Char('m')));
        assert_eq!(st.color_mode, ColorMode::TrueColor);
        st.on_key(key(KeyCode::Char('m')));
        assert_eq!(
            st.color_mode,
            ColorMode::Ansi16,
            "cycling wraps back to the lowest fidelity"
        );
    }

    #[test]
    fn fidelity_is_clamped_to_a_lower_capability() {
        // Seeding truecolor on an Ansi16 terminal clamps down to Ansi16, and M stays put.
        let opts = RenderOpts {
            color: true,
            color_mode: Some(ColorFidelity::TrueColor),
            ..RenderOpts::default()
        };
        let mut st = ImageState::from_settings(&settings(opts), ColorMode::Ansi16);
        assert_eq!(
            st.color_mode,
            ColorMode::Ansi16,
            "truecolor clamps to the Ansi16 capability"
        );
        assert_eq!(st.effective_mode(), ColorMode::Ansi16);
        st.color_menu_open = true;
        st.on_key(key(KeyCode::Char('m')));
        assert_eq!(
            st.color_mode,
            ColorMode::Ansi16,
            "only one fidelity is available; M is a no-op"
        );
    }

    #[test]
    fn colour_off_is_grayscale_and_on_attaches_cell_colour() {
        // Colour off: the encoder attaches no fg/bg, so the frame is byte-identical to grayscale.
        let img = red_pixel();
        let mut fb = Framebuffer::new(0, 0);
        let off = state_with(RenderOpts {
            renderer: Some(Renderer::Blocks),
            ..RenderOpts::default()
        });
        assert_eq!(off.effective_mode(), ColorMode::None);
        let frame_off = render_state(&img, &off, Viewport::new(8, 4), &mut fb);
        assert!(
            frame_off
                .cells()
                .iter()
                .all(|c| c.fg.is_none() && c.bg.is_none()),
            "colour off attaches no cell colour"
        );

        // Colour on with the blocks renderer: fg (top) and bg (bottom) are both attached per cell.
        let mut on = state_with(RenderOpts {
            renderer: Some(Renderer::Blocks),
            ..RenderOpts::default()
        });
        on.color = true;
        assert_eq!(on.effective_mode(), ColorMode::TrueColor);
        let frame_on = render_state(&img, &on, Viewport::new(8, 4), &mut fb);
        assert!(
            frame_on
                .cells()
                .iter()
                .any(|c| c.fg.is_some() && c.bg.is_some()),
            "colour on with blocks attaches both a foreground and background colour"
        );
    }

    #[test]
    fn colour_reset_restores_defaults() {
        let mut st =
            ImageState::from_settings(&settings(RenderOpts::default()), ColorMode::TrueColor);
        st.color_menu_open = true;
        st.color = true;
        st.color_mode = ColorMode::Ansi16;
        st.on_key(key(KeyCode::Char('+'))); // bump contrast
        let bumped = st.contrast;
        assert!(bumped > st.base_contrast);
        assert_eq!(st.on_key(key(KeyCode::Char('r'))), ImageAction::Redraw);
        assert!(!st.color, "reset turns colour off");
        assert_eq!(
            st.color_mode,
            ColorMode::TrueColor,
            "reset restores full fidelity"
        );
        assert_eq!(
            st.contrast, st.base_contrast,
            "reset restores the base contrast"
        );
    }

    #[test]
    fn ctrl_c_quits_even_with_the_menu_open() {
        let mut st = state_with(RenderOpts::default());
        st.color_menu_open = true;
        let ctrl_c = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: rgfx_terminal::KeyModifiers {
                ctrl: true,
                ..rgfx_terminal::KeyModifiers::NONE
            },
        };
        assert_eq!(st.on_key(ctrl_c), ImageAction::Quit);
    }

    #[test]
    fn colour_seed_from_settings_enables_colour_at_capability() {
        let opts = RenderOpts {
            color: true,
            ..RenderOpts::default()
        };
        let st = ImageState::from_settings(&settings(opts), ColorMode::Ansi256);
        assert!(st.color, "--color seeds colour on");
        assert_eq!(
            st.color_mode,
            ColorMode::Ansi256,
            "fidelity defaults to the detected capability"
        );
        assert_eq!(st.effective_mode(), ColorMode::Ansi256);
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
