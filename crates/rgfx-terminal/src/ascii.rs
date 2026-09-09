//! The ASCII density encoder.
//!
//! [`AsciiEncoder`] maps each terminal cell to a single character chosen from a *density ramp* —
//! an ordered string of glyphs from darkest to lightest (the default is [`DEFAULT_RAMP`],
//! `" .:-=+*#%@"`). It samples one framebuffer pixel per cell (a 1×1 subpixel ratio), so the
//! framebuffer should be sized `cols × rows` for the target [`Viewport`] (see
//! [`Viewport::render_size`] with `(1, 1)`).
//!
//! # Luminance pipeline
//!
//! For each cell the encoder reads [`Framebuffer::luma`], applies [`AsciiOptions::gamma`], applies
//! [`AsciiOptions::invert`], then maps the resulting `0.0..=1.0` value onto the ramp: `0.0` selects
//! the first (darkest) glyph and `1.0` the last (lightest). The mapping is monotonic, so a smooth
//! luminance gradient produces a non-decreasing sequence of ramp glyphs.

use crate::color::ColorMode;
use rgfx_core::{Cell, Color, Framebuffer, TerminalEncoder, TerminalFrame, Viewport};

/// The default dark→light density ramp, `" .:-=+*#%@"`.
pub const DEFAULT_RAMP: &str = " .:-=+*#%@";

/// Horizontal framebuffer pixels consumed per terminal cell (one).
pub const SUBPIXEL_X: u16 = 1;

/// Vertical framebuffer pixels consumed per terminal cell (one).
pub const SUBPIXEL_Y: u16 = 1;

/// Tunable options controlling how framebuffer luminance is mapped onto ramp glyphs.
///
/// The pipeline per cell is: sample [`Framebuffer::luma`], apply [`gamma`](Self::gamma), apply
/// [`invert`](Self::invert), then index the [`ramp`](Self::ramp) from dark (first char) to light
/// (last char).
#[derive(Clone, Debug, PartialEq)]
pub struct AsciiOptions {
    /// The density ramp, ordered darkest → lightest. Defaults to [`DEFAULT_RAMP`].
    ///
    /// An empty ramp is treated as a single space so encoding never panics.
    pub ramp: String,
    /// When `true`, luminance is flipped (`1.0 - luma`) before indexing, so bright pixels pick the
    /// dark end of the ramp. Defaults to `false`.
    pub invert: bool,
    /// Gamma exponent applied as `luma.powf(gamma)`. `1.0` is identity; values below `1.0` brighten
    /// mid-tones, values above darken them. Defaults to `1.0`.
    pub gamma: f32,
    /// The color mode for cell foregrounds. [`ColorMode::None`] (the default) keeps pure grayscale
    /// output. Any other mode attaches the sampled pixel color as the cell foreground while the
    /// glyph is still chosen by luminance. Final quantization happens downstream in the serializer.
    pub color: ColorMode,
}

impl Default for AsciiOptions {
    fn default() -> Self {
        Self {
            ramp: DEFAULT_RAMP.to_string(),
            invert: false,
            gamma: 1.0,
            color: ColorMode::None,
        }
    }
}

/// A [`TerminalEncoder`] that renders a framebuffer as ASCII density art.
///
/// Construct one per set of [`AsciiOptions`] and reuse it across frames. The ramp is decoded into
/// a `char` vector once at construction so per-cell encoding is a simple index.
#[derive(Clone, Debug)]
pub struct AsciiEncoder {
    options: AsciiOptions,
    ramp: Vec<char>,
}

impl Default for AsciiEncoder {
    fn default() -> Self {
        Self::with_options(AsciiOptions::default())
    }
}

impl AsciiEncoder {
    /// Creates an encoder with the default [`AsciiOptions`] (the [`DEFAULT_RAMP`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an encoder with explicit [`AsciiOptions`].
    #[must_use]
    pub fn with_options(options: AsciiOptions) -> Self {
        let ramp = ramp_chars(&options.ramp);
        Self { options, ramp }
    }

    /// The options this encoder was built with.
    #[must_use]
    pub fn options(&self) -> &AsciiOptions {
        &self.options
    }

