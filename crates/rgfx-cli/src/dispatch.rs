//! Viewer dispatch: route a detected [`MediaKind`] to the right viewer.
//!
//! Viewer behavior lives behind the [`MediaViewer`] trait so tasks 021–025 can drop in real
//! implementations without touching this dispatch layer. The still-image viewer (021) is real;
//! the remaining media families still have stub viewers that report "not yet implemented" cleanly
//! (a returned error, never a panic).

use crate::config::Settings;
use crate::image_viewer::ImageViewer;
use crate::media::{Input, MediaKind, MeshFormat};

/// Everything a viewer needs to do its job.
///
/// A viewer is deliberately not handed the terminal directly by this struct: per the architectural
/// law it produces frames into a framebuffer and the terminal layer encodes them. A viewer that
/// presents interactively owns its own terminal session (see [`crate::terminal::Session`]); the
/// non-interactive `--output` path needs no terminal at all.
#[derive(Debug)]
pub struct ViewRequest<'a> {
    /// The input being viewed.
    pub input: &'a Input,
    /// The detected media kind.
    pub kind: MediaKind,
    /// The effective settings after merging config and flags.
    pub settings: &'a Settings,
}

/// A media viewer. Implementations render one media family.
///
/// The trait is the plug-in seam for tasks 021–025; the dispatch layer only ever sees this
/// trait, never the concrete viewer.
pub trait MediaViewer {
    /// A short human-readable name for diagnostics (e.g. `"image"`).
    fn name(&self) -> &'static str;

    /// Renders the request. Stubs return an error explaining the feature is pending.
    fn view(&mut self, request: &ViewRequest<'_>) -> anyhow::Result<()>;
}

/// The animated-GIF viewer (task 023): drives a `GifSource` through the frame engine.
#[derive(Debug, Default)]
pub struct GifViewer;
impl MediaViewer for GifViewer {
    fn name(&self) -> &'static str {
        "gif"
    }
    fn view(&mut self, request: &ViewRequest<'_>) -> anyhow::Result<()> {
        crate::playback_viewer::view_gif(request)
    }
}

/// The video viewer (task 023): drives the `ffmpeg` player through the frame engine, or reports a
/// clean note when `ffmpeg` support was not compiled in.
#[derive(Debug, Default)]
pub struct VideoViewer;
impl MediaViewer for VideoViewer {
    fn name(&self) -> &'static str {
        "video"
    }
    fn view(&mut self, request: &ViewRequest<'_>) -> anyhow::Result<()> {
        crate::playback_viewer::view_video(request)
    }
}

/// The interactive 3D-mesh viewer (task 022).
#[derive(Debug)]
pub struct MeshViewer {
    /// The concrete mesh format, so the viewer can pick the right loader.
    pub format: MeshFormat,
}
impl MediaViewer for MeshViewer {
    fn name(&self) -> &'static str {
        "mesh"
    }
    fn view(&mut self, request: &ViewRequest<'_>) -> anyhow::Result<()> {
        crate::mesh_viewer::view(request, self.format)
    }
}

/// Selects the viewer for a media kind.
///
/// Returns `Err` for [`MediaKind::Unknown`] so the caller can print a clean "unsupported"
/// message instead of proceeding.
pub fn viewer_for(kind: MediaKind) -> anyhow::Result<Box<dyn MediaViewer>> {
    match kind {
        MediaKind::Image => Ok(Box::new(ImageViewer)),
        MediaKind::Gif => Ok(Box::new(GifViewer)),
        MediaKind::Video => Ok(Box::new(VideoViewer)),
        MediaKind::Mesh(format) => Ok(Box::new(MeshViewer { format })),
        MediaKind::Unknown => Err(anyhow::anyhow!("unsupported or unrecognized media type")),
    }
}

/// Detects the kind, selects a viewer, and runs it. This is the whole dispatch path.
pub fn dispatch(input: &Input, kind: MediaKind, settings: &Settings) -> anyhow::Result<()> {
    let mut viewer = viewer_for(kind)?;
    let request = ViewRequest {
        input,
        kind,
        settings,
    };
    tracing::info!(viewer = viewer.name(), input = %input.label(), "dispatching");
    viewer.view(&request)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved_settings() -> Settings {
        Settings::resolve(&crate::config::Config::default(), &Default::default())
    }

    #[test]
    fn viewer_selection_matches_kind() {
        assert_eq!(viewer_for(MediaKind::Image).unwrap().name(), "image");
        assert_eq!(viewer_for(MediaKind::Gif).unwrap().name(), "gif");
        assert_eq!(viewer_for(MediaKind::Video).unwrap().name(), "video");
        assert_eq!(
            viewer_for(MediaKind::Mesh(MeshFormat::Stl)).unwrap().name(),
            "mesh"
        );
    }

    #[test]
    fn unknown_kind_has_no_viewer() {
        assert!(viewer_for(MediaKind::Unknown).is_err());
    }

    #[test]
    fn gif_and_video_are_real_and_error_cleanly_on_bad_input() {
        // GIF and video viewers are real now (023). A nonexistent file errors cleanly (never
        // panics, never "not yet implemented"): the GIF viewer fails to read the file, and the
        // video viewer fails to open it (or, without the `ffmpeg` feature, reports the build note).
        let input = Input::parse("does-not-exist.dat");
        let settings = resolved_settings();
        for kind in [MediaKind::Gif, MediaKind::Video] {
            let err = dispatch(&input, kind, &settings).unwrap_err();
            assert!(
                !err.to_string().contains("not yet implemented"),
                "unexpected stub message: {err}"
            );
        }
    }

    #[test]
    fn mesh_viewer_missing_file_is_clean_error() {
        // The mesh viewer is real (022): a missing file errors cleanly, never "not yet implemented".
        let input = Input::parse("does-not-exist.obj");
        let settings = resolved_settings();
        let err = dispatch(&input, MediaKind::Mesh(MeshFormat::Obj), &settings).unwrap_err();
        assert!(err.to_string().contains("loading OBJ"), "got: {err}");
    }

    #[test]
    fn dispatch_unknown_is_clean_error() {
        let input = Input::parse("x.dat");
        let settings = resolved_settings();
        let err = dispatch(&input, MediaKind::Unknown, &settings).unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }
}
