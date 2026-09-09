//! The flagship grayscale Braille encoder.
//!
//! [`BrailleEncoder`] maps a [`Framebuffer`] onto a grid of Unicode Braille glyphs
//! (`U+2800..=U+28FF`), packing a 2×4 block of framebuffer pixels into every terminal cell.
//! Each of the eight pixels in a block drives one Braille dot, so a single cell resolves
//! 2 horizontal × 4 vertical subpixels — the highest spatial density any monospace glyph offers.
//!
//! # Pixel-to-dot layout
//!
//! Braille numbers its dots in two columns of four. The bit each dot contributes to the glyph's
//! offset from `U+2800` is fixed by the Unicode standard:
//!
//! ```text
//! position   bit
//!  1 4       dot1 = 0x01   dot4 = 0x08
//!  2 5       dot2 = 0x02   dot5 = 0x10
//!  3 6       dot3 = 0x04   dot6 = 0x20
//!  7 8       dot7 = 0x40   dot8 = 0x80
//! ```
//!
//! Mapping framebuffer pixels (`dx` in `0..2`, `dy` in `0..4`, relative to the cell's top-left)
//! onto those dots gives column 0 the dots `1,2,3,7` and column 1 the dots `4,5,6,8`. A fully lit
//! block is `0xFF` → `U+28FF` (`⣿`); an empty block is `0x00` → `U+2800` (`⠀`).
//!
//! # Grayscale decision
//!
//! A subpixel is *lit* when its adjusted luminance is at least [`BrailleOptions::threshold`].
//! Luminance comes from [`Framebuffer::luma`] (Rec. 601), then gamma, contrast, and optional edge
//! enhancement are applied before the threshold test. [`BrailleOptions::invert`] flips the final
//! lit/unlit decision.

use crate::color::ColorMode;
use rgfx_core::{Cell, Color, Framebuffer, TerminalEncoder, TerminalFrame, Viewport};

/// The base code point of the Braille Patterns Unicode block.
pub const BRAILLE_BASE: u32 = 0x2800;

/// Horizontal framebuffer pixels consumed per terminal cell.
pub const SUBPIXEL_X: u16 = 2;

/// Vertical framebuffer pixels consumed per terminal cell.
pub const SUBPIXEL_Y: u16 = 4;

/// The Braille dot bit for each pixel of a 2×4 cell, indexed `[dx][dy]`.
///
/// Column 0 (`dx == 0`) carries dots 1, 2, 3, 7; column 1 (`dx == 1`) carries dots 4, 5, 6, 8.
const DOT_MASK: [[u8; 4]; 2] = [
    // dy: 0     1     2     3
    [0x01, 0x02, 0x04, 0x40], // dx = 0 → dots 1, 2, 3, 7
    [0x08, 0x10, 0x20, 0x80], // dx = 1 → dots 4, 5, 6, 8
];

/// The Braille glyph for an 8-bit dot mask.
///
/// Bit `n` (value `1 << n`) corresponds to Braille dots in Unicode order, so `mask` is added
/// directly to [`BRAILLE_BASE`]. Every value in `0..=255` yields a valid glyph in the Braille
/// Patterns block, so this never fails.
#[inline]
#[must_use]
pub fn braille_char(mask: u8) -> char {
    // invariant: 0x2800 + 0xFF = 0x28FF is a valid scalar value in the Braille Patterns block.
    char::from_u32(BRAILLE_BASE + mask as u32).expect("braille code point is always valid")
}

