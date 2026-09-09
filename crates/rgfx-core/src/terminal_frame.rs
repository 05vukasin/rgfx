//! The encoded terminal output: a grid of character cells produced by a [`TerminalEncoder`].
//!
//! [`TerminalEncoder`]: crate::TerminalEncoder

use crate::Color;

/// A single encoded terminal cell: a character plus optional foreground/background colors.
///
/// A `None` color means "use the terminal default"; a grayscale encoder leaves both colors
/// `None` and relies on the glyph alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    /// The glyph to draw in this cell.
    pub ch: char,
    /// Optional foreground color.
    pub fg: Option<Color>,
    /// Optional background color.
    pub bg: Option<Color>,
}

impl Cell {
    /// A cell with a glyph and no explicit colors.
    pub const fn glyph(ch: char) -> Self {
        Self {
            ch,
            fg: None,
            bg: None,
        }
    }

    /// A cell with a glyph and a foreground color.
    pub const fn colored(ch: char, fg: Color) -> Self {
        Self {
            ch,
            fg: Some(fg),
            bg: None,
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Cell::glyph(' ')
    }
}

/// A rectangular grid of encoded [`Cell`]s, row-major, ready to be diffed and written to a
/// terminal by the frame engine.
#[derive(Clone, Debug, Default)]
pub struct TerminalFrame {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
}

impl TerminalFrame {
    /// Creates a frame of `cols` × `rows` cells filled with blank spaces.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
        }
    }

    /// The width in cells.
    #[inline]
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// The height in cells.
    #[inline]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Resizes the frame, reusing the allocation when possible. Contents are reset to blanks.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        self.cells.clear();
        self.cells.resize(cols * rows, Cell::default());
    }

    /// The cell at `(col, row)`. Panics in debug builds if out of bounds.
    #[inline]
    pub fn get(&self, col: usize, row: usize) -> Cell {
        debug_assert!(col < self.cols && row < self.rows);
        self.cells[row * self.cols + col]
    }

    /// Sets the cell at `(col, row)`. Panics in debug builds if out of bounds.
    #[inline]
    pub fn set(&mut self, col: usize, row: usize, cell: Cell) {
        debug_assert!(col < self.cols && row < self.rows);
        let i = row * self.cols + col;
        self.cells[i] = cell;
    }

    /// The full cell grid as a slice, row-major.
    #[inline]
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Renders the frame to a plain `String` (glyphs only, newline-separated rows).
    ///
    /// Colors are ignored; this is the form used for `--output` text export and snapshot tests.
    pub fn to_text(&self) -> String {
        let mut s = String::with_capacity(self.cells.len() + self.rows);
        for row in 0..self.rows {
            if row > 0 {
                s.push('\n');
            }
            for col in 0..self.cols {
                s.push(self.get(col, row).ch);
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_frame_is_blank() {
        let f = TerminalFrame::new(3, 2);
        assert_eq!(f.cols(), 3);
        assert_eq!(f.rows(), 2);
        assert!(f.cells().iter().all(|c| c.ch == ' '));
    }

    #[test]
    fn set_get_and_to_text() {
        let mut f = TerminalFrame::new(2, 2);
        f.set(0, 0, Cell::glyph('a'));
        f.set(1, 0, Cell::glyph('b'));
        f.set(0, 1, Cell::glyph('c'));
        f.set(1, 1, Cell::glyph('d'));
        assert_eq!(f.get(1, 0).ch, 'b');
        assert_eq!(f.to_text(), "ab\ncd");
    }

    #[test]
    fn resize_changes_dimensions() {
        let mut f = TerminalFrame::new(2, 2);
        f.resize(4, 1);
        assert_eq!((f.cols(), f.rows()), (4, 1));
        assert_eq!(f.cells().len(), 4);
    }
}