    /// The ramp glyph for a raw luminance value in `0.0..=1.0`.
    ///
    /// Applies gamma then invert before indexing. `0.0` maps to the first (darkest) glyph and
    /// `1.0` to the last (lightest); the mapping is monotonic in the input luminance.
    #[must_use]
    pub fn glyph_for_luma(&self, luma: f32) -> char {
        let mut l = luma.clamp(0.0, 1.0);
        if self.options.gamma != 1.0 {
            l = l.powf(self.options.gamma);
        }
        if self.options.invert {
            l = 1.0 - l;
        }
        let n = self.ramp.len();
        // `ramp` is guaranteed non-empty by `ramp_chars`, so `n - 1` never underflows.
        let last = n - 1;
        let idx = (l.clamp(0.0, 1.0) * last as f32).round() as usize;
        self.ramp[idx.min(last)]
    }

    /// Encodes the single framebuffer pixel `(px, py)` into one cell.
    ///
    /// Out-of-bounds pixels are treated as fully dark. The glyph is always chosen by luminance;
    /// when [`AsciiOptions::color`] is not [`ColorMode::None`] the sampled pixel color is attached
    /// as the cell foreground (out-of-bounds pixels keep the terminal-default foreground).
    fn encode_cell(&self, frame: &Framebuffer, px: usize, py: usize) -> Cell {
        let in_bounds = frame.in_bounds(px, py);
        let luma = if in_bounds { frame.luma(px, py) } else { 0.0 };
        let ch = self.glyph_for_luma(luma);
        if self.options.color == ColorMode::None || !in_bounds {
            Cell::glyph(ch)
        } else {
            let c = frame.get(px, py);
            Cell::colored(ch, Color::rgb(c.r, c.g, c.b))
        }
    }
}

/// Decodes a ramp string into its characters, falling back to a single space when empty.
fn ramp_chars(ramp: &str) -> Vec<char> {
    let chars: Vec<char> = ramp.chars().collect();
    if chars.is_empty() { vec![' '] } else { chars }
}

impl TerminalEncoder for AsciiEncoder {
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

    /// Fills a framebuffer with a single grey luma value.
    fn solid(width: usize, height: usize, luma: f32) -> Framebuffer {
        let mut fb = Framebuffer::new(width, height);
        fb.clear(Color::rgb(luma, luma, luma));
        fb
    }

    fn ramp() -> Vec<char> {
        DEFAULT_RAMP.chars().collect()
    }

    #[test]
    fn black_maps_to_first_ramp_char_white_to_last() {
        let enc = AsciiEncoder::new();
        let r = ramp();
        assert_eq!(enc.glyph_for_luma(0.0), r[0]);
        assert_eq!(enc.glyph_for_luma(0.0), ' ');
        assert_eq!(enc.glyph_for_luma(1.0), r[r.len() - 1]);
        assert_eq!(enc.glyph_for_luma(1.0), '@');
    }

    #[test]
    fn mapping_is_monotonic_across_a_gradient() {
        let enc = AsciiEncoder::new();
        let r = ramp();
        let mut last_idx = 0usize;
        for i in 0..=100 {
            let l = i as f32 / 100.0;
            let ch = enc.glyph_for_luma(l);
            let idx = r.iter().position(|&c| c == ch).expect("glyph is from ramp");
            assert!(
                idx >= last_idx,
                "luma {l} produced ramp index {idx} < previous {last_idx}"
            );
            last_idx = idx;
        }
        // The full sweep must reach both ends.
        assert_eq!(enc.glyph_for_luma(0.0), r[0]);
        assert_eq!(last_idx, r.len() - 1);
    }

    #[test]
    fn custom_ramp_is_honoured() {
        let enc = AsciiEncoder::with_options(AsciiOptions {
            ramp: "ab".to_string(),
            ..Default::default()
        });
        // Two-char ramp: split at the midpoint via round().
        assert_eq!(enc.glyph_for_luma(0.0), 'a');
        assert_eq!(enc.glyph_for_luma(0.2), 'a');
        assert_eq!(enc.glyph_for_luma(1.0), 'b');
        assert_eq!(enc.glyph_for_luma(0.8), 'b');
    }

    #[test]
    fn invert_swaps_the_ends() {
        let enc = AsciiEncoder::with_options(AsciiOptions {
            invert: true,
            ..Default::default()
        });
        let r = ramp();
        // Black now picks the light end, white the dark end.
        assert_eq!(enc.glyph_for_luma(0.0), r[r.len() - 1]);
        assert_eq!(enc.glyph_for_luma(1.0), r[0]);
    }

