//! The terminal-independent timing and state model behind video playback.
//!
//! This module is pure: it spawns no process and touches no terminal, so its
//! entire state machine — pause/resume, position tracking, adaptive frame
//! skipping, and `-ss` seek-argument formatting — compiles and is unit-tested
//! without the `ffmpeg` cargo feature. The `ffmpeg`-gated [`crate::Player`]
//! layers subprocess control on top of the [`Playback`] engine defined here.
//!
//! Timing is driven through the [`Clock`] abstraction rather than reading the
//! wall clock directly, so tests can advance a deterministic mock clock. In
//! production the [`MonotonicClock`] wraps [`std::time::Instant`].

use std::time::{Duration, Instant};

/// A default upper bound on how many frames the adaptive skipper may drop in a
/// single step to catch up to the wall clock. Bounding the skip keeps a large
/// stall (for example the process being suspended) from discarding a long run
/// of frames all at once.
pub const DEFAULT_MAX_SKIP: u32 = 8;

/// A monotonic time source, abstracted so playback timing can be exercised with
/// a deterministic mock clock instead of real wall-clock time.
pub trait Clock {
    /// Elapsed time since the clock's own reference epoch.
    ///
    /// Implementations must be monotonic non-decreasing; [`Playback`] relies on
    /// this to compute how much playback time has passed between calls.
    fn elapsed(&self) -> Duration;
}

/// A [`Clock`] backed by [`std::time::Instant`], measuring real elapsed time
/// from the moment it was created.
#[derive(Clone, Copy, Debug)]
pub struct MonotonicClock {
    start: Instant,
}

impl MonotonicClock {
    /// Creates a clock whose epoch is the current instant.
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

/// A snapshot of playback state for display or logging by the caller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaybackStatus {
    /// The current playback position from the start of the media.
    pub position: Duration,
    /// The total duration, if the source reported one.
    pub duration: Option<Duration>,
    /// Whether playback is currently paused.
    pub paused: bool,
    /// The source frame rate in frames per second (`0.0` if unknown).
    pub fps: f64,
}

/// The pure timing and state engine for video playback.
///
/// [`Playback`] tracks the playback position, the pause/resume state machine,
/// and the adaptive frame-skip decision, all without knowing anything about
/// `ffmpeg` or the terminal. It is generic over a [`Clock`] so tests can supply
/// a deterministic mock; the production alias uses [`MonotonicClock`].
///
/// # Model
///
/// Position advances only while playing. Frame indices are tracked relative to
/// the current *timing epoch*, which resets on [`Playback::seek_to`] /
/// [`Playback::restart`] — this mirrors an `ffmpeg -ss` respawn, whose output
/// stream begins again at frame zero from the seek point.
#[derive(Clone, Copy, Debug)]
pub struct Playback<C: Clock = MonotonicClock> {
    clock: C,
    fps: f64,
    frame_delay: Option<Duration>,
    duration: Option<Duration>,
    max_skip: u32,
    paused: bool,
    /// Playback time accumulated before the current running segment (excludes
    /// paused intervals). Frozen value while paused.
    played_before: Duration,
    /// `clock.elapsed()` captured when the current running segment began.
    segment_start: Duration,
    /// Absolute position of the current timing epoch, set by a seek.
    seek_base: Duration,
    /// Frames presented since the current timing epoch.
    presented: u64,
}

impl<C: Clock> Playback<C> {
    /// Creates a playback engine for a stream of `fps` frames per second and an
    /// optional total `duration`, dropping at most `max_skip` frames per step
    /// when catching up. Playback starts in the playing (not paused) state.
    ///
    /// An `fps` of `0.0` (or non-finite) marks the frame rate as unknown: the
    /// pacing primitives then return zero and the caller must drive timing
    /// externally.
    pub fn new(fps: f64, duration: Option<Duration>, max_skip: u32, clock: C) -> Self {
        let segment_start = clock.elapsed();
        Self {
            clock,
            fps,
            frame_delay: frame_delay_from_fps(fps),
            duration,
            max_skip,
            paused: false,
            played_before: Duration::ZERO,
            segment_start,
            seek_base: Duration::ZERO,
            presented: 0,
        }
    }

