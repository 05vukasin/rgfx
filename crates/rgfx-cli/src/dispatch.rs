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

/// Builds a "not yet implemented" error with a consistent, user-facing message.
fn not_yet_implemented(feature: &str, task: &str) -> anyhow::Error {
    anyhow::anyhow!("{feature} rendering is not yet implemented (arrives in task {task})")
}

/// Stub animated-GIF viewer (task 022).
#[derive(Debug, Default)]
pub struct GifViewer;
impl MediaViewer for GifViewer {
    fn name(&self) -> &'static str {
        "gif"
    }
    fn view(&mut self, _request: &ViewRequest<'_>) -> anyhow::Result<()> {
        Err(not_yet_implemented("animated GIF", "022"))
    }
}

/// Stub video viewer (task 024).
#[derive(Debug, Default)]
pub struct VideoViewer;
impl MediaViewer for VideoViewer {
    fn name(&self) -> &'static str {
        "video"
    }
    fn view(&mut self, _request: &ViewRequest<'_>) -> anyhow::Result<()> {
        Err(not_yet_implemented("video", "024"))
    }
}

/// Stub 3D-mesh viewer (task 023).
#[derive(Debug)]
pub struct MeshViewer {
    /// The concrete mesh format, so a real viewer can pick the right loader.
    pub format: MeshFormat,
}
impl MediaViewer for MeshViewer {
    fn name(&self) -> &'static str {
        "mesh"
    }
    fn view(&mut self, _request: &ViewRequest<'_>) -> anyhow::Result<()> {
        Err(not_yet_implemented("3D mesh", "023"))
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
    fn stubs_report_not_implemented_without_panicking() {
        // The image viewer is real now (task 021); the remaining families are still stubs.
        let input = Input::parse("x.dat");
        let settings = resolved_settings();
        for kind in [
            MediaKind::Gif,
            MediaKind::Video,
            MediaKind::Mesh(MeshFormat::Gltf),
        ] {
            let err = dispatch(&input, kind, &settings).unwrap_err();
            assert!(
                err.to_string().contains("not yet implemented"),
                "unexpected message: {err}"
            );
        }
    }

    #[test]
    fn dispatch_unknown_is_clean_error() {
        let input = Input::parse("x.dat");
        let settings = resolved_settings();
        let err = dispatch(&input, MediaKind::Unknown, &settings).unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }
}