    #[test]
    fn empty_ramp_never_panics() {
        let enc = AsciiEncoder::with_options(AsciiOptions {
            ramp: String::new(),
            ..Default::default()
        });
        assert_eq!(enc.glyph_for_luma(0.0), ' ');
        assert_eq!(enc.glyph_for_luma(1.0), ' ');
    }

    #[test]
    fn gamma_below_one_brightens_midtones() {
        // gamma 0.5: luma 0.25 -> 0.5, which lands past the ramp midpoint.
        let plain = AsciiEncoder::new();
        let bright = AsciiEncoder::with_options(AsciiOptions {
            gamma: 0.5,
            ..Default::default()
        });
        let r = ramp();
        let plain_idx = r
            .iter()
            .position(|&c| c == plain.glyph_for_luma(0.25))
            .unwrap();
        let bright_idx = r
            .iter()
            .position(|&c| c == bright.glyph_for_luma(0.25))
            .unwrap();
        assert!(bright_idx > plain_idx);
    }

    #[test]
    fn output_dimensions_match_viewport() {
        let enc = AsciiEncoder::new();
        let vp = Viewport::new(5, 3);
        let (w, h) = vp.render_size(SUBPIXEL_X, SUBPIXEL_Y);
        assert_eq!((w, h), (5, 3));
        let frame = enc.encode(&solid(w, h, 1.0), vp);
        assert_eq!((frame.cols(), frame.rows()), (5, 3));
        let text = frame.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        for line in lines {
            assert_eq!(line.chars().count(), 5);
        }
    }

    #[test]
    fn horizontal_gradient_snapshot_is_exact() {
        // A 1-row, 10-col viewport with a left→right luma ramp maps straight onto DEFAULT_RAMP.
        let vp = Viewport::new(10, 1);
        let mut fb = Framebuffer::new(10, 1);
        for x in 0..10 {
            let l = x as f32 / 9.0;
            fb.set(x, 0, Color::rgb(l, l, l));
        }
        let enc = AsciiEncoder::new();
        assert_eq!(enc.encode(&fb, vp).to_text(), DEFAULT_RAMP);
    }

    #[test]
    fn color_mode_none_leaves_cells_grayscale() {
        let enc = AsciiEncoder::new();
        let mut fb = Framebuffer::new(1, 1);
        fb.set(0, 0, Color::rgb(1.0, 0.0, 0.0));
        let cell = enc.encode(&fb, Viewport::new(1, 1)).get(0, 0);
        assert_eq!(cell.fg, None);
    }

    #[test]
    fn color_mode_attaches_sampled_pixel_color() {
        let mut fb = Framebuffer::new(1, 1);
        fb.set(0, 0, Color::rgb(0.25, 0.5, 0.75));
        let enc = AsciiEncoder::with_options(AsciiOptions {
            color: ColorMode::Ansi256,
            ..Default::default()
        });
        let cell = enc.encode(&fb, Viewport::new(1, 1)).get(0, 0);
        assert_eq!(cell.fg, Some(Color::rgb(0.25, 0.5, 0.75)));
        // Glyph is still chosen by luma, exactly as the grayscale path.
        assert_eq!(
            cell.ch,
            enc.glyph_for_luma(Color::rgb(0.25, 0.5, 0.75).luma())
        );
    }

    #[test]
    fn color_mode_out_of_bounds_keeps_default_foreground() {
        let enc = AsciiEncoder::with_options(AsciiOptions {
            color: ColorMode::TrueColor,
            ..Default::default()
        });
        let fb = Framebuffer::new(1, 1);
        // Cell (2,0) is out of bounds → dark glyph, no color.
        let cell = enc.encode(&fb, Viewport::new(3, 1)).get(2, 0);
        assert_eq!(cell.fg, None);
    }

    #[test]
    fn undersized_framebuffer_reads_as_dark() {
        let enc = AsciiEncoder::new();
        let fb = Framebuffer::new(1, 1); // transparent/black
        let frame = enc.encode(&fb, Viewport::new(3, 1));
        // Every cell (in and out of bounds) is dark → first ramp glyph.
        assert_eq!(frame.to_text(), "   ");
    }
}
