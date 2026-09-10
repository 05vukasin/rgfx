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

/// Overlays a bordered, box-drawn panel at the top-left of `frame`: a `title` row followed by one
/// row per entry in `lines`, framed by `┌─┐ │ └─┘` borders. Only the panel's own cells are
/// overwritten, so the render stays visible around it; the panel width tracks the widest line
/// (clamped to the frame) and any rows past the frame height are dropped. No-op for an empty
/// frame.
pub(crate) fn overlay_panel(frame: &mut TerminalFrame, title: &str, lines: &[String]) {
    let cols = frame.cols();
    let rows = frame.rows();
    if cols == 0 || rows == 0 {
        return;
    }

    // Panel = 2 borders + 1 leading pad space + widest text (+1 trailing pad implied by the fit).
    let widest = std::iter::once(title.chars().count())
        .chain(lines.iter().map(|l| l.chars().count()))
        .max()
        .unwrap_or(0);
    let panel_w = (widest + 4).min(cols).max(2);

    let mut row = 0usize;
    panel_border(frame, row, panel_w, '┌', '┐');
    row += 1;

    for text in std::iter::once(title).chain(lines.iter().map(String::as_str)) {
        if row >= rows {
            return; // clipped: skip the remaining rows and the bottom border
        }
        panel_content(frame, row, panel_w, text);
        row += 1;
    }

    if row < rows {
        panel_border(frame, row, panel_w, '└', '┘');
    }
}

/// Draws one horizontal panel border row of width `w` with the given `left`/`right` corner glyphs.
fn panel_border(frame: &mut TerminalFrame, row: usize, w: usize, left: char, right: char) {
    for c in 0..w {
        let ch = if c == 0 {
            left
        } else if c == w - 1 {
            right
        } else {
            '─'
        };
        frame.set(c, row, Cell::glyph(ch));
    }
}

/// Draws one panel content row: vertical borders at the edges and ` text` (leading pad, clipped)
/// filling the interior.
fn panel_content(frame: &mut TerminalFrame, row: usize, w: usize, text: &str) {
    frame.set(0, row, Cell::glyph('│'));
    frame.set(w - 1, row, Cell::glyph('│'));
    let mut interior = format!(" {text}").chars().collect::<Vec<_>>().into_iter();
    for c in 1..w - 1 {
        frame.set(c, row, Cell::glyph(interior.next().unwrap_or(' ')));
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
    fn overlay_panel_draws_a_bordered_box() {
        let mut f = TerminalFrame::new(20, 6);
        overlay_panel(
            &mut f,
            "Light",
            &["mode world".to_string(), "on".to_string()],
        );
        let t = f.to_text();
        let lines: Vec<&str> = t.lines().collect();
        // Top border starts with the corner + a horizontal run; content rows are bracketed by │.
        assert!(lines[0].starts_with('┌'));
        assert!(lines[1].starts_with('│') && lines[1].contains("Light"));
        assert!(lines[2].contains("mode world"));
        // A bottom border row is drawn below the content (title + 2 lines => row 4).
        assert!(lines[4].starts_with('└'));
    }

    #[test]
    fn overlay_panel_clips_to_a_short_frame_without_panicking() {
        let mut f = TerminalFrame::new(8, 1);
        overlay_panel(&mut f, "Light", &["a".to_string(), "b".to_string()]);
        assert_eq!(f.rows(), 1); // only the top border fits; no panic
    }
}
