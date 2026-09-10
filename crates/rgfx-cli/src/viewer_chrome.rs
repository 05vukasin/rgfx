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

/// Draws a bordered, left-aligned panel of text `lines` with its top-left corner at
/// (`left`, `top`), floating over the frame (only the panel's own cells are overwritten; the
/// render behind it is left intact). Used for the modal light menu overlay.
///
/// The panel is sized to the widest line, clipped to the frame, and drawn with Unicode box-drawing
/// borders. A single horizontal space pads each side of the content. No-op for an empty frame or a
/// top-left origin already off the frame.
pub(crate) fn overlay_panel(frame: &mut TerminalFrame, left: usize, top: usize, lines: &[String]) {
    let (cols, rows) = (frame.cols(), frame.rows());
    if cols == 0 || rows == 0 || left >= cols || top >= rows || lines.is_empty() {
        return;
    }
    let content_w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    // Inner width includes one space of padding on each side.
    let inner_w = content_w + 2;

    let mut put = |col: usize, row: usize, ch: char| {
        if col < cols && row < rows {
            frame.set(col, row, Cell::glyph(ch));
        }
    };

    // Top border: ┌──…──┐
    put(left, top, '┌');
    for i in 0..inner_w {
        put(left + 1 + i, top, '─');
    }
    put(left + 1 + inner_w, top, '┐');

    // Content rows: │ text … │ (padded to inner width).
    for (i, line) in lines.iter().enumerate() {
        let row = top + 1 + i;
        if row >= rows {
            break;
        }
        put(left, row, '│');
        put(left + 1, row, ' ');
        let mut col = left + 2;
        for ch in line.chars() {
            put(col, row, ch);
            col += 1;
        }
        // Pad the remainder of the inner width with spaces.
        while col < left + 1 + inner_w {
            put(col, row, ' ');
            col += 1;
        }
        put(left + 1 + inner_w, row, '│');
    }

    // Bottom border: └──…──┘
    let bottom = top + 1 + lines.len();
    if bottom < rows {
        put(left, bottom, '└');
        for i in 0..inner_w {
            put(left + 1 + i, bottom, '─');
        }
        put(left + 1 + inner_w, bottom, '┘');
    }
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

    #[test]
    fn overlay_panel_draws_a_bordered_box_with_content() {
        let mut f = TerminalFrame::new(20, 6);
        let lines = vec!["Light".to_string(), "on".to_string()];
        overlay_panel(&mut f, 0, 0, &lines);
        let t = f.to_text();
        let rows: Vec<&str> = t.lines().collect();
        // Top border, two content rows framed by │, then the bottom border.
        assert!(rows[0].starts_with("┌"));
        assert!(rows[1].starts_with("│ Light"));
        assert!(rows[2].starts_with("│ on"));
        assert!(rows[3].starts_with("└"));
        // Content rows are closed on the right by a matching border.
        assert!(rows[1].contains('│') && rows[1].trim_end().ends_with('│'));
    }

    #[test]
    fn overlay_panel_off_frame_origin_is_a_noop() {
        let mut f = TerminalFrame::new(4, 2);
        overlay_panel(&mut f, 10, 10, &["x".to_string()]); // must not panic
        assert_eq!((f.cols(), f.rows()), (4, 2));
    }
}
