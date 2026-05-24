//! Fetcher-layer errors.

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FetcherError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("manifest: {0}")]
    Manifest(#[from] serde_json::Error),

    #[error("version not found: {0}")]
    VersionNotFound(String),

    #[error("unsupported platform")]
    UnsupportedPlatform,

    #[error("integrity check failed: expected {expected}, got {actual}")]
    IntegrityFailed { expected: String, actual: String },

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
