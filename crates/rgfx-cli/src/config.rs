//! Configuration: file defaults, `~/.config/rgfx/config.toml`, and CLI overrides.
//!
//! Precedence is strictly **defaults < config file < CLI flags**. [`Config`] models the on-disk
//! file (with `serde` defaults filling any missing field), and [`Settings::resolve`] folds a
//! parsed [`Config`] together with the CLI [`RenderOpts`] into the effective [`Settings`] the
//! rest of the program uses.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cli::{DitherMode, RenderOpts, Renderer, Shading};

/// The on-disk configuration file (`config.toml`).
///
/// Every field carries a `#[serde(default)]` so a partial file — or no file at all — still
/// deserializes, with absent fields taking their documented default.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Default terminal encoder.
    pub renderer: Renderer,
    /// Default render width in columns; `None` means "use the terminal width".
    pub width: Option<u32>,
    /// Default target FPS for animated sources.
    pub fps: u32,
    /// Whether color output is enabled by default.
    pub color: bool,
    /// 3D-specific defaults.
    pub three_d: ThreeDConfig,
    /// Video-specific defaults.
    pub video: VideoConfig,
    /// Still-image defaults (tone + dithering).
    pub image: ImageConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            renderer: Renderer::Auto,
            width: None,
            fps: 30,
            color: false,
            three_d: ThreeDConfig::default(),
            video: VideoConfig::default(),
            image: ImageConfig::default(),
        }
    }
}

/// The `[image]` sub-table: image-quality defaults applied by the still-image viewer.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImageConfig {
    /// Default dithering algorithm.
    pub dither: DitherMode,
    /// Default gamma exponent applied to luminance (`1.0` = identity).
    pub gamma: f32,
    /// Default contrast multiplier about mid-grey (`1.0` = identity).
    pub contrast: f32,
    /// Luma threshold used by the 1-bit dithering path.
    pub threshold: f32,
}

impl Default for ImageConfig {
    fn default() -> Self {
        ImageConfig {
            dither: DitherMode::Auto,
            gamma: 1.0,
            contrast: 1.0,
            threshold: 0.5,
        }
    }
}

/// The `[three_d]` sub-table.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThreeDConfig {
    /// Default shading mode.
    pub shading: Shading,
    /// Whether meshes render as wireframe by default.
    pub wireframe: bool,
    /// Vertical field of view in degrees.
    pub fov_degrees: f32,
}

impl Default for ThreeDConfig {
    fn default() -> Self {
        ThreeDConfig {
            shading: Shading::Flat,
            wireframe: false,
            fov_degrees: 45.0,
        }
    }
}

/// The `[video]` sub-table.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct VideoConfig {
    /// Loop playback when the video ends.
    pub loop_playback: bool,
    /// Cap decode FPS (0 = follow the source's native rate).
    pub max_fps: u32,
}

impl Config {
    /// The default config path: `$XDG_CONFIG_HOME/rgfx/config.toml`, else `~/.config/rgfx/config.toml`.
    ///
    /// Returns `None` when neither `XDG_CONFIG_HOME` nor `HOME` is set.
    pub fn default_path() -> Option<PathBuf> {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(xdg).join("rgfx").join("config.toml"));
        }
        let home = std::env::var_os("HOME").filter(|v| !v.is_empty())?;
        Some(
            PathBuf::from(home)
                .join(".config")
                .join("rgfx")
                .join("config.toml"),
        )
    }

    /// Loads the config from [`Config::default_path`], returning defaults if it is absent.
    ///
    /// A missing file is not an error (defaults are used). A present-but-malformed file *is* an
    /// error so the user learns their config is being ignored.
    pub fn load() -> anyhow::Result<Config> {
        match Config::default_path() {
            Some(path) => Config::load_from(&path),
            None => Ok(Config::default()),
        }
    }

    /// Loads the config from a specific path, using defaults when the file does not exist.
    pub fn load_from(path: &Path) -> anyhow::Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => {
                Err(anyhow::Error::new(e)
                    .context(format!("reading config file {}", path.display())))
            }
        }
    }

    /// Parses a config from a TOML string. Missing fields fall back to defaults.
    pub fn from_toml(text: &str) -> anyhow::Result<Config> {
        toml::from_str(text).map_err(|e| anyhow::anyhow!("invalid config: {e}"))
    }
}

/// The effective, fully-resolved settings after merging defaults, file, and CLI flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Effective terminal encoder.
    pub renderer: Renderer,
    /// Effective render width in columns; `None` = follow the terminal.
    pub width: Option<u32>,
    /// Effective target FPS.
    pub fps: u32,
    /// Effective color-output flag.
    pub color: bool,
    /// Effective 3D shading mode.
    pub shading: Shading,
    /// Effective wireframe flag.
    pub wireframe: bool,
    /// Effective vertical field of view in degrees.
    pub fov_degrees: f32,
    /// Loop video playback.
    pub loop_playback: bool,
    /// Effective dithering algorithm for the still-image viewer.
    pub dither: DitherMode,
    /// Effective gamma exponent applied to luminance (`1.0` = identity).
    pub gamma: f32,
    /// Effective contrast multiplier about mid-grey (`1.0` = identity).
    pub contrast: f32,
    /// Effective luma threshold used by the 1-bit dithering path.
    pub threshold: f32,
    /// Optional output-file sink instead of the live terminal.
    pub output: Option<PathBuf>,
    /// Print still images inline like `cat` instead of the full-screen preview.
    pub cat: bool,
}

