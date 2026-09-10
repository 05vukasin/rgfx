//! The frame engine: double-buffered diffing that writes only the cells that changed.
//!
//! [`FrameEngine`] is the performance core of the terminal layer. It keeps the frame currently on
//! screen in a *front* buffer and, each time a new [`TerminalFrame`] is presented, walks the two
//! grids in lockstep and emits terminal output **only for the cells that differ** — one cursor
//! move plus the changed run of glyphs (and, when a [`ColorMode`] is active, the SGR escapes for
//! that run). Unchanged cells cost nothing. Identical consecutive frames emit nothing at all.
//!
//! Two situations force a full redraw instead of a diff: the very first frame (there is nothing on
//! screen to diff against) and any change in the grid's dimensions (a resize invalidates every
//! coordinate). A full redraw clears the screen, homes the cursor, and writes the whole frame via
//! the shared [`AnsiSerializer`], so the diff engine stays consistent with the rest of the
//! pipeline.
//!
//! Both buffers are reused across frames — the engine never allocates a fresh [`TerminalFrame`]
//! per frame. After emitting a frame's output the new contents are copied into the reused *back*
//! buffer and the two buffers are swapped, so the allocations ping-pong and steady-state rendering
//! does not touch the allocator.
//!
//! Per the crate's architectural law this module performs terminal I/O only through the
//! caller-supplied [`Write`], and it issues exactly one `write_all` plus one `flush` per non-empty
//! frame — the "single flush per frame" rule that keeps interactive rendering tear-free.
//!
//! [`ColorMode`]: crate::ColorMode

use std::io::{self, Write};

use rgfx_core::TerminalFrame;

use crate::color::AnsiColor;
use crate::serializer::AnsiSerializer;

/// The `\x1b[0m` reset that returns the terminal to its default colors.
const SGR_RESET: &[u8] = b"\x1b[0m";
/// Clear the entire screen and move the cursor to the home position (row 1, column 1).
const CLEAR_HOME: &[u8] = b"\x1b[2J\x1b[H";

/// A double-buffered terminal renderer that writes only the cells that change between frames.
///
/// Construct one with the [`ColorMode`](crate::ColorMode) the output should be quantized to, then
/// call [`render`](Self::render) once per frame with the next [`TerminalFrame`] and the writer to
/// emit to. The engine owns and reuses its buffers; do not recreate it per frame.
#[derive(Debug)]
pub struct FrameEngine {
    /// The frame currently on screen — what the next frame is diffed against.
    front: TerminalFrame,
    /// A reused scratch buffer that becomes the new front buffer after each frame (swapped in).
    back: TerminalFrame,
    /// The serializer used for full-redraw output; also the source of truth for the color mode.
    serializer: AnsiSerializer,
    /// Reused byte buffer holding a frame's worth of emitted output.
    scratch: Vec<u8>,
    /// Reused string buffer for building a single SGR parameter body.
    color_scratch: String,
    /// Whether at least one frame has been presented (false forces a full redraw).
    initialized: bool,
}

impl FrameEngine {
    /// Creates a frame engine that quantizes color to `mode`.
    ///
    /// The engine starts with empty buffers; the first [`render`](Self::render) call is always a
    /// full redraw.
    #[must_use]
    pub fn new(mode: crate::ColorMode) -> Self {
        Self {
            front: TerminalFrame::default(),
            back: TerminalFrame::default(),
            serializer: AnsiSerializer::new(mode),
            scratch: Vec::new(),
            color_scratch: String::new(),
            initialized: false,
        }
    }

    /// The color mode this engine emits at.
    #[must_use]
    pub fn mode(&self) -> crate::ColorMode {
        self.serializer.mode()
    }

    /// The frame currently on screen (the front buffer).
    ///
    /// Immediately after [`render`](Self::render) returns, this reflects the just-presented frame.
    #[must_use]
    pub fn front(&self) -> &TerminalFrame {
        &self.front
    }

