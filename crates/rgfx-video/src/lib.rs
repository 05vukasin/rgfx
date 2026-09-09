//! `rgfx-video`: decode video frames into the [`rgfx_core::Framebuffer`] pipeline.
//!
//! Following the spec's recommended first strategy, this crate shells out to the
//! external `ffmpeg` and `ffprobe` executables rather than linking any codec
//! library. FFmpeg is therefore a *runtime* dependency, never a build one:
//! `cargo build --workspace` compiles this crate whether or not ffmpeg is
//! installed, and the binaries are located on `PATH` (or via the `RGFX_FFMPEG`
//! / `RGFX_FFPROBE` environment overrides) only when decoding actually runs.
//!
//! # Feature flag
//!
//! The subprocess-driven entry points — [`probe`] and [`VideoSource`] — are
//! gated behind the `ffmpeg` cargo feature. The crate's pure, dependency-free
//! logic is always available and testable without ffmpeg installed:
//!
//! - [`VideoInfo::from_ffprobe_json`] parses `ffprobe`'s JSON report.
//! - [`FrameReader`] chunks a raw `rgb24` byte stream into fixed-size frames.
//! - [`render_rgb24_into`] converts one decoded frame into a reused framebuffer,
//!   aspect-corrected and letterboxed for a target [`rgfx_core::Viewport`],
//!   reusing `rgfx-image`'s fit math.
//! - [`find_ffmpeg`] / [`find_ffprobe`] locate the executables and return an
//!   actionable [`rgfx_core::Error::External`] when they are missing.
//!
//! Nothing here touches the terminal or an encoder: every frame converges on a
//! [`rgfx_core::Framebuffer`], per the rgfx architectural law.
#![warn(missing_docs)]
#![forbid(unsafe_code)]

mod convert;
mod detect;
mod frame;
mod info;

#[cfg(feature = "ffmpeg")]
mod source;

pub use convert::render_rgb24_into;
pub use detect::{find_ffmpeg, find_ffprobe};
pub use frame::FrameReader;
pub use info::VideoInfo;

#[cfg(feature = "ffmpeg")]
pub use source::{VideoSource, probe};

// Re-export the shared vocabulary and the reused render options so downstream
// users can name the exact types this crate returns without separate deps.
pub use rgfx_core::{Error, Result};
pub use rgfx_image::{RenderOptions, ResizeFilter};
