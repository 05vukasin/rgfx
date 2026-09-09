//! Reusable, encoder-agnostic animation primitives.
//!
//! These types are the foundation for procedural avatars, loading spinners, and any other
//! time-based sequence of framebuffers. They deliberately know nothing about Braille, ASCII, or
//! any specific [`TerminalEncoder`]: an [`Animation`] is just an ordered list of [`Frame`]s (each a
//! [`Framebuffer`] plus a [`Duration`]) and a [`LoopPolicy`]. An [`AnimationPlayer`] walks that list
//! over time and implements the [`FrameSource`] trait, so it flows through exactly the same
//! render → encode → present pipeline as a decoded image or video.
//!
//! # Driving a player
//!
//! There are two equivalent ways to advance a player, and you pick one for a given loop:
//!
//! - **Time-accurate** — call [`AnimationPlayer::advance`] with the wall-clock time elapsed since
//!   the last tick (crossing as many frame boundaries as that covers), then render the current
//!   frame with [`FrameSource::next_frame`] passing the render-only behaviour aside. This is what
//!   the deterministic tests drive with controlled [`Duration`]s (a "mock clock").
//! - **Frame-stepped** — treat the player purely as a [`FrameSource`]: each
//!   [`next_frame`](FrameSource::next_frame) renders the current frame and then steps forward by one
//!   whole frame, and [`frame_delay`](FrameSource::frame_delay) reports how long to wait before the
//!   next call. A non-looping animation reports `Ok(false)` once exhausted.
//!
//! [`TerminalEncoder`]: crate::TerminalEncoder

use std::time::Duration;

use crate::{Color, Error, FrameSource, Framebuffer, Result};

/// How an [`Animation`] behaves after its last frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopPolicy {
    /// Play once, then stop on the final frame. A player reports exhaustion via
    /// [`FrameSource::next_frame`] returning `Ok(false)`.
    Once,
    /// Restart from the first frame after the last, forever.
    Loop,
    /// Bounce back and forth: forward to the last frame, then backward to the first, repeating.
    PingPong,
}

/// A single animation frame: the pixels to show and how long to show them.
///
/// The pixels live in a [`Framebuffer`] so a frame composes with the rest of the pipeline without
/// any encoder-specific representation. Build one directly from a framebuffer, or from a [`Sprite`]
/// via [`Frame::from_sprite`].
#[derive(Clone, Debug)]
pub struct Frame {
    image: Framebuffer,
    duration: Duration,
}

impl Frame {
    /// Creates a frame that shows `image` for `duration`.
    #[must_use]
    pub fn new(image: Framebuffer, duration: Duration) -> Self {
        Self { image, duration }
    }

    /// Creates a frame from a [`Sprite`]'s bitmap, shown for `duration`.
    #[must_use]
    pub fn from_sprite(sprite: &Sprite, duration: Duration) -> Self {
        Self::new(sprite.image().clone(), duration)
    }

    /// The pixels shown during this frame.
    #[must_use]
    pub fn image(&self) -> &Framebuffer {
        &self.image
    }

    /// How long this frame is shown.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.duration
    }
}

/// A static bitmap that can be composited onto a [`Framebuffer`].
///
/// A sprite is the reusable building block for authoring animation frames: draw once, then
/// [`blit`](Sprite::blit) it onto a target at an offset. Compositing skips fully transparent source
/// pixels so sprites can be layered.
#[derive(Clone, Debug)]
pub struct Sprite {
    image: Framebuffer,
}

impl Sprite {
    /// Wraps an existing bitmap as a sprite.
    #[must_use]
    pub fn new(image: Framebuffer) -> Self {
        Self { image }
    }

    /// The sprite's bitmap.
    #[must_use]
    pub fn image(&self) -> &Framebuffer {
        &self.image
    }

    /// The sprite width in pixels.
    #[must_use]
    pub fn width(&self) -> usize {
        self.image.width()
    }

    /// The sprite height in pixels.
    #[must_use]
    pub fn height(&self) -> usize {
        self.image.height()
    }

    /// Composites the sprite onto `target` with its top-left at `(x, y)`.
    ///
    /// Pixels that fall outside `target` are clipped, and fully transparent source pixels
    /// (`alpha == 0.0`) are left untouched so existing content shows through.
    pub fn blit(&self, target: &mut Framebuffer, x: usize, y: usize) {
        blit(&self.image, target, x, y);
    }
}

