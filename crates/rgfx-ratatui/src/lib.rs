//! `rgfx-ratatui`: embed an rgfx [`Framebuffer`] inside a [`ratatui`] layout.
//!
//! The one architectural law of rgfx is that every media source converges onto a
//! [`Framebuffer`] and only the terminal layer turns that framebuffer into cells. This crate is
//! that terminal layer for host applications that already drive their own `ratatui` UI: instead
//! of owning the whole screen, an rgfx viewport becomes just another [`ratatui::widgets::Widget`]
//! placed into a [`Rect`] of the host's layout.
//!
//! It does **not** fork the encoders. You bring any [`TerminalEncoder`] from `rgfx-terminal`
//! (the flagship [`BrailleEncoder`](rgfx_terminal::BrailleEncoder), or the ASCII / block
//! encoders) plus a rendered [`Framebuffer`]; the widget encodes the framebuffer into a
//! [`TerminalFrame`] for the widget's [`Rect`] and copies each [`Cell`](rgfx_core::Cell) — glyph and, optionally,
//! quantized color — into the `ratatui` [`Buffer`].
//!
//! # Sizing
//!
//! Terminal-cell dimensions are deliberately separate from framebuffer pixel dimensions. A cell
//! covers `subpixel_x × subpixel_y` framebuffer pixels (2×4 for Braille, 1×2 for half-blocks,
//! 1×1 for ASCII). Before rendering, size your framebuffer for the widget's [`Rect`] with
//! [`render_size`] (or [`RgfxWidget::render_size`]) so the encoder maps pixels onto cells
//! one-to-one. The widget tolerates a size mismatch — it clips to the target [`Rect`] and the
//! encoders treat out-of-bounds subpixels as unlit — but a matched size renders exactly.
//!
//! # Color
//!
//! rgfx encoders attach a raw [`rgfx_core::Color`] to a cell only when their own color mode is
//! enabled; grayscale output carries no color. This widget takes a [`ColorMode`] describing the
//! terminal's fidelity and quantizes each cell's raw color to a `ratatui` [`RatColor`] with
//! [`to_ratatui_color`]. With [`ColorMode::None`] no color is set and the host's default style
//! shows through.
//!
//! # Example
//!
//! ```
//! use ratatui::buffer::Buffer;
//! use ratatui::layout::Rect;
//! use ratatui::widgets::Widget;
//! use rgfx_core::{Color, Framebuffer};
//! use rgfx_terminal::BrailleEncoder;
//! use rgfx_ratatui::RgfxWidget;
//!
//! // A 2×4 white framebuffer fills exactly one Braille cell.
//! let mut fb = Framebuffer::new(2, 4);
//! fb.clear(Color::WHITE);
//! let encoder = BrailleEncoder::new();
//!
//! let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
//! RgfxWidget::new(&encoder, &fb).render(Rect::new(0, 0, 1, 1), &mut buf);
//! assert_eq!(buf[(0, 0)].symbol(), "\u{28FF}"); // ⣿ — every dot lit
//! ```
#![warn(missing_docs)]
#![forbid(unsafe_code)]

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color as RatColor;
use ratatui::widgets::{StatefulWidget, Widget};

use rgfx_core::{Framebuffer, TerminalEncoder, TerminalFrame, Viewport};
use rgfx_terminal::{AnsiColor, ColorMode, SUBPIXEL_X, SUBPIXEL_Y};

// Re-export the ratatui color type under a clear name for downstream callers of
// [`to_ratatui_color`] who don't already import it.
pub use ratatui::style::Color as RatatuiColor;

/// Maps an rgfx [`rgfx_core::Color`] to a `ratatui` [`RatColor`] at the given [`ColorMode`].
///
/// Quantization reuses `rgfx-terminal`'s [`AnsiColor::quantize`] so the color a cell shows inside
/// a `ratatui` layout matches what the standalone rgfx serializer would emit:
///
/// - [`ColorMode::None`] → `None` (leave the cell's existing/default color untouched);
/// - [`ColorMode::Ansi16`] / [`ColorMode::Ansi256`] → [`RatColor::Indexed`] into the palette;
/// - [`ColorMode::TrueColor`] → [`RatColor::Rgb`] with the exact 24-bit value.
#[must_use]
pub fn to_ratatui_color(color: rgfx_core::Color, mode: ColorMode) -> Option<RatColor> {
    AnsiColor::quantize(color, mode).map(|c| match c {
        AnsiColor::Ansi16(i) => RatColor::Indexed(i),
        AnsiColor::Ansi256(i) => RatColor::Indexed(i),
        AnsiColor::Rgb(r, g, b) => RatColor::Rgb(r, g, b),
    })
}