/// Tunable options controlling how framebuffer luminance becomes lit/unlit Braille dots.
///
/// The luminance pipeline for each subpixel is, in order: sample [`Framebuffer::luma`], apply
/// [`gamma`](Self::gamma), apply [`contrast`](Self::contrast), add optional
/// [`edge_enhance`](Self::edge_enhance), clamp to `0.0..=1.0`, then compare against
/// [`threshold`](Self::threshold) and finally apply [`invert`](Self::invert).
///
/// An already-boolean or pre-dithered framebuffer (pixels at pure black/white) passes through
/// cleanly: with the defaults, luma `1.0 >= 0.5` lights a dot and `0.0` does not.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrailleOptions {
    /// Luminance at or above which a subpixel is lit, in `0.0..=1.0`. Defaults to `0.5`.
    pub threshold: f32,
    /// When `true`, the final lit/unlit decision is flipped. Defaults to `false`.
    pub invert: bool,
    /// Gamma exponent applied as `luma.powf(gamma)`. `1.0` is identity; values below `1.0`
    /// brighten mid-tones, values above darken them. Defaults to `1.0`.
    pub gamma: f32,
    /// Contrast multiplier applied around mid-grey as `(luma - 0.5) * contrast + 0.5`. `1.0` is
    /// identity; higher values increase contrast. Defaults to `1.0`.
    pub contrast: f32,
    /// Unsharp-mask edge-enhancement strength. `0.0` disables it (the common case and default);
    /// higher values add `strength * (luma - neighbourhood_mean)` to each subpixel, sharpening
    /// edges before the threshold test.
    pub edge_enhance: f32,
    /// The color mode for cell foregrounds. [`ColorMode::None`] (the default) keeps pure grayscale
    /// output — every cell carries only its glyph. Any other mode attaches a per-cell foreground:
    /// the mean color of the block's lit subpixels. The final quantization to 16/256/truecolor is
    /// performed downstream by the serializer, so the raw color is stored on the cell as-is.
    pub color: ColorMode,
}

impl Default for BrailleOptions {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            invert: false,
            gamma: 1.0,
            contrast: 1.0,
            edge_enhance: 0.0,
            color: ColorMode::None,
        }
    }
}

/// A [`TerminalEncoder`] that renders a framebuffer as monochrome Braille art.
///
/// The encoder is stateless and cheap to clone; construct one per set of [`BrailleOptions`] and
/// reuse it across frames. It expects the framebuffer passed to [`encode`](TerminalEncoder::encode)
/// to be sized `cols * 2 × rows * 4` for the target [`Viewport`] (see [`Viewport::render_size`]),
/// but tolerates a mismatch by treating out-of-bounds subpixels as unlit.
#[derive(Clone, Copy, Debug, Default)]
pub struct BrailleEncoder {
    options: BrailleOptions,
}

impl BrailleEncoder {
    /// Creates an encoder with the default [`BrailleOptions`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an encoder with explicit [`BrailleOptions`].
    #[must_use]
    pub fn with_options(options: BrailleOptions) -> Self {
        Self { options }
    }

    /// The options this encoder was built with.
    #[must_use]
    pub fn options(&self) -> BrailleOptions {
        self.options
    }

    /// The adjusted luminance of a single framebuffer pixel, or `0.0` when out of bounds.
    ///
    /// Applies gamma, contrast, and edge enhancement, then clamps to `0.0..=1.0`. This is the
    /// value compared against the threshold.
    fn adjusted_luma(&self, frame: &Framebuffer, x: usize, y: usize) -> f32 {
        if !frame.in_bounds(x, y) {
            return 0.0;
        }
        let mut luma = frame.luma(x, y);
        if self.options.edge_enhance != 0.0 {
            luma += self.options.edge_enhance * (luma - neighbourhood_mean(frame, x, y));
        }
        // Gamma is only defined for a non-negative base; clamp first so a sharpened value below
        // zero does not produce a NaN from `powf`.
        luma = luma.clamp(0.0, 1.0);
        if self.options.gamma != 1.0 {
            luma = luma.powf(self.options.gamma);
        }
        if self.options.contrast != 1.0 {
            luma = (luma - 0.5) * self.options.contrast + 0.5;
        }
        luma.clamp(0.0, 1.0)
    }

    /// Whether the subpixel at framebuffer `(x, y)` should be a lit Braille dot.
    fn is_lit(&self, frame: &Framebuffer, x: usize, y: usize) -> bool {
        let lit = self.adjusted_luma(frame, x, y) >= self.options.threshold;
        lit ^ self.options.invert
    }