/// Copies the non-transparent pixels of `src` onto `dst` with `src`'s top-left at `(ox, oy)`.
fn blit(src: &Framebuffer, dst: &mut Framebuffer, ox: usize, oy: usize) {
    for sy in 0..src.height() {
        let dy = oy + sy;
        if dy >= dst.height() {
            break;
        }
        for sx in 0..src.width() {
            let dx = ox + sx;
            if dx >= dst.width() {
                break;
            }
            let c = src.get(sx, sy);
            if c.a > 0.0 {
                dst.set(dx, dy, c);
            }
        }
    }
}

/// An ordered sequence of [`Frame`]s plus a [`LoopPolicy`].
///
/// An animation is immutable once built and carries no playback state; drive it with an
/// [`AnimationPlayer`]. Construction validates that the sequence is well-formed so playback can
/// never stall: at least one frame, and every frame's duration strictly positive.
#[derive(Clone, Debug)]
pub struct Animation {
    frames: Vec<Frame>,
    loop_policy: LoopPolicy,
}

impl Animation {
    /// Builds an animation from `frames` with the given `loop_policy`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] if `frames` is empty or any frame has a zero duration (a zero
    /// duration would let a looping player make no progress).
    pub fn new(frames: Vec<Frame>, loop_policy: LoopPolicy) -> Result<Self> {
        if frames.is_empty() {
            return Err(Error::Config("animation has no frames".into()));
        }
        if let Some(pos) = frames.iter().position(|f| f.duration.is_zero()) {
            return Err(Error::Config(format!(
                "animation frame {pos} has a zero duration"
            )));
        }
        Ok(Self {
            frames,
            loop_policy,
        })
    }

    /// The frames, in order.
    #[must_use]
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The number of frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the animation has no frames. Always `false` for a successfully constructed value.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The loop policy.
    #[must_use]
    pub fn loop_policy(&self) -> LoopPolicy {
        self.loop_policy
    }

    /// The summed duration of a single pass over every frame.
    #[must_use]
    pub fn total_duration(&self) -> Duration {
        self.frames.iter().map(|f| f.duration).sum()
    }
}

/// A stateful playhead over an [`Animation`] that implements [`FrameSource`].
///
/// The playhead is a `(frame index, time elapsed within that frame)` pair. Advance it with real
/// elapsed time via [`advance`](Self::advance), or step it one whole frame at a time by using it as
/// a [`FrameSource`]. See the module-level documentation for the two driving styles.
#[derive(Clone, Debug)]
pub struct AnimationPlayer {
    animation: Animation,
    index: usize,
    elapsed: Duration,
    forward: bool,
    finished: bool,
}

impl AnimationPlayer {
    /// Creates a player positioned at the first frame of `animation`.
    #[must_use]
    pub fn new(animation: Animation) -> Self {
        Self {
            animation,
            index: 0,
            elapsed: Duration::ZERO,
            forward: true,
            finished: false,
        }
    }

    /// The animation being played.
    #[must_use]
    pub fn animation(&self) -> &Animation {
        &self.animation
    }

    /// The index of the frame currently under the playhead.
    #[must_use]
    pub fn current_index(&self) -> usize {
        self.index
    }

    /// The frame currently under the playhead.
    #[must_use]
    pub fn current_frame(&self) -> &Frame {
        &self.animation.frames[self.index]
    }

    /// Whether a [`LoopPolicy::Once`] animation has played past its final frame.
    ///
    /// Always `false` for [`LoopPolicy::Loop`] and [`LoopPolicy::PingPong`].
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Resets the playhead to the first frame and clears the finished flag.
    pub fn reset(&mut self) {
        self.index = 0;
        self.elapsed = Duration::ZERO;
        self.forward = true;
        self.finished = false;
    }

    /// Advances the playhead by `dt` of elapsed time, crossing as many frame boundaries as `dt`
    /// covers and honouring the [`LoopPolicy`].
    ///
    /// Once a [`LoopPolicy::Once`] animation reaches the end of its last frame it becomes
    /// [`finished`](Self::is_finished) and further advancing is a no-op until [`reset`](Self::reset).
    pub fn advance(&mut self, mut dt: Duration) {
        if self.finished {
            return;
        }
        loop {
            let frame_len = self.animation.frames[self.index].duration;
            let remaining = frame_len.saturating_sub(self.elapsed);
            if dt < remaining {
                self.elapsed += dt;
                return;
            }
            dt -= remaining;
            self.elapsed = Duration::ZERO;
            if !self.step() {
                return;
            }
        }
    }

