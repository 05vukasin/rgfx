//! The Unicode half-block encoder.
//!
//! [`BlockEncoder`] uses the half-block trick to double vertical resolution: each terminal cell
//! covers a 1×2 stack of framebuffer pixels (a top pixel and a bottom pixel), so the framebuffer
//! should be sized `cols × rows*2` for the target [`Viewport`] (see [`Viewport::render_size`] with
//! `(1, 2)`).
//!
//! # Grayscale glyph selection
//!
//! With color disabled (this task), a cell is rendered purely by its glyph. The encoder samples
//! the top and bottom luminance and picks a glyph that mirrors that vertical split:
//!
//! - a strong top/bottom contrast selects a half-block — [`UPPER_HALF`] (`▀`, top lit) or
//!   [`LOWER_HALF`] (`▄`, bottom lit);
//! - an approximately uniform cell selects a shading glyph from [`SHADE_RAMP`]
//!   (`' '`, `░`, `▒`, `▓`, `█`) by its mean luminance.
//!
//! The two lit halves therefore map cleanly: top-only → `▀`, bottom-only → `▄`, both → `█`,
//! neither → `' '`.
//!
//! # Color (task 006)
//!
//! The half-block convention that task 006 will use is [`UPPER_HALF`] (`▀`) with the foreground set
//! to the *top* pixel color and the background to the *bottom* pixel color, giving independent
//! color per subpixel. [`BlockEncoder::sample_cell`] already returns both pixel colors so the color
//! layer can attach them without reworking the sampling; the grayscale path here just discards them
//! in favour of a glyph.

use crate::color::ColorMode;
use rgfx_core::{Cell, Color, Framebuffer, TerminalEncoder, TerminalFrame, Viewport};

/// Horizontal framebuffer pixels consumed per terminal cell (one).
pub const SUBPIXEL_X: u16 = 1;

/// Vertical framebuffer pixels consumed per terminal cell (two: a top and a bottom).
pub const SUBPIXEL_Y: u16 = 2;

/// Upper half block `▀`; in color mode its foreground is the top pixel, background the bottom.
pub const UPPER_HALF: char = '\u{2580}';
/// Lower half block `▄`.
pub const LOWER_HALF: char = '\u{2584}';
/// Full block `█`.
pub const FULL_BLOCK: char = '\u{2588}';
/// Left half block `▌` (exposed for the color layer; unused by grayscale vertical sampling).
pub const LEFT_HALF: char = '\u{258C}';
/// Right half block `▐` (exposed for the color layer; unused by grayscale vertical sampling).
pub const RIGHT_HALF: char = '\u{2590}';
/// Light shade `░`.
pub const LIGHT_SHADE: char = '\u{2591}';
/// Medium shade `▒`.
pub const MEDIUM_SHADE: char = '\u{2592}';
/// Dark shade `▓`.
pub const DARK_SHADE: char = '\u{2593}';

/// The uniform-cell shading ramp, ordered darkest → lightest:
/// `' '`, [`LIGHT_SHADE`], [`MEDIUM_SHADE`], [`DARK_SHADE`], [`FULL_BLOCK`].
pub const SHADE_RAMP: [char; 5] = [' ', LIGHT_SHADE, MEDIUM_SHADE, DARK_SHADE, FULL_BLOCK];

/// Tunable options for the half-block encoder's grayscale glyph selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockOptions {
    /// When `true`, luminance is flipped (`1.0 - luma`) for both halves before glyph selection.
    /// Defaults to `false`.
    pub invert: bool,
    /// Gamma exponent applied per half as `luma.powf(gamma)`. `1.0` is identity. Defaults to `1.0`.
    pub gamma: f32,
    /// Minimum absolute top/bottom luminance difference that selects a half-block instead of a
    /// uniform shade. Defaults to `0.5`, so a two-tone (black/white) cell always splits.
    pub split_threshold: f32,
    /// The color mode for cell colors. [`ColorMode::None`] (the default) keeps the grayscale
    /// glyph-selection behavior described above. Any other mode switches to the color convention:
    /// the cell always uses [`UPPER_HALF`] with the foreground set to the top pixel color and the
    /// background to the bottom pixel color, giving independent color per vertical subpixel. Final
    /// quantization to 16/256/truecolor happens downstream in the serializer.
    pub color: ColorMode,
}

impl Default for BlockOptions {
    fn default() -> Self {
        Self {
            invert: false,
            gamma: 1.0,
            split_threshold: 0.5,
            color: ColorMode::None,
        }
    }
}

/// A [`TerminalEncoder`] that renders a framebuffer as grayscale Unicode half-blocks.
///
/// Stateless and cheap to clone; construct one per set of [`BlockOptions`] and reuse it.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockEncoder {
    options: BlockOptions,
}

impl BlockEncoder {
    /// Creates an encoder with the default [`BlockOptions`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an encoder with explicit [`BlockOptions`].
    #[must_use]
    pub fn with_options(options: BlockOptions) -> Self {
        Self { options }
    }

    /// The options this encoder was built with.
    #[must_use]
    pub fn options(&self) -> BlockOptions {
        self.options
    }

