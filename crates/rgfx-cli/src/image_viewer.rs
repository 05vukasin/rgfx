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
//! Colour: the full-screen preview has a modal **color menu** (`C`) that turns ANSI colour on and
//! picks a fidelity ([`ColorMode`]). The chosen mode is clamped to the terminal's detected
//! capability and wired into both the encoder options and the [`FrameEngine`]'s serializer mode
//! (rebuilding the engine on a change forces a clean full redraw). With colour off the grayscale
//! path is byte-identical to before. The non-interactive `--output`/text path stays grayscale:
//! [`TerminalFrame::to_text`] emits glyphs only, so that export is deterministic regardless.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use rgfx_core::{Framebuffer, TerminalEncoder, TerminalFrame, Viewport};
use rgfx_image::{BayerSize, DecodedImage, Dither, Preprocess, RenderOptions, Tone};
use rgfx_terminal::{
    AsciiEncoder, AsciiOptions, BlockEncoder, BlockOptions, BrailleEncoder, BrailleOptions,
    ColorMode, Event, FrameEngine, KeyCode, KeyEvent, detect_color_mode, terminal_size,
};

use crate::cli::{ColorDepth, DitherMode, Renderer};
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
/// Contrast delta applied per `+`/`-` press inside the color menu (reuses the tone stage).
const CONTRAST_STEP: f32 = 0.1;
/// The clamp range for interactively-adjusted contrast, keeping it sane and non-negative.
const CONTRAST_RANGE: (f32, f32) = (0.1, 4.0);

/// The fidelity rank of a [`ColorMode`], used to clamp a requested mode down to a capability.
///
/// Ordered `None < Ansi16 < Ansi256 < TrueColor`, so a larger rank means richer color.
fn color_rank(mode: ColorMode) -> u8 {
    match mode {
        ColorMode::None => 0,
        ColorMode::Ansi16 => 1,
        ColorMode::Ansi256 => 2,
        ColorMode::TrueColor => 3,
    }
}

/// Clamps a `requested` color mode down to the terminal's detected `capability`.
///
/// Requesting a richer mode than the terminal supports (e.g. TrueColor on a 16-color `TERM`)
/// yields the capability, so no unsupported escapes are ever emitted; a request at or below the
/// capability passes through unchanged.
fn clamp_color_mode(requested: ColorMode, capability: ColorMode) -> ColorMode {
    if color_rank(requested) > color_rank(capability) {
        capability
    } else {
        requested
    }
}

/// A short lower-case name for a color mode, for the status bar and menu.
fn color_mode_name(mode: ColorMode) -> &'static str {
    match mode {
        ColorMode::None => "off",
        ColorMode::Ansi16 => "ansi16",
        ColorMode::Ansi256 => "ansi256",
        ColorMode::TrueColor => "truecolor",
    }
}

/// Maps the CLI [`ColorDepth`] seed onto a requested [`ColorMode`].
fn depth_to_mode(depth: ColorDepth) -> ColorMode {
    match depth {
        ColorDepth::Ansi16 => ColorMode::Ansi16,
        ColorDepth::Ansi256 => ColorMode::Ansi256,
        ColorDepth::True => ColorMode::TrueColor,
    }
}

/// The viewer-side color model for the full-screen image preview: whether ANSI color is on, the
/// requested fidelity, and whether the modal color menu is open. Pure state with no terminal
/// dependency, so the menu's key routing and the capability clamp are unit-testable headlessly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ColorMenu {
    /// Whether ANSI color output is enabled (the quick on/off, now inside the menu).
    on: bool,
    /// The requested color fidelity (always one of the three colored modes). The effective mode is
    /// this clamped to the terminal capability at render time.
    mode: ColorMode,
    /// Whether the modal color menu is open (captures input while true).
    menu_open: bool,
}

impl ColorMenu {
    /// The default color model: off, truecolor requested (clamped down at render), menu closed.
    fn new() -> Self {
        Self {
            on: false,
            mode: ColorMode::TrueColor,
            menu_open: false,
        }
    }

    /// Seeds the color model from the CLI/config: `--color` sets it on, `--color-mode` seeds the
    /// requested fidelity (defaulting to truecolor, which clamps down to the terminal at render).
    fn from_settings(settings: &Settings) -> Self {
        Self {
            on: settings.color,
            mode: settings
                .color_mode
                .map_or(ColorMode::TrueColor, depth_to_mode),
            menu_open: false,
        }
    }