    /// Steps the playhead forward one frame per the loop policy.
    ///
    /// Returns `false` when a [`LoopPolicy::Once`] animation has just finished (the index is left on
    /// the final frame and [`finished`](Self::is_finished) is set); `true` otherwise.
    fn step(&mut self) -> bool {
        let n = self.animation.frames.len();
        match self.animation.loop_policy {
            LoopPolicy::Once => {
                if self.index + 1 >= n {
                    self.finished = true;
                    return false;
                }
                self.index += 1;
            }
            LoopPolicy::Loop => {
                self.index = (self.index + 1) % n;
            }
            LoopPolicy::PingPong => {
                if n == 1 {
                    return true;
                }
                if self.forward {
                    if self.index + 1 >= n {
                        self.forward = false;
                        self.index -= 1;
                    } else {
                        self.index += 1;
                    }
                } else if self.index == 0 {
                    self.forward = true;
                    self.index += 1;
                } else {
                    self.index -= 1;
                }
            }
        }
        true
    }
}

impl FrameSource for AnimationPlayer {
    /// Renders the current frame into `target`, then steps forward one whole frame.
    ///
    /// `target` is cleared to transparent and the frame's bitmap is composited at its top-left.
    /// Returns `Ok(true)` while the animation is producing frames, and `Ok(false)` once a
    /// [`LoopPolicy::Once`] animation is exhausted (looping animations always return `Ok(true)`).
    fn next_frame(&mut self, target: &mut Framebuffer) -> Result<bool> {
        if self.finished {
            return Ok(false);
        }
        let frame_len = {
            let frame = &self.animation.frames[self.index];
            target.clear(Color::TRANSPARENT);
            blit(&frame.image, target, 0, 0);
            frame.duration
        };
        // Step forward by exactly the time left in this frame → advance one whole frame.
        let remaining = frame_len.saturating_sub(self.elapsed);
        self.advance(remaining);
        Ok(true)
    }

