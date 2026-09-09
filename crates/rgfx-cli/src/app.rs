//! Application entry point: turn a parsed [`Cli`] into actions.
//!
//! [`run`] initializes tracing, loads and merges configuration, and then either handles the
//! `info` subcommand or enters the render path. The render path detects the media kind and
//! dispatches to a viewer; each interactive viewer owns its own terminal session (which restores
//! the terminal on every exit path), while `--output` renders non-interactively with no terminal.

use crate::cli::{Cli, Command};
use crate::config::{Config, Settings};
use crate::dispatch;
use crate::media::{self, Input, MediaKind};

/// Runs `rgfx` for a parsed command line.
///
/// Returns `Ok(())` on success and an `anyhow` error (printed by `main`) on failure. Never
/// panics on bad user input.
pub fn run(cli: Cli) -> anyhow::Result<()> {
    init_tracing();

    let config = Config::load()?;

    match cli.command {
        Some(Command::Info { file, json }) => crate::info::run(&file, json),
        None => {
            let settings = Settings::resolve(&config, &cli.render);
            if cli.render.stream {
                // `--stream` reads a framed protocol from stdin regardless of any FILE argument.
                return crate::stream::run_stream(&settings);
            }
            match cli.file.clone() {
                Some(file) => run_render(&file, &settings),
                None => {
                    // No file and no subcommand: print help and exit cleanly (not an error).
                    print_usage_hint();
                    Ok(())
                }
            }
        }
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

/// Handles the render path for a single input.
fn run_render(file: &str, settings: &Settings) -> anyhow::Result<()> {
    let input = Input::parse(file);
    // `-` buffers the whole of stdin, sniffs it, and routes a still image to the viewer. This
    // must happen before any peek at stdin, since stdin is not seekable.
    if matches!(input, Input::Stdin) {
        return crate::stream::run_stdin(settings);
    }
    let kind = detect_kind(&input)?;

    if !kind.is_supported() {
        anyhow::bail!(
            "cannot render {}: unsupported or unrecognized media type",
            input.label()
        );
    }

    // The viewer owns its own terminal lifecycle when it presents interactively; the `--output`
    // path never touches the terminal. Either way, dispatch is the whole render path.
    dispatch::dispatch(&input, kind, settings)
}

/// Detects the media kind of a file input from its magic bytes and extension.
///
/// The stdin (`-`) case never reaches here: [`run_render`] intercepts it and routes to
/// [`crate::stream`], which buffers and sniffs the whole stream (stdin is not seekable).
fn detect_kind(input: &Input) -> anyhow::Result<MediaKind> {
    Ok(media::detect(input)?)
}

/// Prints a short hint when invoked with no file and no subcommand.
fn print_usage_hint() {
    println!("rgfx: no input given. Try `rgfx <FILE>`, `rgfx info <FILE>`, or `rgfx --help`.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_on_unknown_is_clean_error() {
        // `info` on unrecognized input errors cleanly (never panics); the detailed
        // per-kind behavior is covered by `crate::info` tests.
        let err = crate::info::run("mystery.dat", false).unwrap_err();
        assert!(err.to_string().contains("unsupported"), "got: {err}");
    }

    #[test]
    fn render_unsupported_is_clean_error() {
        let settings = Settings::resolve(&Config::default(), &Default::default());
        let err = run_render("mystery.dat", &settings).unwrap_err();
        assert!(err.to_string().contains("unsupported"), "got: {err}");
    }

    #[test]
    fn render_missing_image_is_clean_error() {
        // A .png (by extension) is supported → dispatch runs the real image viewer, which fails
        // cleanly (never panics) when the file cannot be loaded.
        let settings = Settings::resolve(&Config::default(), &Default::default());
        let err = run_render("does-not-exist.png", &settings).unwrap_err();
        assert!(err.to_string().contains("loading image"), "got: {err}");
    }
}