    /// Playback time elapsed since the current timing epoch, excluding any time
    /// spent paused.
    fn playback_elapsed(&self) -> Duration {
        if self.paused {
            self.played_before
        } else {
            self.played_before + self.clock.elapsed().saturating_sub(self.segment_start)
        }
    }

    /// Resumes playback if paused. No effect if already playing.
    pub fn play(&mut self) {
        if self.paused {
            self.segment_start = self.clock.elapsed();
            self.paused = false;
        }
    }

    /// Pauses playback, freezing the position. No effect if already paused.
    pub fn pause(&mut self) {
        if !self.paused {
            self.played_before = self.playback_elapsed();
            self.paused = true;
        }
    }

    /// Toggles between playing and paused.
    pub fn toggle(&mut self) {
        if self.paused {
            self.play();
        } else {
            self.pause();
        }
    }

    /// Whether playback is currently paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// The source frame rate in frames per second (`0.0` if unknown).
    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// The delay between consecutive frames, or `None` when the frame rate is
    /// unknown.
    pub fn frame_delay(&self) -> Option<Duration> {
        self.frame_delay
    }

    /// The total media duration, if known.
    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }

    /// The current playback position from the start of the media, clamped to
    /// [`Playback::duration`] when known.
    pub fn position(&self) -> Duration {
        let pos = self.seek_base + self.playback_elapsed();
        match self.duration {
            Some(d) if pos > d => d,
            _ => pos,
        }
    }

    /// Repositions playback to `target`, resetting the timing epoch so frame
    /// indices count from the seek point. The pause state is preserved. The
    /// target is clamped to [`Playback::duration`] when known.
    ///
    /// This updates only the timing model; the `ffmpeg`-gated [`crate::Player`]
    /// pairs it with an `ffmpeg` respawn at the same offset.
    pub fn seek_to(&mut self, target: Duration) {
        let target = match self.duration {
            Some(d) => target.min(d),
            None => target,
        };
        self.seek_base = target;
        self.played_before = Duration::ZERO;
        self.segment_start = self.clock.elapsed();
        self.presented = 0;
    }

    /// Repositions playback to the start. Equivalent to seeking to zero.
    pub fn restart(&mut self) {
        self.seek_to(Duration::ZERO);
    }

    /// How long to wait before the next frame is due, or [`Duration::ZERO`] if
    /// it is already due (or the frame rate is unknown).
    ///
    /// This is the pacing primitive a render loop sleeps on between frames.
    /// Callers should check [`Playback::is_paused`] first and idle while paused
    /// rather than spinning on this value.
    pub fn time_until_next_frame(&self) -> Duration {
        let Some(frame_delay) = self.frame_delay else {
            return Duration::ZERO;
        };
        let due = frame_delay.as_secs_f64() * self.presented as f64;
        let elapsed = self.playback_elapsed().as_secs_f64();
        if elapsed >= due {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(due - elapsed)
        }
    }

    /// The number of frames to drop before presenting the next one, so that the
    /// presented frame matches the current wall-clock position.
    ///
    /// Returns `0` when on time, paused, or when the frame rate is unknown, and
    /// never more than the configured `max_skip`.
    pub fn frames_to_skip(&self) -> u32 {
        if self.paused {
            return 0;
        }
        let Some(frame_delay) = self.frame_delay else {
            return 0;
        };
        let frame_delay = frame_delay.as_secs_f64();
        if frame_delay <= 0.0 {
            return 0;
        }
        // The frame index that should be on screen for the current position.
        let ideal = (self.playback_elapsed().as_secs_f64() / frame_delay).floor();
        let behind = ideal - self.presented as f64;
        if behind <= 0.0 {
            0
        } else {
            (behind as u32).min(self.max_skip)
        }
    }

    /// Records that a frame was presented after dropping `skipped` frames,
    /// advancing the presented-frame counter by `skipped + 1`.
    pub fn on_present(&mut self, skipped: u32) {
        self.presented = self.presented.saturating_add(u64::from(skipped) + 1);
    }

    /// A snapshot of the current playback state.
    pub fn status(&self) -> PlaybackStatus {
        PlaybackStatus {
            position: self.position(),
            duration: self.duration,
            paused: self.paused,
            fps: self.fps,
        }
    }
}

