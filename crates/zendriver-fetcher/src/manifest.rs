//! Chrome for Testing manifest fetcher.
//!
//! Fetches and parses the `known-good-versions-with-downloads.json` manifest
//! published by Google's Chrome for Testing project, plus the per-channel
//! `last-known-good-versions-with-downloads.json` manifest used to resolve
//! the `Beta`/`Dev`/`Canary` channels.

use std::collections::HashMap;

use serde::Deserialize;

use crate::error::FetcherError;

#[allow(dead_code, reason = "consumed by resolver in Task 18")]
#[derive(Debug, Deserialize)]
pub(crate) struct KnownGoodVersionsResponse {
    pub versions: Vec<VersionEntry>,
}

#[allow(dead_code, reason = "consumed by resolver in Task 18")]
#[derive(Debug, Deserialize)]
pub(crate) struct VersionEntry {
    pub version: String,
    pub revision: String,
    pub downloads: Downloads,
}

#[allow(dead_code, reason = "consumed by resolver in Task 18")]
#[derive(Debug, Deserialize)]
pub(crate) struct Downloads {
    pub chrome: Vec<Download>,
}

#[allow(dead_code, reason = "consumed by resolver in Task 18")]
#[derive(Debug, Deserialize)]
pub(crate) struct Download {
    pub platform: String,
    pub url: String,
}

#[expect(dead_code, reason = "consumed by resolver in Task 18")]
pub(crate) async fn fetch_manifest() -> Result<KnownGoodVersionsResponse, FetcherError> {
    fetch_manifest_from(
        "https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json",
    )
    .await
}

/// Test helper — same as [`fetch_manifest`] but allows overriding the URL for
/// wiremock-based testing.
pub(crate) async fn fetch_manifest_from(
    url: &str,
) -> Result<KnownGoodVersionsResponse, FetcherError> {
    let resp = crate::tls::get(url).await?;
    let text = resp.text().await?;
    let parsed: KnownGoodVersionsResponse = serde_json::from_str(&text)?;
    Ok(parsed)
}

/// Chrome for Testing's per-channel manifest —
/// `last-known-good-versions-with-downloads.json`. Keyed by channel name
/// (`"Stable"`, `"Beta"`, `"Dev"`, `"Canary"`) rather than a flat version
/// list, since each channel tracks its own latest known-good build.
#[derive(Debug, Deserialize)]
pub(crate) struct ChannelsResponse {
    pub channels: HashMap<String, ChannelEntry>,
}

/// One channel's entry in [`ChannelsResponse`] — same shape as
/// [`VersionEntry`] (the manifest omits only the redundant `channel` name
/// field, already carried as this entry's map key).
#[allow(
    dead_code,
    reason = "revision not consumed by the resolver yet, same as VersionEntry"
)]
#[derive(Debug, Deserialize)]
pub(crate) struct ChannelEntry {
    pub version: String,
    pub revision: String,
    pub downloads: Downloads,
}

/// Same rationale as [`fetch_manifest_from`] — allows overriding the URL
/// for wiremock-based testing.
pub(crate) async fn fetch_channels_manifest_from(
    url: &str,
) -> Result<ChannelsResponse, FetcherError> {
    let resp = crate::tls::get(url).await?;
    let text = resp.text().await?;
    let parsed: ChannelsResponse = serde_json::from_str(&text)?;
    Ok(parsed)
}

/// One entry from GitHub's `GET /repos/{owner}/{repo}/releases` response.
///
/// ungoogled-chromium publishes no manifest — the release list *is* the
/// index, one per OS. Only the fields resolution needs are modelled; GitHub's
/// payload is large and the rest is ignored.
#[derive(Debug, Deserialize)]
pub(crate) struct GitHubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<GitHubAsset>,
}

/// A file attached to a [`GitHubRelease`].
#[derive(Debug, Deserialize)]
pub(crate) struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
}

/// Fetch a repo's release list, newest first (GitHub's own ordering).
///
/// One request with `per_page=100` rather than pagination: each page costs
/// one of the 60 unauthenticated requests per hour, and a single page covers
/// well over a year of ungoogled releases.
pub(crate) async fn fetch_github_releases(
    api_base: &str,
    repo: &str,
) -> Result<Vec<GitHubRelease>, FetcherError> {
    let url = format!(
        "{}/repos/{repo}/releases?per_page=100",
        api_base.trim_end_matches('/')
    );
    let text = crate::tls::get_github(&url).await?;
    Ok(serde_json::from_str(&text)?)
}

/// Fetch the newest revision published for `platform_dir` in Chromium's
/// snapshot bucket.
///
/// `LAST_CHANGE` is a plain-text integer, not JSON. The snapshot bucket has
/// no manifest of any kind — which is precisely why snapshots cannot be
/// resolved by version.
pub(crate) async fn fetch_last_change(
    snapshot_base: &str,
    platform_dir: &str,
) -> Result<u64, FetcherError> {
    let url = format!(
        "{}/{platform_dir}/LAST_CHANGE",
        snapshot_base.trim_end_matches('/')
    );
    let text = crate::tls::get(&url)
        .await?
        .error_for_status()?
        .text()
        .await?;
    text.trim().parse::<u64>().map_err(|_| {
        FetcherError::VersionNotFound(format!(
            "{url} returned {:?}, which is not a revision number",
            text.chars().take(64).collect::<String>()
        ))
    })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const FIXTURE_JSON: &str = r#"{"versions":[{"version":"120.0.6099.234","revision":"1234","downloads":{"chrome":[{"platform":"linux64","url":"https://example.com/chrome-linux64.zip"},{"platform":"mac-x64","url":"https://example.com/chrome-mac-x64.zip"}]}}]}"#;

    #[tokio::test]
    async fn parses_known_good_versions_from_stub_server() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/known-good-versions.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(FIXTURE_JSON))
            .mount(&server)
            .await;

        let url = format!("{}/known-good-versions.json", server.uri());
        let manifest = fetch_manifest_from(&url).await.unwrap();

        assert_eq!(manifest.versions.len(), 1);
        let v = &manifest.versions[0];
        assert_eq!(v.version, "120.0.6099.234");
        assert_eq!(v.revision, "1234");
        assert_eq!(v.downloads.chrome.len(), 2);
        assert_eq!(v.downloads.chrome[0].platform, "linux64");
        assert_eq!(
            v.downloads.chrome[0].url,
            "https://example.com/chrome-linux64.zip"
        );
        assert_eq!(v.downloads.chrome[1].platform, "mac-x64");
        assert_eq!(
            v.downloads.chrome[1].url,
            "https://example.com/chrome-mac-x64.zip"
        );
    }
}
