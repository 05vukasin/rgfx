//! Shared "chrome" for the interactive viewers: the bottom status/options bar.
//!
//! The 3D, image, and gif/video viewers all overlay a two-line bar (an info line plus a key-help
//! line) across the bottom rows of the encoded [`TerminalFrame`]. This module holds the one
//! implementation they share so the bar looks and behaves the same everywhere.

use rgfx_core::{Cell, TerminalFrame};

/// Writes `text` (clipped to the frame width) into `row`, blanking the rest of the row so the
/// overlaid line fully replaces whatever glyphs were underneath. No-op if `row` is out of range.
pub(crate) fn write_line(frame: &mut TerminalFrame, row: usize, text: &str) {
    let cols = frame.cols();
    if row >= frame.rows() || cols == 0 {
        return;
    }
    let mut col = 0;
    for ch in text.chars() {
        if col >= cols {
            break;
        }
        frame.set(col, row, Cell::glyph(ch));
        col += 1;
    }
    while col < cols {
        frame.set(col, row, Cell::glyph(' '));
        col += 1;
    }
}

/// Overlays the two-line bottom bar: `status` on the second-to-last row (when there is room) and
/// `help` on the last row. No-op for an empty frame.
pub(crate) fn overlay_bottom_bar(frame: &mut TerminalFrame, status: &str, help: &str) {
    let rows = frame.rows();
    if rows == 0 || frame.cols() == 0 {
        return;
    }
    if rows >= 2 {
        write_line(frame, rows - 2, status);
    }
    write_line(frame, rows - 1, help);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_line_clips_and_blanks_to_width() {
        let mut f = TerminalFrame::new(4, 2);
        write_line(&mut f, 0, "abcdef"); // longer than width → clipped
        write_line(&mut f, 1, "x"); // shorter → padded with spaces
        let t = f.to_text();
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "abcd");
        assert_eq!(lines[1], "x   ");
    }

    #[test]
    fn overlay_places_status_and_help_on_bottom_two_rows() {
        let mut f = TerminalFrame::new(10, 3);
        overlay_bottom_bar(&mut f, "status", "help");
        let t = f.to_text();
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[1].starts_with("status"));
        assert!(lines[2].starts_with("help"));
    }

    #[test]
    fn out_of_range_row_is_a_noop() {
        let mut f = TerminalFrame::new(4, 1);
        write_line(&mut f, 5, "nope"); // must not panic
        assert_eq!(f.rows(), 1);
    }
}
