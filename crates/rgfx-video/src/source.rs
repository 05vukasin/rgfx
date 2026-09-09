//! The subprocess-driven decoding entry points: [`probe`] and [`VideoSource`].
//!
//! This module is compiled only with the `ffmpeg` cargo feature. It never links
//! ffmpeg; it locates the `ffmpeg`/`ffprobe` executables at runtime and drives
//! them as child processes, so a missing binary yields an actionable
//! [`Error::External`] rather than a build failure or panic.

use crate::convert::render_rgb24_into;
use crate::detect::{find_ffmpeg, find_ffprobe};
use crate::frame::FrameReader;
use crate::info::VideoInfo;
use rgfx_core::{Error, FrameSource, Framebuffer, Result, Viewport};
use rgfx_image::RenderOptions;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

/// Probes `path` with `ffprobe` and returns a [`VideoInfo`] summary.
///
/// Runs `ffprobe -v error -print_format json -show_streams -show_format <path>`
/// and parses its JSON report.
///
/// # Errors
///
/// Returns [`Error::External`] if `ffprobe` is missing or exits non-zero, and
/// [`Error::Decode`] if its output cannot be parsed or has no video stream.
/// Never panics.
pub fn probe(path: impl AsRef<Path>) -> Result<VideoInfo> {
    let path = path.as_ref();
    let ffprobe = find_ffprobe()?;
    let output = Command::new(&ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-print_format")
        .arg("json")
        .arg("-show_streams")
        .arg("-show_format")
        .arg(path)
        .output()
        .map_err(|e| Error::External(format!("failed to run ffprobe: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::External(format!(
            "ffprobe failed for {}: {}",
            path.display(),
            stderr.trim()
        )));
    }

    let json = String::from_utf8_lossy(&output.stdout);
    VideoInfo::from_ffprobe_json(&json)
}

/// A [`FrameSource`] that decodes a video file into framebuffers via `ffmpeg`.
///
/// On [`VideoSource::open`] the file is probed for its dimensions and frame
/// rate, then `ffmpeg` is spawned to emit a raw `rgb24` stream on stdout. Each
/// [`FrameSource::next_frame`] reads exactly one frame and converts it into the
/// caller's reused [`Framebuffer`], honoring the target [`Viewport`].
///
/// The child process is killed and reaped when the source is exhausted and
/// again on drop, so no zombie process is left behind.
pub struct VideoSource {
    child: Child,
    reader: FrameReader<ChildStdout>,
    info: VideoInfo,
    viewport: Viewport,
    opts: RenderOptions,
    finished: bool,
}

impl VideoSource {
    /// Opens `path` for decoding into `viewport` using `opts`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::External`] if `ffmpeg`/`ffprobe` are missing or fail to
    /// spawn, or [`Error::Decode`]/[`Error::Config`] if the probe reports an
    /// unusable stream. Never panics.
    pub fn open(path: impl AsRef<Path>, viewport: Viewport, opts: RenderOptions) -> Result<Self> {
        Self::open_at(path, viewport, opts, None)
    }

