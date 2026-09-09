//! The ANSI serializer: turns an encoded [`TerminalFrame`] into a batched byte stream.
//!
//! [`AnsiSerializer`] walks the cell grid row by row and appends each glyph, emitting an SGR color
//! escape **only when the resolved color changes** from the previously written cell. A run of
//! adjacent cells sharing a foreground/background therefore pays for exactly one escape, which
//! keeps the byte stream small and avoids visible churn.
//!
//! Per the crate's architectural law, the serializer performs no I/O: it returns a `Vec<u8>` that
//! the CLI (or the frame-diff engine) flushes through a [`BufferedWriter`]. Colors are quantized
//! here, at serialization time, according to the [`ColorMode`] the serializer was constructed with
//! — the encoders upstream only attach raw [`rgfx_core::Color`]s.
//!
//! [`BufferedWriter`]: crate::BufferedWriter

use crate::color::{AnsiColor, ColorMode};
use rgfx_core::TerminalFrame;

/// The `\x1b[0m` reset that returns the terminal to its default colors.
const SGR_RESET: &str = "\x1b[0m";

/// Serializes [`TerminalFrame`]s into ANSI byte streams at a fixed [`ColorMode`].
///
/// Construct one per output color mode and reuse it across frames; it holds no per-frame state.
#[derive(Clone, Copy, Debug)]
pub struct AnsiSerializer {
    mode: ColorMode,
}

impl AnsiSerializer {
    /// Creates a serializer that emits color at the given [`ColorMode`].
    #[must_use]
    pub fn new(mode: ColorMode) -> Self {
        Self { mode }
    }

    /// The color mode this serializer emits at.
    #[must_use]
    pub fn mode(&self) -> ColorMode {
        self.mode
    }

    /// Serializes `frame` into an ANSI byte stream.
    ///
    /// Rows are separated by a single `\n`. When the mode is [`ColorMode::None`], only glyphs are
    /// written and no escape sequences appear. Otherwise an SGR escape is emitted whenever a cell's
    /// resolved foreground or background differs from the previously written cell's, and a single
    /// `\x1b[0m` reset is appended at the end if any color was written.
    #[must_use]
    pub fn serialize(&self, frame: &TerminalFrame) -> Vec<u8> {
        let mut out = String::with_capacity(frame.cols() * frame.rows() + frame.rows());

        // The colors currently active in the (virtual) terminal. `None` means the terminal
        // default. We start assuming defaults are in effect.
        let mut last_fg: Option<AnsiColor> = None;
        let mut last_bg: Option<AnsiColor> = None;
        let mut emitted_color = false;

        for row in 0..frame.rows() {
            if row > 0 {
                out.push('\n');
            }
            for col in 0..frame.cols() {
                let cell = frame.get(col, row);

                if self.mode != ColorMode::None {
                    let fg = cell.fg.and_then(|c| AnsiColor::quantize(c, self.mode));
                    let bg = cell.bg.and_then(|c| AnsiColor::quantize(c, self.mode));

                    if fg != last_fg || bg != last_bg {
                        self.push_transition(&mut out, last_fg, fg, last_bg, bg);
                        last_fg = fg;
                        last_bg = bg;
                        emitted_color = true;
                    }
                }

                out.push(cell.ch);
            }
        }

        if emitted_color {
            out.push_str(SGR_RESET);
        }

        out.into_bytes()
    }

