//! ANSI color: fidelity modes, terminal capability detection, RGB quantization, and the SGR
//! escape sequences that set a cell's foreground/background.
//!
//! This module turns an [`rgfx_core::Color`] (linear-ish RGBA floats) into one of three terminal
//! representations depending on the active [`ColorMode`]:
//!
//! - [`ColorMode::Ansi16`] — nearest color in the 16-entry [`ANSI16_PALETTE`] (SGR `30`–`37` /
//!   `90`–`97` for foreground, `40`–`47` / `100`–`107` for background);
//! - [`ColorMode::Ansi256`] — nearest index in the xterm 256-color palette, using the 6×6×6 color
//!   cube and the 24-step grayscale ramp (SGR `38;5;n` / `48;5;n`);
//! - [`ColorMode::TrueColor`] — the exact 24-bit value (SGR `38;2;R;G;B` / `48;2;R;G;B`).
//!
//! [`ColorMode::None`] disables color entirely: encoders keep their grayscale glyph behavior and
//! the serializer emits no SGR at all.
//!
//! Quantization is deliberately kept here, downstream of the encoders: the encoders attach raw
//! [`Color`]s to cells and the serializer quantizes them to whatever [`ColorMode`] it was built
//! with. No function in this module writes to stdout.

use rgfx_core::Color;

/// The color fidelity a terminal frame is rendered at.
///
/// The default is [`ColorMode::None`], which preserves the pure-grayscale behavior of the
/// encoders and suppresses every escape sequence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorMode {
    /// No color: grayscale glyphs only, no SGR sequences emitted.
    #[default]
    None,
    /// The 16-color ANSI palette ([`ANSI16_PALETTE`]).
    Ansi16,
    /// The 256-color xterm palette (6×6×6 cube plus a 24-step gray ramp).
    Ansi256,
    /// 24-bit truecolor.
    TrueColor,
}

/// The 16 standard ANSI colors as 8-bit RGB.
///
/// This is the canonical VGA palette, which is also the first 16 entries of the xterm 256-color
/// palette. Index `0..8` are the normal colors and `8..16` the bright variants.
pub const ANSI16_PALETTE: [(u8, u8, u8); 16] = [
    (0, 0, 0),       // 0  black
    (128, 0, 0),     // 1  red
    (0, 128, 0),     // 2  green
    (128, 128, 0),   // 3  yellow
    (0, 0, 128),     // 4  blue
    (128, 0, 128),   // 5  magenta
    (0, 128, 128),   // 6  cyan
    (192, 192, 192), // 7  white / silver
    (128, 128, 128), // 8  bright black / gray
    (255, 0, 0),     // 9  bright red
    (0, 255, 0),     // 10 bright green
    (255, 255, 0),   // 11 bright yellow
    (0, 0, 255),     // 12 bright blue
    (255, 0, 255),   // 13 bright magenta
    (0, 255, 255),   // 14 bright cyan
    (255, 255, 255), // 15 bright white
];

/// The six intensity levels of each axis of the xterm 6×6×6 color cube.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The squared Euclidean distance between two 8-bit RGB triples.
#[inline]
fn dist2(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| {
        let diff = i32::from(x) - i32::from(y);
        (diff * diff) as u32
    };
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// The index into [`ANSI16_PALETTE`] whose color is nearest to `color`, by squared RGB distance.
#[must_use]
pub fn ansi16_index(color: Color) -> u8 {
    let (r, g, b, _) = color.to_u8();
    let target = (r, g, b);
    let mut best = 0u8;
    let mut best_dist = u32::MAX;
    for (i, &entry) in ANSI16_PALETTE.iter().enumerate() {
        let d = dist2(target, entry);
        if d < best_dist {
            best_dist = d;
            best = i as u8;
        }
    }
    best
}

/// The nearest of the six cube levels to a channel value, returned as `(axis_index, level_value)`.
fn nearest_cube_axis(v: u8) -> (u8, u8) {
    let mut best = 0u8;
    let mut best_dist = u32::MAX;
    for (i, &level) in CUBE_LEVELS.iter().enumerate() {
        let diff = i32::from(v) - i32::from(level);
        let d = (diff * diff) as u32;
        if d < best_dist {
            best_dist = d;
            best = i as u8;
        }
    }
    (best, CUBE_LEVELS[best as usize])
}

/// The xterm 256-color palette index nearest to `color`.
///
/// The candidate from the 6×6×6 color cube (indices `16..232`) is compared against the candidate
/// from the 24-step grayscale ramp (indices `232..256`) and the closer of the two, by squared RGB
/// distance, is returned. Near-gray colors therefore snap to the finer gray ramp while saturated
/// colors snap to the cube. The 16 system colors (`0..16`) are intentionally never emitted, since
/// their real RGB is terminal-dependent.
#[must_use]
pub fn ansi256_index(color: Color) -> u8 {
    let (r, g, b, _) = color.to_u8();
    let target = (r, g, b);

    // Color-cube candidate.
    let (ri, rv) = nearest_cube_axis(r);
    let (gi, gv) = nearest_cube_axis(g);
    let (bi, bv) = nearest_cube_axis(b);
    let cube_index = 16 + 36 * ri + 6 * gi + bi;
    let cube_dist = dist2(target, (rv, gv, bv));

    // Grayscale-ramp candidate: the ramp value nearest the channel average minimizes distance.
    let avg = ((u32::from(r) + u32::from(g) + u32::from(b)) / 3) as i32;
    // Ramp values are 8, 18, ..., 238 (24 steps): value = 8 + 10 * step.
    let step = (((avg - 8) + 5) / 10).clamp(0, 23) as u8;
    let gray_val = 8 + 10 * step;
    let gray_index = 232 + step;
    let gray_dist = dist2(target, (gray_val, gray_val, gray_val));

    if gray_dist < cube_dist {
        gray_index
    } else {
        cube_index
    }
}

/// A color resolved to a concrete terminal representation, ready to be written as SGR parameters.
///
/// Produced by [`AnsiColor::quantize`]. Two colors that quantize to the same representation compare
/// equal, which is what lets the serializer suppress redundant escapes between adjacent cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnsiColor {
    /// An index `0..16` into [`ANSI16_PALETTE`].
    Ansi16(u8),
    /// An index `0..256` into the xterm 256-color palette.
    Ansi256(u8),
    /// An exact 24-bit RGB triple.
    Rgb(u8, u8, u8),
}

