//! The image-quality stage: tone adjustments and dithering applied to a
//! [`Framebuffer`] in place.
//!
//! These are pure framebuffer→framebuffer transforms. They never touch the
//! terminal or an encoder; the encoder (Braille/ASCII/blocks) consumes the
//! already-processed framebuffer, per the rgfx architectural law.
//!
//! # Pipeline order
//!
//! [`Preprocess::apply`] runs, in order:
//! 1. **Tone** — per-channel `gamma`, then `contrast` about mid-grey, then
//!    `brightness`, each clamped to `0.0..=1.0`.
//! 2. **Sharpen** — an optional unsharp-mask pass over the colour channels.
//! 3. **Dither** — reduces the luma/threshold path to pure black/white using the
//!    chosen algorithm. This is what makes 1-bit Braille output look good.
//!
//! Dithering writes a grayscale value (`0.0` or `1.0`) into every channel and
//! preserves each pixel's alpha, discarding colour — it is the last stage before
//! a grayscale encoder thresholds the result.

use rgfx_core::{Color, Framebuffer};

/// The size of an ordered [`Dither::Bayer`] threshold matrix.
///
/// Each variant is an `n × n` matrix; larger matrices spread the ordered pattern
/// over more tone levels at the cost of a more visible tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BayerSize {
    /// A 2×2 matrix (4 tone levels).
    Two,
    /// A 4×4 matrix (16 tone levels). A good default for Braille.
    #[default]
    Four,
    /// An 8×8 matrix (64 tone levels).
    Eight,
    /// A 16×16 matrix (256 tone levels).
    Sixteen,
}

impl BayerSize {
    /// The matrix side length `n`.
    pub const fn n(self) -> usize {
        match self {
            BayerSize::Two => 2,
            BayerSize::Four => 4,
            BayerSize::Eight => 8,
            BayerSize::Sixteen => 16,
        }
    }
}

/// The dithering algorithm applied on the luma/threshold path.
///
/// Every non-[`None`](Dither::None) variant reduces the framebuffer to pure
/// black/white pixels (each channel `0.0` or `1.0`, alpha preserved) so a
/// grayscale encoder produces good 1-bit output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Dither {
    /// No dithering: tone/sharpen still apply, but colour is left untouched.
    #[default]
    None,
    /// Threshold each pixel's luma at the configured threshold (no diffusion).
    ThresholdOnly,
    /// Floyd–Steinberg error diffusion: the classic serpentine-free 7/3/5/1 kernel.
    FloydSteinberg,
    /// Atkinson error diffusion: lighter diffusion (6/8 of the error), higher contrast.
    Atkinson,
    /// Ordered (Bayer) dithering with a matrix of the given size. Deterministic.
    Bayer(BayerSize),
}

/// Per-channel tone controls, applied before dithering.
///
/// All fields are independent; the defaults form an identity transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    /// Gamma exponent applied as `v.powf(gamma)` (`1.0` = identity). Values above
    /// `1.0` darken midtones; values below `1.0` brighten them. Must be positive.
    pub gamma: f32,
    /// Contrast multiplier about mid-grey: `(v - 0.5) * contrast + 0.5`
    /// (`1.0` = identity, `0.0` = flat grey, `>1.0` = more contrast).
    pub contrast: f32,
    /// Additive brightness offset applied after contrast (`0.0` = identity).
    pub brightness: f32,
    /// Optional unsharp-mask amount. `Some(a)` blends `a` of the high-pass detail
    /// back in: `v + a * (v - blur(v))`. `None` disables the pass.
    pub sharpen: Option<f32>,
}

impl Tone {
    /// The identity tone transform (no change).
    pub const IDENTITY: Tone = Tone {
        gamma: 1.0,
        contrast: 1.0,
        brightness: 0.0,
        sharpen: None,
    };

    /// Whether the gamma/contrast/brightness channel pass would change any value.
    fn needs_levels(&self) -> bool {
        self.gamma != 1.0 || self.contrast != 1.0 || self.brightness != 0.0
    }
}