/// The framebuffer pixel size that fills a widget's [`Rect`] for the given subpixel factors.
///
/// Use `(2, 4)` for Braille, `(1, 2)` for half-blocks, and `(1, 1)` for ASCII. Size your
/// [`Framebuffer`] to this before rendering into it so the encoder maps its subpixels onto the
/// widget's cells exactly.
#[must_use]
pub fn render_size(area: Rect, subpixel_x: u16, subpixel_y: u16) -> (usize, usize) {
    Viewport::new(area.width, area.height).render_size(subpixel_x, subpixel_y)
}

/// Copies an encoded [`TerminalFrame`] into `buf`, clipped to `area`.
///
/// Glyphs are always written; foreground/background colors are written only when the source cell
/// carries a color and `mode` is not [`ColorMode::None`].
fn blit(frame: &TerminalFrame, mode: ColorMode, area: Rect, buf: &mut Buffer) {
    let cols = frame.cols().min(area.width as usize);
    let rows = frame.rows().min(area.height as usize);
    for row in 0..rows {
        for col in 0..cols {
            let src = frame.get(col, row);
            let x = area.x + col as u16;
            let y = area.y + row as u16;
            let Some(dst) = buf.cell_mut((x, y)) else {
                continue;
            };
            dst.set_char(src.ch);
            if let Some(fg) = src.fg.and_then(|c| to_ratatui_color(c, mode)) {
                dst.set_fg(fg);
            }
            if let Some(bg) = src.bg.and_then(|c| to_ratatui_color(c, mode)) {
                dst.set_bg(bg);
            }
        }
    }
}

/// A [`ratatui`] widget that renders a borrowed rgfx [`Framebuffer`] through a
/// [`TerminalEncoder`] into the widget's [`Rect`].
///
/// The widget borrows both the encoder and the framebuffer, so it is cheap to construct per frame
/// and never allocates a framebuffer of its own. Configure the terminal color fidelity with
/// [`color_mode`](Self::color_mode) and, if you render at a non-Braille density, the subpixel
/// factor with [`subpixel`](Self::subpixel) (used only by [`render_size`](Self::render_size)).
///
/// See the [crate-level example](crate) for end-to-end usage.
#[derive(Clone, Copy, Debug)]
pub struct RgfxWidget<'a, E: TerminalEncoder> {
    encoder: &'a E,
    framebuffer: &'a Framebuffer,
    color_mode: ColorMode,
    subpixel: (u16, u16),
}

impl<'a, E: TerminalEncoder> RgfxWidget<'a, E> {
    /// Creates a widget that renders `framebuffer` through `encoder`.
    ///
    /// The default color mode is [`ColorMode::None`] (grayscale glyphs) and the default subpixel
    /// factor is Braille's `2×4`. Adjust both with the builder methods.
    #[must_use]
    pub fn new(encoder: &'a E, framebuffer: &'a Framebuffer) -> Self {
        Self {
            encoder,
            framebuffer,
            color_mode: ColorMode::None,
            subpixel: (SUBPIXEL_X, SUBPIXEL_Y),
        }
    }

    /// Sets the terminal color fidelity used to quantize each cell's color.
    #[must_use]
    pub fn color_mode(mut self, mode: ColorMode) -> Self {
        self.color_mode = mode;
        self
    }

    /// Sets the encoder's subpixel factor, used only by [`render_size`](Self::render_size).
    ///
    /// Use `(2, 4)` for Braille, `(1, 2)` for half-blocks, `(1, 1)` for ASCII.
    #[must_use]
    pub fn subpixel(mut self, subpixel_x: u16, subpixel_y: u16) -> Self {
        self.subpixel = (subpixel_x, subpixel_y);
        self
    }

    /// The framebuffer pixel size that fills `area` for this widget's subpixel factor.
    ///
    /// Size the framebuffer to this before rendering into it. Equivalent to
    /// [`render_size(area, subpixel_x, subpixel_y)`](render_size) with this widget's factor.
    #[must_use]
    pub fn render_size(&self, area: Rect) -> (usize, usize) {
        render_size(area, self.subpixel.0, self.subpixel.1)
    }
}