impl AnsiColor {
    /// Quantizes `color` to the representation demanded by `mode`.
    ///
    /// Returns `None` for [`ColorMode::None`] (the caller should emit no color for the cell).
    #[must_use]
    pub fn quantize(color: Color, mode: ColorMode) -> Option<AnsiColor> {
        match mode {
            ColorMode::None => None,
            ColorMode::Ansi16 => Some(AnsiColor::Ansi16(ansi16_index(color))),
            ColorMode::Ansi256 => Some(AnsiColor::Ansi256(ansi256_index(color))),
            ColorMode::TrueColor => {
                let (r, g, b, _) = color.to_u8();
                Some(AnsiColor::Rgb(r, g, b))
            }
        }
    }

    /// Appends the SGR parameter body for this color to `out`, as foreground when
    /// `foreground` is `true`, otherwise as background. Does not include the `\x1b[` / `m` frame.
    pub(crate) fn write_params(self, foreground: bool, out: &mut String) {
        use std::fmt::Write as _;
        match self {
            AnsiColor::Ansi16(i) => {
                let base: u16 = if i < 8 {
                    if foreground { 30 } else { 40 }
                } else {
                    if foreground { 90 } else { 100 }
                };
                let code = base + u16::from(i % 8);
                // invariant: writing to a String is infallible.
                let _ = write!(out, "{code}");
            }
            AnsiColor::Ansi256(i) => {
                let lead = if foreground { "38;5;" } else { "48;5;" };
                let _ = write!(out, "{lead}{i}");
            }
            AnsiColor::Rgb(r, g, b) => {
                let lead = if foreground { "38;2;" } else { "48;2;" };
                let _ = write!(out, "{lead}{r};{g};{b}");
            }
        }
    }

    /// The full SGR escape sequence that sets this color as the foreground.
    #[must_use]
    pub fn foreground_escape(self) -> String {
        let mut params = String::new();
        self.write_params(true, &mut params);
        format!("\x1b[{params}m")
    }

    /// The full SGR escape sequence that sets this color as the background.
    #[must_use]
    pub fn background_escape(self) -> String {
        let mut params = String::new();
        self.write_params(false, &mut params);
        format!("\x1b[{params}m")
    }
}

/// Detects a sensible default [`ColorMode`] from the process environment.
///
/// Reads `COLORTERM` and `TERM`. This is only a *default*; callers may override the returned mode.
/// See [`detect_from_env`] for the exact decision rules (and a testable, env-free entry point).
#[must_use]
pub fn detect_color_mode() -> ColorMode {
    let colorterm = std::env::var("COLORTERM").ok();
    let term = std::env::var("TERM").ok();
    detect_from_env(colorterm.as_deref(), term.as_deref())
}

