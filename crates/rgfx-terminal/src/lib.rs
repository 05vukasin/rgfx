//! `rgfx-terminal`: the safe terminal lifecycle and I/O layer for rgfx.
//!
//! This crate owns everything that talks to the real terminal: enabling raw mode, switching to
//! the alternate screen, hiding the cursor, optionally capturing the mouse, reading input, and —
//! most importantly — **restoring every one of those on every exit path**, including normal
//! return, an error, `Ctrl+C`, or a panic. The encoders (Braille/ASCII/blocks) live in this same
//! crate as separate modules; color handling and the frame-diff engine build on top of what is
//! exposed here.
//!
//! The three public building blocks are:
//! - [`Terminal`] — the RAII lifecycle guard. Construct it to enter graphics mode; drop it (or let
//!   it drop during unwinding) to restore the terminal to exactly how it was found.
//! - [`Event`] — an rgfx-local input event enum so downstream crates never depend on
//!   `crossterm`'s types directly.
//! - [`BufferedWriter`] — a batching writer that accumulates a whole frame's bytes and flushes
//!   them to the terminal in a single syscall. The diff engine (task 007) builds on this.
#![warn(missing_docs)]
#![forbid(unsafe_code)]

mod ascii;
mod avatar;
mod block;
mod braille;
mod clock;
mod color;
mod event;
mod frame_engine;
mod serializer;
mod terminal;
mod writer;

pub use ascii::{AsciiEncoder, AsciiOptions, DEFAULT_RAMP};
pub use avatar::AvatarState;
pub use block::{
    BlockEncoder, BlockOptions, DARK_SHADE, FULL_BLOCK, LEFT_HALF, LIGHT_SHADE, LOWER_HALF,
    MEDIUM_SHADE, RIGHT_HALF, SHADE_RAMP, UPPER_HALF,
};
pub use braille::{
    BRAILLE_BASE, BrailleEncoder, BrailleOptions, SUBPIXEL_X, SUBPIXEL_Y, braille_char,
    braille_dots,
};
pub use clock::{Clock, FrameClock, FramePacing, SystemClock};
pub use color::{
    ANSI16_PALETTE, AnsiColor, ColorMode, ansi16_index, ansi256_index, detect_color_mode,
    detect_from_env,
};
pub use event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind, map_event,
};
pub use frame_engine::FrameEngine;
pub use serializer::AnsiSerializer;
pub use terminal::{
    Backend, CrosstermBackend, Terminal, TerminalOptions, install_panic_hook, terminal_size,
};
pub use writer::BufferedWriter;