impl<E: TerminalEncoder> Widget for RgfxWidget<'_, E> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let frame = self
            .encoder
            .encode(self.framebuffer, Viewport::new(area.width, area.height));
        blit(&frame, self.color_mode, area, buf);
    }
}

// Rendering does not consume the borrows, so a shared reference is a widget too. This lets a host
// keep the `RgfxWidget` and render it into several areas without rebuilding it.
impl<E: TerminalEncoder> Widget for &RgfxWidget<'_, E> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let frame = self
            .encoder
            .encode(self.framebuffer, Viewport::new(area.width, area.height));
        blit(&frame, self.color_mode, area, buf);
    }
}

/// A [`StatefulWidget`] whose state is the [`Framebuffer`] to render.
///
/// This is the ownership-friendly counterpart to [`RgfxWidget`]: the widget borrows only the
/// encoder, and the host passes its long-lived, reused framebuffer as the render state. Use it
/// with [`ratatui::Frame::render_stateful_widget`] when you keep one framebuffer across frames
/// and don't want to thread its borrow through widget construction.
///
/// The framebuffer is taken as `&mut` to satisfy the [`StatefulWidget`] contract, but rendering
/// only reads it.
#[derive(Clone, Copy, Debug)]
pub struct RgfxStatefulWidget<'a, E: TerminalEncoder> {
    encoder: &'a E,
    color_mode: ColorMode,
}

impl<'a, E: TerminalEncoder> RgfxStatefulWidget<'a, E> {
    /// Creates a stateful widget rendering through `encoder`, defaulting to [`ColorMode::None`].
    #[must_use]
    pub fn new(encoder: &'a E) -> Self {
        Self {
            encoder,
            color_mode: ColorMode::None,
        }
    }

    /// Sets the terminal color fidelity used to quantize each cell's color.
    #[must_use]
    pub fn color_mode(mut self, mode: ColorMode) -> Self {
        self.color_mode = mode;
        self
    }
}

