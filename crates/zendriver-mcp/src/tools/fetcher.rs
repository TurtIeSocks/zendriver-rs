//! Chrome-for-Testing fetcher tool — `browser_install_chrome`. Gated
//! behind the `fetcher` feature.
//!
//! ## v0 input shape
//!
//! The fetcher lib supports both an explicit version string and a release
//! channel via [`VersionSpec`]. For v0 the MCP wire surface keeps both
//! optional inputs but routes them with a documented precedence: if
//! `version` is set, the explicit string wins and `channel` is ignored;
//! otherwise `channel` (case-insensitive: `stable` / `beta` / `dev` /
//! `canary`) maps to [`VersionSpec::Channel`]; if neither is set the
//! fetcher falls back to its own [`VersionSpec::Latest`] default.
//!
//! All four channels (`stable` / `beta` / `dev` / `canary`) are wired
//! end-to-end in the lib: `stable` resolves through the flat
//! known-good-versions manifest, the other three through Chrome for
//! Testing's separate per-channel manifest. An unsupported combination
//! (e.g. a channel with no download for the resolved platform) still
//! surfaces [`FetcherError::UnsupportedPlatform`] at resolve time — the MCP
//! layer doesn't pre-reject it, it just lets the lib's own error tell the
//! caller.
//!
//! ## "list installed" intentionally dropped
//!
//! The plan listed a sibling `browser_list_installed_chromes` tool, but
//! the fetcher lib does not expose a cache-listing API and we don't want
//! the MCP layer reaching into the filesystem layout by hand for v0.
//! Dropped per the plan's API Reality note.
//!
//! [`VersionSpec`]: zendriver_fetcher::VersionSpec
//! [`VersionSpec::Channel`]: zendriver_fetcher::VersionSpec::Channel
//! [`VersionSpec::Latest`]: zendriver_fetcher::VersionSpec::Latest
//! [`FetcherError::UnsupportedPlatform`]: zendriver_fetcher::FetcherError::UnsupportedPlatform

#![cfg(feature = "fetcher")]

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
// `Channel` here is the fetcher's release channel (Stable/Beta/Dev/Canary).
// The crate root renamed it to `FetcherChannel` in the parity work (the bare
// `Channel` name now belongs to the browser-brand enum); alias it back so the
// fetcher-channel parsing below reads naturally.
use zendriver::{Fetcher, FetcherChannel as Channel, VersionSpec, ZendriverError};

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;

/// Input for `browser_install_chrome`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallInput {
    /// Exact Chrome-for-Testing version to install (e.g. `"126.0.6478.182"`).
    /// When set, the lookup uses [`VersionSpec::Explicit`] and `channel`
    /// is ignored. Unknown versions surface
    /// [`FetcherError::VersionNotFound`].
    ///
    /// [`VersionSpec::Explicit`]: zendriver_fetcher::VersionSpec::Explicit
    /// [`FetcherError::VersionNotFound`]: zendriver_fetcher::FetcherError::VersionNotFound
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Release channel selector. Accepted (case-insensitive): `stable`,
    /// `beta`, `dev`, `canary`. Used only when `version` is unset. All four
    /// channels are wired end-to-end in the lib. Unknown strings surface
    /// [`McpServerError`] without reaching the fetcher.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Override the cache directory root. Defaults to the OS cache dir
    /// (`$XDG_CACHE_HOME/zendriver/chrome` on Linux,
    /// `~/Library/Caches/zendriver/chrome` on macOS). Useful for CI runs
    /// that mount a shared persistent volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<String>,
}

/// Output of `browser_install_chrome`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InstallOutput {
    /// Filesystem path to the runnable Chrome binary on the MCP server
    /// host (not the client's machine).
    pub path: String,
    /// Echo of the caller's `version` input. `None` when `version` was
    /// not set (the fetcher resolved a channel or latest).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_requested: Option<String>,
    /// Echo of the caller's `channel` input. `None` when `channel` was
    /// not set (the fetcher used `version` or latest).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_requested: Option<String>,
}

