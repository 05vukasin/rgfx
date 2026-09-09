//! `rgfx-terminal`: the safe terminal lifecycle and I/O layer for rgfx.
//!
//! This crate owns everything that talks to the real terminal: enabling raw mode, switching to
//! the alternate screen, hiding the cursor, optionally capturing the mouse, reading input, and —
//! most importantly — **restoring every one of those on every exit path**, including normal
//! return, an error, `Ctrl+C`, or a panic. The encoders (Braille/ASCII/blocks), color handling,
//! and the frame-diff engine are separate crates that build on top of what is exposed here; this
//! crate deliberately knows nothing about them.
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

mod event;
mod terminal;
mod writer;

pub use event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind, map_event,
};
pub use terminal::{Backend, CrosstermBackend, Terminal, TerminalOptions, install_panic_hook};
pub use writer::BufferedWriter;