impl Default for Tone {
    fn default() -> Self {
        Tone::IDENTITY
    }
}

/// The image-quality options threaded through the render path.
///
/// The default is an identity transform, so a framebuffer passed through
/// [`Preprocess::apply`] with default options is unchanged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preprocess {
    /// Tone (gamma/contrast/brightness) and optional sharpen.
    pub tone: Tone,
    /// The dithering algorithm applied to the luma/threshold path.
    pub dither: Dither,
    /// The luma threshold in `0.0..=1.0` used by threshold-only and error-diffusion
    /// dithering to decide lit vs unlit. `0.5` is the neutral midpoint.
    pub threshold: f32,
}

impl Preprocess {
    /// The identity preprocess (no tone change, no dithering).
    pub const IDENTITY: Preprocess = Preprocess {
        tone: Tone::IDENTITY,
        dither: Dither::None,
        threshold: 0.5,
    };

    /// Whether applying this would leave the framebuffer unchanged.
    pub fn is_identity(&self) -> bool {
        self.dither == Dither::None && self.tone == Tone::IDENTITY
    }

    /// Applies tone, then sharpen, then dithering to `fb` in place.
    ///
    /// This is a no-op when [`Preprocess::is_identity`] is true or the framebuffer
    /// is empty. Allocation is avoided unless a stage requires it (error-diffusion
    /// dithering uses one scratch luma buffer; sharpen uses one source copy).
    pub fn apply(&self, fb: &mut Framebuffer) {
        if self.is_identity() || fb.is_empty() {
            return;
        }

        if self.tone.needs_levels() {
            for c in fb.color_mut() {
                c.r = tone_channel(c.r, &self.tone);
                c.g = tone_channel(c.g, &self.tone);
                c.b = tone_channel(c.b, &self.tone);
            }
        }

        if let Some(amount) = self.tone.sharpen {
            sharpen(fb, amount);
        }

        match self.dither {
            Dither::None => {}
            Dither::ThresholdOnly => threshold_only(fb, self.threshold),
            Dither::FloydSteinberg => error_diffuse(fb, self.threshold, FLOYD_STEINBERG),
            Dither::Atkinson => error_diffuse(fb, self.threshold, ATKINSON),
            Dither::Bayer(size) => bayer(fb, size),
        }
    }
}

impl Default for Preprocess {
    fn default() -> Self {
        Preprocess::IDENTITY
    }
}

/// Applies gamma, contrast, then brightness to one channel, clamped to `0.0..=1.0`.
fn tone_channel(v: f32, t: &Tone) -> f32 {
    let mut v = v.clamp(0.0, 1.0);
    if t.gamma != 1.0 {
        v = v.powf(t.gamma);
    }
    v = (v - 0.5) * t.contrast + 0.5;
    v += t.brightness;
    v.clamp(0.0, 1.0)
}

/// Writes a grayscale value into a pixel while preserving its alpha.
#[inline]
fn set_mono(fb: &mut Framebuffer, x: usize, y: usize, v: f32) {
    let a = fb.get(x, y).a;
    fb.set(x, y, Color::new(v, v, v, a));
}

/// Thresholds every pixel's luma to pure black or white.
fn threshold_only(fb: &mut Framebuffer, threshold: f32) {
    let (w, h) = (fb.width(), fb.height());
    for y in 0..h {
        for x in 0..w {
            let v = if fb.luma(x, y) >= threshold { 1.0 } else { 0.0 };
            set_mono(fb, x, y, v);
        }
    }
}

/// One error-diffusion tap: an `(dx, dy)` offset and its weight over `divisor`.
struct Diffusion {
    taps: &'static [(isize, isize, f32)],
    divisor: f32,
}

/// Floyd–Steinberg weights (7/3/5/1 over 16).
const FLOYD_STEINBERG: Diffusion = Diffusion {
    taps: &[(1, 0, 7.0), (-1, 1, 3.0), (0, 1, 5.0), (1, 1, 1.0)],
    divisor: 16.0,
};

