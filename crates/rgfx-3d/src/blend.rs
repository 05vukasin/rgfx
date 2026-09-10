//! Blender `.blend` loading by exporting to glTF with a headless Blender.
//!
//! `.blend` is Blender's internal, version-specific binary format — there is no practical native
//! parser. Instead, mirroring how `rgfx-video` shells out to `ffmpeg`, this module runs a headless
//! Blender (`blender -b … --python … -- out.glb`) to export the scene to a temporary `.glb`, then
//! loads that through the existing [`crate::load_gltf`] path so the result flows through the same
//! framebuffer pipeline as every other mesh.
//!
//! Blender is a **runtime** dependency: the base build never needs it. When it is missing, loading
//! returns an actionable [`rgfx_core::Error::External`] rather than panicking.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use rgfx_core::{Error, Result, Scene};

use crate::GltfStats;

/// The Python run inside Blender to export the opened scene to a single GLB. `argv[-1]` is the
/// output path (everything after `--` is passed through to the script).
const EXPORT_SCRIPT: &str = r#"
import bpy, sys
out = sys.argv[-1]
bpy.ops.export_scene.gltf(filepath=out, export_format='GLB', use_selection=False)
"#;

/// Locates the Blender executable: the `RGFX_BLENDER` override, else `blender` on `PATH`.
fn blender_program() -> std::ffi::OsString {
    std::env::var_os("RGFX_BLENDER").unwrap_or_else(|| "blender".into())
}

/// Whether a Blender executable can be found and launched. Useful for `info`/diagnostics.
pub fn blender_available() -> bool {
    Command::new(blender_program())
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A temp path unique to this process + call, cleaned up on drop.
struct TempGlb(PathBuf);

impl TempGlb {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("rgfx-blend-{}-{}.glb", std::process::id(), n));
        TempGlb(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempGlb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        // The export script is removed by its own guard; nothing else to clean.
    }
}

/// A temp `.py` script file, cleaned up on drop.
struct TempScript(PathBuf);

impl TempScript {
    fn write(contents: &str) -> Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("rgfx-blend-export-{}-{}.py", std::process::id(), n));
        std::fs::write(&p, contents)?;
        Ok(TempScript(p))
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Builds the headless-export argument list: `-b <in> --python <script> -- <out>`.
fn export_args(input: &Path, script: &Path, output: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("-b"),
        input.to_path_buf(),
        PathBuf::from("--python"),
        script.to_path_buf(),
        PathBuf::from("--"),
        output.to_path_buf(),
    ]
}

/// Loads a Blender `.blend` file by exporting it to glTF with a headless Blender, returning the
/// scene. Requires the `blender` executable at runtime (override with `RGFX_BLENDER`).
pub fn load_blend(path: impl AsRef<Path>) -> Result<Scene> {
    load_blend_with_stats(path).map(|(scene, _)| scene)
}

/// [`load_blend`] that also returns the exported-mesh [`GltfStats`].
pub fn load_blend_with_stats(path: impl AsRef<Path>) -> Result<(Scene, GltfStats)> {
    let input = path.as_ref();
    if !input.exists() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("blend file not found: {}", input.display()),
        )));
    }

    let out = TempGlb::new();
    let script = TempScript::write(EXPORT_SCRIPT)?;
    let args = export_args(input, script.path(), out.path());

    let output = Command::new(blender_program())
        .args(args.iter().map(|p| p.as_os_str()).collect::<Vec<&OsStr>>())
        .output()
        .map_err(|e| {
            Error::External(format!(
                "could not run Blender ({}): {e}. Install Blender, or set RGFX_BLENDER to its path, \
                 or export the model to .glb/.gltf and open that instead.",
                blender_program().to_string_lossy()
            ))
        })?;

    if !output.status.success() || !out.path().exists() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(Error::External(format!(
            "Blender failed to export {} to glTF (exit {:?}).\n{}",
            input.display(),
            output.status.code(),
            tail
        )));
    }

    // The exported GLB flows through the normal glTF path.
    crate::load_gltf_with_stats(out.path())
    // `out` and `script` are removed when their guards drop at end of scope.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_args_have_the_expected_shape() {
        let args = export_args(
            Path::new("/in.blend"),
            Path::new("/tmp/s.py"),
            Path::new("/tmp/o.glb"),
        );
        let strs: Vec<String> = args
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            strs,
            vec![
                "-b",
                "/in.blend",
                "--python",
                "/tmp/s.py",
                "--",
                "/tmp/o.glb"
            ]
        );
    }

    #[test]
    fn program_honors_env_override() {
        // Default is `blender`; the override is read from RGFX_BLENDER. We only assert the default
        // here (mutating process env in tests is racy and unsafe under edition 2024).
        if std::env::var_os("RGFX_BLENDER").is_none() {
            assert_eq!(blender_program(), std::ffi::OsString::from("blender"));
        }
    }

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        let err = load_blend("/definitely/not/here.blend").unwrap_err();
        assert!(matches!(err, Error::Io(_)));
    }

    #[test]
    fn export_script_targets_the_last_argv() {
        // Guards against silently dropping the GLB export format.
        assert!(EXPORT_SCRIPT.contains("export_scene.gltf"));
        assert!(EXPORT_SCRIPT.contains("export_format='GLB'"));
    }
}
