//! Subprocess-backed playback control built on the pure [`Playback`] engine.
//!
//! This module is compiled only with the `ffmpeg` cargo feature: it drives a
//! [`VideoSource`] child process. [`Player`] pairs that source with a
//! [`Playback`] timing model to offer pause/resume, seek (by respawning
//! `ffmpeg` at a `-ss` offset), restart, adaptive frame skipping, and state
//! queries. All timing and skip decisions live in [`Playback`]; this type only
//! translates them into decoder actions. It never touches the terminal — the
//! CLI owns the render loop and paces it with [`Player::time_until_next_frame`].

use crate::playback::{MonotonicClock, Playback, PlaybackStatus};
use crate::source::VideoSource;
use rgfx_core::{Framebuffer, Result, Viewport};
use rgfx_image::RenderOptions;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A controllable video player: a [`VideoSource`] plus playback timing.
///
/// The player produces frames and timing; it does not render to the terminal.
/// A typical loop asks [`Player::time_until_next_frame`] how long to sleep,
/// then calls [`Player::next_frame`] to advance (which drops frames as needed
/// to stay in sync), and encodes the resulting [`Framebuffer`] elsewhere.
pub struct Player {
    path: PathBuf,
    viewport: Viewport,
    opts: RenderOptions,
    source: VideoSource,
    playback: Playback<MonotonicClock>,
}

impl Player {
    /// Opens `path` for playback into `viewport` using `opts`, dropping at most
    /// `max_skip` frames per step when catching up.
    ///
    /// Playback starts in the playing state. See
    /// [`crate::playback::DEFAULT_MAX_SKIP`] for a sensible default bound.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`VideoSource::open`] when probing or
    /// spawning `ffmpeg` fails. Never panics.
    pub fn open(
        path: impl AsRef<Path>,
        viewport: Viewport,
        opts: RenderOptions,
        max_skip: u32,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let source = VideoSource::open(&path, viewport, opts)?;
        let info = source.info();
        let duration = info
            .duration_secs
            .and_then(|s| (s.is_finite() && s >= 0.0).then(|| Duration::from_secs_f64(s)));
        let playback = Playback::monotonic(info.fps, duration, max_skip);
        Ok(Self {
            path,
            viewport,
            opts,
            source,
            playback,
        })
    }

    /// Resumes playback. No effect if already playing.
    pub fn play(&mut self) {
        self.playback.play();
    }

    /// Pauses playback, halting frame advancement. No effect if already paused.
    pub fn pause(&mut self) {
        self.playback.pause();
    }

    /// Toggles between playing and paused.
    pub fn toggle(&mut self) {
        self.playback.toggle();
    }

    /// Whether playback is currently paused.
    pub fn is_paused(&self) -> bool {
        self.playback.is_paused()
    }

    /// Seeks to `target` by respawning `ffmpeg` at that offset (`-ss`) and
    /// resetting the timing epoch. The pause state is preserved.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`VideoSource::open_at`] if respawning the
    /// decoder fails. On error the previous source has already been dropped, so
    /// the player must not be used further. Never panics.
    pub fn seek(&mut self, target: Duration) -> Result<()> {
        self.source = VideoSource::open_at(&self.path, self.viewport, self.opts, Some(target))?;
        self.playback.seek_to(target);
        Ok(())
    }

    /// Restarts playback from the beginning by respawning `ffmpeg`.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`VideoSource::open`] if respawning fails.
    /// Never panics.
    pub fn restart(&mut self) -> Result<()> {
        self.source = VideoSource::open(&self.path, self.viewport, self.opts)?;
        self.playback.restart();
        Ok(())
    }

    /// How long to wait before the next frame is due (the CLI's sleep primitive).
    ///
    /// Returns [`Duration::ZERO`] when a frame is already due or the frame rate
    /// is unknown. Callers should idle while [`Player::is_paused`].
    pub fn time_until_next_frame(&self) -> Duration {
        self.playback.time_until_next_frame()
    }

    /// A snapshot of the current playback state (position, duration, paused, fps).
    pub fn status(&self) -> PlaybackStatus {
        self.playback.status()
    }

    /// Advances playback by one presented frame, rendering it into `target`.
    ///
    /// When behind schedule, up to `max_skip` frames are decoded and discarded
    /// first so the presented frame matches the wall clock. While paused this
    /// is a no-op that leaves `target` unchanged and returns `Ok(true)`, so the
    /// caller keeps displaying the last frame. Returns `Ok(false)` once the
    /// source is exhausted.
    ///
    /// # Errors
    ///
    /// Propagates decode/IO errors from the underlying [`VideoSource`]. Never
    /// panics.
    pub fn next_frame(&mut self, target: &mut Framebuffer) -> Result<bool> {
        use rgfx_core::FrameSource;

        if self.playback.is_paused() {
            return Ok(true);
        }

        let skip = self.playback.frames_to_skip();
        let mut dropped = 0;
        for _ in 0..skip {
            if !self.source.skip_frame()? {
                // Reached end of stream while dropping frames.
                self.playback.on_present(dropped);
                return Ok(false);
            }
            dropped += 1;
        }

        let produced = self.source.next_frame(target)?;
        if produced {
            self.playback.on_present(dropped);
        }
        Ok(produced)
    }

    /// The probed stream metadata of the current source.
    pub fn info(&self) -> &crate::VideoInfo {
        self.source.info()
    }
}