/// Atkinson weights (1/8 to each of six neighbours; 6/8 of the error propagates).
const ATKINSON: Diffusion = Diffusion {
    taps: &[
        (1, 0, 1.0),
        (2, 0, 1.0),
        (-1, 1, 1.0),
        (0, 1, 1.0),
        (1, 1, 1.0),
        (0, 2, 1.0),
    ],
    divisor: 8.0,
};

/// Error-diffusion dithering over a scratch luma buffer.
fn error_diffuse(fb: &mut Framebuffer, threshold: f32, kernel: Diffusion) {
    let (w, h) = (fb.width(), fb.height());
    // One scratch allocation for the whole pass; the framebuffer is not resized.
    let mut luma: Vec<f32> = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            luma.push(fb.luma(x, y));
        }
    }

    for y in 0..h {
        for x in 0..w {
            let old = luma[y * w + x];
            let new = if old >= threshold { 1.0 } else { 0.0 };
            let err = old - new;
            for &(dx, dy, weight) in kernel.taps {
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                    luma[ny as usize * w + nx as usize] += err * weight / kernel.divisor;
                }
            }
            set_mono(fb, x, y, new);
        }
    }
}

/// Ordered (Bayer) dithering with a matrix of the requested size.
fn bayer(fb: &mut Framebuffer, size: BayerSize) {
    let n = size.n();
    let matrix = bayer_matrix(n);
    let levels = (n * n) as f32;
    let (w, h) = (fb.width(), fb.height());
    for y in 0..h {
        for x in 0..w {
            // Normalised threshold in (0, 1) for this cell of the tiled matrix.
            let t = (matrix[(y % n) * n + (x % n)] as f32 + 0.5) / levels;
            let v = if fb.luma(x, y) >= t { 1.0 } else { 0.0 };
            set_mono(fb, x, y, v);
        }
    }
}

/// Builds the `n × n` Bayer threshold matrix (values `0..n*n`) by recursive
/// doubling from the 1×1 base. `n` must be a power of two, which every
/// [`BayerSize`] guarantees.
fn bayer_matrix(n: usize) -> Vec<u32> {
    let mut m = vec![0u32];
    let mut size = 1usize;
    while size < n {
        let ns = size * 2;
        let mut next = vec![0u32; ns * ns];
        for i in 0..ns {
            for j in 0..ns {
                let base = 4 * m[(i % size) * size + (j % size)];
                let quadrant = match (i / size, j / size) {
                    (0, 0) => 0,
                    (0, 1) => 2,
                    (1, 0) => 3,
                    _ => 1,
                };
                next[i * ns + j] = base + quadrant;
            }
        }
        m = next;
        size = ns;
    }
    m
}