/// Decides a default [`ColorMode`] from explicit `COLORTERM` / `TERM` values.
///
/// The rules, in order:
/// 1. `COLORTERM` equal to `truecolor` or `24bit` (case-insensitive) → [`ColorMode::TrueColor`].
/// 2. `TERM` containing `256color` → [`ColorMode::Ansi256`].
/// 3. `TERM` unset, empty, or exactly `dumb` → [`ColorMode::None`].
/// 4. any other non-empty `TERM` → [`ColorMode::Ansi16`].
#[must_use]
pub fn detect_from_env(colorterm: Option<&str>, term: Option<&str>) -> ColorMode {
    if let Some(ct) = colorterm {
        let ct = ct.trim().to_ascii_lowercase();
        if ct == "truecolor" || ct == "24bit" {
            return ColorMode::TrueColor;
        }
    }
    match term {
        Some(t) if t.contains("256color") => ColorMode::Ansi256,
        None => ColorMode::None,
        Some(t) if t.is_empty() || t == "dumb" => ColorMode::None,
        Some(_) => ColorMode::Ansi16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_mode_defaults_to_none() {
        assert_eq!(ColorMode::default(), ColorMode::None);
    }

    #[test]
    fn truecolor_foreground_escape_is_exact() {
        let c = AnsiColor::Rgb(10, 20, 30);
        assert_eq!(c.foreground_escape(), "\x1b[38;2;10;20;30m");
        // And via quantize from a known RGB.
        let q =
            AnsiColor::quantize(Color::from_u8(255, 128, 0, 255), ColorMode::TrueColor).unwrap();
        assert_eq!(q, AnsiColor::Rgb(255, 128, 0));
        assert_eq!(q.foreground_escape(), "\x1b[38;2;255;128;0m");
    }

    #[test]
    fn truecolor_background_escape_is_exact() {
        assert_eq!(
            AnsiColor::Rgb(1, 2, 3).background_escape(),
            "\x1b[48;2;1;2;3m"
        );
    }

    #[test]
    fn ansi16_quantization_table() {
        let case = |r, g, b, expected| {
            assert_eq!(ansi16_index(Color::from_u8(r, g, b, 255)), expected);
        };
        case(0, 0, 0, 0); // black
        case(128, 0, 0, 1); // red
        case(0, 128, 0, 2); // green
        case(255, 0, 0, 9); // bright red
        case(0, 255, 0, 10); // bright green
        case(0, 0, 255, 12); // bright blue
        case(255, 255, 255, 15); // bright white
        // A near-black off-color still snaps to black.
        case(8, 4, 2, 0);
    }

    #[test]
    fn ansi16_foreground_and_background_codes() {
        // Normal color (index 1, red): fg 31, bg 41.
        assert_eq!(AnsiColor::Ansi16(1).foreground_escape(), "\x1b[31m");
        assert_eq!(AnsiColor::Ansi16(1).background_escape(), "\x1b[41m");
        // Bright color (index 9, bright red): fg 91, bg 101.
        assert_eq!(AnsiColor::Ansi16(9).foreground_escape(), "\x1b[91m");
        assert_eq!(AnsiColor::Ansi16(9).background_escape(), "\x1b[101m");
    }

    #[test]
    fn ansi256_quantization_table() {
        let idx = |r, g, b| ansi256_index(Color::from_u8(r, g, b, 255));
        // Pure primaries land exactly on cube corners.
        assert_eq!(idx(0, 0, 0), 16); // cube origin
        assert_eq!(idx(255, 255, 255), 231); // cube far corner
        assert_eq!(idx(255, 0, 0), 196); // 16 + 36*5
        assert_eq!(idx(0, 255, 0), 46); // 16 + 6*5
        assert_eq!(idx(0, 0, 255), 21); // 16 + 5
        // Mid gray snaps to the finer grayscale ramp, not the cube.
        assert_eq!(idx(128, 128, 128), 244); // 232 + 12, ramp value 128
    }

    #[test]
    fn ansi256_escapes_use_the_indexed_form() {
        assert_eq!(
            AnsiColor::Ansi256(196).foreground_escape(),
            "\x1b[38;5;196m"
        );
        assert_eq!(
            AnsiColor::Ansi256(244).background_escape(),
            "\x1b[48;5;244m"
        );
    }

    #[test]
    fn quantize_none_yields_no_color() {
        assert_eq!(AnsiColor::quantize(Color::WHITE, ColorMode::None), None);
    }

    #[test]
    fn detection_picks_truecolor_for_colorterm() {
        assert_eq!(
            detect_from_env(Some("truecolor"), Some("xterm-256color")),
            ColorMode::TrueColor
        );
        assert_eq!(detect_from_env(Some("24bit"), None), ColorMode::TrueColor);
        // Case-insensitive.
        assert_eq!(
            detect_from_env(Some("TrueColor"), None),
            ColorMode::TrueColor
        );
    }

    #[test]
    fn detection_degrades_by_term() {
        // 256color TERM, no COLORTERM → Ansi256.
        assert_eq!(
            detect_from_env(None, Some("xterm-256color")),
            ColorMode::Ansi256
        );
        // A plain color terminal → Ansi16.
        assert_eq!(detect_from_env(None, Some("xterm")), ColorMode::Ansi16);
        assert_eq!(detect_from_env(None, Some("screen")), ColorMode::Ansi16);
        // dumb / empty / unset → None.
        assert_eq!(detect_from_env(None, Some("dumb")), ColorMode::None);
        assert_eq!(detect_from_env(None, Some("")), ColorMode::None);
        assert_eq!(detect_from_env(None, None), ColorMode::None);
        // An unrelated COLORTERM value does not force truecolor.
        assert_eq!(
            detect_from_env(Some("1"), Some("xterm-256color")),
            ColorMode::Ansi256
        );
    }
}