    /// Restores the color model to its defaults, leaving the menu open/closed as it was.
    fn reset(&mut self) {
        let menu_open = self.menu_open;
        *self = Self::new();
        self.menu_open = menu_open;
    }

    /// Cycles the requested fidelity: Ansi16 → Ansi256 → TrueColor → Ansi16.
    fn cycle_mode(&mut self) {
        self.mode = match self.mode {
            ColorMode::Ansi16 => ColorMode::Ansi256,
            ColorMode::Ansi256 => ColorMode::TrueColor,
            // TrueColor wraps back to Ansi16; None is not a requestable state, treat it as the start.
            ColorMode::TrueColor | ColorMode::None => ColorMode::Ansi16,
        };
    }

    /// The effective color mode handed to the encoder and the [`FrameEngine`]: [`ColorMode::None`]
    /// when color is off, otherwise the requested mode clamped to the terminal `capability`.
    fn effective_mode(&self, capability: ColorMode) -> ColorMode {
        if self.on {
            clamp_color_mode(self.mode, capability)
        } else {
            ColorMode::None
        }
    }
}

/// What a key press means to the full-screen image preview loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageAction {
    Quit,
    Redraw,
    Ignore,
}

/// The interactive state of the full-screen image preview: the active encoder, dithering, and
/// toggles, plus the base tone carried over from the CLI/config.
struct ImageState {
    renderer: ResolvedRenderer,
    dither: DitherMode,
    invert: bool,
    /// The modal color model (on/off, requested fidelity, menu open).
    color: ColorMenu,
    gamma: f32,
    contrast: f32,
    threshold: f32,
    show_ui: bool,
}

impl ImageState {
    fn from_settings(settings: &Settings) -> Self {
        Self {
            renderer: ResolvedRenderer::resolve(settings.renderer, settings.color),
            dither: settings.dither,
            invert: false,
            color: ColorMenu::from_settings(settings),
            gamma: settings.gamma,
            contrast: settings.contrast,
            threshold: settings.threshold,
            show_ui: true,
        }
    }

    /// The effective color mode for the current state and terminal `capability`, wired identically
    /// into the encoder and the [`FrameEngine`].
    fn effective_color(&self, capability: ColorMode) -> ColorMode {
        self.color.effective_mode(capability)
    }