    /// Encodes the 2×4 block whose top-left framebuffer pixel is `(px, py)` into one cell.
    ///
    /// The dot math is unchanged from the grayscale path. When [`BrailleOptions::color`] is not
    /// [`ColorMode::None`], the cell additionally carries a foreground: the mean color of the lit
    /// subpixels in the block (a block with no lit dots keeps the terminal-default foreground).
    fn encode_cell(&self, frame: &Framebuffer, px: usize, py: usize) -> Cell {
        let mut mask = 0u8;
        let mut sum = (0.0f32, 0.0f32, 0.0f32);
        let mut lit_count = 0u32;
        for (dx, column) in DOT_MASK.iter().enumerate() {
            for (dy, &bit) in column.iter().enumerate() {
                let (x, y) = (px + dx, py + dy);
                if self.is_lit(frame, x, y) {
                    mask |= bit;
                    if self.options.color != ColorMode::None && frame.in_bounds(x, y) {
                        let c = frame.get(x, y);
                        sum = (sum.0 + c.r, sum.1 + c.g, sum.2 + c.b);
                        lit_count += 1;
                    }
                }
            }
        }
        let ch = braille_char(mask);
        if self.options.color == ColorMode::None || lit_count == 0 {
            Cell::glyph(ch)
        } else {
            let n = lit_count as f32;
            Cell::colored(ch, Color::rgb(sum.0 / n, sum.1 / n, sum.2 / n))
        }
    }
}

/// The mean luminance of the 4-connected neighbourhood of `(x, y)`, edges clamped.
fn neighbourhood_mean(frame: &Framebuffer, x: usize, y: usize) -> f32 {
    let clamp_x = |v: isize| v.clamp(0, frame.width() as isize - 1) as usize;
    let clamp_y = |v: isize| v.clamp(0, frame.height() as isize - 1) as usize;
    let (xi, yi) = (x as isize, y as isize);
    let sum = frame.luma(clamp_x(xi - 1), y)
        + frame.luma(clamp_x(xi + 1), y)
        + frame.luma(x, clamp_y(yi - 1))
        + frame.luma(x, clamp_y(yi + 1));
    sum * 0.25
}

impl TerminalEncoder for BrailleEncoder {
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
    use rgfx_core::Color;

    /// Fills the whole framebuffer with a single luma value via a grey color.
    fn solid(width: usize, height: usize, luma: f32) -> Framebuffer {
        let mut fb = Framebuffer::new(width, height);
        fb.clear(Color::rgb(luma, luma, luma));
        fb
    }

    #[test]
    fn full_block_is_u28ff_and_empty_is_u2800() {
        let enc = BrailleEncoder::new();
        let vp = Viewport::new(1, 1);

        let full = enc.encode(&solid(2, 4, 1.0), vp);
        assert_eq!(full.get(0, 0).ch, '⣿');
        assert_eq!(full.get(0, 0).ch as u32, 0x28FF);

        let empty = enc.encode(&solid(2, 4, 0.0), vp);
        assert_eq!(empty.get(0, 0).ch, '⠀');
        assert_eq!(empty.get(0, 0).ch as u32, 0x2800);
    }

    #[test]
    fn braille_char_covers_the_block() {
        assert_eq!(braille_char(0x00), '⠀');
        assert_eq!(braille_char(0xFF), '⣿');
        // Round-trips: the glyph offset from the base equals the mask.
        for mask in 0..=255u8 {
            assert_eq!(braille_char(mask) as u32 - BRAILLE_BASE, mask as u32);
        }
    }

    #[test]
    fn per_dot_mask_table_matches_unicode() {
        // (dx, dy) -> expected mask bit, per the Unicode dot numbering.
        let cases = [
            (0, 0, 0x01u8), // dot 1
            (0, 1, 0x02),   // dot 2
            (0, 2, 0x04),   // dot 3
            (0, 3, 0x40),   // dot 7
            (1, 0, 0x08),   // dot 4
            (1, 1, 0x10),   // dot 5
            (1, 2, 0x20),   // dot 6
            (1, 3, 0x80),   // dot 8
        ];
        let enc = BrailleEncoder::new();
        let vp = Viewport::new(1, 1);
        for (dx, dy, bit) in cases {
            let mut fb = solid(2, 4, 0.0);
            fb.set(dx, dy, Color::WHITE);
            let frame = enc.encode(&fb, vp);
            let expected = braille_char(bit);
            assert_eq!(
                frame.get(0, 0).ch,
                expected,
                "pixel ({dx},{dy}) should set bit {bit:#04x}"
            );
            // DOT_MASK constant agrees with the observed bit.
            assert_eq!(DOT_MASK[dx][dy], bit);
        }
    }

