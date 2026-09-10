//! The `rgfx info <FILE>` implementation: inspect a file and print a concise,
//! non-interactive metadata report.
//!
//! This is the one place in the CLI that is deliberately allowed to write to
//! stdout without a terminal session: `info` neither renders nor enters raw
//! mode, it just describes a file. Routing is by [`MediaKind`] (reusing
//! [`crate::media`]); the per-kind details come from the library crates:
//! meshes from `rgfx-3d`'s loader stats, still images / GIFs from the `image`
//! decoder, and videos from `rgfx-video`'s `probe` (behind its `ffmpeg`
//! feature). Unknown or corrupt input yields a clean error, never a panic.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::media::{self, Input, MediaKind, MeshFormat};

/// A metadata report for a single input, rendered either as aligned key/value
/// text or as JSON (`--json`).
///
/// Fields are populated per media kind; absent fields are omitted from both the
/// human output and the JSON. The struct round-trips through `serde_json` so the
/// `--json` output is valid, machine-consumable JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// The input label (file path, or `<stdin>`).
    pub input: String,
    /// The detected media kind: `image`, `gif`, `video`, `mesh`, or `unknown`.
    pub kind: String,
    /// The concrete container/format (e.g. `PNG`, `glTF`, `mp4`), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Pixel width, for images/gif/video.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Pixel height, for images/gif/video.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Number of animation frames (still images report `1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<u64>,
    /// The pixel color type, for images/gif (e.g. `Rgba8`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_type: Option<String>,
    /// Mesh count, for meshes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meshes: Option<usize>,
    /// Total vertex count, for meshes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertices: Option<usize>,
    /// Total triangle count, for meshes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triangles: Option<usize>,
    /// Declared material count, for meshes that track materials (glTF).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub materials: Option<usize>,
    /// Declared animation count, for meshes that track animations (glTF).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub animations: Option<usize>,
    /// Per-animation `name (duration)` summaries, for meshes that declare animations (glTF).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animation_names: Vec<String>,
    /// Bounding-box size along each axis `[x, y, z]`, for meshes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounding_box: Option<[f32; 3]>,
    /// Video codec name (e.g. `h264`), when probed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// Frames per second, for video.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    /// Duration in seconds, for video.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    /// Non-fatal notes (e.g. why a video could not be probed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl Report {
    /// A bare report carrying just the input label and kind.
    fn new(input: &Input, kind: &str) -> Self {
        Self {
            input: input.label(),
            kind: kind.to_string(),
            format: None,
            width: None,
            height: None,
            frames: None,
            color_type: None,
            meshes: None,
            vertices: None,
            triangles: None,
            materials: None,
            animations: None,
            animation_names: Vec::new(),
            bounding_box: None,
            codec: None,
            fps: None,
            duration_secs: None,
            notes: Vec::new(),
        }
    }

    /// The ordered key/value rows for the human-readable rendering. Only present
    /// fields are included, in a stable, kind-agnostic order.
    fn rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = vec![("input", self.input.clone()), ("kind", self.kind.clone())];
        if let Some(v) = &self.format {
            rows.push(("format", v.clone()));
        }
        if let (Some(w), Some(h)) = (self.width, self.height) {
            rows.push(("dimensions", format!("{w} x {h}")));
        }
        if let Some(v) = self.frames {
            rows.push(("frames", v.to_string()));
        }
        if let Some(v) = &self.color_type {
            rows.push(("color type", v.clone()));
        }
        if let Some(v) = self.meshes {
            rows.push(("meshes", v.to_string()));
        }
        if let Some(v) = self.vertices {
            rows.push(("vertices", v.to_string()));
        }
        if let Some(v) = self.triangles {
            rows.push(("triangles", v.to_string()));
        }
        if let Some(v) = self.materials {
            rows.push(("materials", v.to_string()));
        }
        if let Some(v) = self.animations {
            rows.push(("animations", v.to_string()));
        }
        for name in &self.animation_names {
            rows.push(("animation", name.clone()));
        }
        if let Some([x, y, z]) = self.bounding_box {
            rows.push(("bounding box", format!("{x:.3} x {y:.3} x {z:.3}")));
        }
        if let Some(v) = &self.codec {
            rows.push(("codec", v.clone()));
        }
        if let Some(v) = self.fps {
            rows.push(("fps", format!("{v:.2}")));
        }
        if let Some(v) = self.duration_secs {
            rows.push(("duration", format!("{v:.2}s")));
        }
        rows
    }

    /// Renders the report as aligned `key: value` lines (trailing newline included).
    pub fn to_human(&self) -> String {
        let rows = self.rows();
        let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        let mut out = String::new();
        for (key, value) in rows {
            out.push_str(&format!("{key:<width$}  {value}\n"));
        }
        for note in &self.notes {
            out.push_str(&format!("note: {note}\n"));
        }
        out
    }

    /// Serializes the report as pretty JSON.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).context("serializing info report to JSON")
    }
}

