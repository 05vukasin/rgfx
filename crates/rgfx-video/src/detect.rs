//! Locating the `ffmpeg` / `ffprobe` executables at runtime.
//!
//! Detection is a pure filesystem/`PATH` lookup with no process spawn, so a
//! missing tool is reported as an actionable [`Error::External`] rather than a
//! panic. The resolution logic is split into a pure [`resolve_tool`] helper so
//! the "missing binary" and override paths are unit-testable without ffmpeg
//! installed and without mutating process-global environment state.

use rgfx_core::{Error, Result};
use std::ffi::OsStr;
use std::path::PathBuf;

/// Locates the `ffmpeg` executable.
///
/// Honors the `RGFX_FFMPEG` environment override (an explicit path to the
/// binary) before searching `PATH`.
///
/// # Errors
///
/// Returns [`Error::External`] with an actionable install hint when the binary
/// cannot be found.
pub fn find_ffmpeg() -> Result<PathBuf> {
    find_tool("ffmpeg", "RGFX_FFMPEG")
}

/// Locates the `ffprobe` executable.
///
/// Honors the `RGFX_FFPROBE` environment override before searching `PATH`.
///
/// # Errors
///
/// Returns [`Error::External`] with an actionable install hint when the binary
/// cannot be found.
pub fn find_ffprobe() -> Result<PathBuf> {
    find_tool("ffprobe", "RGFX_FFPROBE")
}

/// Reads the relevant environment and delegates to [`resolve_tool`].
fn find_tool(name: &str, env_var: &str) -> Result<PathBuf> {
    let override_val = std::env::var_os(env_var);
    let path_var = std::env::var_os("PATH");
    resolve_tool(name, env_var, override_val.as_deref(), path_var.as_deref())
}

/// Resolves `name` to an executable path using the already-read `override_val`
/// (if any) and `path_var`, without touching global state.
///
/// The override takes precedence: if set, it must point at an existing file.
/// Otherwise each directory in `path_var` is searched for `name`.
fn resolve_tool(
    name: &str,
    env_var: &str,
    override_val: Option<&OsStr>,
    path_var: Option<&OsStr>,
) -> Result<PathBuf> {
    if let Some(explicit) = override_val {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
        return Err(Error::External(format!(
            "{env_var} points to `{}`, which is not a file",
            path.display()
        )));
    }

    if let Some(path_var) = path_var {
        for dir in std::env::split_paths(path_var) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(Error::External(format!(
        "`{name}` was not found on PATH. Install FFmpeg (https://ffmpeg.org/download.html) \
         so that `{name}` is available, or set {env_var} to its full path."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_binary_is_actionable_external_error_not_panic() {
        // No override, and a PATH that contains no such binary.
        let err = resolve_tool(
            "rgfx-definitely-not-a-real-binary-xyzzy",
            "RGFX_FFMPEG",
            None,
            Some(OsStr::new("/nonexistent-a:/nonexistent-b")),
        )
        .unwrap_err();
        match err {
            Error::External(msg) => {
                assert!(msg.contains("rgfx-definitely-not-a-real-binary-xyzzy"));
                assert!(
                    msg.contains("FFmpeg"),
                    "message should be actionable: {msg}"
                );
                assert!(msg.contains("RGFX_FFMPEG"));
            }
            other => panic!("expected External, got {other:?}"),
        }
    }

    #[test]
    fn absent_path_var_still_errors_cleanly() {
        let err = resolve_tool("ffmpeg", "RGFX_FFMPEG", None, None).unwrap_err();
        assert!(matches!(err, Error::External(_)));
    }

    #[test]
    fn override_to_a_real_file_is_used() {
        // The test binary itself is a guaranteed-existing file.
        let real = std::env::current_exe().expect("test exe path");
        let found = resolve_tool("ffmpeg", "RGFX_FFMPEG", Some(real.as_os_str()), None).unwrap();
        assert_eq!(found, real);
    }

    #[test]
    fn override_to_missing_file_is_error() {
        let err = resolve_tool(
            "ffmpeg",
            "RGFX_FFMPEG",
            Some(OsStr::new("/no/such/rgfx/ffmpeg/binary/here")),
            None,
        )
        .unwrap_err();
        match err {
            Error::External(msg) => assert!(msg.contains("not a file")),
            other => panic!("expected External, got {other:?}"),
        }
    }

    #[test]
    fn public_finders_return_result_without_panicking() {
        // Whatever the host has installed, these must return a Result, not panic.
        let _ = find_ffmpeg();
        let _ = find_ffprobe();
    }
}
