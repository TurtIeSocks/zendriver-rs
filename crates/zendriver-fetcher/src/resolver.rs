//! Version + platform → download URL resolution.
//!
//! Given a parsed Chrome-for-Testing manifest plus a
//! [`VersionSpec`]/[`Platform`] pair, picks the matching `(version, url)`
//! tuple — or returns a specific [`FetcherError`] when no match is possible.
//!
//! Split out from manifest fetching so unit tests can drive the resolver
//! with hand-built manifests, without touching the network.

use crate::error::FetcherError;
use crate::manifest::KnownGoodVersionsResponse;
use crate::platform::Platform;
use crate::version::{Channel, VersionSpec};

/// Resolve a `(version_string, download_url)` pair from a parsed manifest.
///
/// # Errors
///
/// - [`FetcherError::VersionNotFound`] if [`VersionSpec::Explicit`] does not
///   match any entry in the manifest.
/// - [`FetcherError::UnsupportedPlatform`] if the manifest has no download
///   for `platform`, or if a non-stable channel is requested (the
///   `latest-versions-per-milestone.json` endpoint is not wired in P5).
#[allow(dead_code, reason = "consumed by Fetcher::ensure_chrome in Task 21")]
pub(crate) async fn resolve_download_url(
    manifest: &KnownGoodVersionsResponse,
    spec: &VersionSpec,
    platform: Platform,
) -> Result<(String, String), FetcherError> {
    // Pick the manifest entry matching the version spec.
    let entry = match spec {
        VersionSpec::Latest | VersionSpec::Stable | VersionSpec::Channel(Channel::Stable) => {
            manifest
                .versions
                .last()
                .ok_or_else(|| FetcherError::VersionNotFound("manifest is empty".to_string()))?
        }
        VersionSpec::Channel(Channel::Beta | Channel::Dev | Channel::Canary) => {
            return Err(FetcherError::UnsupportedPlatform);
        }
        VersionSpec::Explicit(want) => manifest
            .versions
            .iter()
            .find(|v| &v.version == want)
            .ok_or_else(|| FetcherError::VersionNotFound(want.clone()))?,
    };

    // Walk that entry's downloads.chrome for a matching platform.
    let key = platform.as_cft_str();
    let download = entry
        .downloads
        .chrome
        .iter()
        .find(|d| d.platform == key)
        .ok_or(FetcherError::UnsupportedPlatform)?;

    Ok((entry.version.clone(), download.url.clone()))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn fixture_manifest() -> KnownGoodVersionsResponse {
        // Hand-built equivalent of the wiremock fixture in manifest.rs:
        // one version with linux64 + mac-x64 downloads.
        let json = r#"{
            "versions": [
                {
                    "version": "120.0.6099.234",
                    "revision": "1234",
                    "downloads": {
                        "chrome": [
                            {"platform": "linux64", "url": "https://example.com/chrome-linux64.zip"},
                            {"platform": "mac-x64", "url": "https://example.com/chrome-mac-x64.zip"}
                        ]
                    }
                }
            ]
        }"#;
        serde_json::from_str(json).unwrap()
    }

    #[tokio::test]
    async fn latest_returns_last_entry_url_for_matching_platform() {
        let manifest = fixture_manifest();
        let (version, url) =
            resolve_download_url(&manifest, &VersionSpec::Latest, Platform::LinuxX64)
                .await
                .unwrap();
        assert_eq!(version, "120.0.6099.234");
        assert_eq!(url, "https://example.com/chrome-linux64.zip");
    }

    #[tokio::test]
    async fn explicit_unknown_version_returns_version_not_found() {
        let manifest = fixture_manifest();
        let err = resolve_download_url(
            &manifest,
            &VersionSpec::Explicit("999.0".to_string()),
            Platform::LinuxX64,
        )
        .await
        .unwrap_err();
        match err {
            FetcherError::VersionNotFound(v) => assert_eq!(v, "999.0"),
            other => panic!("expected VersionNotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn beta_channel_returns_unsupported_platform() {
        let manifest = fixture_manifest();
        let err = resolve_download_url(
            &manifest,
            &VersionSpec::Channel(Channel::Beta),
            Platform::LinuxX64,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FetcherError::UnsupportedPlatform));
    }
}
