//! Frame timing: target-FPS pacing and adaptive frame skipping.
//!
//! Diffing decides *what* to draw; this module decides *when*. [`FrameClock`] paces presentation to
//! a target frame rate: when a frame is ready ahead of schedule it reports how long to wait, and
//! when rendering has fallen more than a whole frame behind (a slow decode, a stall) it reports a
//! [`FramePacing::Skip`] so the caller can drop that frame and resynchronize instead of spiraling
//! further behind. This is what keeps video playback at the right wall-clock speed.
//!
//! Time is abstracted behind the [`Clock`] trait so the pacing math is fully deterministic under a
//! mock clock in tests — no real sleeping, no wall-clock flakiness. Production code uses
//! [`SystemClock`], which reads a monotonic [`Instant`] and sleeps the current thread.

use std::time::{Duration, Instant};

/// A monotonic time source plus a sleep primitive, abstracted so timing can be mocked in tests.
///
/// [`now`](Clock::now) returns elapsed time since the clock's own epoch (construction), which is
/// all the pacing math needs and which a mock can drive to exact values.
pub trait Clock {
    /// The monotonic time elapsed since this clock's epoch.
    fn now(&self) -> Duration;
    /// Blocks for (at least) `dur`. A mock may instead record the request and advance its clock.
    fn sleep(&self, dur: Duration);
}

/// The real clock: a monotonic [`Instant`] and `std::thread::sleep`.
#[derive(Clone, Copy, Debug)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    /// Creates a clock whose epoch is now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }

    fn sleep(&self, dur: Duration) {
        std::thread::sleep(dur);
    }
}

/// What the caller should do at a frame boundary, as decided by [`FrameClock::schedule`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramePacing {
    /// Ahead of schedule: wait this long (e.g. via [`Clock::sleep`]) before presenting the frame.
    Sleep(Duration),
    /// On schedule (or only slightly behind): present the frame immediately.
    Present,
    /// More than a whole frame behind: drop this frame and let the clock resynchronize.
    Skip,
}

/// Paces frame presentation to a target frame rate with adaptive skipping when behind.
///
/// Call [`schedule`](Self::schedule) once per frame to get a [`FramePacing`] decision without
/// blocking, or [`pace`](Self::pace) to additionally perform the sleep through the [`Clock`]. Each
/// call advances the internal schedule by one frame period.
#[derive(Clone, Copy, Debug)]
pub struct FrameClock<C: Clock> {
    clock: C,
    frame_duration: Duration,
    deadline: Duration,
    started: bool,
}

impl FrameClock<SystemClock> {
    /// Creates a [`FrameClock`] backed by a fresh [`SystemClock`] at `target_fps`.
    #[must_use]
    pub fn with_target_fps(target_fps: f64) -> Self {
        Self::new(SystemClock::new(), target_fps)
    }
}

impl<C: Clock> FrameClock<C> {
    /// Creates a clock pacing to `target_fps` frames per second.
    ///
    /// A non-finite or non-positive `target_fps` disables pacing (zero frame duration), so every
    /// frame is presented immediately with no sleeping and no skipping.
    #[must_use]
    pub fn new(clock: C, target_fps: f64) -> Self {
        let frame_duration = if target_fps.is_finite() && target_fps > 0.0 {
            Duration::from_secs_f64(1.0 / target_fps)
        } else {
            Duration::ZERO
        };
        Self::with_frame_duration(clock, frame_duration)
    }

    /// Creates a clock with an explicit per-frame period.
    ///
    /// A zero `frame_duration` disables pacing (every frame is presented immediately).
    #[must_use]
    pub fn with_frame_duration(clock: C, frame_duration: Duration) -> Self {
        Self {
            clock,
            frame_duration,
            deadline: Duration::ZERO,
            started: false,
        }
    }

    /// The target period between frames.
    #[must_use]
    pub fn frame_duration(&self) -> Duration {
        self.frame_duration
    }

    /// The target frame rate in frames per second, or [`f64::INFINITY`] when pacing is disabled.
    #[must_use]
    pub fn target_fps(&self) -> f64 {
        if self.frame_duration.is_zero() {
            f64::INFINITY
        } else {
            1.0 / self.frame_duration.as_secs_f64()
        }
    }

    /// A shared reference to the underlying clock.
    #[must_use]
    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// Decides how to present the next frame, advancing the schedule by one frame period.
    ///
    /// The decision, given the current time `now` and the scheduled `deadline` for this frame:
    /// - the very first frame presents immediately and arms the schedule;
    /// - `now < deadline` → ahead of schedule, [`FramePacing::Sleep`] for the remainder;
    /// - behind by less than one frame → [`FramePacing::Present`] immediately;
    /// - behind by a whole frame or more → [`FramePacing::Skip`], and the deadline jumps forward
    ///   past `now` so the clock catches up instead of accumulating lag.
    ///
    /// This performs no blocking; use [`pace`](Self::pace) to also sleep.
    pub fn schedule(&mut self) -> FramePacing {
        let now = self.clock.now();

        if !self.started {
            self.started = true;
            self.deadline = now + self.frame_duration;
            return FramePacing::Present;
        }

        if self.frame_duration.is_zero() {
            // Pacing disabled: always present, keep the deadline pinned to now.
            self.deadline = now;
            return FramePacing::Present;
        }

        if now < self.deadline {
            let wait = self.deadline - now;
            self.deadline += self.frame_duration;
            FramePacing::Sleep(wait)
        } else {
            let behind = now - self.deadline;
            if behind >= self.frame_duration {
                // Fallen a whole frame (or more) behind: skip this frame and resync the deadline
                // to just after `now` so we do not spiral.
                let whole = (behind.as_nanos() / self.frame_duration.as_nanos()) as u32 + 1;
                self.deadline += self.frame_duration * whole;
                FramePacing::Skip
            } else {
                self.deadline += self.frame_duration;
                FramePacing::Present
            }
        }
    }