    /// The top and bottom pixel colors for the cell whose top pixel is `(px, py)`.
    ///
    /// Out-of-bounds pixels read as [`Color::BLACK`]. This is the seam task 006 builds on: the
    /// color encoder emits [`UPPER_HALF`] with `fg = top`, `bg = bottom`.
    #[must_use]
    pub fn sample_cell(&self, frame: &Framebuffer, px: usize, py: usize) -> (Color, Color) {
        let sample = |x: usize, y: usize| {
            if frame.in_bounds(x, y) {
                frame.get(x, y)
            } else {
                Color::BLACK
            }
        };
        (sample(px, py), sample(px, py + 1))
    }

    /// Applies gamma then invert to a raw luminance value.
    fn adjust(&self, luma: f32) -> f32 {
        let mut l = luma.clamp(0.0, 1.0);
        if self.options.gamma != 1.0 {
            l = l.powf(self.options.gamma);
        }
        if self.options.invert {
            l = 1.0 - l;
        }
        l.clamp(0.0, 1.0)
    }

    /// The grayscale glyph for a top/bottom luminance pair (each already `0.0..=1.0`, raw).
    ///
    /// A top/bottom contrast of at least [`BlockOptions::split_threshold`] selects a half-block;
    /// otherwise the mean luminance selects a [`SHADE_RAMP`] glyph.
    #[must_use]
    pub fn glyph_for_halves(&self, top: f32, bottom: f32) -> char {
        let t = self.adjust(top);
        let b = self.adjust(bottom);
        if (t - b).abs() >= self.options.split_threshold {
            if t >= b { UPPER_HALF } else { LOWER_HALF }
        } else {
            let mean = (t + b) * 0.5;
            let last = SHADE_RAMP.len() - 1;
            let idx = (mean * last as f32).round() as usize;
            SHADE_RAMP[idx.min(last)]
        }
    }

    /// Encodes the 1×2 cell whose top pixel is `(px, py)` into one cell.
    ///
    /// In grayscale mode this selects a glyph from the top/bottom luminance. When
    /// [`BlockOptions::color`] is not [`ColorMode::None`], it instead emits [`UPPER_HALF`] with the
    /// top pixel as foreground and the bottom pixel as background, reusing [`sample_cell`] without
    /// touching the sampling math.
    ///
    /// [`sample_cell`]: BlockEncoder::sample_cell
    fn encode_cell(&self, frame: &Framebuffer, px: usize, py: usize) -> Cell {
        let (top, bottom) = self.sample_cell(frame, px, py);
        if self.options.color == ColorMode::None {
            Cell::glyph(self.glyph_for_halves(top.luma(), bottom.luma()))
        } else {
            Cell {
                ch: UPPER_HALF,
                fg: Some(top),
                bg: Some(bottom),
            }
        }
    }
}