impl Settings {
    /// Folds a config file and CLI options into the effective settings.
    ///
    /// A CLI value overrides the config only when the user actually supplied it: `Option` flags
    /// override when `Some`, and the boolean flags (`--color`, `--wireframe`) override only when
    /// present (`true`) — they cannot switch a config `true` back to `false`, matching clap's
    /// store-true semantics for a skeleton without explicit negation flags.
    pub fn resolve(config: &Config, opts: &RenderOpts) -> Settings {
        Settings {
            renderer: opts.renderer.unwrap_or(config.renderer),
            width: opts.width.or(config.width),
            fps: opts.fps.unwrap_or(config.fps),
            color: opts.color || config.color,
            shading: opts.shading.unwrap_or(config.three_d.shading),
            wireframe: opts.wireframe || config.three_d.wireframe,
            fov_degrees: config.three_d.fov_degrees,
            loop_playback: config.video.loop_playback,
            dither: opts.dither.unwrap_or(config.image.dither),
            gamma: opts.gamma.unwrap_or(config.image.gamma),
            contrast: opts.contrast.unwrap_or(config.image.contrast),
            threshold: config.image.threshold,
            output: opts.output.clone(),
            cat: opts.cat,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.renderer, Renderer::Auto);
        assert_eq!(c.width, None);
        assert_eq!(c.fps, 30);
        assert!(!c.color);
        assert_eq!(c.three_d.shading, Shading::Flat);
        assert_eq!(c.video.max_fps, 0);
        assert_eq!(c.image.dither, DitherMode::Auto);
        assert_eq!(c.image.gamma, 1.0);
        assert_eq!(c.image.contrast, 1.0);
    }

    #[test]
    fn image_flags_override_config() {
        let cfg = Config::from_toml(
            r#"
                [image]
                dither = "floyd"
                gamma = 2.2
            "#,
        )
        .unwrap();
        assert_eq!(cfg.image.dither, DitherMode::Floyd);
        // Absent flags keep the file values; a present flag wins.
        let s = Settings::resolve(&cfg, &RenderOpts::default());
        assert_eq!(s.dither, DitherMode::Floyd);
        assert_eq!(s.gamma, 2.2);

        let s2 = Settings::resolve(
            &cfg,
            &RenderOpts {
                dither: Some(DitherMode::Atkinson),
                contrast: Some(1.5),
                ..RenderOpts::default()
            },
        );
        assert_eq!(s2.dither, DitherMode::Atkinson);
        assert_eq!(s2.contrast, 1.5);
        assert_eq!(s2.gamma, 2.2);
    }

    #[test]
    fn empty_toml_yields_defaults() {
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
    }

    #[test]
    fn partial_toml_overlays_defaults() {
        let text = r#"
            fps = 60
            color = true
            [three_d]
            wireframe = true
        "#;
        let c = Config::from_toml(text).unwrap();
        assert_eq!(c.fps, 60);
        assert!(c.color);
        assert!(c.three_d.wireframe);
        // Untouched fields keep defaults.
        assert_eq!(c.renderer, Renderer::Auto);
        assert_eq!(c.three_d.shading, Shading::Flat);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::from_toml("bogus = 1").is_err());
    }

    #[test]
    fn precedence_defaults_only() {
        let cfg = Config::default();
        let opts = RenderOpts::default();
        let s = Settings::resolve(&cfg, &opts);
        assert_eq!(s.renderer, Renderer::Auto);
        assert_eq!(s.fps, 30);
        assert!(!s.color);
        assert_eq!(s.shading, Shading::Flat);
        assert!(!s.wireframe);
    }

    #[test]
    fn precedence_file_over_defaults() {
        let cfg = Config::from_toml(
            r#"
                renderer = "ascii"
                fps = 12
                color = true
                [three_d]
                shading = "smooth"
            "#,
        )
        .unwrap();
        let opts = RenderOpts::default();
        let s = Settings::resolve(&cfg, &opts);
        assert_eq!(s.renderer, Renderer::Ascii);
        assert_eq!(s.fps, 12);
        assert!(s.color);
        assert_eq!(s.shading, Shading::Smooth);
    }

    #[test]
    fn precedence_flags_over_file() {
        let cfg = Config::from_toml(
            r#"
                renderer = "ascii"
                fps = 12
                [three_d]
                shading = "smooth"
            "#,
        )
        .unwrap();
        let opts = RenderOpts {
            renderer: Some(Renderer::Braille),
            fps: Some(24),
            shading: Some(Shading::Depth),
            wireframe: true,
            color: true,
            width: Some(200),
            ..RenderOpts::default()
        };
        let s = Settings::resolve(&cfg, &opts);
        assert_eq!(s.renderer, Renderer::Braille);
        assert_eq!(s.fps, 24);
        assert_eq!(s.shading, Shading::Depth);
        assert!(s.wireframe);
        assert!(s.color);
        assert_eq!(s.width, Some(200));
    }

    #[test]
    fn width_flag_overrides_but_absent_flag_keeps_file() {
        let cfg = Config::from_toml("width = 100").unwrap();
        let s_no_flag = Settings::resolve(&cfg, &RenderOpts::default());
        assert_eq!(s_no_flag.width, Some(100));

        let s_flag = Settings::resolve(
            &cfg,
            &RenderOpts {
                width: Some(64),
                ..RenderOpts::default()
            },
        );
        assert_eq!(s_flag.width, Some(64));
    }

    #[test]
    fn default_path_prefers_xdg() {
        // Just assert the shape; we don't mutate global env in a shared test process beyond read.
        if let Some(p) = Config::default_path() {
            assert!(p.ends_with("rgfx/config.toml"));
        }
    }

    #[test]
    fn load_from_missing_file_is_defaults() {
        let c = Config::load_from(Path::new("/no/such/rgfx/config.toml")).unwrap();
        assert_eq!(c, Config::default());
    }
}