    #[test]
    fn threshold_boundary_is_inclusive() {
        let opts = BrailleOptions {
            threshold: 0.5,
            ..Default::default()
        };
        let enc = BrailleEncoder::with_options(opts);
        let vp = Viewport::new(1, 1);

        // Exactly at threshold → lit (>=).
        assert_eq!(enc.encode(&solid(2, 4, 0.5), vp).get(0, 0).ch, '⣿');
        // Just below → unlit.
        assert_eq!(enc.encode(&solid(2, 4, 0.4999), vp).get(0, 0).ch, '⠀');
    }

    #[test]
    fn invert_flips_lit_and_unlit() {
        let opts = BrailleOptions {
            invert: true,
            ..Default::default()
        };
        let enc = BrailleEncoder::with_options(opts);
        let vp = Viewport::new(1, 1);

        // Bright block, inverted → empty.
        assert_eq!(enc.encode(&solid(2, 4, 1.0), vp).get(0, 0).ch, '⠀');
        // Dark block, inverted → full.
        assert_eq!(enc.encode(&solid(2, 4, 0.0), vp).get(0, 0).ch, '⣿');
    }

    #[test]
    fn threshold_on_a_gradient_moves_the_lit_boundary() {
        // A 2-wide, 4-tall column with a vertical luma ramp: rows 0..4 → 0.1, 0.4, 0.6, 0.9.
        let lumas = [0.1f32, 0.4, 0.6, 0.9];
        let mut fb = Framebuffer::new(2, 4);
        for (y, &l) in lumas.iter().enumerate() {
            fb.set(0, y, Color::rgb(l, l, l));
            fb.set(1, y, Color::rgb(l, l, l));
        }
        let vp = Viewport::new(1, 1);

        // threshold 0.5 → rows 2,3 lit in both columns: dots 3,7 (col0) and 6,8 (col1).
        let mid = BrailleEncoder::with_options(BrailleOptions {
            threshold: 0.5,
            ..Default::default()
        });
        let expected_mid = 0x04 | 0x40 | 0x20 | 0x80;
        assert_eq!(mid.encode(&fb, vp).get(0, 0).ch, braille_char(expected_mid));

        // threshold 0.05 → every row lit.
        let low = BrailleEncoder::with_options(BrailleOptions {
            threshold: 0.05,
            ..Default::default()
        });
        assert_eq!(low.encode(&fb, vp).get(0, 0).ch, '⣿');

        // threshold 0.95 → nothing lit.
        let high = BrailleEncoder::with_options(BrailleOptions {
            threshold: 0.95,
            ..Default::default()
        });
        assert_eq!(high.encode(&fb, vp).get(0, 0).ch, '⠀');
    }

    #[test]
    fn gamma_and_contrast_are_identity_at_one() {
        let a = BrailleEncoder::new();
        let b = BrailleEncoder::with_options(BrailleOptions {
            gamma: 1.0,
            contrast: 1.0,
            ..Default::default()
        });
        let vp = Viewport::new(1, 1);
        let fb = solid(2, 4, 0.7);
        assert_eq!(
            a.encode(&fb, vp).get(0, 0).ch,
            b.encode(&fb, vp).get(0, 0).ch
        );
    }

    #[test]
    fn contrast_pushes_values_away_from_mid_grey() {
        // Luma 0.6 is below threshold after (0.6-0.5)*contrast+0.5 only when contrast lifts it.
        // With high contrast, 0.6 → 0.5 + 0.1*3 = 0.8 (lit); 0.4 → 0.2 (unlit).
        let enc = BrailleEncoder::with_options(BrailleOptions {
            contrast: 3.0,
            threshold: 0.5,
            ..Default::default()
        });
        let vp = Viewport::new(1, 1);
        assert_eq!(enc.encode(&solid(2, 4, 0.6), vp).get(0, 0).ch, '⣿');
        assert_eq!(enc.encode(&solid(2, 4, 0.4), vp).get(0, 0).ch, '⠀');
    }