    /// Diffs `next` against the frame on screen, writes only what changed to `out`, and records
    /// `next` as the new on-screen frame.
    ///
    /// On the first call, or whenever `next`'s dimensions differ from the frame on screen, the
    /// whole frame is redrawn (screen cleared, cursor homed, full grid written). Otherwise only the
    /// changed cell runs are emitted, each preceded by a single cursor move.
    ///
    /// Output is accumulated in an internal buffer and sent to `out` with exactly one `write_all`
    /// followed by one `flush`. If nothing changed (identical frames) nothing is written and `out`
    /// is not flushed.
    ///
    /// # Errors
    /// Propagates any [`io::Error`] from writing to or flushing `out`.
    pub fn render<W: Write>(&mut self, next: &TerminalFrame, out: &mut W) -> io::Result<()> {
        self.scratch.clear();

        let resized = next.cols() != self.front.cols() || next.rows() != self.front.rows();
        if !self.initialized || resized {
            self.emit_full_redraw(next);
        } else {
            self.emit_diff(next);
        }

        // Record `next` as the new on-screen frame by copying it into the reused back buffer and
        // swapping. This keeps both allocations alive and recycled across frames.
        Self::copy_into(&mut self.back, next);
        std::mem::swap(&mut self.front, &mut self.back);
        self.initialized = true;

        if !self.scratch.is_empty() {
            out.write_all(&self.scratch)?;
            out.flush()?;
        }
        Ok(())
    }

    /// Appends a full-frame redraw (clear + home + the whole serialized grid) to the scratch buffer.
    ///
    /// The interactive viewers run in raw mode, where a bare `\n` line-feeds **without** returning
    /// the cursor to column 0, so relying on the serializer's `\n` row separators would staircase
    /// every row to the right. Each serialized row is therefore preceded by an absolute cursor move
    /// (`\x1b[{row+1};1H`), matching the diff path, so the redraw positions correctly in raw mode.
    /// Cursor moves do not touch SGR state, so the serializer's cross-row color continuity is
    /// preserved. (File output via [`TerminalFrame::to_text`] is unaffected — it keeps plain `\n`.)
    fn emit_full_redraw(&mut self, next: &TerminalFrame) {
        self.scratch.extend_from_slice(CLEAR_HOME);
        // The serializer separates rows with a single `\n` (and never emits `\n` within a row), so
        // splitting on `\n` recovers exactly one segment per grid row, SGR escapes still embedded.
        let bytes = self.serializer.serialize(next);
        for (row, line) in bytes.split(|&b| b == b'\n').enumerate() {
            // Cursor move to (row+1, 1), 1-based: ESC [ <row+1> ; 1 H.
            self.scratch.extend_from_slice(b"\x1b[");
            push_uint(&mut self.scratch, row + 1);
            self.scratch.extend_from_slice(b";1H");
            self.scratch.extend_from_slice(line);
        }
    }

    /// Appends the changed cell runs of `next` (relative to the front buffer) to the scratch buffer.
    fn emit_diff(&mut self, next: &TerminalFrame) {
        let cols = next.cols();
        let rows = next.rows();
        for row in 0..rows {
            let mut col = 0;
            while col < cols {
                if next.get(col, row) != self.front.get(col, row) {
                    let start = col;
                    while col < cols && next.get(col, row) != self.front.get(col, row) {
                        col += 1;
                    }
                    self.emit_run(next, row, start, col);
                } else {
                    col += 1;
                }
            }
        }
    }

    /// Emits one changed run `[start, end)` on `row`: a cursor move followed by the run's glyphs.
    ///
    /// Color state is treated as fresh at the start of every run (the preceding cursor move breaks
    /// any carried-over run), so the first colored cell emits its escape and a single reset closes
    /// the run if any color was written.
    fn emit_run(&mut self, next: &TerminalFrame, row: usize, start: usize, end: usize) {
        // Cursor move to (row, start), 1-based: ESC [ <row+1> ; <start+1> H.
        self.scratch.extend_from_slice(b"\x1b[");
        push_uint(&mut self.scratch, row + 1);
        self.scratch.push(b';');
        push_uint(&mut self.scratch, start + 1);
        self.scratch.push(b'H');

        let mode = self.serializer.mode();
        let mut last_fg: Option<AnsiColor> = None;
        let mut last_bg: Option<AnsiColor> = None;
        let mut emitted_color = false;
        let mut glyph = [0u8; 4];

        for col in start..end {
            let cell = next.get(col, row);
            if mode != crate::ColorMode::None {
                let fg = cell.fg.and_then(|c| AnsiColor::quantize(c, mode));
                let bg = cell.bg.and_then(|c| AnsiColor::quantize(c, mode));
                if fg != last_fg || bg != last_bg {
                    self.push_transition(last_fg, fg, last_bg, bg);
                    last_fg = fg;
                    last_bg = bg;
                    emitted_color = true;
                }
            }
            self.scratch
                .extend_from_slice(cell.ch.encode_utf8(&mut glyph).as_bytes());
        }

        if emitted_color {
            self.scratch.extend_from_slice(SGR_RESET);
        }
    }