/// Handles `rgfx info <FILE>`: build the report and print it.
pub fn run(file: &str, json: bool) -> Result<()> {
    let input = Input::parse(file);
    let report = build_report(&input)?;
    if json {
        println!("{}", report.to_json()?);
    } else {
        print!("{}", report.to_human());
    }
    Ok(())
}

/// Detects the kind and builds the appropriate [`Report`].
fn build_report(input: &Input) -> Result<Report> {
    match input {
        Input::File(path) => {
            let kind = media::detect_file(path)
                .with_context(|| format!("inspecting {}", path.display()))?;
            report_for_file(input, path, kind)
        }
        Input::Stdin => {
            let bytes = read_stdin()?;
            let kind = media::detect_bytes(&bytes, None);
            report_from_bytes(input, kind, &bytes)
        }
    }
}

/// Reads all of standard input into memory (for stdin `info`).
fn read_stdin() -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .context("reading standard input")?;
    Ok(bytes)
}

/// Builds a report for a file on disk.
fn report_for_file(input: &Input, path: &Path, kind: MediaKind) -> Result<Report> {
    match kind {
        MediaKind::Image => {
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            image_report(input, &bytes)
        }
        MediaKind::Gif => {
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            gif_report(input, &bytes)
        }
        MediaKind::Mesh(format) => mesh_report(input, path, format),
        MediaKind::Video => Ok(video_report(input, path)),
        MediaKind::Unknown => Err(unknown_error(input)),
    }
}

/// Builds a report for stdin-provided bytes (images and GIFs only).
fn report_from_bytes(input: &Input, kind: MediaKind, bytes: &[u8]) -> Result<Report> {
    match kind {
        MediaKind::Image => image_report(input, bytes),
        MediaKind::Gif => gif_report(input, bytes),
        MediaKind::Mesh(_) | MediaKind::Video => anyhow::bail!(
            "reading this media kind from standard input is not supported; pass a file path"
        ),
        MediaKind::Unknown => Err(unknown_error(input)),
    }
}

/// A clean error for unrecognized input.
fn unknown_error(input: &Input) -> anyhow::Error {
    anyhow::anyhow!("{}: unsupported or unrecognized media type", input.label())
}

/// Builds a still-image report (format, dimensions, color type) from bytes.
fn image_report(input: &Input, bytes: &[u8]) -> Result<Report> {
    let format = image::guess_format(bytes).map(format_label).ok();
    let decoded =
        image::load_from_memory(bytes).context("decoding image (unsupported or corrupt)")?;
    let mut report = Report::new(input, "image");
    report.format = format;
    report.width = Some(decoded.width());
    report.height = Some(decoded.height());
    report.frames = Some(1);
    report.color_type = Some(format!("{:?}", decoded.color()));
    Ok(report)
}

/// Builds an animated-GIF report (dimensions, frame count, color type) from bytes.
fn gif_report(input: &Input, bytes: &[u8]) -> Result<Report> {
    use image::AnimationDecoder;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
        .context("opening GIF (unsupported or corrupt)")?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .context("decoding GIF frames")?;
    let (width, height) = frames
        .first()
        .map(|f| {
            let buf = f.buffer();
            (buf.width(), buf.height())
        })
        .unwrap_or((0, 0));
    let mut report = Report::new(input, "gif");
    report.format = Some("GIF".to_string());
    report.width = Some(width);
    report.height = Some(height);
    report.frames = Some(frames.len() as u64);
    // GIF frames are composited to 8-bit RGBA by the decoder.
    report.color_type = Some("Rgba8".to_string());
    Ok(report)
}

