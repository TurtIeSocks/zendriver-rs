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
        /// The SHA256 the caller pinned via [`crate::Fetcher::expected_sha256`].
        /// The CfT manifest publishes no per-download hashes, so this never
        /// comes from the manifest.
        expected: String,
        /// SHA256 computed from the downloaded bytes.
        actual: String,
    },

    /// The downloaded zip could not be extracted.
    #[error("extraction: {0}")]
    Extraction(String),

    /// The requested [`crate::VersionSpec`] cannot be honoured by the
    /// requested [`crate::Distribution`].
    ///
    /// Carries the reason *and* the alternative, because every case is "you
    /// asked for something this index cannot express": an explicit version
    /// against the revision-keyed snapshot bucket, or a release channel
    /// against a distribution that has none. Resolving those to "the newest
    /// build" instead would hand back a browser other than the one requested,
    /// which is worse than failing.
    #[error("{distribution} cannot resolve {selector}: {reason}")]
    UnsupportedSelector {
        /// Distribution that refused the selector.
        distribution: &'static str,
        /// The selector, as a short human-readable phrase.
        selector: String,
        /// Why it cannot be honoured, and what to use instead.
        reason: String,
    },

    /// The release exists but publishes no asset for the target platform.
    ///
    /// Routine for ungoogled-chromium, whose three per-OS repos release on
    /// independent cadences — a version present on Windows may simply not be
    /// built for macOS yet.
    #[error("{repo} release {tag} has no asset for {platform} (expected one ending in {suffix})")]
    AssetNotFound {
        /// `owner/name` of the repo searched.
        repo: String,
        /// Release tag searched.
        tag: String,
        /// Platform whose asset is missing.
        platform: &'static str,
        /// Asset filename suffix that was looked for.
        suffix: &'static str,
    },

    /// GitHub's API refused the request for rate-limiting reasons.
    ///
    /// Unauthenticated `api.github.com` allows 60 requests per hour per IP;
    /// setting `GITHUB_TOKEN` raises it to 5000. Surfaced as its own variant
    /// so the fix is in the message instead of buried in an HTTP 403.
    #[error(
        "github api rate limit reached (unauthenticated limit is 60 requests/hour{reset_hint}); \
         set GITHUB_TOKEN to raise it to 5000/hour"
    )]
    GitHubRateLimited {
        /// `", resets at <unix timestamp>"` when GitHub sent an
        /// `x-ratelimit-reset` header, otherwise empty.
        reset_hint: String,
    },

    /// The downloaded archive is in a format this host cannot unpack.
    ///
    /// In practice this is ungoogled-chromium's macOS `.dmg`, which is
    /// unpacked with `hdiutil` and therefore only on a macOS host.
    #[error("unsupported archive: {0}")]
    UnsupportedArchive(String),
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

    /// The whole point of this variant is that the message says what to do
    /// instead, so the message is what gets pinned.
    #[test]
    fn display_unsupported_selector_names_the_alternative() {
        let e = FetcherError::UnsupportedSelector {
            distribution: "Chromium snapshot",
            selector: "version 151.0.7922.76".into(),
            reason: "snapshots are keyed by revision; use --revision".into(),
        };
        assert_eq!(
            e.to_string(),
            "Chromium snapshot cannot resolve version 151.0.7922.76: \
             snapshots are keyed by revision; use --revision"
        );
    }

    #[test]
    fn display_rate_limited_names_the_limit_and_the_fix() {
        let e = FetcherError::GitHubRateLimited {
            reset_hint: ", resets at 1786023295".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("60 requests/hour"), "{msg}");
        assert!(msg.contains("GITHUB_TOKEN"), "{msg}");
        assert!(msg.contains("resets at 1786023295"), "{msg}");
    }

    #[test]
    fn display_asset_not_found_names_repo_tag_and_suffix() {
        let e = FetcherError::AssetNotFound {
            repo: "ungoogled-software/ungoogled-chromium-macos".into(),
            tag: "151.0.7922.71-1.1".into(),
            platform: "mac-arm64",
            suffix: "_arm64-macos.dmg",
        };
        assert_eq!(
            e.to_string(),
            "ungoogled-software/ungoogled-chromium-macos release 151.0.7922.71-1.1 \
             has no asset for mac-arm64 (expected one ending in _arm64-macos.dmg)"
        );
    }
}