    /// Appends a single SGR escape covering exactly the foreground/background that changed.
    ///
    /// A color that becomes `None` (terminal default) is written as `39` (fg) / `49` (bg). Mirrors
    /// [`AnsiSerializer`]'s transition logic so diffed runs match full-redraw output byte for byte.
    fn push_transition(
        &mut self,
        last_fg: Option<AnsiColor>,
        fg: Option<AnsiColor>,
        last_bg: Option<AnsiColor>,
        bg: Option<AnsiColor>,
    ) {
        self.color_scratch.clear();
        if fg != last_fg {
            match fg {
                Some(c) => c.write_params(true, &mut self.color_scratch),
                None => self.color_scratch.push_str("39"),
            }
        }
        if bg != last_bg {
            if !self.color_scratch.is_empty() {
                self.color_scratch.push(';');
            }
            match bg {
                Some(c) => c.write_params(false, &mut self.color_scratch),
                None => self.color_scratch.push_str("49"),
            }
        }
        if !self.color_scratch.is_empty() {
            self.scratch.extend_from_slice(b"\x1b[");
            self.scratch
                .extend_from_slice(self.color_scratch.as_bytes());
            self.scratch.push(b'm');
        }
    }

    /// Copies `src`'s dimensions and cells into `dst`, reusing `dst`'s existing allocation.
    ///
    /// Only resizes (which can reallocate) when the dimensions actually differ; a same-size copy
    /// overwrites in place and never touches the allocator.
    fn copy_into(dst: &mut TerminalFrame, src: &TerminalFrame) {
        if dst.cols() != src.cols() || dst.rows() != src.rows() {
            dst.resize(src.cols(), src.rows());
        }
        for row in 0..src.rows() {
            for col in 0..src.cols() {
                dst.set(col, row, src.get(col, row));
            }
        }
    }
}