/// Unsharp mask over the colour channels using a 3×3 box blur, clamped at edges.
fn sharpen(fb: &mut Framebuffer, amount: f32) {
    let (w, h) = (fb.width(), fb.height());
    if w == 0 || h == 0 {
        return;
    }
    // One source copy so neighbour reads see the pre-sharpen values.
    let src: Vec<Color> = fb.color().to_vec();
    for y in 0..h {
        for x in 0..w {
            let (mut sr, mut sg, mut sb) = (0.0f32, 0.0f32, 0.0f32);
            let mut count = 0.0f32;
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let nx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                    let ny = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                    let c = src[ny * w + nx];
                    sr += c.r;
                    sg += c.g;
                    sb += c.b;
                    count += 1.0;
                }
            }
            let (br, bg, bb) = (sr / count, sg / count, sb / count);
            let orig = src[y * w + x];
            fb.set(
                x,
                y,
                Color::new(
                    (orig.r + amount * (orig.r - br)).clamp(0.0, 1.0),
                    (orig.g + amount * (orig.g - bg)).clamp(0.0, 1.0),
                    (orig.b + amount * (orig.b - bb)).clamp(0.0, 1.0),
                    orig.a,
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(w: usize, h: usize, v: f32) -> Framebuffer {
        let mut fb = Framebuffer::new(w, h);
        fb.clear(Color::new(v, v, v, 1.0));
        fb
    }

    fn lit_ratio(fb: &Framebuffer) -> f32 {
        let lit = fb.color().iter().filter(|c| c.r >= 0.5).count();
        lit as f32 / fb.len() as f32
    }

    #[test]
    fn identity_is_noop() {
        let mut fb = filled(8, 8, 0.42);
        let before = fb.color().to_vec();
        Preprocess::IDENTITY.apply(&mut fb);
        assert_eq!(fb.color(), before.as_slice());
    }

    #[test]
    fn floyd_steinberg_midgray_is_about_half_lit() {
        let mut fb = filled(64, 64, 0.5);
        let pre = Preprocess {
            dither: Dither::FloydSteinberg,
            ..Preprocess::IDENTITY
        };
        pre.apply(&mut fb);
        // Every pixel must be pure black or white after dithering.
        assert!(
            fb.color().iter().all(|c| c.r == 0.0 || c.r == 1.0),
            "dither must produce 1-bit output"
        );
        let ratio = lit_ratio(&fb);
        assert!(
            (ratio - 0.5).abs() < 0.1,
            "expected ~50% lit on mid-gray, got {ratio}"
        );
    }

    #[test]
    fn atkinson_midgray_is_about_half_lit() {
        let mut fb = filled(64, 64, 0.5);
        let pre = Preprocess {
            dither: Dither::Atkinson,
            ..Preprocess::IDENTITY
        };
        pre.apply(&mut fb);
        let ratio = lit_ratio(&fb);
        assert!(
            (ratio - 0.5).abs() < 0.15,
            "expected ~50% lit on mid-gray, got {ratio}"
        );
    }

    #[test]
    fn bayer_matrix_4x4_matches_known_values() {
        // The canonical 4×4 Bayer matrix.
        let expected: [u32; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
        assert_eq!(bayer_matrix(4), expected);
    }

    #[test]
    fn bayer_matrix_2x2_matches_known_values() {
        assert_eq!(bayer_matrix(2), [0u32, 2, 3, 1]);
    }

    #[test]
    fn bayer_midgray_is_deterministic_checkerboard() {
        let mut fb = filled(4, 4, 0.5);
        let pre = Preprocess {
            dither: Dither::Bayer(BayerSize::Four),
            ..Preprocess::IDENTITY
        };
        pre.apply(&mut fb);
        // With a flat 0.5 field and the 4×4 matrix, lit iff bayer <= 7: an exact
        // checkerboard. Snapshot it as a boolean grid.
        let grid: Vec<bool> = fb.color().iter().map(|c| c.r >= 0.5).collect();
        #[rustfmt::skip]
        let expected = vec![
            true,  false, true,  false,
            false, true,  false, true,
            true,  false, true,  false,
            false, true,  false, true,
        ];
        assert_eq!(grid, expected);
    }

    #[test]
    fn threshold_only_splits_at_threshold() {
        let mut fb = Framebuffer::new(3, 1);
        fb.set(0, 0, Color::new(0.2, 0.2, 0.2, 1.0));
        fb.set(1, 0, Color::new(0.5, 0.5, 0.5, 1.0));
        fb.set(2, 0, Color::new(0.9, 0.9, 0.9, 1.0));
        let pre = Preprocess {
            dither: Dither::ThresholdOnly,
            threshold: 0.5,
            ..Preprocess::IDENTITY
        };
        pre.apply(&mut fb);
        assert_eq!(fb.get(0, 0).r, 0.0);
        assert_eq!(fb.get(1, 0).r, 1.0); // exactly at threshold => lit
        assert_eq!(fb.get(2, 0).r, 1.0);
    }

    #[test]
    fn dither_preserves_alpha() {
        let mut fb = Framebuffer::new(2, 1);
        fb.set(0, 0, Color::new(0.9, 0.9, 0.9, 0.25));
        fb.set(1, 0, Color::new(0.1, 0.1, 0.1, 0.75));
        let pre = Preprocess {
            dither: Dither::ThresholdOnly,
            ..Preprocess::IDENTITY
        };
        pre.apply(&mut fb);
        assert_eq!(fb.get(0, 0).a, 0.25);
        assert_eq!(fb.get(1, 0).a, 0.75);
    }

    #[test]
    fn tone_channel_is_monotonic_and_clamped() {
        let tone = Tone {
            gamma: 2.2,
            contrast: 1.5,
            brightness: 0.1,
            sharpen: None,
        };
        let mut prev = f32::NEG_INFINITY;
        let mut i = 0;
        while i <= 100 {
            let v = i as f32 / 100.0;
            let out = tone_channel(v, &tone);
            assert!((0.0..=1.0).contains(&out), "out of range: {out}");
            assert!(out >= prev - 1e-6, "not monotonic at v={v}: {out} < {prev}");
            prev = out;
            i += 1;
        }
    }

    #[test]
    fn brightness_clamps_high_and_low() {
        let hi = Tone {
            brightness: 2.0,
            ..Tone::IDENTITY
        };
        assert_eq!(tone_channel(0.5, &hi), 1.0);
        let lo = Tone {
            brightness: -2.0,
            ..Tone::IDENTITY
        };
        assert_eq!(tone_channel(0.5, &lo), 0.0);
    }

    #[test]
    fn gamma_darkens_midtones_when_above_one() {
        let t = Tone {
            gamma: 2.0,
            ..Tone::IDENTITY
        };
        // 0.5^2 = 0.25 < 0.5
        assert!((tone_channel(0.5, &t) - 0.25).abs() < 1e-6);
        // Endpoints are fixed points of gamma.
        assert_eq!(tone_channel(0.0, &t), 0.0);
        assert_eq!(tone_channel(1.0, &t), 1.0);
    }

    #[test]
    fn contrast_increases_spread_around_midgray() {
        let t = Tone {
            contrast: 2.0,
            ..Tone::IDENTITY
        };
        // Below mid-gray goes lower, above goes higher; mid-gray is a fixed point.
        assert!(tone_channel(0.4, &t) < 0.4);
        assert!(tone_channel(0.6, &t) > 0.6);
        assert!((tone_channel(0.5, &t) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn sharpen_increases_contrast_across_a_step_edge() {
        // A horizontal step edge: left half 0.3, right half 0.7.
        let (w, h) = (8usize, 4usize);
        let mut fb = Framebuffer::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = if x < w / 2 { 0.3 } else { 0.7 };
                fb.set(x, y, Color::new(v, v, v, 1.0));
            }
        }
        let pre = Preprocess {
            tone: Tone {
                sharpen: Some(1.0),
                ..Tone::IDENTITY
            },
            ..Preprocess::IDENTITY
        };
        pre.apply(&mut fb);
        let y = h / 2;
        // The dark pixel just left of the edge should darken below 0.3, and the
        // light pixel just right should brighten above 0.7: local contrast rises.
        let left = fb.get(w / 2 - 1, y).r;
        let right = fb.get(w / 2, y).r;
        assert!(left < 0.3, "left edge did not darken: {left}");
        assert!(right > 0.7, "right edge did not brighten: {right}");
        assert!(
            right - left > 0.4,
            "edge contrast did not increase: {}",
            right - left
        );
    }

    #[test]
    fn apply_is_noop_on_empty_framebuffer() {
        let mut fb = Framebuffer::new(0, 0);
        let pre = Preprocess {
            dither: Dither::FloydSteinberg,
            ..Preprocess::IDENTITY
        };
        pre.apply(&mut fb); // must not panic
        assert!(fb.is_empty());
    }
}
