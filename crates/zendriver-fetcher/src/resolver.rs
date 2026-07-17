//! Version + platform → download URL resolution.
//!
//! Given a parsed Chrome-for-Testing manifest plus a
//! [`VersionSpec`]/[`Platform`] pair, picks the matching `(version, url)`
//! tuple — or returns a specific [`FetcherError`] when no match is possible.
//!
//! Split out from manifest fetching so unit tests can drive the resolver
//! with hand-built manifests, without touching the network.

use crate::error::FetcherError;
use crate::manifest::{ChannelsResponse, KnownGoodVersionsResponse};
use crate::platform::Platform;
use crate::version::{Channel, VersionSpec};

/// Resolve a `(version_string, download_url)` pair from a parsed
/// [`KnownGoodVersionsResponse`] manifest.
///
/// Only reached for [`VersionSpec::Latest`]/[`VersionSpec::Stable`]/
/// [`VersionSpec::Channel(Channel::Stable)`](VersionSpec::Channel)/
/// [`VersionSpec::Explicit`] — the non-stable channels resolve through
/// [`resolve_channel_download_url`] against a different (per-channel)
/// manifest instead, since `known-good-versions-with-downloads.json` only
/// ever tracks the stable channel's history.
///
/// # Errors
///
/// - [`FetcherError::VersionNotFound`] if [`VersionSpec::Explicit`] does not
///   match any entry in the manifest.
/// - [`FetcherError::UnsupportedPlatform`] if the manifest has no download
///   for `platform`.
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
            // Callers should route these through `resolve_channel_download_url`
            // + `ChannelsResponse` instead (see `Fetcher::ensure_chrome`).
            // Defensive fallback if this function is ever called directly
            // with a non-stable channel against the flat manifest.
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

/// Resolve a `(version_string, download_url)` pair for a non-stable
/// [`Channel`] from a parsed [`ChannelsResponse`] manifest (Chrome for
/// Testing's `last-known-good-versions-with-downloads.json`).
///
/// # Errors
///
/// - [`FetcherError::VersionNotFound`] if the manifest's `channels` map has
///   no entry for `channel` (unexpected — CfT always publishes all four —
///   but the map is caller-supplied JSON, so this stays a recoverable error
///   rather than a panic).
/// - [`FetcherError::UnsupportedPlatform`] if the channel entry has no
///   download for `platform`.
pub(crate) async fn resolve_channel_download_url(
    manifest: &ChannelsResponse,
    channel: Channel,
    platform: Platform,
) -> Result<(String, String), FetcherError> {
    let channel_key = channel.as_cft_str();
    let entry = manifest.channels.get(channel_key).ok_or_else(|| {
        FetcherError::VersionNotFound(format!("channel {channel_key} not present in manifest"))
    })?;

    let platform_key = platform.as_cft_str();
    let download = entry
        .downloads
        .chrome
        .iter()
        .find(|d| d.platform == platform_key)
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
    async fn beta_channel_returns_unsupported_platform_against_flat_manifest() {
        // `resolve_download_url` + the flat `KnownGoodVersionsResponse`
        // manifest never resolve a non-stable channel — that now routes
        // through `resolve_channel_download_url` + `ChannelsResponse`
        // instead (see the tests below). This just pins the defensive
        // fallback in case `resolve_download_url` is ever called directly
        // with a non-stable channel.
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

    fn fixture_channels_manifest() -> ChannelsResponse {
        // Hand-built equivalent of Chrome for Testing's
        // `last-known-good-versions-with-downloads.json`: keyed by channel
        // name rather than a flat version list.
        let json = r#"{
            "timestamp": "2026-07-16T00:00:00.000Z",
            "channels": {
                "Stable": {
                    "channel": "Stable",
                    "version": "120.0.6099.234",
                    "revision": "1234",
                    "downloads": {
                        "chrome": [
                            {"platform": "linux64", "url": "https://example.com/stable-linux64.zip"}
                        ]
                    }
                },
                "Beta": {
                    "channel": "Beta",
                    "version": "121.0.6100.10",
                    "revision": "1235",
                    "downloads": {
                        "chrome": [
                            {"platform": "linux64", "url": "https://example.com/beta-linux64.zip"}
                        ]
                    }
                },
                "Dev": {
                    "channel": "Dev",
                    "version": "122.0.6101.5",
                    "revision": "1236",
                    "downloads": {
                        "chrome": [
                            {"platform": "mac-x64", "url": "https://example.com/dev-mac-x64.zip"}
                        ]
                    }
                },
                "Canary": {
                    "channel": "Canary",
                    "version": "123.0.6102.1",
                    "revision": "1237",
                    "downloads": {
                        "chrome": [
                            {"platform": "linux64", "url": "https://example.com/canary-linux64.zip"}
                        ]
                    }
                }
            }
        }"#;
        serde_json::from_str(json).unwrap()
    }

    #[tokio::test]
    async fn beta_channel_resolves_from_channels_manifest() {
        let manifest = fixture_channels_manifest();
        let (version, url) =
            resolve_channel_download_url(&manifest, Channel::Beta, Platform::LinuxX64)
                .await
                .unwrap();
        assert_eq!(version, "121.0.6100.10");
        assert_eq!(url, "https://example.com/beta-linux64.zip");
    }

    #[tokio::test]
    async fn dev_and_canary_channels_resolve_from_channels_manifest() {
        let manifest = fixture_channels_manifest();

        let (version, url) =
            resolve_channel_download_url(&manifest, Channel::Dev, Platform::MacX64)
                .await
                .unwrap();
        assert_eq!(version, "122.0.6101.5");
        assert_eq!(url, "https://example.com/dev-mac-x64.zip");

        let (version, url) =
            resolve_channel_download_url(&manifest, Channel::Canary, Platform::LinuxX64)
                .await
                .unwrap();
        assert_eq!(version, "123.0.6102.1");
        assert_eq!(url, "https://example.com/canary-linux64.zip");
    }

    #[tokio::test]
    async fn channel_missing_platform_returns_unsupported_platform() {
        let manifest = fixture_channels_manifest();
        // "Dev" only has a mac-x64 download in the fixture.
        let err = resolve_channel_download_url(&manifest, Channel::Dev, Platform::LinuxX64)
            .await
            .unwrap_err();
        assert!(matches!(err, FetcherError::UnsupportedPlatform));
    }

    #[tokio::test]
    async fn channel_missing_from_manifest_returns_version_not_found() {
        let json = r#"{ "timestamp": "2026-07-16T00:00:00.000Z", "channels": {} }"#;
        let manifest: ChannelsResponse = serde_json::from_str(json).unwrap();
        let err = resolve_channel_download_url(&manifest, Channel::Beta, Platform::LinuxX64)
            .await
            .unwrap_err();
        match err {
            FetcherError::VersionNotFound(msg) => assert!(msg.contains("Beta")),
            other => panic!("expected VersionNotFound, got {other:?}"),
        }
    }
}
