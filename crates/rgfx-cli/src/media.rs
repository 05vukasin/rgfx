//! Input source handling and media-kind auto-detection.
//!
//! Detection uses two signals: the file extension and the leading "magic" bytes of the file.
//! Magic bytes are authoritative when they yield a confident match (they survive renamed
//! files); the extension is the fallback and the only signal for text formats such as OBJ that
//! have no reliable magic number.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Where the bytes to render come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// A file on disk.
    File(PathBuf),
    /// Standard input (the `-` argument).
    Stdin,
}

impl Input {
    /// Parses a CLI file argument into an [`Input`]. `-` maps to [`Input::Stdin`].
    pub fn parse(arg: &str) -> Self {
        if arg == "-" {
            Input::Stdin
        } else {
            Input::File(PathBuf::from(arg))
        }
    }

    /// A human-readable label for diagnostics.
    pub fn label(&self) -> String {
        match self {
            Input::File(p) => p.display().to_string(),
            Input::Stdin => "<stdin>".to_string(),
        }
    }
}

/// The concrete file format family behind a mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshFormat {
    /// Wavefront OBJ (`.obj`).
    Obj,
    /// Stereolithography (`.stl`), ASCII or binary.
    Stl,
    /// glTF / GLB (`.gltf`, `.glb`).
    Gltf,
    /// Blender scene (`.blend`), loaded by exporting to glTF via a headless Blender.
    Blend,
}

/// The kind of media an input holds, as far as `rgfx` needs to route it to a viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// A still raster image (png, jpeg, bmp, webp, …).
    Image,
    /// An animated GIF.
    Gif,
    /// A video container decoded frame by frame.
    Video,
    /// A 3D mesh in one of the supported formats.
    Mesh(MeshFormat),
    /// Nothing we recognize.
    Unknown,
}

impl MediaKind {
    /// Classifies purely by a lower-cased file extension (without the leading dot).
    ///
    /// Returns [`MediaKind::Unknown`] for anything unrecognized.
    pub fn from_extension(ext: &str) -> MediaKind {
        match ext.to_ascii_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "bmp" | "webp" | "tiff" | "tif" | "tga" => MediaKind::Image,
            "gif" => MediaKind::Gif,
            "mp4" | "mkv" | "mov" | "webm" | "avi" | "m4v" => MediaKind::Video,
            "obj" => MediaKind::Mesh(MeshFormat::Obj),
            "stl" => MediaKind::Mesh(MeshFormat::Stl),
            "gltf" | "glb" => MediaKind::Mesh(MeshFormat::Gltf),
            "blend" => MediaKind::Mesh(MeshFormat::Blend),
            _ => MediaKind::Unknown,
        }
    }

    /// Classifies from the leading bytes of a file.
    ///
    /// Returns `None` when the bytes carry no confident signal (e.g. text formats), so the
    /// caller can fall back to the extension.
    pub fn from_magic(bytes: &[u8]) -> Option<MediaKind> {
        // PNG: 89 50 4E 47 0D 0A 1A 0A
        if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
            return Some(MediaKind::Image);
        }
        // JPEG: FF D8 FF
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(MediaKind::Image);
        }
        // BMP: "BM"
        if bytes.starts_with(b"BM") {
            return Some(MediaKind::Image);
        }
        // GIF: "GIF87a" / "GIF89a"
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            return Some(MediaKind::Gif);
        }
        // GLB (binary glTF): magic "glTF" at offset 0.
        if bytes.starts_with(b"glTF") {
            return Some(MediaKind::Mesh(MeshFormat::Gltf));
        }
        // Blender: uncompressed `.blend` files start with "BLENDER". (Compressed saves are caught
        // by the `.blend` extension fallback instead.)
        if bytes.starts_with(b"BLENDER") {
            return Some(MediaKind::Mesh(MeshFormat::Blend));
        }
        // RIFF containers: WEBP (image) and AVI (video) share the "RIFF" prefix.
        if bytes.len() >= 12 && bytes.starts_with(b"RIFF") {
            match &bytes[8..12] {
                b"WEBP" => return Some(MediaKind::Image),
                b"AVI " => return Some(MediaKind::Video),
                _ => {}
            }
        }
        // ISO base media (mp4/mov/m4v): "ftyp" box tag at offset 4.
        if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
            return Some(MediaKind::Video);
        }
        // Matroska / WebM: EBML header 1A 45 DF A3.
        if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            return Some(MediaKind::Video);
        }
        None
    }

    /// Whether this kind is something we can (eventually) render.
    pub fn is_supported(self) -> bool {
        !matches!(self, MediaKind::Unknown)
    }
}

/// Number of leading bytes read from a file for magic-byte detection.
const MAGIC_PROBE_LEN: usize = 16;

/// Detects the [`MediaKind`] of an [`Input`].
///
/// For a file, magic bytes are consulted first, then the extension. For stdin, only the
/// provided already-buffered `peek` bytes are available (the caller must read them because
/// stdin is not seekable); the extension is unavailable.
pub fn detect(input: &Input) -> io::Result<MediaKind> {
    match input {
        Input::File(path) => detect_file(path),
        // Stdin detection needs bytes the caller has already buffered; without them we can only
        // report Unknown. `detect_bytes` is the seam the stdin path uses.
        Input::Stdin => Ok(MediaKind::Unknown),
    }
}

