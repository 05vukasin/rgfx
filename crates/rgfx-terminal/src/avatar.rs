//! Built-in avatar state animations for AI-CLI avatars and loading spinners.
//!
//! Each [`AvatarState`] maps to a small [`Animation`] of Braille/Unicode frames. The frames are
//! authored as framebuffers (lit pixels), so they flow through the ordinary
//! [`BrailleEncoder`](crate::BrailleEncoder) → [`FrameEngine`](crate::FrameEngine) pipeline just
//! like an image or a video frame — there are no ad-hoc terminal writes here. The spinner-style
//! states loop forever; the terminal ones ([`AvatarState::Success`], [`AvatarState::Error`]) play
//! once and stop on their final symbol.
//!
//! ```no_run
//! use rgfx_terminal::AvatarState;
//! // Drive it like any other `FrameSource`.
//! let mut player = AvatarState::Thinking.player();
//! # let _ = &mut player;
//! ```

use rgfx_core::{Animation, AnimationPlayer, Color, Frame, Framebuffer, LoopPolicy};
use std::time::Duration;

use crate::braille::{BRAILLE_BASE, braille_dots};

/// Framebuffer height, in pixels, of a single-row Braille animation frame (one Braille cell tall).
const CELL_H: usize = 4;
/// Framebuffer width, in pixels, of a single Braille cell.
const CELL_W: usize = 2;

/// A built-in avatar/spinner state with a ready-made [`Animation`].
///
/// Construct the animation with [`animation`](Self::animation) or a live playhead with
/// [`player`](Self::player).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarState {
    /// A slow, gentle "breathing" pulse shown while the avatar is idle.
    Idle,
    /// A ponder animation shown while a response is being computed.
    Thinking,
    /// A mouth-movement animation shown while the avatar is talking.
    Speaking,
    /// The classic Braille spinner shown during indeterminate progress.
    Loading,
    /// A checkmark that draws in once and holds — a terminal success state.
    Success,
    /// A cross that draws in once and holds — a terminal error state.
    Error,
}

impl AvatarState {
    /// Every built-in state, handy for exhaustive iteration and tests.
    pub const ALL: [AvatarState; 6] = [
        AvatarState::Idle,
        AvatarState::Thinking,
        AvatarState::Speaking,
        AvatarState::Loading,
        AvatarState::Success,
        AvatarState::Error,
    ];

    /// Builds the [`Animation`] for this state.
    ///
    /// The frame data is a compile-time constant, so construction is infallible.
    #[must_use]
    pub fn animation(self) -> Animation {
        match self {
            // A slow pulse from a low line up to a fuller shape and back (PingPong).
            AvatarState::Idle => glyph_animation(&['⠤', '⠒', '⠉', '⠛'], 450, LoopPolicy::PingPong),
            // Dots building up and down as the avatar "thinks".
            AvatarState::Thinking => {
                glyph_animation(&['⠁', '⠉', '⠋', '⠛', '⠟', '⠿'], 110, LoopPolicy::PingPong)
            }
            // Two cells opening and closing like a moving mouth.
            AvatarState::Speaking => row_animation(
                &[['⠶', '⠶'], ['⠛', '⠛'], ['⠿', '⠿'], ['⠤', '⠤']],
                140,
                LoopPolicy::Loop,
            ),
            // The canonical ten-frame Braille spinner.
            AvatarState::Loading => glyph_animation(
                &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'],
                80,
                LoopPolicy::Loop,
            ),
            // A checkmark drawn stroke by stroke, holding on the final symbol.
            AvatarState::Success => {
                let strokes: [&[(usize, usize)]; 3] = [
                    &[(0, 2)],
                    &[(0, 2), (1, 3)],
                    &[(0, 2), (1, 3), (2, 2), (3, 1)],
                ];
                pixel_animation(4, 4, &strokes, 120, LoopPolicy::Once)
            }
            // A cross drawn stroke by stroke, holding on the final symbol.
            AvatarState::Error => {
                let strokes: [&[(usize, usize)]; 3] = [
                    &[(0, 0), (1, 1), (2, 2), (3, 3)],
                    &[(0, 0), (1, 1), (2, 2), (3, 3), (3, 0)],
                    &[
                        (0, 0),
                        (1, 1),
                        (2, 2),
                        (3, 3),
                        (3, 0),
                        (2, 1),
                        (1, 2),
                        (0, 3),
                    ],
                ];
                pixel_animation(4, 4, &strokes, 120, LoopPolicy::Once)
            }
        }
    }

    /// Builds an [`AnimationPlayer`] positioned at the start of this state's animation.
    #[must_use]
    pub fn player(self) -> AnimationPlayer {
        AnimationPlayer::new(self.animation())
    }
}

/// The lit 2×4 subpixels of a Braille glyph, or an all-unlit block for a non-Braille code point.
fn glyph_dots(glyph: char) -> [[bool; 4]; 2] {
    let mask = (glyph as u32)
        .checked_sub(BRAILLE_BASE)
        .filter(|d| *d <= 0xFF)
        .map_or(0u8, |d| d as u8);
    braille_dots(mask)
}