    /// Like [`schedule`](Self::schedule), but performs the [`Clock::sleep`] when the decision is
    /// [`FramePacing::Sleep`]. Returns the decision so the caller can still act on a `Skip`.
    pub fn pace(&mut self) -> FramePacing {
        let pacing = self.schedule();
        if let FramePacing::Sleep(dur) = pacing {
            self.clock.sleep(dur);
        }
        pacing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// A fully controllable clock. `now` is set directly; `sleep` is recorded and advances `now`,
    /// so a [`FrameClock::pace`] call behaves as if time really elapsed.
    #[derive(Default)]
    struct MockClock {
        now: Cell<Duration>,
        sleeps: RefCell<Vec<Duration>>,
    }

    impl MockClock {
        fn set(&self, now: Duration) {
            self.now.set(now);
        }
        fn sleeps(&self) -> Vec<Duration> {
            self.sleeps.borrow().clone()
        }
    }

    impl Clock for &MockClock {
        fn now(&self) -> Duration {
            self.now.get()
        }
        fn sleep(&self, dur: Duration) {
            self.sleeps.borrow_mut().push(dur);
            self.now.set(self.now.get() + dur);
        }
    }

    const FRAME: Duration = Duration::from_millis(100); // 10 fps

    fn clock_at_10fps(mock: &MockClock) -> FrameClock<&MockClock> {
        FrameClock::with_frame_duration(mock, FRAME)
    }

    #[test]
    fn fps_maps_to_frame_duration() {
        let mock = MockClock::default();
        let fc = FrameClock::new(&mock, 60.0);
        assert_eq!(fc.frame_duration(), Duration::from_secs_f64(1.0 / 60.0));
        // Roundtripping fps -> nanosecond Duration -> fps loses a little precision.
        assert!((fc.target_fps() - 60.0).abs() < 1e-3);
    }

    #[test]
    fn first_frame_presents_immediately() {
        let mock = MockClock::default();
        let mut fc = clock_at_10fps(&mock);
        assert_eq!(fc.schedule(), FramePacing::Present);
    }

    #[test]
    fn ahead_of_schedule_sleeps_the_remainder() {
        let mock = MockClock::default();
        let mut fc = clock_at_10fps(&mock);
        // First frame at t=0 arms the deadline at 100ms.
        assert_eq!(fc.schedule(), FramePacing::Present);
        // Second frame is ready instantly (still t=0): sleep the full frame.
        assert_eq!(fc.schedule(), FramePacing::Sleep(FRAME));
    }

    #[test]
    fn slightly_behind_presents_without_skipping() {
        let mock = MockClock::default();
        let mut fc = clock_at_10fps(&mock);
        assert_eq!(fc.schedule(), FramePacing::Present); // deadline -> 100ms
        // 20ms late: within one frame, so present now (no skip).
        mock.set(Duration::from_millis(120));
        assert_eq!(fc.schedule(), FramePacing::Present); // deadline -> 200ms
    }

    #[test]
    fn far_behind_skips_and_resynchronizes() {
        let mock = MockClock::default();
        let mut fc = clock_at_10fps(&mock);
        assert_eq!(fc.schedule(), FramePacing::Present); // deadline -> 100ms
        // 350ms elapsed: 250ms behind, i.e. more than two whole frames -> skip.
        mock.set(Duration::from_millis(350));
        assert_eq!(fc.schedule(), FramePacing::Skip);
        // The deadline resynced past now (to 400ms). A frame ready now must wait ~50ms.
        assert_eq!(fc.schedule(), FramePacing::Sleep(Duration::from_millis(50)));
    }

    #[test]
    fn pace_sleeps_through_the_clock() {
        let mock = MockClock::default();
        let mut fc = clock_at_10fps(&mock);
        assert_eq!(fc.pace(), FramePacing::Present); // first frame, no sleep
        // Second frame ready instantly -> pace() should sleep one full frame via the clock.
        let pacing = fc.pace();
        assert_eq!(pacing, FramePacing::Sleep(FRAME));
        assert_eq!(mock.sleeps(), vec![FRAME]);
        // Having slept, the mock clock advanced to the deadline; the next frame is on time.
        assert_eq!(fc.schedule(), FramePacing::Sleep(FRAME));
    }

    #[test]
    fn zero_fps_disables_pacing() {
        let mock = MockClock::default();
        let mut fc = FrameClock::new(&mock, 0.0);
        assert_eq!(fc.frame_duration(), Duration::ZERO);
        assert!(fc.target_fps().is_infinite());
        assert_eq!(fc.schedule(), FramePacing::Present);
        mock.set(Duration::from_millis(500));
        assert_eq!(fc.schedule(), FramePacing::Present);
    }
}
