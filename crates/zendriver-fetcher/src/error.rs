//! Fetcher-layer errors.

/// Errors surfaced by [`crate::Fetcher`] during manifest fetch, download,
/// extract, or verification.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FetcherError {
    /// HTTP request failed (network down, 4xx/5xx response, etc.).
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    /// Local filesystem I/O failed (cache write, file permission, ...).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Manifest JSON could not be parsed.
    #[error("manifest: {0}")]
    Manifest(#[from] serde_json::Error),

    /// The requested [`crate::VersionSpec::Explicit`] string was not present
    /// in the manifest. Carries the requested version string.
    #[error("version not found: {0}")]
    VersionNotFound(String),

    /// The current platform is not covered by Chrome for Testing, or the
    /// requested non-stable [`crate::Channel`] is not yet wired.
    #[error("unsupported platform")]
    UnsupportedPlatform,

    /// SHA256 checksum mismatch on the downloaded archive.
    #[error("integrity check failed: expected {expected}, got {actual}")]
    IntegrityFailed {
        /// SHA256 from the manifest.
        expected: String,
        /// SHA256 computed from the downloaded bytes.
        actual: String,
    },

    /// The downloaded zip could not be extracted.
    #[error("extraction: {0}")]
    Extraction(String),
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_version_not_found() {
        let e = FetcherError::VersionNotFound("123.4.5.6".into());
        assert_eq!(e.to_string(), "version not found: 123.4.5.6");
    }

    #[test]
    fn display_integrity_failed() {
        let e = FetcherError::IntegrityFailed {
            expected: "abc".into(),
            actual: "def".into(),
        };
        assert_eq!(
            e.to_string(),
            "integrity check failed: expected abc, got def"
        );
    }

    #[test]
    fn display_unsupported_platform() {
        let e = FetcherError::UnsupportedPlatform;
        assert_eq!(e.to_string(), "unsupported platform");
    }
}
