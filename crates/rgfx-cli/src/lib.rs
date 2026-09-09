//! `rgfx-cli`: the `rgfx` binary's library.
//!
//! This crate wires together the pieces that turn a command line into a rendered terminal
//! session, while keeping the actual rendering behind traits so the media-specific viewer
//! crates (tasks 021–025) can plug in without touching dispatch:
//!
//! - [`cli`] — the `clap` argument surface (`<FILE>`/`-`, flags, and the `info` subcommand).
//! - [`media`] — [`media::MediaKind`] auto-detection from file extension and magic bytes.
//! - [`config`] — loading `~/.config/rgfx/config.toml` and merging it with CLI flags
//!   (defaults < file < flags).
//! - [`dispatch`] — the [`dispatch::MediaViewer`] trait plus the real still-image viewer and stub
//!   viewers that report "not yet implemented" cleanly until the remaining viewers land.
//! - [`image_viewer`] — the still-image viewer (task 021): decode → framebuffer → encode → present.
//! - [`info`] — the non-interactive `info <FILE>` metadata report (aligned text or `--json`).
//! - [`terminal`] — an interactive [`terminal::Session`] over `rgfx-terminal` that guarantees
//!   terminal cleanup on every exit path.
//! - [`stream`] — the stdin/pipe paths: `rgfx -` (buffer + sniff a still image) and `--stream`
//!   (a minimal framed protocol rendered continuously via a [`rgfx_core::FrameSource`]).
//! - [`app`] — the [`app::run`] entry point that loads config and dispatches.
//!
//! The one architectural law still holds here: this crate never decodes or rasterizes media
//! itself. It composes `rgfx-image` (decode) and `rgfx-terminal` (encode) and routes an
//! [`Input`](media::Input) to the right viewer.
#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod app;
pub mod cli;
pub mod config;
pub mod dispatch;
pub mod image_viewer;
pub mod info;
pub mod media;
pub mod mesh_viewer;
pub mod playback_viewer;
pub mod stream;
pub mod terminal;

pub use app::run;
pub use cli::Cli;
