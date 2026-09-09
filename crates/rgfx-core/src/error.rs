//! The shared error type for rgfx library crates.

/// Errors produced across the rgfx library crates.
///
/// Variants are intentionally coarse: loaders, decoders, and renderers map their own errors
/// into these. The `rgfx-cli` binary wraps this with `anyhow` for user-facing reporting.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// An underlying I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The input's format is recognized but not supported.
    #[error("unsupported format: {0}")]
    Unsupported(String),

    /// A file could not be decoded/parsed.
    #[error("decode error: {0}")]
    Decode(String),

    /// The geometry of a mesh or scene is invalid (e.g. degenerate, out of range).
    #[error("invalid geometry: {0}")]
    Geometry(String),

    /// A required external dependency (such as `ffmpeg`) is missing or failed.
    #[error("external tool error: {0}")]
    External(String),

    /// A configuration value was invalid.
    #[error("configuration error: {0}")]
    Config(String),
}

/// A convenient result alias for rgfx library APIs.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_converts() {
        let e: Error = std::io::Error::new(std::io::ErrorKind::NotFound, "nope").into();
        assert!(matches!(e, Error::Io(_)));
        assert!(e.to_string().contains("io error"));
    }

    #[test]
    fn display_messages_are_prefixed() {
        assert_eq!(
            Error::Unsupported("xyz".into()).to_string(),
            "unsupported format: xyz"
        );
    }
}