    /// The time remaining before the playhead should leave the current frame.
    fn frame_delay(&self) -> Option<Duration> {
        if self.finished {
            return None;
        }
        let frame_len = self.animation.frames[self.index].duration;
        Some(frame_len.saturating_sub(self.elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-pixel framebuffer whose single pixel carries `luma` (used to identify frames).
    fn dot(luma: f32) -> Framebuffer {
        let mut fb = Framebuffer::new(1, 1);
        fb.set(0, 0, Color::rgb(luma, luma, luma));
        fb
    }

    fn frame(luma: f32, ms: u64) -> Frame {
        Frame::new(dot(luma), Duration::from_millis(ms))
    }

    fn three_frames(policy: LoopPolicy) -> AnimationPlayer {
        let anim = Animation::new(
            vec![frame(0.1, 100), frame(0.2, 100), frame(0.3, 100)],
            policy,
        )
        .unwrap();
        AnimationPlayer::new(anim)
    }

    #[test]
    fn empty_animation_is_rejected() {
        assert!(Animation::new(vec![], LoopPolicy::Loop).is_err());
    }

    #[test]
    fn zero_duration_frame_is_rejected() {
        let bad = Frame::new(dot(1.0), Duration::ZERO);
        assert!(Animation::new(vec![bad], LoopPolicy::Loop).is_err());
    }

    #[test]
    fn total_duration_sums_frames() {
        let p = three_frames(LoopPolicy::Loop);
        assert_eq!(p.animation().total_duration(), Duration::from_millis(300));
    }

    #[test]
    fn advance_stays_within_a_frame() {
        let mut p = three_frames(LoopPolicy::Loop);
        p.advance(Duration::from_millis(50));
        assert_eq!(p.current_index(), 0);
        p.advance(Duration::from_millis(49));
        assert_eq!(p.current_index(), 0);
    }

    #[test]
    fn advance_crosses_frame_boundaries_deterministically() {
        let mut p = three_frames(LoopPolicy::Loop);
        // Exactly one frame boundary.
        p.advance(Duration::from_millis(100));
        assert_eq!(p.current_index(), 1);
        // Cross two more (index 1 -> 2 -> 0 with 10ms carried into frame 0).
        p.advance(Duration::from_millis(210));
        assert_eq!(p.current_index(), 0);
    }

    #[test]
    fn loop_wraps_around() {
        let mut p = three_frames(LoopPolicy::Loop);
        p.advance(Duration::from_millis(300));
        assert_eq!(p.current_index(), 0);
        assert!(!p.is_finished());
    }

    #[test]
    fn once_finishes_after_last_frame() {
        let mut p = three_frames(LoopPolicy::Once);
        p.advance(Duration::from_millis(250));
        assert_eq!(p.current_index(), 2);
        assert!(!p.is_finished());
        // Consume the final frame's remaining 50ms and one more tick.
        p.advance(Duration::from_millis(60));
        assert!(p.is_finished());
        // Further advancing is a no-op.
        p.advance(Duration::from_millis(1000));
        assert_eq!(p.current_index(), 2);
    }

    #[test]
    fn ping_pong_bounces() {
        let mut p = three_frames(LoopPolicy::PingPong);
        let indices: Vec<usize> = (0..6)
            .map(|_| {
                p.advance(Duration::from_millis(100));
                p.current_index()
            })
            .collect();
        // 0 ->1->2->1->0->1->2
        assert_eq!(indices, vec![1, 2, 1, 0, 1, 2]);
        assert!(!p.is_finished());
    }

    #[test]
    fn next_frame_renders_current_and_steps_one_frame() {
        let mut p = three_frames(LoopPolicy::Loop);
        let mut fb = Framebuffer::new(1, 1);

        assert!(p.next_frame(&mut fb).unwrap());
        assert!((fb.luma(0, 0) - 0.1).abs() < 1e-6, "first frame rendered");
        assert_eq!(p.current_index(), 1, "stepped one whole frame");

        assert!(p.next_frame(&mut fb).unwrap());
        assert!((fb.luma(0, 0) - 0.2).abs() < 1e-6);
        assert_eq!(p.current_index(), 2);
    }

    #[test]
    fn next_frame_reports_exhaustion_for_once() {
        let mut p = three_frames(LoopPolicy::Once);
        let mut fb = Framebuffer::new(1, 1);
        // Three frames each render once and return Ok(true).
        for _ in 0..3 {
            assert!(p.next_frame(&mut fb).unwrap());
        }
        assert!(p.is_finished());
        assert!(
            !p.next_frame(&mut fb).unwrap(),
            "exhausted after last frame"
        );
        assert_eq!(p.frame_delay(), None);
    }

    #[test]
    fn frame_delay_reports_time_left_in_frame() {
        let mut p = three_frames(LoopPolicy::Loop);
        assert_eq!(p.frame_delay(), Some(Duration::from_millis(100)));
        p.advance(Duration::from_millis(30));
        assert_eq!(p.frame_delay(), Some(Duration::from_millis(70)));
    }

    #[test]
    fn reset_returns_to_start() {
        let mut p = three_frames(LoopPolicy::Once);
        p.advance(Duration::from_millis(1000));
        assert!(p.is_finished());
        p.reset();
        assert_eq!(p.current_index(), 0);
        assert!(!p.is_finished());
    }

    #[test]
    fn sprite_blit_composites_and_clips() {
        let mut sprite_fb = Framebuffer::new(2, 2);
        sprite_fb.set(0, 0, Color::WHITE);
        sprite_fb.set(1, 1, Color::WHITE);
        let sprite = Sprite::new(sprite_fb);
        assert_eq!((sprite.width(), sprite.height()), (2, 2));

        let mut target = Framebuffer::new(3, 3);
        sprite.blit(&mut target, 1, 1);
        // (1,1) and (2,2) become white; transparent source pixels leave target untouched.
        assert_eq!(target.get(1, 1), Color::WHITE);
        assert_eq!(target.get(2, 2), Color::WHITE);
        assert_eq!(target.get(1, 2), Color::TRANSPARENT);
    }

    #[test]
    fn next_frame_clears_target_before_blitting() {
        // A target larger than the 1x1 frame must not retain stale pixels outside the frame.
        let mut p = three_frames(LoopPolicy::Loop);
        let mut fb = Framebuffer::new(2, 2);
        fb.set(1, 1, Color::WHITE);
        p.next_frame(&mut fb).unwrap();
        assert_eq!(fb.get(1, 1), Color::TRANSPARENT, "stale pixel cleared");
    }
}