/// Builds a mesh report from the relevant `rgfx-3d` loader's statistics.
fn mesh_report(input: &Input, path: &Path, format: MeshFormat) -> Result<Report> {
    let mut report = Report::new(input, "mesh");
    match format {
        MeshFormat::Obj => {
            let scene = rgfx_3d::load_obj(path)
                .with_context(|| format!("loading OBJ {}", path.display()))?;
            let stats = rgfx_3d::ObjStats::from_scene(&scene);
            report.format = Some("OBJ".to_string());
            report.meshes = Some(stats.meshes);
            report.vertices = Some(stats.vertices);
            report.triangles = Some(stats.triangles);
            report.bounding_box = stats.bounds.map(bbox_size);
        }
        MeshFormat::Stl => {
            let scene = rgfx_3d::load_stl(path)
                .with_context(|| format!("loading STL {}", path.display()))?;
            let stats = rgfx_3d::StlStats::from_scene(&scene);
            report.format = Some("STL".to_string());
            report.meshes = Some(scene.meshes.len());
            report.vertices = Some(stats.vertex_count);
            report.triangles = Some(stats.triangle_count);
            report.bounding_box = stats.bounding_box.map(bbox_size);
        }
        MeshFormat::Gltf => {
            let (_scene, stats) = rgfx_3d::load_gltf_with_stats(path)
                .with_context(|| format!("loading glTF {}", path.display()))?;
            report.format = Some("glTF".to_string());
            report.meshes = Some(stats.mesh_count);
            report.vertices = Some(stats.vertex_count);
            report.triangles = Some(stats.triangle_count);
            report.materials = Some(stats.material_count);
            report.animations = Some(stats.animation_count);
            report.animation_names = animation_summaries(&stats.animations);
            report.bounding_box = stats.bounding_box.map(bbox_size);
        }
        MeshFormat::Blend => {
            let (_scene, stats) = rgfx_3d::load_blend_with_stats(path).with_context(|| {
                format!(
                    "loading Blender file {} (via headless export)",
                    path.display()
                )
            })?;
            report.format = Some("Blender (via glTF export)".to_string());
            report.meshes = Some(stats.mesh_count);
            report.vertices = Some(stats.vertex_count);
            report.triangles = Some(stats.triangle_count);
            report.materials = Some(stats.material_count);
            report.animations = Some(stats.animation_count);
            report.animation_names = animation_summaries(&stats.animations);
            report.bounding_box = stats.bounding_box.map(bbox_size);
        }
    }
    Ok(report)
}

/// Formats each animation as `name (1.50s)`, tagging skinned animations so the (unsupported)
/// skinning is visible in `rgfx info`.
fn animation_summaries(animations: &[rgfx_3d::AnimationInfo]) -> Vec<String> {
    animations
        .iter()
        .map(|a| {
            let skin = if a.skinned { " [skinned]" } else { "" };
            format!("{} ({:.2}s){}", a.name, a.duration, skin)
        })
        .collect()
}

/// The `[x, y, z]` extent of a bounding box.
fn bbox_size(bbox: rgfx_core::BoundingBox) -> [f32; 3] {
    bbox.size().to_array()
}

/// Builds a video report. The container comes from the file extension; the
/// codec/resolution/fps/duration come from `rgfx-video`'s `probe`, which is only
/// available with the `ffmpeg` feature. Without it, or if probing fails, a clear
/// note is attached instead of panicking.
fn video_report(input: &Input, path: &Path) -> Report {
    let mut report = Report::new(input, "video");
    report.format = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    #[cfg(feature = "ffmpeg")]
    {
        match rgfx_video::probe(path) {
            Ok(info) => {
                report.width = Some(info.width);
                report.height = Some(info.height);
                report.fps = Some(info.fps);
                report.duration_secs = info.duration_secs;
                report.codec = info.codec;
            }
            Err(e) => {
                report
                    .notes
                    .push(format!("could not probe video with ffprobe: {e}"));
            }
        }
    }
    #[cfg(not(feature = "ffmpeg"))]
    {
        report.notes.push(
            "video probing needs the `ffmpeg` feature (rebuild with `--features ffmpeg`) \
             and an installed ffprobe; showing container only"
                .to_string(),
        );
    }

    report
}