/// Resolve, download (on cache miss), and return the path to a runnable
/// Chrome-for-Testing binary. See module docs for `version` / `channel`
/// precedence.
///
/// Does not need a browser open — the fetcher is independent of the
/// `SessionState`'s [`zendriver::Browser`] slot. The `state` argument is
/// kept for signature symmetry with every other tool handler.
pub async fn install_chrome(
    _state: Arc<Mutex<SessionState>>,
    input: InstallInput,
) -> Result<InstallOutput, ErrorData> {
    // Capture the inputs we'll echo before they're moved into the fetcher
    // configuration. Clone is cheap — these are short user-supplied
    // strings.
    let version_requested = input.version.clone();
    let channel_requested = input.channel.clone();

    let mut f = Fetcher::new();
    if let Some(c) = &input.cache_dir {
        f = f.cache_dir(PathBuf::from(c));
    }
    // version wins over channel — the lib has no merge mode and both
    // arms map onto a single VersionSpec slot, so we pick a documented
    // precedence rather than rejecting the combination outright.
    let spec = match (input.version, input.channel) {
        (Some(v), _) => Some(VersionSpec::Explicit(v)),
        (None, Some(c)) => Some(VersionSpec::Channel(parse_channel(&c)?)),
        (None, None) => None,
    };
    if let Some(spec) = spec {
        f = f.version(spec);
    }
    let path = f.ensure_chrome().await.map_err(|e| {
        // Route through the lib's `From<FetcherError> for ZendriverError`
        // so the existing `map_error` knows how to format it.
        map_error(McpServerError::from(ZendriverError::from(e)))
    })?;
    Ok(InstallOutput {
        path: path.display().to_string(),
        version_requested,
        channel_requested,
    })
}

/// Map a case-insensitive channel string onto the lib's [`Channel`] enum.
///
/// Rejecting unknown strings here (rather than passing through and letting
/// the lib refuse later) keeps the wire-side validation explicit so an
/// agent sees a clear "unknown channel" message rather than a downstream
/// `UnsupportedPlatform` that's actually about manifest coverage.
fn parse_channel(s: &str) -> Result<Channel, ErrorData> {
    match s.to_ascii_lowercase().as_str() {
        "stable" => Ok(Channel::Stable),
        "beta" => Ok(Channel::Beta),
        "dev" => Ok(Channel::Dev),
        "canary" => Ok(Channel::Canary),
        other => Err(ErrorData::invalid_request(
            format!(
                "Unknown channel `{other}`. Expected one of: stable, beta, dev, canary (case-insensitive)."
            ),
            None,
        )),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    //! No-browser, no-network unit coverage.
    //!
    //! The happy path needs real network + filesystem and is gated behind
    //! `integration-tests + fetcher`. Here we cover the input-shape
    //! validation: an unknown channel string surfaces a clear MCP error
    //! before any fetcher work runs, and a bogus version string surfaces
    //! the lib's own `VersionNotFound` (verified via the no-cache path
    //! where the manifest fetch fails / version is absent — too racy for a
    //! pure unit test, so we settle for the channel-validation arm here).

    use super::*;

    #[tokio::test]
    async fn install_with_unknown_channel_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = install_chrome(
            state,
            InstallInput {
                version: None,
                channel: Some("frob".into()),
                cache_dir: None,
            },
        )
        .await
        .expect_err("expected channel rejection");
        assert!(err.message.contains("Unknown channel `frob`"));
    }

    #[test]
    fn parse_channel_is_case_insensitive() {
        assert_eq!(parse_channel("Stable").unwrap(), Channel::Stable);
        assert_eq!(parse_channel("BETA").unwrap(), Channel::Beta);
        assert_eq!(parse_channel("dev").unwrap(), Channel::Dev);
        assert_eq!(parse_channel("Canary").unwrap(), Channel::Canary);
    }

    #[test]
    fn parse_channel_rejects_unknown() {
        let err = parse_channel("nightly").unwrap_err();
        assert!(err.message.contains("Unknown channel"));
        assert!(err.message.contains("nightly"));
    }
}