/// Detects a file's kind from magic bytes with an extension fallback.
pub fn detect_file(path: &Path) -> io::Result<MediaKind> {
    let mut buf = [0u8; MAGIC_PROBE_LEN];
    let n = match File::open(path) {
        Ok(mut f) => f.read(&mut buf).unwrap_or(0),
        // If the file cannot be opened we still classify by extension so `info`/error messages
        // stay useful; the caller will surface the real IO error when it tries to load.
        Err(_) => 0,
    };
    Ok(detect_bytes(&buf[..n], extension_of(path)))
}

/// Combines magic-byte and extension detection.
///
/// `magic` may be empty (unknown) and `ext` may be `None`. Magic wins when confident; the
/// extension is the fallback; otherwise [`MediaKind::Unknown`].
pub fn detect_bytes(magic: &[u8], ext: Option<&str>) -> MediaKind {
    if let Some(kind) = MediaKind::from_magic(magic) {
        return kind;
    }
    if let Some(ext) = ext {
        return MediaKind::from_extension(ext);
    }
    MediaKind::Unknown
}

/// The lower-cased extension of a path, if any.
fn extension_of(path: &Path) -> Option<&str> {
    path.extension().and_then(|e| e.to_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_parses_stdin_and_files() {
        assert_eq!(Input::parse("-"), Input::Stdin);
        assert_eq!(
            Input::parse("a/b.png"),
            Input::File(PathBuf::from("a/b.png"))
        );
    }

    #[test]
    fn extension_detection_table() {
        let cases: &[(&str, MediaKind)] = &[
            ("png", MediaKind::Image),
            ("PNG", MediaKind::Image),
            ("jpg", MediaKind::Image),
            ("jpeg", MediaKind::Image),
            ("bmp", MediaKind::Image),
            ("gif", MediaKind::Gif),
            ("mp4", MediaKind::Video),
            ("mkv", MediaKind::Video),
            ("obj", MediaKind::Mesh(MeshFormat::Obj)),
            ("stl", MediaKind::Mesh(MeshFormat::Stl)),
            ("glb", MediaKind::Mesh(MeshFormat::Gltf)),
            ("gltf", MediaKind::Mesh(MeshFormat::Gltf)),
            ("blend", MediaKind::Mesh(MeshFormat::Blend)),
            ("txt", MediaKind::Unknown),
            ("", MediaKind::Unknown),
        ];
        for (ext, expected) in cases {
            assert_eq!(MediaKind::from_extension(ext), *expected, "ext = {ext:?}");
        }
    }

    #[test]
    fn magic_byte_detection_table() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        assert_eq!(MediaKind::from_magic(&png), Some(MediaKind::Image));

        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0];
        assert_eq!(MediaKind::from_magic(&jpeg), Some(MediaKind::Image));

        assert_eq!(MediaKind::from_magic(b"GIF89a...."), Some(MediaKind::Gif));
        assert_eq!(MediaKind::from_magic(b"GIF87a...."), Some(MediaKind::Gif));

        // ISO-BMFF mp4: any 4-byte size prefix followed by "ftyp".
        let mp4 = *b"\x00\x00\x00\x18ftypmp42";
        assert_eq!(MediaKind::from_magic(&mp4), Some(MediaKind::Video));

        assert_eq!(
            MediaKind::from_magic(b"BLENDER-v300RENDH"),
            Some(MediaKind::Mesh(MeshFormat::Blend))
        );

        let glb = *b"glTF\x02\x00\x00\x00";
        assert_eq!(
            MediaKind::from_magic(&glb),
            Some(MediaKind::Mesh(MeshFormat::Gltf))
        );

        let webp = *b"RIFF\x00\x00\x00\x00WEBPVP8 ";
        assert_eq!(MediaKind::from_magic(&webp), Some(MediaKind::Image));

        let avi = *b"RIFF\x00\x00\x00\x00AVI LIST";
        assert_eq!(MediaKind::from_magic(&avi), Some(MediaKind::Video));

        let mkv = [0x1A, 0x45, 0xDF, 0xA3, 0, 0];
        assert_eq!(MediaKind::from_magic(&mkv), Some(MediaKind::Video));

        // Text / no signal.
        assert_eq!(MediaKind::from_magic(b"solid teapot\n"), None);
        assert_eq!(MediaKind::from_magic(b""), None);
    }

    #[test]
    fn combined_detection_prefers_magic_then_extension() {
        // A PNG renamed to .obj is still an image by magic.
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_bytes(&png, Some("obj")), MediaKind::Image);

        // OBJ text has no magic → extension decides.
        assert_eq!(
            detect_bytes(b"v 0 0 0\n", Some("obj")),
            MediaKind::Mesh(MeshFormat::Obj)
        );

        // No magic, no extension → Unknown.
        assert_eq!(detect_bytes(b"", None), MediaKind::Unknown);

        // No magic, unknown extension → Unknown.
        assert_eq!(detect_bytes(b"random", Some("dat")), MediaKind::Unknown);
    }

    #[test]
    fn detect_file_uses_extension_when_file_missing() {
        // Nonexistent path: open fails, falls back to extension.
        let kind = detect_file(Path::new("/no/such/file.png")).unwrap();
        assert_eq!(kind, MediaKind::Image);
    }

    #[test]
    fn detect_file_reads_real_magic_bytes() {
        let dir = std::env::temp_dir();
        let path = dir.join("rgfx_media_test.bin");
        // GIF magic, but a misleading .obj extension → magic must win.
        std::fs::write(&path, b"GIF89a\x01\x00\x01\x00").unwrap();
        let renamed = dir.join("rgfx_media_test.obj");
        std::fs::rename(&path, &renamed).unwrap();
        let kind = detect_file(&renamed).unwrap();
        std::fs::remove_file(&renamed).ok();
        assert_eq!(kind, MediaKind::Gif);
    }
}