    /// Adjusts contrast by `delta`, clamped to [`CONTRAST_RANGE`].
    fn adjust_contrast(&mut self, delta: f32) {
        let (lo, hi) = CONTRAST_RANGE;
        self.contrast = (self.contrast + delta).clamp(lo, hi);
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

    fn on_key(&mut self, key: KeyEvent) -> ImageAction {
        // While the color menu is open it captures input: it routes color/tone keys and Esc/C
        // close it. The camera-less image controls (R/D/I/F) are suspended until it closes.
        if self.color.menu_open {
            return self.on_menu_key(key);
        }
        if crate::terminal::is_quit(key) {
            return ImageAction::Quit;
        }
        match key.code {
            KeyCode::Char('r') | KeyCode::Char('R') => self.cycle_renderer(),
            KeyCode::Char('d') | KeyCode::Char('D') => self.cycle_dither(),
            KeyCode::Char('i') | KeyCode::Char('I') => self.invert = !self.invert,
            KeyCode::Char('c') | KeyCode::Char('C') => self.color.menu_open = true,
            KeyCode::Char('f') | KeyCode::Char('F') => self.show_ui = !self.show_ui,
            _ => return ImageAction::Ignore,
        }
        ImageAction::Redraw
    }

    /// Routes a key while the color menu is open. `Ctrl+C` still quits; `Esc`/`C` close the menu;
    /// the remaining keys drive color and the shared tone/renderer stages. Returns the loop action.
    fn on_menu_key(&mut self, key: KeyEvent) -> ImageAction {
        // Ctrl+C always quits, even out of the modal menu.
        if key.modifiers.ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            return ImageAction::Quit;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('C') => {
                self.color.menu_open = false;
            }
            KeyCode::Char('o') | KeyCode::Char('O') | KeyCode::Char(' ') => {
                self.color.on = !self.color.on;
            }
            KeyCode::Char('m') | KeyCode::Char('M') => self.color.cycle_mode(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.adjust_contrast(CONTRAST_STEP),
            KeyCode::Char('-') | KeyCode::Char('_') => self.adjust_contrast(-CONTRAST_STEP),
            // Quick-pick the blocks renderer for the richest fg/bg color (two colors per cell).
            KeyCode::Char('b') | KeyCode::Char('B') => self.renderer = ResolvedRenderer::Blocks,
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.color.reset();
                self.contrast = 1.0;
            }
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

    /// The encoder for the current state at the given effective `color` mode. The mode is already
    /// clamped and gated by on/off (see [`ColorMenu::effective_mode`]); [`ColorMode::None`] keeps
    /// the encoder grayscale, matching the same mode the [`FrameEngine`] serializes at.
    fn encoder(&self, color: ColorMode) -> Box<dyn TerminalEncoder> {
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
/// aspect-correct by the image pipeline, then overlays the options bar and (when open) the color
/// menu. `effective` is the color mode wired into the encoder — the same mode the caller's
/// [`FrameEngine`] serializes at; `capability` is the terminal's detected max (shown in the menu).
fn render_state(
    image: &DecodedImage,
    state: &ImageState,
    bounds: Viewport,
    effective: ColorMode,
    capability: ColorMode,
    fb: &mut Framebuffer,
) -> TerminalFrame {
    let mut opts = state.renderer.render_options();
    opts.preprocess = state.preprocess();
    image.render_into(fb, bounds, &opts);
    let mut frame = state.encoder(effective).encode(fb, bounds);
    if state.show_ui {
        overlay_image_status(&mut frame, image, state, effective);
    }
    // The modal color menu draws over everything (even with the status bar hidden) since the user
    // explicitly opened it.
    if state.color.menu_open {
        overlay_color_menu(&mut frame, state, capability);
    }
    frame
}

/// Draws the two-line options bar (mode info + key help) across the bottom rows of `frame`.
/// `effective` is the color mode actually rendered (`off` when disabled or clamped away).
fn overlay_image_status(
    frame: &mut TerminalFrame,
    image: &DecodedImage,
    state: &ImageState,
    effective: ColorMode,
) {
    let status = format!(
        "{}x{} | {} | dither:{} | color:{} | invert:{}",
        image.width(),
        image.height(),
        state.renderer_name(),
        dither_name(state.dither),
        color_mode_name(effective),
        if state.invert { "on" } else { "off" },
    );
    let help = "R:renderer  D:dither  I:invert  C:color-menu  F:ui  Q:quit";
    crate::viewer_chrome::overlay_bottom_bar(frame, &status, help);
}

/// Draws the modal color-menu panel (on/off, requested + effective mode, terminal max, renderer,
/// contrast, and key help) floating near the top-left of `frame`.
fn overlay_color_menu(frame: &mut TerminalFrame, state: &ImageState, capability: ColorMode) {
    let c = &state.color;
    let effective = c.effective_mode(capability);
    let lines = vec![
        "Color menu".to_string(),
        format!("color:     {}", if c.on { "on" } else { "off" }),
        format!("mode:      {}", color_mode_name(c.mode)),
        format!("shown:     {}", color_mode_name(effective)),
        format!("max:       {}", color_mode_name(capability)),
        format!("renderer:  {}", state.renderer_name()),
        format!("contrast:  {:.2}", state.contrast),
        String::new(),
        "O/Space on/off   M mode".to_string(),
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
    // The terminal's detected color capability: the ceiling every requested mode is clamped to.
    let capability = detect_color_mode();
    let mut state = ImageState::from_settings(settings);
    // The engine's serializer mode must match the encoder's color mode. Build it at the current
    // effective mode; when the menu changes that mode we rebuild the engine (below), which resets
    // its buffers and forces a clean full redraw so stale escapes never linger.
    let mut engine = FrameEngine::new(state.effective_color(capability));
    let mut fb = Framebuffer::new(0, 0);
    let mut viewport = session.viewport()?;
    let mut dirty = true;

    loop {
        if dirty {
            let effective = state.effective_color(capability);
            if engine.mode() != effective {
                // Rebuilding drops the front buffer, so the next render is a full redraw.
                engine = FrameEngine::new(effective);
            }
            let frame = render_state(image, &state, viewport, effective, capability, &mut fb);
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

    #[test]
    fn image_controls_cycle_renderer_dither_and_toggles() {
        let mut st = ImageState::from_settings(&settings(RenderOpts::default()));
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
        let st = ImageState::from_settings(&settings(RenderOpts::default()));
        let mut fb = Framebuffer::new(0, 0);
        let frame = render_state(
            &img,
            &st,
            Viewport::new(80, 12),
            ColorMode::None,
            ColorMode::None,
            &mut fb,
        );
        assert_eq!(frame.cols(), 80);
        assert_eq!(frame.rows(), 12);
        // The bottom row is the key-help line (fits at 80 cols), not blank braille.
        assert!(frame.to_text().lines().last().unwrap().contains("Q:quit"));
    }

    #[test]
    fn c_opens_color_menu_and_routes_keys_then_closes() {
        let mut st = ImageState::from_settings(&settings(RenderOpts::default()));
        assert!(!st.color.menu_open);
        assert!(
            !st.color.on,
            "default is color off (byte-identical grayscale)"
        );

        // C opens the menu (does not toggle color directly).
        assert_eq!(st.on_key(key(KeyCode::Char('c'))), ImageAction::Redraw);
        assert!(st.color.menu_open, "C opens the modal color menu");
        assert!(!st.color.on, "opening the menu must not toggle color on");

        // While open, O toggles color on, M cycles fidelity, B picks the blocks renderer.
        assert_eq!(st.on_key(key(KeyCode::Char('o'))), ImageAction::Redraw);
        assert!(st.color.on, "O turns color on inside the menu");
        let m0 = st.color.mode;
        assert_eq!(st.on_key(key(KeyCode::Char('m'))), ImageAction::Redraw);
        assert_ne!(st.color.mode, m0, "M cycles the requested fidelity");
        assert_eq!(st.on_key(key(KeyCode::Char('b'))), ImageAction::Redraw);
        assert_eq!(
            st.renderer,
            ResolvedRenderer::Blocks,
            "B quick-picks blocks"
        );

        // +/- adjust contrast (the shared tone stage).
        let c0 = st.contrast;
        assert_eq!(st.on_key(key(KeyCode::Char('+'))), ImageAction::Redraw);
        assert!(st.contrast > c0, "+ raises contrast");
        assert_eq!(st.on_key(key(KeyCode::Char('-'))), ImageAction::Redraw);
        assert!((st.contrast - c0).abs() < 1e-6, "- returns contrast");

        // C closes it again; controls resume.
        assert_eq!(st.on_key(key(KeyCode::Char('c'))), ImageAction::Redraw);
        assert!(!st.color.menu_open, "C closes the menu");
    }

    #[test]
    fn closed_menu_leaves_image_controls_and_menu_captures_them() {
        let mut st = ImageState::from_settings(&settings(RenderOpts::default()));
        // Closed: R cycles the renderer as before.
        let start = st.renderer;
        assert_eq!(st.on_key(key(KeyCode::Char('r'))), ImageAction::Redraw);
        assert_ne!(
            st.renderer, start,
            "R cycles the renderer while the menu is closed"
        );

        // Open the menu; now R is the in-menu reset, not the renderer cycle.
        st.on_key(key(KeyCode::Char('c')));
        st.on_key(key(KeyCode::Char('o'))); // color on
        st.on_key(key(KeyCode::Char('m'))); // change fidelity away from default
        st.contrast = 2.0;
        let before = st.renderer;
        assert_eq!(st.on_key(key(KeyCode::Char('r'))), ImageAction::Redraw);
        assert_eq!(
            st.renderer, before,
            "R inside the menu does not cycle the renderer"
        );
        assert!(!st.color.on, "R resets color to its defaults (off)");
        assert_eq!(
            st.color.mode,
            ColorMode::TrueColor,
            "R resets the requested fidelity"
        );
        assert!(
            (st.contrast - 1.0).abs() < 1e-6,
            "R resets contrast to identity"
        );
        assert!(st.color.menu_open, "reset keeps the menu open");
    }

    #[test]
    fn ctrl_c_quits_even_inside_the_menu() {
        let mut st = ImageState::from_settings(&settings(RenderOpts::default()));
        st.on_key(key(KeyCode::Char('c'))); // open
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
    fn effective_mode_is_off_when_disabled_and_clamped_when_on() {
        let mut c = ColorMenu::new();
        // Off → None regardless of the requested mode or capability.
        c.mode = ColorMode::TrueColor;
        assert_eq!(c.effective_mode(ColorMode::TrueColor), ColorMode::None);
        // On, requesting more than the terminal supports → clamped down to the capability.
        c.on = true;
        assert_eq!(c.effective_mode(ColorMode::Ansi16), ColorMode::Ansi16);
        assert_eq!(c.effective_mode(ColorMode::Ansi256), ColorMode::Ansi256);
        assert_eq!(c.effective_mode(ColorMode::TrueColor), ColorMode::TrueColor);
        // A request at or below the capability passes through.
        c.mode = ColorMode::Ansi16;
        assert_eq!(c.effective_mode(ColorMode::TrueColor), ColorMode::Ansi16);
    }

    #[test]
    fn color_mode_seeds_from_cli_flags() {
        // --color --color-mode 256 seeds the menu on at Ansi256.
        let s = settings(RenderOpts {
            color: true,
            color_mode: Some(crate::cli::ColorDepth::Ansi256),
            ..RenderOpts::default()
        });
        let st = ImageState::from_settings(&s);
        assert!(st.color.on);
        assert_eq!(st.color.mode, ColorMode::Ansi256);
        // Without --color-mode the requested fidelity defaults to truecolor.
        let s2 = settings(RenderOpts {
            color: true,
            ..RenderOpts::default()
        });
        assert_eq!(
            ImageState::from_settings(&s2).color.mode,
            ColorMode::TrueColor
        );
    }

    #[test]
    fn color_off_frame_is_byte_identical_to_grayscale() {
        // Colour off must produce exactly the same cells (no fg/bg) as the pre-color grayscale path.
        let img = red_pixel();
        let st = ImageState::from_settings(&settings(RenderOpts::default()));
        let mut fb = Framebuffer::new(0, 0);
        let frame = render_state(
            &img,
            &st,
            Viewport::new(40, 8),
            ColorMode::None,
            ColorMode::TrueColor,
            &mut fb,
        );
        // No cell carries a color when the effective mode is None.
        assert!(
            frame
                .cells()
                .iter()
                .all(|c| c.fg.is_none() && c.bg.is_none()),
            "grayscale output must attach no color"
        );
    }

    #[test]
    fn blocks_renderer_attaches_fg_and_bg_when_color_on() {
        // A colored fixture through the blocks encoder in color mode gives two colors per cell.
        let img = red_pixel();
        let mut st = ImageState::from_settings(&settings(RenderOpts::default()));
        st.renderer = ResolvedRenderer::Blocks;
        st.color.on = true;
        st.show_ui = false; // avoid the status bar overwriting cells with colorless glyphs
        let mut fb = Framebuffer::new(0, 0);
        let frame = render_state(
            &img,
            &st,
            Viewport::new(8, 4),
            ColorMode::TrueColor,
            ColorMode::TrueColor,
            &mut fb,
        );
        let top_left = frame.get(0, 0);
        assert!(top_left.fg.is_some(), "blocks color sets a foreground");
        assert!(
            top_left.bg.is_some(),
            "blocks color sets a background (fg=top, bg=bottom)"
        );

        // The same fixture with color off attaches nothing.
        st.color.on = false;
        let frame_off = render_state(
            &img,
            &st,
            Viewport::new(8, 4),
            ColorMode::None,
            ColorMode::TrueColor,
            &mut fb,
        );
        assert!(frame_off.get(0, 0).fg.is_none());
        assert!(frame_off.get(0, 0).bg.is_none());
    }

    #[test]
    fn requesting_truecolor_on_ansi16_terminal_emits_ansi16_escapes() {
        // End-to-end clamp: request TrueColor but the terminal only supports Ansi16. The engine
        // serializes at the clamped mode, so the bytes carry a 16-color SGR (`9x`), never a
        // truecolor `38;2;` sequence.
        use rgfx_terminal::AnsiSerializer;
        let img = red_pixel();
        let mut st = ImageState::from_settings(&settings(RenderOpts::default()));
        st.renderer = ResolvedRenderer::Blocks;
        st.color.on = true;
        st.color.mode = ColorMode::TrueColor;
        st.show_ui = false;

        let capability = ColorMode::Ansi16;
        let effective = st.effective_color(capability);
        assert_eq!(
            effective,
            ColorMode::Ansi16,
            "TrueColor clamps to the Ansi16 terminal"
        );

        let mut fb = Framebuffer::new(0, 0);
        let frame = render_state(
            &img,
            &st,
            Viewport::new(8, 4),
            effective,
            capability,
            &mut fb,
        );
        let bytes = AnsiSerializer::new(effective).serialize(&frame);
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            !text.contains("38;2;"),
            "no truecolor escapes on an Ansi16 terminal"
        );
        assert!(
            text.contains('\u{1b}'),
            "some SGR escape is emitted for on-color output"
        );
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