/// A short label for an [`image::ImageFormat`] (e.g. `PNG`, `JPEG`).
fn format_label(format: image::ImageFormat) -> String {
    let s = format!("{format:?}");
    s.to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A unit cube centered at the origin: 8 shared corners, 6 quad faces
    /// (triangulated to 12 triangles), no normals.
    const CUBE_OBJ: &str = "\
o cube
v -1 -1 -1
v  1 -1 -1
v  1  1 -1
v -1  1 -1
v -1 -1  1
v  1 -1  1
v  1  1  1
v -1  1  1
f 1 2 3 4
f 5 8 7 6
f 1 5 6 2
f 2 6 7 3
f 3 7 8 4
f 4 8 5 1
";

    fn temp_file(contents: &[u8], suffix: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let unique = format!(
            "rgfx-info-{}-{:?}{suffix}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        path.push(unique);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(contents)
            .unwrap();
        path
    }

    #[test]
    fn mesh_info_for_obj_reports_expected_counts() {
        let path = temp_file(CUBE_OBJ.as_bytes(), ".obj");
        let input = Input::File(path.clone());
        let report = build_report(&input).expect("cube obj should describe");
        std::fs::remove_file(&path).ok();

        assert_eq!(report.kind, "mesh");
        assert_eq!(report.format.as_deref(), Some("OBJ"));
        assert_eq!(report.meshes, Some(1));
        assert_eq!(report.vertices, Some(8));
        assert_eq!(report.triangles, Some(12));
        let bb = report.bounding_box.expect("cube has bounds");
        assert!((bb[0] - 2.0).abs() < 1e-4);
        assert!((bb[1] - 2.0).abs() < 1e-4);
        assert!((bb[2] - 2.0).abs() < 1e-4);

        // Human output includes the key fields.
        let human = report.to_human();
        assert!(human.contains("triangles"), "human output: {human}");
        assert!(human.contains("12"), "human output: {human}");
    }

    #[test]
    fn json_output_is_valid_and_round_trips() {
        let path = temp_file(CUBE_OBJ.as_bytes(), ".obj");
        let input = Input::File(path.clone());
        let report = build_report(&input).unwrap();
        std::fs::remove_file(&path).ok();

        let json = report.to_json().unwrap();
        // Parses as generic JSON.
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(value["kind"], "mesh");
        assert_eq!(value["triangles"], 12);
        // Round-trips back to an identical report.
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
    }

    #[test]
    fn image_info_for_embedded_png() {
        // The 1x1 opaque-red PNG embedded in rgfx-image.
        let png = rgfx_image::doctest_png();
        let path = temp_file(png, ".png");
        let input = Input::File(path.clone());
        let report = build_report(&input).expect("png should describe");
        std::fs::remove_file(&path).ok();

        assert_eq!(report.kind, "image");
        assert_eq!(report.format.as_deref(), Some("PNG"));
        assert_eq!(report.width, Some(1));
        assert_eq!(report.height, Some(1));
        assert_eq!(report.frames, Some(1));
        assert_eq!(report.color_type.as_deref(), Some("Rgba8"));
    }

    #[test]
    fn unknown_input_is_clean_error_not_panic() {
        let path = temp_file(b"not any known media", ".dat");
        let input = Input::File(path.clone());
        let err = build_report(&input).unwrap_err();
        std::fs::remove_file(&path).ok();
        assert!(err.to_string().contains("unsupported"), "got: {err}");
    }

    #[test]
    fn corrupt_image_errors_cleanly() {
        // PNG magic but truncated/garbage body → decode error, not a panic.
        let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(b"garbage");
        let path = temp_file(&bytes, ".png");
        let input = Input::File(path.clone());
        let err = build_report(&input).unwrap_err();
        std::fs::remove_file(&path).ok();
        assert!(err.to_string().contains("decoding image"), "got: {err}");
    }

    #[test]
    fn animated_gltf_info_lists_animation_names_and_durations() {
        // The shared fixture in the rgfx-3d crate carries one "slide" animation of 1.0s.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../rgfx-3d/tests/assets/animated_triangle.glb");
        let input = Input::File(path.clone());
        let report = build_report(&input).expect("animated glb should describe");
        assert_eq!(report.kind, "mesh");
        assert_eq!(report.animations, Some(1));
        assert_eq!(report.animation_names, vec!["slide (1.00s)".to_string()]);
        // The human rendering surfaces the named animation row.
        let text = report.to_human();
        assert!(text.contains("slide (1.00s)"), "got:\n{text}");
    }
}
