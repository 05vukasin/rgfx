//! Application entry point: turn a parsed [`Cli`] into actions.
//!
//! [`run`] initializes tracing, loads and merges configuration, and then either handles the
//! `info` subcommand or enters the render path. The render path owns a [`TerminalGuard`] so the
//! terminal is always restored, then dispatches to a viewer.

use std::io::Read;

use crate::cli::{Cli, Command};
use crate::config::{Config, Settings};
use crate::dispatch;
use crate::media::{self, Input, MediaKind};
use crate::terminal::TerminalGuard;

/// Runs `rgfx` for a parsed command line.
///
/// Returns `Ok(())` on success and an `anyhow` error (printed by `main`) on failure. Never
/// panics on bad user input.
pub fn run(cli: Cli) -> anyhow::Result<()> {
    init_tracing();

    let config = Config::load()?;

    match cli.command {
        Some(Command::Info { file }) => run_info(&file),
        None => match cli.file.clone() {
            Some(file) => {
                let settings = Settings::resolve(&config, &cli.render);
                run_render(&file, &settings)
            }
            None => {
                // No file and no subcommand: print help and exit cleanly (not an error).
                print_usage_hint();
                Ok(())
            }
        },
    }
}

/// Initializes `tracing` from the `RGFX_LOG`/`RUST_LOG` environment, writing to stderr.
///
/// Safe to call more than once across a process (e.g. in tests): a failure to install the
/// global subscriber because one already exists is ignored.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("RGFX_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Handles `rgfx info <FILE>`: detect and describe, without rendering.
fn run_info(file: &str) -> anyhow::Result<()> {
    let input = Input::parse(file);
    let kind = detect_kind(&input)?;
    println!("input:  {}", input.label());
    println!("kind:   {}", describe_kind(kind));
    println!("viewer: {}", viewer_label(kind));
    Ok(())
}

/// Handles the render path for a single input.
fn run_render(file: &str, settings: &Settings) -> anyhow::Result<()> {
    let input = Input::parse(file);
    let kind = detect_kind(&input)?;

    if !kind.is_supported() {
        anyhow::bail!(
            "cannot render {}: unsupported or unrecognized media type",
            input.label()
        );
    }

    // Enter the terminal *after* we know the input is renderable, so `info`-style failures never
    // flip the terminal into raw mode. The guard restores the terminal on every exit path.
    let guard = TerminalGuard::enter()?;
    let viewport = guard.viewport();
    let result = dispatch::dispatch(&input, kind, settings, viewport);
    // `guard` drops here (or on unwind), guaranteeing teardown before the error propagates.
    drop(guard);
    result
}

/// Detects the media kind of an input, reading a stdin peek when needed.
fn detect_kind(input: &Input) -> anyhow::Result<MediaKind> {
    match input {
        Input::File(_) => Ok(media::detect(input)?),
        Input::Stdin => {
            // stdin is not seekable: peek the leading bytes for magic detection. The real
            // render path (later tasks) will need to buffer the whole stream; the skeleton only
            // classifies.
            let mut buf = [0u8; 16];
            let n = std::io::stdin().lock().read(&mut buf).unwrap_or(0);
            Ok(media::detect_bytes(&buf[..n], None))
        }
    }
}

/// A short description of a media kind for `info` output.
fn describe_kind(kind: MediaKind) -> &'static str {
    use media::MeshFormat::*;
    match kind {
        MediaKind::Image => "image",
        MediaKind::Gif => "animated gif",
        MediaKind::Video => "video",
        MediaKind::Mesh(Obj) => "3D mesh (OBJ)",
        MediaKind::Mesh(Stl) => "3D mesh (STL)",
        MediaKind::Mesh(Gltf) => "3D mesh (glTF/GLB)",
        MediaKind::Unknown => "unknown",
    }
}

/// The viewer that would handle a kind, for `info` output.
fn viewer_label(kind: MediaKind) -> &'static str {
    match dispatch::viewer_for(kind) {
        Ok(v) => v.name(),
        Err(_) => "none (unsupported)",
    }
}

/// Prints a short hint when invoked with no file and no subcommand.
fn print_usage_hint() {
    println!("rgfx: no input given. Try `rgfx <FILE>`, `rgfx info <FILE>`, or `rgfx --help`.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_on_unknown_is_ok_and_labels_unsupported() {
        // `info` never fails on unknown input; it just reports it.
        assert!(run_info("mystery.dat").is_ok());
        assert_eq!(viewer_label(MediaKind::Unknown), "none (unsupported)");
    }

    #[test]
    fn render_unsupported_is_clean_error() {
        let settings = Settings::resolve(&Config::default(), &Default::default());
        let err = run_render("mystery.dat", &settings).unwrap_err();
        assert!(err.to_string().contains("unsupported"), "got: {err}");
    }

    #[test]
    fn render_supported_reaches_stub_viewer() {
        // A .png (by extension) is supported → dispatch runs → stub reports not-implemented.
        let settings = Settings::resolve(&Config::default(), &Default::default());
        let err = run_render("picture.png", &settings).unwrap_err();
        assert!(
            err.to_string().contains("not yet implemented"),
            "got: {err}"
        );
    }

    #[test]
    fn describe_covers_all_kinds() {
        use media::MeshFormat::*;
        assert_eq!(describe_kind(MediaKind::Image), "image");
        assert_eq!(describe_kind(MediaKind::Mesh(Stl)), "3D mesh (STL)");
        assert_eq!(describe_kind(MediaKind::Unknown), "unknown");
    }
}