impl<E: TerminalEncoder> StatefulWidget for RgfxStatefulWidget<'_, E> {
    type State = Framebuffer;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.is_empty() {
            return;
        }
        let frame = self
            .encoder
            .encode(state, Viewport::new(area.width, area.height));
        blit(&frame, self.color_mode, area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rgfx_core::Color;
    use rgfx_terminal::{BlockEncoder, BrailleEncoder, BrailleOptions};

    /// A framebuffer of `w × h` cleared to a single color.
    fn solid(w: usize, h: usize, c: Color) -> Framebuffer {
        let mut fb = Framebuffer::new(w, h);
        fb.clear(c);
        fb
    }

    #[test]
    fn render_size_maps_rect_by_subpixel_factor() {
        let area = Rect::new(0, 0, 100, 40);
        assert_eq!(render_size(area, 2, 4), (200, 160)); // Braille
        assert_eq!(render_size(area, 1, 2), (100, 80)); // half-blocks
        assert_eq!(render_size(area, 1, 1), (100, 40)); // ASCII
        // The widget helper agrees with the free function for the default (Braille) factor.
        let fb = Framebuffer::new(0, 0);
        let enc = BrailleEncoder::new();
        assert_eq!(RgfxWidget::new(&enc, &fb).render_size(area), (200, 160));
    }

    #[test]
    fn full_white_block_renders_the_full_braille_glyph() {
        let fb = solid(2, 4, Color::WHITE);
        let enc = BrailleEncoder::new();
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        RgfxWidget::new(&enc, &fb).render(area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "\u{28FF}"); // ⣿
    }

    #[test]
    fn empty_black_block_renders_the_empty_braille_glyph() {
        let fb = solid(2, 4, Color::BLACK);
        let enc = BrailleEncoder::new();
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        RgfxWidget::new(&enc, &fb).render(area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "\u{2800}"); // ⠀
    }

    #[test]
    fn grayscale_mode_leaves_colors_at_default() {
        // Even a colored framebuffer stays uncolored when the encoder is grayscale.
        let fb = solid(2, 4, Color::rgb(1.0, 0.0, 0.0));
        let enc = BrailleEncoder::new(); // BrailleOptions::color defaults to None
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        RgfxWidget::new(&enc, &fb)
            .color_mode(ColorMode::TrueColor)
            .render(area, &mut buf);
        assert_eq!(buf[(0, 0)].fg, RatColor::Reset);
        assert_eq!(buf[(0, 0)].bg, RatColor::Reset);
    }

    #[test]
    fn truecolor_cell_carries_exact_rgb() {
        // A color-enabled encoder attaches the block mean color; the widget quantizes it.
        // Yellow has luma ~0.886, comfortably above the default 0.5 threshold, so every dot lights.
        let fb = solid(2, 4, Color::rgb(1.0, 1.0, 0.0));
        let enc = BrailleEncoder::with_options(BrailleOptions {
            color: ColorMode::TrueColor,
            ..Default::default()
        });
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        RgfxWidget::new(&enc, &fb)
            .color_mode(ColorMode::TrueColor)
            .render(area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "\u{28FF}");
        assert_eq!(buf[(0, 0)].fg, RatColor::Rgb(255, 255, 0));
    }

    #[test]
    fn render_is_clipped_to_the_target_rect() {
        // A 2×2 cell frame drawn into a 1×1 area writes only the top-left cell; the rest of a
        // larger buffer is untouched.
        let fb = solid(4, 8, Color::WHITE); // 2×2 Braille cells
        let enc = BrailleEncoder::new();
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 3));
        // Render into a 1×1 sub-rect offset into the buffer.
        RgfxWidget::new(&enc, &fb).render(Rect::new(1, 1, 1, 1), &mut buf);
        assert_eq!(buf[(1, 1)].symbol(), "\u{28FF}");
        // Neighbours outside the target rect stay blank.
        assert_eq!(buf[(0, 0)].symbol(), " ");
        assert_eq!(buf[(2, 1)].symbol(), " ");
        assert_eq!(buf[(1, 2)].symbol(), " ");
    }

    #[test]
    fn writes_at_the_rect_offset() {
        let fb = solid(2, 4, Color::WHITE);
        let enc = BrailleEncoder::new();
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 5));
        RgfxWidget::new(&enc, &fb).render(Rect::new(3, 2, 1, 1), &mut buf);
        assert_eq!(buf[(3, 2)].symbol(), "\u{28FF}");
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn zero_area_renders_nothing() {
        let fb = solid(2, 4, Color::WHITE);
        let enc = BrailleEncoder::new();
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 2));
        RgfxWidget::new(&enc, &fb).render(Rect::new(0, 0, 0, 0), &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn stateful_widget_renders_from_state_framebuffer() {
        let mut fb = solid(2, 4, Color::WHITE);
        let enc = BrailleEncoder::new();
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        StatefulWidget::render(RgfxStatefulWidget::new(&enc), area, &mut buf, &mut fb);
        assert_eq!(buf[(0, 0)].symbol(), "\u{28FF}");
    }

    #[test]
    fn shared_reference_is_renderable() {
        let fb = solid(2, 4, Color::WHITE);
        let enc = BrailleEncoder::new();
        let widget = RgfxWidget::new(&enc, &fb);
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        (&widget).render(area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "\u{28FF}");
    }

    #[test]
    fn to_ratatui_color_maps_each_mode() {
        let red = Color::rgb(1.0, 0.0, 0.0);
        assert_eq!(to_ratatui_color(red, ColorMode::None), None);
        assert_eq!(
            to_ratatui_color(red, ColorMode::TrueColor),
            Some(RatColor::Rgb(255, 0, 0))
        );
        // 16- and 256-color modes resolve to palette indices.
        assert!(matches!(
            to_ratatui_color(red, ColorMode::Ansi16),
            Some(RatColor::Indexed(_))
        ));
        assert!(matches!(
            to_ratatui_color(red, ColorMode::Ansi256),
            Some(RatColor::Indexed(_))
        ));
    }

    #[test]
    fn block_encoder_also_works_through_the_widget() {
        // Proves the widget is encoder-agnostic (not Braille-specific): a half-block encoder at
        // 1×2 subpixels renders a white top / black bottom cell as the upper-half glyph.
        let mut fb = Framebuffer::new(1, 2);
        fb.set(0, 0, Color::WHITE);
        fb.set(0, 1, Color::BLACK);
        let enc = BlockEncoder::new();
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        RgfxWidget::new(&enc, &fb)
            .subpixel(1, 2)
            .color_mode(ColorMode::TrueColor)
            .render(area, &mut buf);
        // The block encoder emits a non-space glyph for a half-lit cell.
        assert_ne!(buf[(0, 0)].symbol(), " ");
    }
}
