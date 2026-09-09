//! A batching buffered writer for terminal output.
//!
//! The one performance rule for terminal output is: **one flush per frame**. Rendering a frame
//! cell-by-cell with `print!` triggers a syscall per write and tears visibly. [`BufferedWriter`]
//! instead accumulates all the bytes for a frame in an in-memory buffer and writes them to the
//! underlying sink in a single [`flush`](BufferedWriter::flush) — one `write_all` plus one
//! `flush` on the inner writer.
//!
//! It is generic over any [`std::io::Write`], so the frame-diff engine (task 007) and the tests
//! can drive it with an in-memory `Vec<u8>` instead of a real terminal.

use std::io::{self, Write};

/// A writer that batches a frame's worth of output and flushes it in one shot.
///
/// Bytes queued via [`queue`](Self::queue) / [`queue_bytes`](Self::queue_bytes) accumulate in an
/// internal buffer and are not sent anywhere until [`flush`](Self::flush) is called. The internal
/// buffer's capacity is retained across frames so steady-state rendering does not reallocate.
#[derive(Debug)]
pub struct BufferedWriter<W: Write> {
    inner: W,
    buf: Vec<u8>,
}

impl<W: Write> BufferedWriter<W> {
    /// Wraps `inner` in a fresh buffered writer with an empty buffer.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buf: Vec::new(),
        }
    }

    /// Wraps `inner`, pre-allocating `capacity` bytes for the frame buffer.
    pub fn with_capacity(inner: W, capacity: usize) -> Self {
        Self {
            inner,
            buf: Vec::with_capacity(capacity),
        }
    }

    /// Appends string bytes to the frame buffer without touching the underlying writer.
    pub fn queue(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
    }

    /// Appends raw bytes to the frame buffer without touching the underlying writer.
    pub fn queue_bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The number of bytes currently queued and not yet flushed.
    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    /// Whether nothing is queued.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Writes all queued bytes to the underlying writer in a single `write_all`, flushes the
    /// underlying writer once, then clears the frame buffer (retaining its capacity).
    ///
    /// If nothing is queued this is a no-op and the underlying writer is left untouched — no
    /// spurious flush is issued for an empty frame.
    pub fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        self.inner.write_all(&self.buf)?;
        self.inner.flush()?;
        self.buf.clear();
        Ok(())
    }

    /// Discards any queued bytes without writing them.
    pub fn discard(&mut self) {
        self.buf.clear();
    }

    /// A shared reference to the underlying writer.
    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    /// A mutable reference to the underlying writer.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Consumes the buffered writer, returning the underlying writer. Any unflushed bytes are
    /// dropped; call [`flush`](Self::flush) first to keep them.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};

    /// An in-memory writer that counts how many times `flush` and `write` are called, so tests
    /// can assert the "single flush per frame" contract.
    #[derive(Default)]
    struct CountingWriter {
        data: Vec<u8>,
        writes: usize,
        flushes: usize,
    }

    impl Write for CountingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.data.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn queue_does_not_write_until_flush() {
        let mut w = BufferedWriter::new(CountingWriter::default());
        w.queue("hello ");
        w.queue("world");
        assert_eq!(w.buffered_len(), 11);
        assert_eq!(w.get_ref().writes, 0);
        assert_eq!(w.get_ref().flushes, 0);

        w.flush().unwrap();
        assert_eq!(w.get_ref().data, b"hello world");
        assert_eq!(w.buffered_len(), 0);
        assert!(w.is_empty());
    }

    #[test]
    fn flush_batches_many_queues_into_one_write_and_one_flush() {
        let mut w = BufferedWriter::new(CountingWriter::default());
        for _ in 0..100 {
            w.queue("x");
        }
        w.flush().unwrap();
        assert_eq!(
            w.get_ref().writes,
            1,
            "should write the whole frame in one write_all"
        );
        assert_eq!(w.get_ref().flushes, 1, "exactly one flush per frame");
        assert_eq!(w.get_ref().data.len(), 100);
    }

    #[test]
    fn empty_flush_is_a_noop() {
        let mut w = BufferedWriter::new(CountingWriter::default());
        w.flush().unwrap();
        assert_eq!(w.get_ref().writes, 0);
        assert_eq!(w.get_ref().flushes, 0);
    }

    #[test]
    fn discard_drops_queued_bytes() {
        let mut w = BufferedWriter::new(CountingWriter::default());
        w.queue("garbage");
        w.discard();
        w.flush().unwrap();
        assert!(w.get_ref().data.is_empty());
        assert_eq!(w.get_ref().flushes, 0);
    }

    #[test]
    fn buffer_capacity_is_retained_across_frames() {
        let mut w = BufferedWriter::with_capacity(CountingWriter::default(), 64);
        w.queue("frame one");
        w.flush().unwrap();
        w.queue("frame two");
        w.flush().unwrap();
        assert_eq!(w.get_ref().data, b"frame oneframe two");
        assert_eq!(w.get_ref().writes, 2);
        assert_eq!(w.get_ref().flushes, 2);
    }
}