/// A framebuffer reproducing a horizontal run of Braille `glyphs`, one cell each.
fn row_framebuffer(glyphs: &[char]) -> Framebuffer {
    let mut fb = Framebuffer::new(glyphs.len() * CELL_W, CELL_H);
    for (cell, &glyph) in glyphs.iter().enumerate() {
        let dots = glyph_dots(glyph);
        for (dx, column) in dots.iter().enumerate() {
            for (dy, &lit) in column.iter().enumerate() {
                if lit {
                    fb.set(cell * CELL_W + dx, dy, Color::WHITE);
                }
            }
        }
    }
    fb
}

/// A single-cell-per-frame animation from a sequence of Braille glyphs.
fn glyph_animation(glyphs: &[char], ms: u64, policy: LoopPolicy) -> Animation {
    let frames = glyphs
        .iter()
        .map(|&g| Frame::new(row_framebuffer(&[g]), Duration::from_millis(ms)))
        .collect();
    // invariant: `glyphs` is a non-empty compile-time constant and `ms` > 0, so this never errors.
    Animation::new(frames, policy).expect("built-in glyph animation is well-formed")
}

/// A multi-cell-per-frame animation from a sequence of Braille glyph rows.
fn row_animation<const W: usize>(rows: &[[char; W]], ms: u64, policy: LoopPolicy) -> Animation {
    let frames = rows
        .iter()
        .map(|row| Frame::new(row_framebuffer(row), Duration::from_millis(ms)))
        .collect();
    // invariant: `rows` is a non-empty compile-time constant and `ms` > 0, so this never errors.
    Animation::new(frames, policy).expect("built-in row animation is well-formed")
}

/// An animation whose frames light explicit `(x, y)` pixels in a `width`×`height` framebuffer.
fn pixel_animation(
    width: usize,
    height: usize,
    frames: &[&[(usize, usize)]],
    ms: u64,
    policy: LoopPolicy,
) -> Animation {
    let frames = frames
        .iter()
        .map(|lit| {
            let mut fb = Framebuffer::new(width, height);
            for &(x, y) in *lit {
                fb.set(x, y, Color::WHITE);
            }
            Frame::new(fb, Duration::from_millis(ms))
        })
        .collect();
    // invariant: `frames` is a non-empty compile-time constant and `ms` > 0, so this never errors.
    Animation::new(frames, policy).expect("built-in pixel animation is well-formed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrailleEncoder, SUBPIXEL_X, SUBPIXEL_Y};
    use rgfx_core::{FrameSource, TerminalEncoder, Viewport};

    #[test]
    fn every_state_has_a_nonempty_well_formed_animation() {
        for state in AvatarState::ALL {
            let anim = state.animation();
            assert!(!anim.is_empty(), "{state:?} must have frames");
            for frame in anim.frames() {
                assert!(!frame.duration().is_zero(), "{state:?} frame duration > 0");
                assert!(!frame.image().is_empty(), "{state:?} frame has pixels");
            }
        }
    }

    #[test]
    fn terminal_states_play_once_others_repeat() {
        assert_eq!(
            AvatarState::Success.animation().loop_policy(),
            LoopPolicy::Once
        );
        assert_eq!(
            AvatarState::Error.animation().loop_policy(),
            LoopPolicy::Once
        );
        assert_ne!(
            AvatarState::Loading.animation().loop_policy(),
            LoopPolicy::Once
        );
    }

    #[test]
    fn loading_matches_the_canonical_spinner_glyphs() {
        // Each frame is one Braille cell; encoding it back must reproduce the source spinner glyph.
        let expected = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let mut player = AvatarState::Loading.player();
        let enc = BrailleEncoder::new();
        let vp = Viewport::new(1, 1);
        let (w, h) = vp.render_size(SUBPIXEL_X, SUBPIXEL_Y);
        let mut fb = Framebuffer::new(w, h);
        for &want in &expected {
            assert!(player.next_frame(&mut fb).unwrap());
            let text = enc.encode(&fb, vp).to_text();
            assert_eq!(text, want.to_string());
        }
    }

    #[test]
    fn every_state_encodes_without_panic() {
        let enc = BrailleEncoder::new();
        for state in AvatarState::ALL {
            let mut player = state.player();
            let anim_len = state.animation().len();
            // Size a viewport wide enough for the widest frame.
            let cols = state
                .animation()
                .frames()
                .iter()
                .map(|f| f.image().width().div_ceil(SUBPIXEL_X as usize))
                .max()
                .unwrap_or(1)
                .max(1);
            let vp = Viewport::new(cols as u16, 1);
            let (w, h) = vp.render_size(SUBPIXEL_X, SUBPIXEL_Y);
            let mut fb = Framebuffer::new(w, h);
            for _ in 0..anim_len {
                if !player.next_frame(&mut fb).unwrap() {
                    break;
                }
                let frame = enc.encode(&fb, vp);
                assert_eq!(frame.rows(), 1);
            }
        }
    }
}