    /// Appends a single SGR escape covering exactly the foreground/background that changed.
    ///
    /// A color that becomes `None` (terminal default) is written as `39` (fg) / `49` (bg).
    fn push_transition(
        &self,
        out: &mut String,
        last_fg: Option<AnsiColor>,
        fg: Option<AnsiColor>,
        last_bg: Option<AnsiColor>,
        bg: Option<AnsiColor>,
    ) {
        let mut params = String::new();

        if fg != last_fg {
            match fg {
                Some(c) => c.write_params(true, &mut params),
                None => params.push_str("39"),
            }
        }
        if bg != last_bg {
            if !params.is_empty() {
                params.push(';');
            }
            match bg {
                Some(c) => c.write_params(false, &mut params),
                None => params.push_str("49"),
            }
        }

        if !params.is_empty() {
            out.push_str("\x1b[");
            out.push_str(&params);
            out.push('m');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rgfx_core::{Cell, Color, TerminalFrame};

    fn as_str(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("serialized output is valid UTF-8")
    }

    /// Counts non-overlapping occurrences of `needle` in `haystack`.
    fn count(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    #[test]
    fn none_mode_emits_glyphs_only() {
        let mut frame = TerminalFrame::new(3, 1);
        frame.set(0, 0, Cell::colored('a', Color::rgb(1.0, 0.0, 0.0)));
        frame.set(1, 0, Cell::colored('b', Color::rgb(0.0, 1.0, 0.0)));
        frame.set(2, 0, Cell::glyph('c'));
        let out = as_str(AnsiSerializer::new(ColorMode::None).serialize(&frame));
        assert_eq!(out, "abc");
    }

    #[test]
    fn truecolor_run_emits_one_sgr_for_repeated_color() {
        // Three adjacent cells share the same fg; only one SGR should appear before them.
        let red = Color::from_u8(200, 10, 20, 255);
        let mut frame = TerminalFrame::new(3, 1);
        for (i, ch) in ['x', 'y', 'z'].into_iter().enumerate() {
            frame.set(i, 0, Cell::colored(ch, red));
        }
        let out = as_str(AnsiSerializer::new(ColorMode::TrueColor).serialize(&frame));
        assert_eq!(out, "\x1b[38;2;200;10;20mxyz\x1b[0m");
        assert_eq!(count(&out, "\x1b[38;2;200;10;20m"), 1);
    }

    #[test]
    fn color_change_between_adjacent_cells_re_emits() {
        let red = Color::from_u8(200, 10, 20, 255);
        let blue = Color::from_u8(20, 30, 200, 255);
        let mut frame = TerminalFrame::new(4, 1);
        frame.set(0, 0, Cell::colored('a', red));
        frame.set(1, 0, Cell::colored('b', red));
        frame.set(2, 0, Cell::colored('c', blue));
        frame.set(3, 0, Cell::colored('d', blue));
        let out = as_str(AnsiSerializer::new(ColorMode::TrueColor).serialize(&frame));
        // Exactly two color transitions across the row of two runs.
        assert_eq!(out, "\x1b[38;2;200;10;20mab\x1b[38;2;20;30;200mcd\x1b[0m");
    }

    #[test]
    fn distinct_colors_that_quantize_equal_do_not_re_emit() {
        // Two slightly different reds both quantize to bright-red (index 9) in Ansi16 mode.
        let mut frame = TerminalFrame::new(2, 1);
        frame.set(0, 0, Cell::colored('a', Color::from_u8(250, 4, 4, 255)));
        frame.set(1, 0, Cell::colored('b', Color::from_u8(255, 0, 0, 255)));
        let out = as_str(AnsiSerializer::new(ColorMode::Ansi16).serialize(&frame));
        assert_eq!(out, "\x1b[91mab\x1b[0m");
        assert_eq!(count(&out, "\x1b["), 2); // one SGR + one reset
    }

    #[test]
    fn foreground_and_background_combine_in_one_escape() {
        let mut frame = TerminalFrame::new(1, 1);
        frame.set(
            0,
            0,
            Cell {
                ch: '\u{2580}',
                fg: Some(Color::from_u8(10, 20, 30, 255)),
                bg: Some(Color::from_u8(40, 50, 60, 255)),
            },
        );
        let out = as_str(AnsiSerializer::new(ColorMode::TrueColor).serialize(&frame));
        assert_eq!(out, "\x1b[38;2;10;20;30;48;2;40;50;60m\u{2580}\x1b[0m");
    }

    #[test]
    fn returning_to_default_emits_reset_codes() {
        let mut frame = TerminalFrame::new(2, 1);
        frame.set(0, 0, Cell::colored('a', Color::from_u8(200, 10, 20, 255)));
        frame.set(1, 0, Cell::glyph('b')); // no color → back to default fg
        let out = as_str(AnsiSerializer::new(ColorMode::TrueColor).serialize(&frame));
        assert_eq!(out, "\x1b[38;2;200;10;20ma\x1b[39mb\x1b[0m");
    }

    #[test]
    fn rows_are_newline_separated_and_color_state_persists() {
        let red = Color::from_u8(200, 10, 20, 255);
        let mut frame = TerminalFrame::new(1, 2);
        frame.set(0, 0, Cell::colored('a', red));
        frame.set(0, 1, Cell::colored('b', red)); // same color on next row → no re-emit
        let out = as_str(AnsiSerializer::new(ColorMode::TrueColor).serialize(&frame));
        assert_eq!(out, "\x1b[38;2;200;10;20ma\nb\x1b[0m");
    }

    #[test]
    fn all_default_color_frame_emits_no_escapes() {
        let mut frame = TerminalFrame::new(2, 1);
        frame.set(0, 0, Cell::glyph('a'));
        frame.set(1, 0, Cell::glyph('b'));
        let out = as_str(AnsiSerializer::new(ColorMode::TrueColor).serialize(&frame));
        assert_eq!(out, "ab");
    }
}