/// Appends the base-10 ASCII digits of `n` to `buf` without allocating.
fn push_uint(buf: &mut Vec<u8>, mut n: usize) {
    if n == 0 {
        buf.push(b'0');
        return;
    }
    // usize is at most 20 decimal digits (u64::MAX == 18446744073709551615).
    let mut tmp = [0u8; 20];
    let mut i = tmp.len();
    while n > 0 {
        i -= 1;
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    buf.extend_from_slice(&tmp[i..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ColorMode;
    use rgfx_core::{Cell, Color, TerminalFrame};
    use std::io::{self, Write};

    /// A `Write` that records bytes and counts `write`/`flush` calls, so tests can assert both the
    /// exact output and the "single flush per frame" contract.
    #[derive(Default)]
    struct RecordingWriter {
        data: Vec<u8>,
        flushes: usize,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.data.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    /// A `cols`×`rows` frame whose every cell holds `ch`.
    fn filled(cols: usize, rows: usize, ch: char) -> TerminalFrame {
        let mut f = TerminalFrame::new(cols, rows);
        for row in 0..rows {
            for col in 0..cols {
                f.set(col, row, Cell::glyph(ch));
            }
        }
        f
    }

    /// Builds the expected full-redraw bytes for `frame`: clear+home, then every serialized row
    /// preceded by an absolute cursor move (the raw-mode-safe positioning `emit_full_redraw` uses).
    fn expected_full_redraw(frame: &TerminalFrame) -> Vec<u8> {
        let mut expected = b"\x1b[2J\x1b[H".to_vec();
        let text = frame.to_text();
        for (row, line) in text.split('\n').enumerate() {
            expected.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
            expected.extend_from_slice(line.as_bytes());
        }
        expected
    }

    #[test]
    fn first_frame_is_a_full_redraw() {
        let mut engine = FrameEngine::new(ColorMode::None);
        let frame = filled(3, 2, 'a');
        let mut out = RecordingWriter::default();
        engine.render(&frame, &mut out).unwrap();

        assert_eq!(out.data, expected_full_redraw(&frame));
        assert_eq!(out.flushes, 1, "one flush for the full redraw");
    }

    #[test]
    fn full_redraw_positions_every_row_for_raw_mode() {
        // Regression for #034: in raw mode a bare `\n` line-feeds without a carriage return, so a
        // full redraw that joined rows with `\n` staircased. Assert on the emitted BYTES (a PTY
        // capture that split on `\n` would mask this): every row must be positioned by an absolute
        // cursor move and there must be no bare `\n` left to staircase.
        let mut engine = FrameEngine::new(ColorMode::None);
        let frame = filled(3, 4, 'a');
        let mut out = RecordingWriter::default();
        engine.render(&frame, &mut out).unwrap();

        assert!(
            !out.data.contains(&b'\n'),
            "full redraw must not emit a bare newline that would staircase in raw mode"
        );
        // One `\x1b[<row>;1H` cursor move per grid row (rows are 1-based).
        for row in 1..=4 {
            let needle = format!("\x1b[{row};1H").into_bytes();
            assert!(
                out.data
                    .windows(needle.len())
                    .any(|w| w == needle.as_slice()),
                "row {row} must be positioned with an absolute cursor move"
            );
        }
    }

    #[test]
    fn identical_full_redraws_both_position_rows() {
        // Two consecutive full redraws (forced by a resize between them) must each position rows.
        let mut engine = FrameEngine::new(ColorMode::None);
        let frame = filled(2, 3, 'a');

        let mut first = RecordingWriter::default();
        engine.render(&frame, &mut first).unwrap();
        assert_eq!(first.data, expected_full_redraw(&frame));

        // A different size forces another full redraw; it too must be positioned per row.
        let bigger = filled(2, 4, 'a');
        let mut second = RecordingWriter::default();
        engine.render(&bigger, &mut second).unwrap();
        assert_eq!(second.data, expected_full_redraw(&bigger));
    }

    #[test]
    fn single_cell_change_writes_only_that_run() {
        let mut engine = FrameEngine::new(ColorMode::None);
        let a = filled(3, 2, 'a');
        let mut sink = RecordingWriter::default();
        engine.render(&a, &mut sink).unwrap();

        let mut b = a.clone();
        b.set(1, 0, Cell::glyph('X'));
        let mut out = RecordingWriter::default();
        engine.render(&b, &mut out).unwrap();

        // Cursor to row 1, col 2 (1-based) then just the one changed glyph. No color, no reset.
        assert_eq!(out.data, b"\x1b[1;2HX");
        assert_eq!(out.flushes, 1);
    }

    #[test]
    fn contiguous_changes_coalesce_into_one_run() {
        let mut engine = FrameEngine::new(ColorMode::None);
        let a = filled(5, 1, 'a');
        let mut sink = RecordingWriter::default();
        engine.render(&a, &mut sink).unwrap();

        // Change columns 1,2,3 (contiguous) → one cursor move, three glyphs.
        let mut b = a.clone();
        b.set(1, 0, Cell::glyph('X'));
        b.set(2, 0, Cell::glyph('Y'));
        b.set(3, 0, Cell::glyph('Z'));
        let mut out = RecordingWriter::default();
        engine.render(&b, &mut out).unwrap();
        assert_eq!(out.data, b"\x1b[1;2HXYZ");
    }

    #[test]
    fn separated_changes_emit_separate_runs() {
        let mut engine = FrameEngine::new(ColorMode::None);
        let a = filled(5, 1, 'a');
        let mut sink = RecordingWriter::default();
        engine.render(&a, &mut sink).unwrap();

        // Change columns 0 and 4 (a gap of unchanged cells between) → two runs.
        let mut b = a.clone();
        b.set(0, 0, Cell::glyph('P'));
        b.set(4, 0, Cell::glyph('Q'));
        let mut out = RecordingWriter::default();
        engine.render(&b, &mut out).unwrap();
        assert_eq!(out.data, b"\x1b[1;1HP\x1b[1;5HQ");
    }

    #[test]
    fn whole_row_change_writes_the_full_row_run() {
        let mut engine = FrameEngine::new(ColorMode::None);
        let a = filled(3, 2, 'a');
        let mut sink = RecordingWriter::default();
        engine.render(&a, &mut sink).unwrap();

        // Replace all of row 1 with 'b'.
        let mut b = a.clone();
        for col in 0..3 {
            b.set(col, 1, Cell::glyph('b'));
        }
        let mut out = RecordingWriter::default();
        engine.render(&b, &mut out).unwrap();
        assert_eq!(out.data, b"\x1b[2;1Hbbb");
    }

    #[test]
    fn identical_frames_emit_nothing() {
        let mut engine = FrameEngine::new(ColorMode::None);
        let a = filled(4, 3, 'a');
        let mut sink = RecordingWriter::default();
        engine.render(&a, &mut sink).unwrap();

        let mut out = RecordingWriter::default();
        engine.render(&a, &mut out).unwrap();
        assert!(out.data.is_empty(), "no bytes for an unchanged frame");
        assert_eq!(out.flushes, 0, "no flush issued for an empty frame");
    }

    #[test]
    fn resize_triggers_a_full_redraw() {
        let mut engine = FrameEngine::new(ColorMode::None);
        let a = filled(3, 2, 'a');
        let mut sink = RecordingWriter::default();
        engine.render(&a, &mut sink).unwrap();

        let c = filled(5, 1, 'c');
        let mut out = RecordingWriter::default();
        engine.render(&c, &mut out).unwrap();

        assert_eq!(out.data, expected_full_redraw(&c));
    }

    #[test]
    fn colored_single_cell_change_emits_sgr_and_reset() {
        let mut engine = FrameEngine::new(ColorMode::TrueColor);
        let a = filled(3, 1, 'a');
        let mut sink = RecordingWriter::default();
        engine.render(&a, &mut sink).unwrap();

        let mut b = a.clone();
        b.set(1, 0, Cell::colored('X', Color::from_u8(200, 10, 20, 255)));
        let mut out = RecordingWriter::default();
        engine.render(&b, &mut out).unwrap();

        // Cursor move, then the fresh color for the run, the glyph, then a reset.
        assert_eq!(out.data, b"\x1b[1;2H\x1b[38;2;200;10;20mX\x1b[0m");
    }

    #[test]
    fn buffers_are_reused_across_frames_no_reallocation() {
        let mut engine = FrameEngine::new(ColorMode::None);
        let mut sink = RecordingWriter::default();

        // Render several same-sized frames and capture the front buffer's backing pointer each
        // time. Because presenting swaps the two reused buffers, the pointer ping-pongs between
        // exactly two allocations; if either were reallocated the parity check below would fail.
        let mut ptrs = Vec::new();
        for i in 0..5 {
            let frame = filled(8, 4, char::from(b'a' + i as u8));
            engine.render(&frame, &mut sink).unwrap();
            ptrs.push(engine.front().cells().as_ptr());
        }

        // Frames 2 and 4 share one allocation; frames 3 and 5 share the other. Stable => no realloc.
        assert_eq!(
            ptrs[1], ptrs[3],
            "even frames must reuse the same allocation"
        );
        assert_eq!(
            ptrs[2], ptrs[4],
            "odd frames must reuse the same allocation"
        );
        assert_eq!(engine.front().cells().len(), 32);
    }

    #[test]
    fn front_buffer_tracks_the_presented_frame() {
        let mut engine = FrameEngine::new(ColorMode::None);
        let a = filled(2, 2, 'a');
        let mut sink = RecordingWriter::default();
        engine.render(&a, &mut sink).unwrap();
        assert_eq!(engine.front().to_text(), a.to_text());

        let mut b = a.clone();
        b.set(0, 0, Cell::glyph('Z'));
        engine.render(&b, &mut sink).unwrap();
        assert_eq!(engine.front().to_text(), b.to_text());
    }
}
