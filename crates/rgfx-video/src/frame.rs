//! Chunking a raw `rgb24` byte stream into fixed-size frames.
//!
//! `ffmpeg -f rawvideo -pix_fmt rgb24 -` emits an unframed stream of
//! `width * height * 3` bytes per frame with no delimiters. [`FrameReader`]
//! turns any [`Read`] source into a sequence of exact-size frame buffers,
//! reusing a single backing allocation. It is generic over the reader so it can
//! be unit tested against an in-memory byte buffer without spawning ffmpeg.

use rgfx_core::{Error, Result};
use std::io::{self, Read};

/// Reads fixed-size `rgb24` frames from an underlying byte stream.
///
/// Each frame is `width * height * 3` bytes. The same internal buffer is reused
/// across frames, so [`FrameReader::next_frame`] performs no per-frame heap
/// allocation.
#[derive(Debug)]
pub struct FrameReader<R> {
    reader: R,
    frame_len: usize,
    buf: Vec<u8>,
}

impl<R: Read> FrameReader<R> {
    /// Creates a reader that yields `width` × `height` `rgb24` frames from
    /// `reader`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] if the dimensions are zero or their byte size
    /// overflows `usize`.
    pub fn new(reader: R, width: u32, height: u32) -> Result<Self> {
        let frame_len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(3))
            .ok_or_else(|| Error::Config("video frame byte size overflows usize".into()))?;
        if frame_len == 0 {
            return Err(Error::Config(format!(
                "video frame has zero size ({width}x{height})"
            )));
        }
        Ok(Self {
            reader,
            frame_len,
            buf: vec![0u8; frame_len],
        })
    }

    /// The size of one frame in bytes (`width * height * 3`).
    pub fn frame_len(&self) -> usize {
        self.frame_len
    }

    /// Reads the next full frame.
    ///
    /// Returns `Ok(Some(frame))` with a slice into the reused buffer, `Ok(None)`
    /// at a clean end of stream (a frame boundary), or [`Error::Decode`] if the
    /// stream ends partway through a frame (truncated).
    ///
    /// # Errors
    ///
    /// Propagates underlying [`Error::Io`] read failures and reports truncated
    /// trailing data as [`Error::Decode`]. Never panics.
    pub fn next_frame(&mut self) -> Result<Option<&[u8]>> {
        let mut filled = 0;
        while filled < self.frame_len {
            match self.reader.read(&mut self.buf[filled..]) {
                Ok(0) => {
                    if filled == 0 {
                        return Ok(None);
                    }
                    return Err(Error::Decode(format!(
                        "truncated video frame: read {filled} of {} bytes",
                        self.frame_len
                    )));
                }
                Ok(n) => filled += n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::Io(e)),
            }
        }
        Ok(Some(&self.buf[..self.frame_len]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn chunks_synthetic_stream_into_frames() {
        // 2x2 rgb24 => 12 bytes per frame; build 3 distinct frames.
        let frame_len = 2 * 2 * 3;
        let mut data = Vec::new();
        for f in 0..3u8 {
            data.extend(std::iter::repeat_n(f, frame_len));
        }
        let mut reader = FrameReader::new(Cursor::new(data), 2, 2).unwrap();
        assert_eq!(reader.frame_len(), frame_len);

        for f in 0..3u8 {
            let frame = reader.next_frame().unwrap().expect("frame present");
            assert_eq!(frame.len(), frame_len);
            assert!(frame.iter().all(|&b| b == f), "frame {f} bytes");
        }
        // Clean EOF at a frame boundary.
        assert!(reader.next_frame().unwrap().is_none());
        // Idempotent at EOF.
        assert!(reader.next_frame().unwrap().is_none());
    }

    #[test]
    fn handles_reads_split_across_frame_boundary() {
        // A reader that dribbles a few bytes at a time exercises the fill loop.
        struct Dribble {
            data: Vec<u8>,
            pos: usize,
            chunk: usize,
        }
        impl Read for Dribble {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                let remaining = self.data.len() - self.pos;
                let n = remaining.min(self.chunk).min(out.len());
                out[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
                self.pos += n;
                Ok(n)
            }
        }

        let frame_len = 3 * 3; // 3x1 rgb24 = 9 bytes
        let data: Vec<u8> = (0..(frame_len as u8 * 2)).collect();
        let dribble = Dribble {
            data: data.clone(),
            pos: 0,
            chunk: 4,
        };
        let mut reader = FrameReader::new(dribble, 3, 1).unwrap();
        let f0 = reader.next_frame().unwrap().unwrap().to_vec();
        let f1 = reader.next_frame().unwrap().unwrap().to_vec();
        assert_eq!(f0, data[..frame_len]);
        assert_eq!(f1, data[frame_len..]);
        assert!(reader.next_frame().unwrap().is_none());
    }

    #[test]
    fn truncated_frame_is_decode_error() {
        // 10 bytes is not a whole number of 12-byte frames.
        let mut reader = FrameReader::new(Cursor::new(vec![0u8; 10]), 2, 2).unwrap();
        assert!(matches!(reader.next_frame(), Err(Error::Decode(_))));
    }

    #[test]
    fn zero_dimensions_rejected() {
        assert!(matches!(
            FrameReader::new(Cursor::new(Vec::new()), 0, 4),
            Err(Error::Config(_))
        ));
    }
}