impl TerminalEncoder for BlockEncoder {
    fn encode(&self, frame: &Framebuffer, viewport: Viewport) -> TerminalFrame {
        let cols = viewport.cols as usize;
        let rows = viewport.rows as usize;
        let mut out = TerminalFrame::new(cols, rows);
        for row in 0..rows {
            for col in 0..cols {
                let px = col * SUBPIXEL_X as usize;
                let py = row * SUBPIXEL_Y as usize;
                out.set(col, row, self.encode_cell(frame, px, py));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1×2 framebuffer with explicit top/bottom greys.
    fn two_tone(top: f32, bottom: f32) -> Framebuffer {
        let mut fb = Framebuffer::new(1, 2);
        fb.set(0, 0, Color::rgb(top, top, top));
        fb.set(0, 1, Color::rgb(bottom, bottom, bottom));
        fb
    }

    #[test]
    fn two_tone_selects_the_right_half_block() {
        let enc = BlockEncoder::new();
        let vp = Viewport::new(1, 1);
        // top lit, bottom dark → upper half.
        assert_eq!(enc.encode(&two_tone(1.0, 0.0), vp).get(0, 0).ch, UPPER_HALF);
        // bottom lit, top dark → lower half.
        assert_eq!(enc.encode(&two_tone(0.0, 1.0), vp).get(0, 0).ch, LOWER_HALF);
        // both lit → full block.
        assert_eq!(enc.encode(&two_tone(1.0, 1.0), vp).get(0, 0).ch, FULL_BLOCK);
        // neither lit → space.
        assert_eq!(enc.encode(&two_tone(0.0, 0.0), vp).get(0, 0).ch, ' ');
    }

    #[test]
    fn uniform_cell_uses_the_shade_ramp() {
        let enc = BlockEncoder::new();
        // Both halves equal → no split → shade by mean.
        assert_eq!(enc.glyph_for_halves(0.0, 0.0), ' ');
        assert_eq!(enc.glyph_for_halves(1.0, 1.0), FULL_BLOCK);
        // Mean 0.5 → medium shade (index round(0.5*4)=2).
        assert_eq!(enc.glyph_for_halves(0.5, 0.5), MEDIUM_SHADE);
        // Mean 0.25 → light shade (round(0.25*4)=1).
        assert_eq!(enc.glyph_for_halves(0.25, 0.25), LIGHT_SHADE);
        // Mean 0.75 → dark shade (round(0.75*4)=3).
        assert_eq!(enc.glyph_for_halves(0.75, 0.75), DARK_SHADE);
    }

    #[test]
    fn shade_ramp_is_monotonic() {
        let enc = BlockEncoder::new();
        let mut last = 0usize;
        for i in 0..=100 {
            let l = i as f32 / 100.0;
            let ch = enc.glyph_for_halves(l, l);
            let idx = SHADE_RAMP.iter().position(|&c| c == ch).unwrap();
            assert!(idx >= last, "mean {l} gave shade index {idx} < {last}");
            last = idx;
        }
        assert_eq!(last, SHADE_RAMP.len() - 1);
    }

    #[test]
    fn invert_flips_the_split_direction() {
        let enc = BlockEncoder::with_options(BlockOptions {
            invert: true,
            ..Default::default()
        });
        // top lit, bottom dark, inverted → top becomes dark, bottom lit → lower half.
        assert_eq!(enc.glyph_for_halves(1.0, 0.0), LOWER_HALF);
        assert_eq!(enc.glyph_for_halves(0.0, 1.0), UPPER_HALF);
    }

    #[test]
    fn sample_cell_exposes_top_and_bottom_colors_for_color_layer() {
        let enc = BlockEncoder::new();
        let mut fb = Framebuffer::new(1, 2);
        fb.set(0, 0, Color::rgb(1.0, 0.0, 0.0));
        fb.set(0, 1, Color::rgb(0.0, 0.0, 1.0));
        let (top, bottom) = enc.sample_cell(&fb, 0, 0);
        assert_eq!(top, Color::rgb(1.0, 0.0, 0.0));
        assert_eq!(bottom, Color::rgb(0.0, 0.0, 1.0));
    }

    #[test]
    fn out_of_bounds_bottom_pixel_reads_as_black() {
        // A 1×1 framebuffer: the bottom pixel of the cell is out of bounds.
        let enc = BlockEncoder::new();
        let mut fb = Framebuffer::new(1, 1);
        fb.set(0, 0, Color::WHITE);
        // top lit (white), bottom missing (black) → upper half.
        assert_eq!(
            enc.encode(&fb, Viewport::new(1, 1)).get(0, 0).ch,
            UPPER_HALF
        );
    }

    #[test]
    fn output_dimensions_match_viewport() {
        let enc = BlockEncoder::new();
        let vp = Viewport::new(6, 4);
        let (w, h) = vp.render_size(SUBPIXEL_X, SUBPIXEL_Y);
        assert_eq!((w, h), (6, 8));
        let mut fb = Framebuffer::new(w, h);
        fb.clear(Color::WHITE);
        let frame = enc.encode(&fb, vp);
        assert_eq!((frame.cols(), frame.rows()), (6, 4));
        let text = frame.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        for line in lines {
            assert_eq!(line.chars().count(), 6);
        }
    }

    #[test]
    fn color_mode_none_leaves_cells_grayscale() {
        let enc = BlockEncoder::new();
        let cell = enc
            .encode(&two_tone(1.0, 0.0), Viewport::new(1, 1))
            .get(0, 0);
        assert_eq!(cell.ch, UPPER_HALF);
        assert_eq!((cell.fg, cell.bg), (None, None));
    }

    #[test]
    fn color_mode_uses_upper_half_with_top_fg_and_bottom_bg() {
        let mut fb = Framebuffer::new(1, 2);
        fb.set(0, 0, Color::rgb(1.0, 0.0, 0.0));
        fb.set(0, 1, Color::rgb(0.0, 0.0, 1.0));
        let enc = BlockEncoder::with_options(BlockOptions {
            color: ColorMode::TrueColor,
            ..Default::default()
        });
        let cell = enc.encode(&fb, Viewport::new(1, 1)).get(0, 0);
        assert_eq!(cell.ch, UPPER_HALF);
        assert_eq!(cell.fg, Some(Color::rgb(1.0, 0.0, 0.0)));
        assert_eq!(cell.bg, Some(Color::rgb(0.0, 0.0, 1.0)));
    }

    #[test]
    fn checkerboard_snapshot_is_exact() {
        // 2 cols × 1 row viewport → 2×2 framebuffer. Left cell: top white/bottom black → ▀.
        // Right cell: top black/bottom white → ▄.
        let vp = Viewport::new(2, 1);
        let (w, h) = vp.render_size(SUBPIXEL_X, SUBPIXEL_Y);
        assert_eq!((w, h), (2, 2));
        let mut fb = Framebuffer::new(w, h);
        fb.set(0, 0, Color::WHITE);
        fb.set(0, 1, Color::BLACK);
        fb.set(1, 0, Color::BLACK);
        fb.set(1, 1, Color::WHITE);
        let enc = BlockEncoder::new();
        let expected: String = [UPPER_HALF, LOWER_HALF].iter().collect();
        assert_eq!(enc.encode(&fb, vp).to_text(), expected);
    }
}