    /// Opens `path` for decoding into `viewport`, starting playback at `start`.
    ///
    /// When `start` is `Some`, `ffmpeg` is spawned with an input `-ss` seek so
    /// its output stream begins at (approximately, ±1 frame) that offset. This
    /// backs [`crate::Player::seek`] by respawning the decoder at the target
    /// position. `None` decodes from the beginning, exactly like
    /// [`VideoSource::open`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::External`] if `ffmpeg`/`ffprobe` are missing or fail to
    /// spawn, or [`Error::Decode`]/[`Error::Config`] if the probe reports an
    /// unusable stream. Never panics.
    pub fn open_at(
        path: impl AsRef<Path>,
        viewport: Viewport,
        opts: RenderOptions,
        start: Option<Duration>,
    ) -> Result<Self> {
        let path = path.as_ref();
        let info = probe(path)?;
        let ffmpeg = find_ffmpeg()?;

        let mut command = Command::new(&ffmpeg);
        command.arg("-nostdin").arg("-loglevel").arg("error");
        // Input seeking (`-ss` before `-i`) is fast and frame-accurate enough
        // for scrubbing; place it ahead of the input as ffmpeg requires.
        if let Some(offset) = start {
            command.arg("-ss").arg(crate::playback::format_ss(offset));
        }
        let mut child = command
            .arg("-i")
            .arg(path)
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("rgb24")
            .arg("-")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::External(format!("failed to spawn ffmpeg: {e}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::External("ffmpeg produced no stdout pipe".into()))?;

        let reader = match FrameReader::new(stdout, info.width, info.height) {
            Ok(reader) => reader,
            Err(e) => {
                // Reap the child we already spawned before bubbling the error up.
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };

        Ok(Self {
            child,
            reader,
            info,
            viewport,
            opts,
            finished: false,
        })
    }

    /// The probed stream metadata (dimensions, frame rate, duration, codec).
    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// The viewport this source renders into.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Decodes and discards the next frame without converting it into a
    /// framebuffer.
    ///
    /// This backs the player's adaptive frame skipping: dropping a frame here
    /// avoids the resize/blit work that [`FrameSource::next_frame`] performs,
    /// so catching up stays cheap. Returns `Ok(true)` if a frame was dropped,
    /// `Ok(false)` at end of stream.
    ///
    /// # Errors
    ///
    /// Propagates the same decode/IO errors as [`FrameSource::next_frame`],
    /// reaping the child before reporting. Never panics.
    pub fn skip_frame(&mut self) -> Result<bool> {
        if self.finished {
            return Ok(false);
        }
        match self.reader.next_frame() {
            Ok(Some(_)) => Ok(true),
            Ok(None) => {
                self.finished = true;
                self.reap();
                Ok(false)
            }
            Err(e) => {
                self.finished = true;
                self.reap();
                Err(e)
            }
        }
    }

    /// Kills and reaps the ffmpeg child, ignoring errors (it may already have
    /// exited on its own at end of stream).
    fn reap(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl FrameSource for VideoSource {
    fn next_frame(&mut self, target: &mut Framebuffer) -> Result<bool> {
        if self.finished {
            return Ok(false);
        }
        let (w, h) = (self.info.width, self.info.height);
        match self.reader.next_frame() {
            Ok(Some(frame)) => {
                render_rgb24_into(frame, w, h, target, self.viewport, &self.opts)?;
                Ok(true)
            }
            Ok(None) => {
                self.finished = true;
                self.reap();
                Ok(false)
            }
            Err(e) => {
                // A decode/IO failure ends the stream; reap before reporting.
                self.finished = true;
                self.reap();
                Err(e)
            }
        }
    }

    fn frame_delay(&self) -> Option<Duration> {
        self.info.frame_delay()
    }
}

impl Drop for VideoSource {
    fn drop(&mut self) {
        // Ensure the child is never left running, even if the caller stops
        // iterating early (broken pipe on ffmpeg's side is expected here).
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Generates a tiny test clip with ffmpeg, or returns `None` if ffmpeg is
    /// not installed (so the test degrades to a skip rather than a failure).
    fn make_test_clip(secs: u32, fps: u32, w: u32, h: u32) -> Option<std::path::PathBuf> {
        let ffmpeg = find_ffmpeg().ok()?;
        let mut path = std::env::temp_dir();
        path.push(format!("rgfx_video_test_{}.mp4", std::process::id()));
        let status = Command::new(ffmpeg)
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg(format!("testsrc=duration={secs}:size={w}x{h}:rate={fps}"))
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        if status.success() && path.is_file() {
            Some(path)
        } else {
            None
        }
    }

    #[test]
    #[ignore = "requires ffmpeg/ffprobe installed; run with --ignored"]
    fn end_to_end_probe_and_decode_real_clip() {
        let Some(clip) = make_test_clip(1, 10, 32, 24) else {
            // ffmpeg not available: nothing to assert.
            return;
        };

        // Probe reports the requested geometry and frame rate.
        let info = probe(&clip).unwrap();
        assert_eq!((info.width, info.height), (32, 24));
        assert!((info.fps - 10.0).abs() < 0.5, "fps {}", info.fps);

        // Decoding yields roughly the expected number of frames (10 fps * 1s).
        let mut src =
            VideoSource::open(&clip, Viewport::new(20, 10), RenderOptions::braille()).unwrap();
        assert!(src.frame_delay().is_some());
        let mut fb = Framebuffer::new(0, 0);
        let mut count = 0;
        while src.next_frame(&mut fb).unwrap() {
            assert_eq!((fb.width(), fb.height()), (40, 40));
            count += 1;
        }
        assert!((9..=11).contains(&count), "decoded {count} frames");
        // Exhausted source keeps returning false.
        assert!(!src.next_frame(&mut fb).unwrap());

        let _ = std::fs::remove_file(&clip);
    }

    #[test]
    #[ignore = "requires ffprobe installed; run with --ignored"]
    fn probe_errors_on_non_video_input() {
        if find_ffprobe().is_err() {
            return;
        }
        let mut path = std::env::temp_dir();
        path.push(format!("rgfx_video_notvideo_{}.txt", std::process::id()));
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"not a video").unwrap();
        }
        assert!(probe(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
