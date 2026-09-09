//! Plays a built-in avatar animation through the real frame-diff pipeline, then exits cleanly.
//!
//! Run with an optional state name (defaults to `loading`):
//!
//! ```text
//! cargo run -p rgfx-terminal --example avatar -- --state thinking
//! ```
//!
//! Available states: `idle`, `thinking`, `speaking`, `loading`, `success`, `error`. The looping
//! states run for a fixed number of frames so the example always terminates; the `success` and
//! `error` states play once and stop on their final symbol.

use std::io::{self, Write};
use std::thread::sleep;

use rgfx_core::{FrameSource, Framebuffer, TerminalEncoder, Viewport};
use rgfx_terminal::{AvatarState, BrailleEncoder, ColorMode, FrameEngine, SUBPIXEL_X, SUBPIXEL_Y};

/// Maps a CLI state name to an [`AvatarState`].
fn parse_state(name: &str) -> Option<AvatarState> {
    match name.to_ascii_lowercase().as_str() {
        "idle" => Some(AvatarState::Idle),
        "thinking" => Some(AvatarState::Thinking),
        "speaking" => Some(AvatarState::Speaking),
        "loading" => Some(AvatarState::Loading),
        "success" => Some(AvatarState::Success),
        "error" => Some(AvatarState::Error),
        _ => None,
    }
}

fn main() -> io::Result<()> {
    // Parse an optional `--state <name>` argument.
    let mut state = AvatarState::Loading;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--state" {
            if let Some(name) = args.next() {
                match parse_state(&name) {
                    Some(s) => state = s,
                    None => {
                        eprintln!("unknown state '{name}'; using 'loading'");
                    }
                }
            }
        }
    }

    let animation = state.animation();

    // Size the viewport to hold the widest frame (one Braille cell tall).
    let cols = animation
        .frames()
        .iter()
        .map(|f| f.image().width().div_ceil(SUBPIXEL_X as usize))
        .max()
        .unwrap_or(1)
        .max(1);
    let viewport = Viewport::new(cols as u16, 1);
    let (w, h) = viewport.render_size(SUBPIXEL_X, SUBPIXEL_Y);

    let mut framebuffer = Framebuffer::new(w, h);
    let encoder = BrailleEncoder::new();
    let mut engine = FrameEngine::new(ColorMode::None);
    let mut player = state.player();

    let stdout = io::stdout();
    let mut out = stdout.lock();

    // Cap the run so looping states still exit; `Once` states break early when exhausted.
    let max_frames = 60usize;
    for _ in 0..max_frames {
        // Duration of the frame about to be rendered (the playhead steps forward inside next_frame).
        let dwell = player.current_frame().duration();
        if !player
            .next_frame(&mut framebuffer)
            .expect("built-in animations never fail to produce a frame")
        {
            break;
        }
        let frame = encoder.encode(&framebuffer, viewport);
        engine.render(&frame, &mut out)?;
        sleep(dwell);
    }

    // Leave the cursor on a fresh line below the animation.
    writeln!(out)?;
    out.flush()
}