impl Playback<MonotonicClock> {
    /// Creates a playback engine driven by a real [`MonotonicClock`].
    pub fn monotonic(fps: f64, duration: Option<Duration>, max_skip: u32) -> Self {
        Self::new(fps, duration, max_skip, MonotonicClock::new())
    }
}

/// Derives the per-frame delay from a frame rate, returning `None` when the
/// rate is unknown or non-positive.
fn frame_delay_from_fps(fps: f64) -> Option<Duration> {
    if fps.is_finite() && fps > 0.0 {
        Some(Duration::from_secs_f64(1.0 / fps))
    } else {
        None
    }
}

/// Formats a seek offset as the seconds string `ffmpeg` expects after `-ss`
/// (for example `2.500` or `90.000`), with millisecond precision.
///
/// Only the `ffmpeg`-gated decoder (and this module's tests) build the seek
/// argument, so the helper is compiled just for those configurations.
#[cfg(any(feature = "ffmpeg", test))]
pub(crate) fn format_ss(offset: Duration) -> String {
    format!("{:.3}", offset.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A [`Clock`] whose elapsed time is set explicitly by the test, giving a
    /// deterministic mock timeline.
    struct ManualClock {
        now: Cell<Duration>,
    }

    impl ManualClock {
        fn new() -> Self {
            Self {
                now: Cell::new(Duration::ZERO),
            }
        }

        /// Sets the absolute elapsed time.
        fn set_ms(&self, ms: u64) {
            self.now.set(Duration::from_millis(ms));
        }
    }

    impl Clock for &ManualClock {
        fn elapsed(&self) -> Duration {
            self.now.get()
        }
    }

    /// A 10 fps engine (100 ms/frame) with a 10 s duration, driven by `clock`.
    fn engine(clock: &ManualClock) -> Playback<&ManualClock> {
        Playback::new(10.0, Some(Duration::from_secs(10)), DEFAULT_MAX_SKIP, clock)
    }

    #[test]
    fn pause_freezes_position_and_resume_continues() {
        let clock = ManualClock::new();
        let mut pb = engine(&clock);
        assert!(!pb.is_paused());

        // Play for 250 ms.
        clock.set_ms(250);
        assert_eq!(pb.position(), Duration::from_millis(250));

        // Pause: position stays put even as the clock keeps moving.
        pb.pause();
        assert!(pb.is_paused());
        clock.set_ms(1000);
        assert_eq!(pb.position(), Duration::from_millis(250));

        // Resume: the paused interval (750 ms) does not count.
        pb.play();
        assert!(!pb.is_paused());
        clock.set_ms(1400); // 400 ms of real time since resume
        assert_eq!(pb.position(), Duration::from_millis(650));
    }

    #[test]
    fn toggle_flips_state() {
        let clock = ManualClock::new();
        let mut pb = engine(&clock);
        pb.toggle();
        assert!(pb.is_paused());
        pb.toggle();
        assert!(!pb.is_paused());
    }

    #[test]
    fn paused_engine_never_skips() {
        let clock = ManualClock::new();
        let mut pb = engine(&clock);
        pb.pause();
        clock.set_ms(5_000);
        assert_eq!(pb.frames_to_skip(), 0);
    }

    #[test]
    fn skip_count_tracks_how_far_behind() {
        let clock = ManualClock::new();
        let pb = engine(&clock);
        // On time at t=0: nothing presented, no skip.
        assert_eq!(pb.frames_to_skip(), 0);

        // 350 ms elapsed at 100 ms/frame => frame index 3 should show; with
        // nothing presented yet we are behind by 3.
        clock.set_ms(350);
        assert_eq!(pb.frames_to_skip(), 3);

        // 990 ms => index 9, still under the max_skip bound of 8 -> clamped.
        clock.set_ms(990);
        assert_eq!(pb.frames_to_skip(), DEFAULT_MAX_SKIP);
    }

    #[test]
    fn skip_bound_is_configurable() {
        let clock = ManualClock::new();
        let pb = Playback::new(10.0, None, 2, &clock);
        clock.set_ms(350); // behind by 3
        assert_eq!(pb.frames_to_skip(), 2);
    }

    #[test]
    fn presenting_frames_advances_and_clears_the_backlog() {
        let clock = ManualClock::new();
        let mut pb = engine(&clock);
        clock.set_ms(350); // behind by 3
        let skip = pb.frames_to_skip();
        assert_eq!(skip, 3);
        // Drop the skipped frames and present one: 4 frames consumed.
        pb.on_present(skip);
        // Now caught up (index 3 presented, index 3 is the current frame).
        assert_eq!(pb.frames_to_skip(), 0);
    }

    #[test]
    fn unknown_fps_disables_pacing() {
        let clock = ManualClock::new();
        let pb = Playback::new(0.0, None, DEFAULT_MAX_SKIP, &clock);
        assert_eq!(pb.frame_delay(), None);
        assert_eq!(pb.frames_to_skip(), 0);
        assert_eq!(pb.time_until_next_frame(), Duration::ZERO);
    }

    #[test]
    fn time_until_next_frame_counts_down_then_hits_zero() {
        let clock = ManualClock::new();
        let pb = engine(&clock);
        // Next frame (index 0) is due at t=0 already.
        assert_eq!(pb.time_until_next_frame(), Duration::ZERO);

        // After presenting one frame, the next is due at 100 ms.
        let mut pb = pb;
        pb.on_present(0);
        clock.set_ms(40);
        assert_eq!(pb.time_until_next_frame(), Duration::from_millis(60));
        clock.set_ms(100);
        assert_eq!(pb.time_until_next_frame(), Duration::ZERO);
    }

    #[test]
    fn seek_repositions_and_resets_frame_epoch() {
        let clock = ManualClock::new();
        let mut pb = engine(&clock);
        clock.set_ms(200);
        pb.on_present(1); // presented two frames

        pb.seek_to(Duration::from_secs(5));
        assert_eq!(pb.position(), Duration::from_secs(5));
        // Frame epoch reset: nothing behind immediately after the seek.
        assert_eq!(pb.frames_to_skip(), 0);

        // Position advances from the seek base as time passes.
        clock.set_ms(500); // 300 ms after the seek at t=200
        assert_eq!(pb.position(), Duration::from_millis(5300));
    }

    #[test]
    fn seek_clamps_to_duration() {
        let clock = ManualClock::new();
        let mut pb = engine(&clock);
        pb.seek_to(Duration::from_secs(999));
        assert_eq!(pb.position(), Duration::from_secs(10));
    }

    #[test]
    fn restart_returns_to_zero() {
        let clock = ManualClock::new();
        let mut pb = engine(&clock);
        clock.set_ms(4_000);
        pb.restart();
        assert_eq!(pb.position(), Duration::ZERO);
    }

    #[test]
    fn position_is_clamped_to_duration() {
        let clock = ManualClock::new();
        let pb = engine(&clock);
        clock.set_ms(60_000); // far past the 10 s duration
        assert_eq!(pb.position(), Duration::from_secs(10));
    }

    #[test]
    fn status_reports_the_snapshot() {
        let clock = ManualClock::new();
        let mut pb = engine(&clock);
        clock.set_ms(1_000);
        pb.pause();
        let s = pb.status();
        assert_eq!(s.position, Duration::from_secs(1));
        assert_eq!(s.duration, Some(Duration::from_secs(10)));
        assert!(s.paused);
        assert!((s.fps - 10.0).abs() < 1e-9);
    }

    #[test]
    fn format_ss_uses_millisecond_precision() {
        assert_eq!(format_ss(Duration::from_millis(2500)), "2.500");
        assert_eq!(format_ss(Duration::from_secs(90)), "90.000");
        assert_eq!(format_ss(Duration::ZERO), "0.000");
        assert_eq!(format_ss(Duration::from_millis(1234)), "1.234");
    }
}