    #[test]
    fn output_dimensions_match_viewport() {
        let enc = BrailleEncoder::new();
        let vp = Viewport::new(7, 3);
        let (w, h) = vp.render_size(SUBPIXEL_X, SUBPIXEL_Y);
        let frame = enc.encode(&solid(w, h, 1.0), vp);
        assert_eq!(frame.cols(), 7);
        assert_eq!(frame.rows(), 3);
        // Line count and per-line char count of the text form match the viewport.
        let text = frame.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        for line in lines {
            assert_eq!(line.chars().count(), 7);
        }
    }

    #[test]
    fn undersized_framebuffer_treats_missing_pixels_as_unlit() {
        // Viewport wants 2×4 pixels but the framebuffer only supplies the top-left pixel.
        let enc = BrailleEncoder::new();
        let mut fb = Framebuffer::new(1, 1);
        fb.set(0, 0, Color::WHITE);
        let frame = enc.encode(&fb, Viewport::new(1, 1));
        // Only dot 1 (0x01) is in bounds and lit.
        assert_eq!(frame.get(0, 0).ch, braille_char(0x01));
    }

    #[test]
    fn color_mode_none_leaves_cells_grayscale() {
        let enc = BrailleEncoder::new();
        let cell = enc.encode(&solid(2, 4, 1.0), Viewport::new(1, 1)).get(0, 0);
        assert_eq!(cell.fg, None);
        assert_eq!(cell.bg, None);
    }

    #[test]
    fn color_mode_attaches_mean_of_lit_subpixels() {
        // A 2×4 block: top row lit red, everything else black. Only the lit (red) pixels feed the
        // mean, so the foreground is pure red regardless of the dark pixels.
        let mut fb = solid(2, 4, 0.0);
        fb.set(0, 0, Color::rgb(1.0, 0.0, 0.0));
        fb.set(1, 0, Color::rgb(1.0, 0.0, 0.0));
        // A low threshold so the red pixels (luma 0.299) count as lit.
        let enc = BrailleEncoder::with_options(BrailleOptions {
            threshold: 0.1,
            color: ColorMode::TrueColor,
            ..Default::default()
        });
        let cell = enc.encode(&fb, Viewport::new(1, 1)).get(0, 0);
        assert_eq!(cell.fg, Some(Color::rgb(1.0, 0.0, 0.0)));
        // Glyph math is untouched: dots 1 and 4 lit → mask 0x09.
        assert_eq!(cell.ch, braille_char(0x09));
    }

    #[test]
    fn color_mode_with_no_lit_dots_keeps_default_foreground() {
        let enc = BrailleEncoder::with_options(BrailleOptions {
            color: ColorMode::TrueColor,
            ..Default::default()
        });
        let cell = enc.encode(&solid(2, 4, 0.0), Viewport::new(1, 1)).get(0, 0);
        assert_eq!(cell.ch, '⠀');
        assert_eq!(cell.fg, None);
    }

    #[test]
    fn diagonal_snapshot_is_exact() {
        // A 2-cell-wide, 1-cell-tall viewport → 4×4 pixel framebuffer. Draw the main diagonal.
        let vp = Viewport::new(2, 1);
        let (w, h) = vp.render_size(SUBPIXEL_X, SUBPIXEL_Y);
        assert_eq!((w, h), (4, 4));
        let mut fb = Framebuffer::new(w, h);
        for i in 0..4 {
            fb.set(i, i, Color::WHITE);
        }
        let enc = BrailleEncoder::new();
        let text = enc.encode(&fb, vp).to_text();

        // Left cell: lit at (0,0)=dot1 0x01 and (1,1)=dot5 0x10 → mask 0x11.
        // Right cell: pixel (2,2)→local(0,2)=dot3 0x04 and (3,3)→local(1,3)=dot8 0x80 → 0x84.
        let expected: String = [braille_char(0x11), braille_char(0x84)].iter().collect();
        assert_eq!(text, expected);
        // Sanity: exact code points (U+2811 and U+2884).
        let cps: Vec<u32> = text.chars().map(|c| c as u32).collect();
        assert_eq!(cps, vec![0x2811, 0x2884]);
    }
}
