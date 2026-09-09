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
//! - [`dispatch`] — the [`dispatch::MediaViewer`] trait plus stub viewers that report
//!   "not yet implemented" cleanly until the real viewers land.
//! - [`terminal`] — a setup/teardown guard that guarantees terminal cleanup on every exit path.
//! - [`app`] — the [`app::run`] entry point that owns the terminal and dispatches.
//!
//! The one architectural law still holds here: this crate never decodes or rasterizes media
//! itself. It only routes an [`Input`](media::Input) to the right viewer.
#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod app;
pub mod cli;
pub mod config;
pub mod dispatch;
pub mod media;
pub mod terminal;

pub use app::run;
pub use cli::Cli;
