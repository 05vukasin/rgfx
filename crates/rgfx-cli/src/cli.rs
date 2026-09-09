//! The `clap` command-line surface for `rgfx`.
//!
//! The default invocation renders a file: `rgfx <FILE> [flags]` (or `rgfx - [flags]` to read
//! from stdin). A single subcommand, `info <FILE>`, prints detected media information without
//! rendering. Rendering options double as overrides for values otherwise taken from the config
//! file — see [`crate::config`].

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Render images, video, animations and 3D meshes directly in your terminal.
#[derive(Parser, Debug, Clone)]
#[command(name = "rgfx", version)]
pub struct Cli {
    /// Input file to render, or `-` to read from standard input.
    ///
    /// Optional so that `rgfx info <FILE>` and `rgfx --help` work without a top-level file.
    #[arg(value_name = "FILE")]
    pub file: Option<String>,

    /// Rendering options. Shared with the config file; any flag set here overrides the config.
    #[command(flatten)]
    pub render: RenderOpts,

    /// Optional subcommand. When present it takes precedence over the positional `FILE`.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Subcommands understood by `rgfx`.
#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Print the detected media kind and basic details for a file, without rendering it.
    Info {
        /// File to inspect, or `-` for standard input.
        #[arg(value_name = "FILE")]
        file: String,
    },
}

/// Rendering options shared between the CLI and the config file.
///
/// Every field is optional at the CLI level: an unset flag means "fall back to the config file
/// value (or its default)". Merging happens in [`crate::config::Settings::resolve`].
#[derive(clap::Args, Debug, Clone, Default)]
pub struct RenderOpts {
    /// Terminal encoder to use.
    #[arg(long, value_enum, value_name = "MODE")]
    pub renderer: Option<Renderer>,

    /// Target render width in terminal columns. Defaults to the current terminal width.
    #[arg(long, value_name = "N")]
    pub width: Option<u32>,

    /// Target frames per second for animated sources (gif/video/3D).
    #[arg(long, value_name = "N")]
    pub fps: Option<u32>,

    /// Draw 3D meshes as wireframe instead of shaded surfaces.
    #[arg(long)]
    pub wireframe: bool,

    /// Shading mode for 3D meshes.
    #[arg(long, value_enum, value_name = "MODE")]
    pub shading: Option<Shading>,

    /// Enable ANSI color output (default is monochrome).
    #[arg(long)]
    pub color: bool,

    /// Write the encoded output to a file instead of the live terminal.
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

/// Terminal encoder selection.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Renderer {
    /// Braille dots: 2×4 subpixels per cell, highest spatial resolution.
    Braille,
    /// ASCII density ramp: one glyph per cell.
    Ascii,
    /// Unicode half/quadrant blocks with color.
    Blocks,
    /// Choose an encoder automatically based on the media and terminal.
    Auto,
}

/// Shading mode for the 3D pipeline.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Shading {
    /// Constant color, no lighting.
    Unlit,
    /// Per-face flat lighting.
    Flat,
    /// Per-vertex smooth (Gouraud) lighting.
    Smooth,
    /// Visualize surface normals as color.
    Normals,
    /// Visualize depth as grayscale.
    Depth,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("args should parse")
    }

    #[test]
    fn clap_definition_is_valid() {
        // Catches ambiguous args / duplicate flags at test time.
        Cli::command().debug_assert();
    }

    #[test]
    fn bare_file_argument() {
        let cli = parse(&["rgfx", "image.png"]);
        assert_eq!(cli.file.as_deref(), Some("image.png"));
        assert!(cli.command.is_none());
        // Flags default to unset / false.
        assert_eq!(cli.render.renderer, None);
        assert!(!cli.render.wireframe);
        assert!(!cli.render.color);
    }

    #[test]
    fn stdin_dash_is_accepted() {
        let cli = parse(&["rgfx", "-"]);
        assert_eq!(cli.file.as_deref(), Some("-"));
    }

    #[test]
    fn all_render_flags() {
        let cli = parse(&[
            "rgfx",
            "model.obj",
            "--renderer",
            "braille",
            "--width",
            "120",
            "--fps",
            "24",
            "--wireframe",
            "--shading",
            "flat",
            "--color",
            "--output",
            "out.txt",
        ]);
        assert_eq!(cli.render.renderer, Some(Renderer::Braille));
        assert_eq!(cli.render.width, Some(120));
        assert_eq!(cli.render.fps, Some(24));
        assert!(cli.render.wireframe);
        assert_eq!(cli.render.shading, Some(Shading::Flat));
        assert!(cli.render.color);
        assert_eq!(
            cli.render.output.as_deref(),
            Some(std::path::Path::new("out.txt"))
        );
    }

    #[test]
    fn info_subcommand() {
        let cli = parse(&["rgfx", "info", "scene.glb"]);
        match cli.command {
            Some(Command::Info { file }) => assert_eq!(file, "scene.glb"),
            other => panic!("expected info subcommand, got {other:?}"),
        }
    }

    #[test]
    fn renderer_value_enum_rejects_unknown() {
        let err = Cli::try_parse_from(["rgfx", "x.png", "--renderer", "hologram"]);
        assert!(err.is_err(), "unknown renderer value must be rejected");
    }

    #[test]
    fn no_arguments_parses_without_error() {
        // `rgfx` with no file/subcommand parses; run() decides what to do (print help).
        let cli = parse(&["rgfx"]);
        assert!(cli.file.is_none());
        assert!(cli.command.is_none());
    }
}
