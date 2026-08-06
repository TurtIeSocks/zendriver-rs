//! Version + platform → download URL resolution, per distribution.
//!
//! This is the *whole* distribution axis. Everything after resolution —
//! streaming, SHA256, unpacking, the atomic cache — is shared, so each
//! distribution's contribution is a single function that turns a
//! [`VersionSpec`] + [`Platform`] into a [`Resolved`].
//!
//! The three indexes are shaped very differently:
//!
//! - **Chrome for Testing** publishes one JSON manifest covering every
//!   version and platform. A lookup is a filter over a list.
//! - **ungoogled-chromium** publishes no manifest. Three per-OS GitHub repos
//!   each cut their own releases, tagged `<chrome version>-<packaging>`, so
//!   an explicit version matches on the tag *prefix* and availability is
//!   answered per platform rather than assumed uniform.
//! - **Chromium snapshots** are keyed by revision, with no version index at
//!   all. That makes [`VersionSpec::Explicit`] unanswerable, and it is
//!   refused rather than silently served the newest build.
//!
//! Every function here is pure — it takes an already-fetched manifest or
//! release list — so the tests drive them from inline JSON literals with no
//! network involved.

use std::path::PathBuf;

use crate::archive::Archive;
use crate::cache::{
    UNGOOGLED_APP_BUNDLE, UNGOOGLED_APPIMAGE_NAME, cft_binary_subpath, snapshot_binary_subpath,
    ungoogled_binary_subpath,
};
use crate::distribution::Distribution;
use crate::error::FetcherError;
use crate::manifest::{ChannelsResponse, GitHubRelease, KnownGoodVersionsResponse};
use crate::platform::Platform;
use crate::version::{Channel, VersionSpec};

/// Canonical base for Chromium's continuous-build snapshot bucket.
pub(crate) const DEFAULT_SNAPSHOT_BASE: &str =
    "https://commondatastorage.googleapis.com/chromium-browser-snapshots";

/// Canonical base for GitHub's REST API.
pub(crate) const DEFAULT_GITHUB_API_BASE: &str = "https://api.github.com";

/// A download, fully pinned down.
///
/// Carries the archive shape and binary location alongside the URL because
/// those differ per distribution *and* per platform: a CfT zip, an ungoogled
/// AppImage and an ungoogled disk image need three different unpack steps and
/// land the executable in three different places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    /// Cache directory name and the build identity reported to the caller:
    /// a Chrome version for CfT/ungoogled, `r<revision>` for snapshots.
    pub build_id: String,
    /// Where to download it from.
    pub url: String,
    /// How the download is packaged.
    pub archive: Archive,
    /// Executable path relative to the build directory.
    pub binary_subpath: PathBuf,
}

/// One selectable build, for menus and listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// Selector that resolves exactly this build.
    pub spec: VersionSpec,
    /// One-line description, e.g. `151.0.7922.71 (tag 151.0.7922.71-1.1)`.
    pub label: String,
}

// ---------------------------------------------------------------------------
// Chrome for Testing
// ---------------------------------------------------------------------------

/// Resolve against Chrome for Testing's flat
/// `known-good-versions-with-downloads.json` manifest.
///
/// Reached for [`VersionSpec::Latest`]/[`VersionSpec::Stable`]/
/// [`VersionSpec::Channel(Channel::Stable)`](VersionSpec::Channel)/
/// [`VersionSpec::Explicit`] — the non-stable channels resolve through
/// [`resolve_cft_channel`] against the per-channel manifest instead, since
/// the flat manifest only ever tracks stable's history.
///
/// # Errors
///
/// - [`FetcherError::VersionNotFound`] if [`VersionSpec::Explicit`] does not
///   match any entry in the manifest.
/// - [`FetcherError::UnsupportedPlatform`] if the manifest has no download
///   for `platform`.
/// - [`FetcherError::UnsupportedSelector`] for [`VersionSpec::Revision`],
///   which Chrome for Testing does not index.
pub(crate) fn resolve_cft(
    manifest: &KnownGoodVersionsResponse,
    spec: &VersionSpec,
    platform: Platform,
) -> Result<Resolved, FetcherError> {
    let entry = match spec {
        VersionSpec::Latest | VersionSpec::Stable | VersionSpec::Channel(Channel::Stable) => {
            manifest
                .versions
                .last()
                .ok_or_else(|| FetcherError::VersionNotFound("manifest is empty".to_string()))?
        }
        VersionSpec::Channel(Channel::Beta | Channel::Dev | Channel::Canary) => {
            // Callers route these through `resolve_cft_channel` +
            // `ChannelsResponse` (see `Fetcher::ensure_chrome`). Defensive
            // fallback if this is ever called directly with a non-stable
            // channel against the flat manifest.
            return Err(FetcherError::UnsupportedPlatform);
        }
        VersionSpec::Revision(rev) => return Err(revision_unsupported(spec_dist_cft(), *rev)),
        VersionSpec::Explicit(want) => manifest
            .versions
            .iter()
            .find(|v| &v.version == want)
            .ok_or_else(|| FetcherError::VersionNotFound(want.clone()))?,
    };

    let key = platform.as_cft_str();
    let download = entry
        .downloads
        .chrome
        .iter()
        .find(|d| d.platform == key)
        .ok_or(FetcherError::UnsupportedPlatform)?;

    Ok(cft_resolved(&entry.version, &download.url, platform))
}

/// Resolve a non-stable [`Channel`] from Chrome for Testing's
/// `last-known-good-versions-with-downloads.json`.
///
/// # Errors
///
/// - [`FetcherError::VersionNotFound`] if the manifest's `channels` map has
///   no entry for `channel`.
/// - [`FetcherError::UnsupportedPlatform`] if the channel entry has no
///   download for `platform`.
pub(crate) fn resolve_cft_channel(
    manifest: &ChannelsResponse,
    channel: Channel,
    platform: Platform,
) -> Result<Resolved, FetcherError> {
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

    Ok(cft_resolved(&entry.version, &download.url, platform))
}

fn cft_resolved(version: &str, url: &str, platform: Platform) -> Resolved {
    Resolved {
        build_id: version.to_string(),
        url: url.to_string(),
        archive: Archive::Zip {
            top_dir: platform.cft_top_dir().to_string(),
        },
        binary_subpath: cft_binary_subpath(platform),
    }
}

/// Versions the manifest actually publishes for `platform`, newest first.
pub(crate) fn list_cft(manifest: &KnownGoodVersionsResponse, platform: Platform) -> Vec<Build> {
    let key = platform.as_cft_str();
    manifest
        .versions
        .iter()
        .rev() // the manifest is oldest-first
        .filter(|v| v.downloads.chrome.iter().any(|d| d.platform == key))
        .map(|v| Build {
            spec: VersionSpec::Explicit(v.version.clone()),
            label: v.version.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// ungoogled-chromium
// ---------------------------------------------------------------------------

/// Resolve against one per-OS ungoogled-chromium repo's release list.
///
/// `releases` is GitHub's own newest-first ordering. Drafts and prereleases
/// are skipped, and a release only counts if it actually carries an asset for
/// `platform` — the three repos release independently, so "this version
/// exists" and "this version exists *for your platform*" are different
/// questions and only the second one matters.
///
/// # Errors
///
/// - [`FetcherError::VersionNotFound`] if no release matches the spec.
/// - [`FetcherError::AssetNotFound`] if the matched release publishes nothing
///   for `platform`.
/// - [`FetcherError::UnsupportedSelector`] for channels and revisions, which
///   ungoogled-chromium does not publish.
pub(crate) fn resolve_ungoogled(
    releases: &[GitHubRelease],
    spec: &VersionSpec,
    platform: Platform,
    repo: &str,
) -> Result<Resolved, FetcherError> {
    let suffix = platform.ungoogled_asset_suffix();

    let release = match spec {
        VersionSpec::Latest | VersionSpec::Stable | VersionSpec::Channel(Channel::Stable) => {
            // Newest release that has a build for *this* platform.
            releases
                .iter()
                .filter(|r| is_published(r))
                .find(|r| asset_for(r, suffix).is_some())
                .ok_or_else(|| {
                    FetcherError::VersionNotFound(format!(
                        "{repo} publishes no release with a {suffix} asset"
                    ))
                })?
        }
        VersionSpec::Channel(channel) => {
            return Err(FetcherError::UnsupportedSelector {
                distribution: Distribution::UngoogledChromium.title(),
                selector: format!("the {} channel", channel.as_cft_str()),
                reason: "ungoogled-chromium cuts a single release stream per platform and has no \
                         channels; use the latest release or pin an explicit version"
                    .to_string(),
            });
        }
        VersionSpec::Revision(rev) => {
            return Err(revision_unsupported(
                Distribution::UngoogledChromium.title(),
                *rev,
            ));
        }
        VersionSpec::Explicit(want) => releases
            .iter()
            .filter(|r| is_published(r))
            .find(|r| tag_matches_version(&r.tag_name, want))
            .ok_or_else(|| {
                FetcherError::VersionNotFound(format!("{want} (searched {repo} releases)"))
            })?,
    };

    let asset = asset_for(release, suffix).ok_or_else(|| FetcherError::AssetNotFound {
        repo: repo.to_string(),
        tag: release.tag_name.clone(),
        platform: platform.as_cft_str(),
        suffix,
    })?;

    let archive = ungoogled_archive(platform, &asset.name);
    // The Windows zip's top-level directory is the asset name without its
    // extension, so the binary path has to be threaded through from the asset
    // rather than derived from the platform alone.
    let archive_top = asset.name.trim_end_matches(".zip");

    Ok(Resolved {
        build_id: chrome_version_of_tag(&release.tag_name).to_string(),
        url: asset.browser_download_url.clone(),
        archive,
        binary_subpath: ungoogled_binary_subpath(platform, archive_top),
    })
}

/// Releases with a build for `platform`, newest first.
pub(crate) fn list_ungoogled(releases: &[GitHubRelease], platform: Platform) -> Vec<Build> {
    let suffix = platform.ungoogled_asset_suffix();
    releases
        .iter()
        .filter(|r| is_published(r))
        .filter(|r| asset_for(r, suffix).is_some())
        .map(|r| {
            let version = chrome_version_of_tag(&r.tag_name);
            Build {
                spec: VersionSpec::Explicit(version.to_string()),
                label: format!("{version}  (tag {})", r.tag_name),
            }
        })
        .collect()
}

fn is_published(release: &GitHubRelease) -> bool {
    !release.draft && !release.prerelease
}

fn asset_for<'a>(
    release: &'a GitHubRelease,
    suffix: &str,
) -> Option<&'a crate::manifest::GitHubAsset> {
    release.assets.iter().find(|a| a.name.ends_with(suffix))
}

/// Does `tag` name Chrome version `want`?
///
/// ungoogled tags are `<chrome version>-<packaging revision>` —
/// `151.0.7922.71-1.1` on Windows/macOS, `151.0.7922.71-1` on portablelinux —
/// so matching is by prefix, not equality. The `-` boundary check is what
/// stops `151.0.7922.7` matching `151.0.7922.71-1.1`; a bare `starts_with`
/// would hand back a different build than the one requested.
fn tag_matches_version(tag: &str, want: &str) -> bool {
    tag == want
        || tag
            .strip_prefix(want)
            .is_some_and(|rest| rest.starts_with('-'))
}

/// Chrome version carried by an ungoogled tag: everything before the
/// packaging suffix.
fn chrome_version_of_tag(tag: &str) -> &str {
    tag.split('-').next().unwrap_or(tag)
}

fn ungoogled_archive(platform: Platform, asset_name: &str) -> Archive {
    match platform {
        Platform::LinuxX64 => Archive::Executable {
            file_name: UNGOOGLED_APPIMAGE_NAME.to_string(),
        },
        Platform::MacX64 | Platform::MacArm64 => Archive::AppBundleDmg {
            app_dir: UNGOOGLED_APP_BUNDLE.to_string(),
        },
        Platform::Win32 | Platform::Win64 => Archive::Zip {
            top_dir: asset_name.trim_end_matches(".zip").to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Chromium snapshots
// ---------------------------------------------------------------------------

/// Which revision a spec selects, or `None` for "whatever `LAST_CHANGE` says".
///
/// Pure, so the refusals are testable without a network stub — and they are
/// the interesting part. The snapshot bucket is laid out
/// `<platform>/<revision>/…` and publishes no version index whatsoever, so
/// there is nothing to look `151.0.7922.76` up in. Answering such a request
/// with the newest snapshot would return a *different browser* than the one
/// asked for, which is worse than an error, so it is refused with the
/// alternative named.
pub(crate) fn snapshot_revision_for(spec: &VersionSpec) -> Result<Option<u64>, FetcherError> {
    match spec {
        VersionSpec::Latest | VersionSpec::Stable | VersionSpec::Channel(Channel::Stable) => {
            Ok(None)
        }
        VersionSpec::Revision(rev) => Ok(Some(*rev)),
        VersionSpec::Channel(channel) => Err(FetcherError::UnsupportedSelector {
            distribution: Distribution::ChromiumSnapshot.title(),
            selector: format!("the {} channel", channel.as_cft_str()),
            reason: "the snapshot bucket is a continuous per-commit build with no channels; \
                     use the latest snapshot or pin a revision"
                .to_string(),
        }),
        VersionSpec::Explicit(version) => Err(FetcherError::UnsupportedSelector {
            distribution: Distribution::ChromiumSnapshot.title(),
            selector: format!("version {version}"),
            reason: format!(
                "snapshots are keyed by revision, not version, and the bucket publishes no \
                 version index — there is nothing to look {version} up in. Pin a revision \
                 instead (VersionSpec::Revision / --revision), or take the newest snapshot \
                 with --version latest"
            ),
        }),
    }
}

/// Build the download for a known snapshot revision.
///
/// The bucket layout is `<base>/<Platform>/<revision>/<stem>.zip`, and the
/// zip's single top-level directory is that same `<stem>`.
pub(crate) fn resolve_snapshot(base: &str, revision: u64, platform: Platform) -> Resolved {
    let dir = platform.as_snapshot_str();
    let stem = platform.snapshot_archive_stem();
    Resolved {
        // `r` prefix so a revision can never be mistaken for — or collide
        // with — a version directory in the cache.
        build_id: format!("r{revision}"),
        url: format!("{}/{dir}/{revision}/{stem}.zip", base.trim_end_matches('/')),
        archive: Archive::Zip {
            top_dir: stem.to_string(),
        },
        binary_subpath: snapshot_binary_subpath(platform),
    }
}

// ---------------------------------------------------------------------------
// shared
// ---------------------------------------------------------------------------

fn spec_dist_cft() -> &'static str {
    Distribution::ChromeForTesting.title()
}

fn revision_unsupported(distribution: &'static str, revision: u64) -> FetcherError {
    FetcherError::UnsupportedSelector {
        distribution,
        selector: format!("revision {revision}"),
        reason: "this distribution is indexed by version, not by Chromium revision; \
                 use --distribution snapshot to fetch by revision"
            .to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn fixture_manifest() -> KnownGoodVersionsResponse {
        // Oldest-first, like the real manifest: one older version and one
        // newer, with different platform coverage.
        let json = r#"{
            "versions": [
                {
                    "version": "119.0.6045.105",
                    "revision": "1230",
                    "downloads": {
                        "chrome": [
                            {"platform": "linux64", "url": "https://example.com/119-linux64.zip"}
                        ]
                    }
                },
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

    #[test]
    fn latest_returns_last_entry_url_for_matching_platform() {
        let r = resolve_cft(
            &fixture_manifest(),
            &VersionSpec::Latest,
            Platform::LinuxX64,
        )
        .unwrap();
        assert_eq!(r.build_id, "120.0.6099.234");
        assert_eq!(r.url, "https://example.com/chrome-linux64.zip");
        assert_eq!(
            r.archive,
            Archive::Zip {
                top_dir: "chrome-linux64".into()
            }
        );
    }

    #[test]
    fn explicit_unknown_version_returns_version_not_found() {
        let err = resolve_cft(
            &fixture_manifest(),
            &VersionSpec::Explicit("999.0".to_string()),
            Platform::LinuxX64,
        )
        .unwrap_err();
        match err {
            FetcherError::VersionNotFound(v) => assert_eq!(v, "999.0"),
            other => panic!("expected VersionNotFound, got {other:?}"),
        }
    }

    #[test]
    fn beta_channel_returns_unsupported_platform_against_flat_manifest() {
        // The flat manifest never resolves a non-stable channel — that routes
        // through `resolve_cft_channel` + `ChannelsResponse` instead. This
        // pins the defensive fallback.
        let err = resolve_cft(
            &fixture_manifest(),
            &VersionSpec::Channel(Channel::Beta),
            Platform::LinuxX64,
        )
        .unwrap_err();
        assert!(matches!(err, FetcherError::UnsupportedPlatform));
    }

    #[test]
    fn cft_refuses_a_revision_and_points_at_snapshots() {
        let err = resolve_cft(
            &fixture_manifest(),
            &VersionSpec::Revision(1674890),
            Platform::LinuxX64,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, FetcherError::UnsupportedSelector { .. }));
        assert!(msg.contains("revision 1674890"), "{msg}");
        assert!(msg.contains("--distribution snapshot"), "{msg}");
    }

    #[test]
    fn list_cft_is_newest_first_and_platform_filtered() {
        let manifest = fixture_manifest();

        let linux = list_cft(&manifest, Platform::LinuxX64);
        assert_eq!(linux.len(), 2);
        assert_eq!(linux[0].label, "120.0.6099.234");
        assert_eq!(linux[1].label, "119.0.6045.105");

        // Only the newer entry has a mac-x64 download.
        let mac = list_cft(&manifest, Platform::MacX64);
        assert_eq!(mac.len(), 1);
        assert_eq!(mac[0].spec, VersionSpec::Explicit("120.0.6099.234".into()));

        // ...and none has a win64 one.
        assert!(list_cft(&manifest, Platform::Win64).is_empty());
    }

    fn fixture_channels_manifest() -> ChannelsResponse {
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

    #[test]
    fn beta_channel_resolves_from_channels_manifest() {
        let r = resolve_cft_channel(
            &fixture_channels_manifest(),
            Channel::Beta,
            Platform::LinuxX64,
        )
        .unwrap();
        assert_eq!(r.build_id, "121.0.6100.10");
        assert_eq!(r.url, "https://example.com/beta-linux64.zip");
    }

    #[test]
    fn dev_and_canary_channels_resolve_from_channels_manifest() {
        let manifest = fixture_channels_manifest();

        let r = resolve_cft_channel(&manifest, Channel::Dev, Platform::MacX64).unwrap();
        assert_eq!(r.build_id, "122.0.6101.5");
        assert_eq!(r.url, "https://example.com/dev-mac-x64.zip");

        let r = resolve_cft_channel(&manifest, Channel::Canary, Platform::LinuxX64).unwrap();
        assert_eq!(r.build_id, "123.0.6102.1");
        assert_eq!(r.url, "https://example.com/canary-linux64.zip");
    }

    #[test]
    fn channel_missing_platform_returns_unsupported_platform() {
        // "Dev" only has a mac-x64 download in the fixture.
        let err = resolve_cft_channel(
            &fixture_channels_manifest(),
            Channel::Dev,
            Platform::LinuxX64,
        )
        .unwrap_err();
        assert!(matches!(err, FetcherError::UnsupportedPlatform));
    }

    #[test]
    fn channel_missing_from_manifest_returns_version_not_found() {
        let json = r#"{ "timestamp": "2026-07-16T00:00:00.000Z", "channels": {} }"#;
        let manifest: ChannelsResponse = serde_json::from_str(json).unwrap();
        let err = resolve_cft_channel(&manifest, Channel::Beta, Platform::LinuxX64).unwrap_err();
        match err {
            FetcherError::VersionNotFound(msg) => assert!(msg.contains("Beta")),
            other => panic!("expected VersionNotFound, got {other:?}"),
        }
    }

    // -- ungoogled ---------------------------------------------------------

    /// Trimmed to the fields the resolver reads, in GitHub's newest-first
    /// order, and modelling the availability skew that actually existed on
    /// 2026-08-06: Windows had 151, macOS was still on 150.
    fn fixture_windows_releases() -> Vec<GitHubRelease> {
        let json = r#"[
            {
                "tag_name": "151.0.7922.71-1.1",
                "draft": false,
                "prerelease": false,
                "assets": [
                    {"name": "ungoogled-chromium_151.0.7922.71-1.1_installer_x64.exe",
                     "browser_download_url": "https://example.com/151-installer-x64.exe"},
                    {"name": "ungoogled-chromium_151.0.7922.71-1.1_windows_x64.zip",
                     "browser_download_url": "https://example.com/151-windows-x64.zip"},
                    {"name": "ungoogled-chromium_151.0.7922.71-1.1_windows_x86.zip",
                     "browser_download_url": "https://example.com/151-windows-x86.zip"}
                ]
            },
            {
                "tag_name": "150.0.7871.186-1.1",
                "draft": false,
                "prerelease": false,
                "assets": [
                    {"name": "ungoogled-chromium_150.0.7871.186-1.1_windows_x64.zip",
                     "browser_download_url": "https://example.com/150-windows-x64.zip"}
                ]
            }
        ]"#;
        serde_json::from_str(json).unwrap()
    }

    fn fixture_macos_releases() -> Vec<GitHubRelease> {
        let json = r#"[
            {
                "tag_name": "150.0.7871.46-1.1",
                "draft": false,
                "prerelease": false,
                "assets": [
                    {"name": "ungoogled-chromium_150.0.7871.46-1.1_arm64-macos.dmg",
                     "browser_download_url": "https://example.com/150-arm64.dmg"},
                    {"name": "ungoogled-chromium_150.0.7871.46-1.1_x86_64-macos.dmg",
                     "browser_download_url": "https://example.com/150-x86_64.dmg"}
                ]
            }
        ]"#;
        serde_json::from_str(json).unwrap()
    }

    fn fixture_linux_releases() -> Vec<GitHubRelease> {
        // portablelinux uses a *shorter* packaging suffix (`-1`, not `-1.1`)
        // and attaches `.zsync` sidecars next to each AppImage.
        let json = r#"[
            {
                "tag_name": "151.0.7922.71-1",
                "draft": false,
                "prerelease": false,
                "assets": [
                    {"name": "ungoogled-chromium-151.0.7922.71-1-x86_64.AppImage",
                     "browser_download_url": "https://example.com/151-x86_64.AppImage"},
                    {"name": "ungoogled-chromium-151.0.7922.71-1-x86_64.AppImage.zsync",
                     "browser_download_url": "https://example.com/151-x86_64.AppImage.zsync"},
                    {"name": "ungoogled-chromium-151.0.7922.71-1-x86_64_linux.tar.xz",
                     "browser_download_url": "https://example.com/151-x86_64.tar.xz"}
                ]
            }
        ]"#;
        serde_json::from_str(json).unwrap()
    }

    /// The headline ungoogled behaviour: a tag carries a packaging suffix, so
    /// an explicit version matches the tag's *prefix*.
    #[test]
    fn explicit_version_matches_an_ungoogled_tag_by_prefix() {
        let r = resolve_ungoogled(
            &fixture_windows_releases(),
            &VersionSpec::Explicit("151.0.7922.71".into()),
            Platform::Win64,
            "ungoogled-software/ungoogled-chromium-windows",
        )
        .unwrap();
        assert_eq!(r.build_id, "151.0.7922.71");
        assert_eq!(r.url, "https://example.com/151-windows-x64.zip");
    }

    /// Both suffix shapes in the wild resolve: `-1.1` on Windows/macOS and
    /// `-1` on portablelinux.
    #[test]
    fn prefix_matching_handles_both_packaging_suffix_shapes() {
        assert!(tag_matches_version("151.0.7922.71-1.1", "151.0.7922.71"));
        assert!(tag_matches_version("151.0.7922.71-1", "151.0.7922.71"));
        // The full tag also works, for anyone copying it off the releases page.
        assert!(tag_matches_version(
            "151.0.7922.71-1.1",
            "151.0.7922.71-1.1"
        ));
    }

    /// A bare `starts_with` would match here and hand back build `…71` for a
    /// request for `…7`. The `-` boundary is what prevents it.
    #[test]
    fn prefix_matching_stops_at_the_packaging_separator() {
        assert!(!tag_matches_version("151.0.7922.71-1.1", "151.0.7922.7"));
        assert!(!tag_matches_version("151.0.7922.71-1.1", "151.0"));
        assert!(!tag_matches_version("151.0.7922.71-1.1", "150.0.7871.46"));
    }

    #[test]
    fn ungoogled_picks_the_right_asset_and_archive_per_platform() {
        // Windows: a zip whose top-level dir is the asset name minus `.zip`.
        let win = resolve_ungoogled(
            &fixture_windows_releases(),
            &VersionSpec::Latest,
            Platform::Win64,
            "repo",
        )
        .unwrap();
        assert_eq!(
            win.archive,
            Archive::Zip {
                top_dir: "ungoogled-chromium_151.0.7922.71-1.1_windows_x64".into()
            }
        );
        assert_eq!(
            win.binary_subpath,
            std::path::Path::new("ungoogled-chromium_151.0.7922.71-1.1_windows_x64/chrome.exe")
        );

        // Linux: the AppImage, never the `.zsync` sidecar or the tarball.
        let linux = resolve_ungoogled(
            &fixture_linux_releases(),
            &VersionSpec::Latest,
            Platform::LinuxX64,
            "repo",
        )
        .unwrap();
        assert_eq!(linux.url, "https://example.com/151-x86_64.AppImage");
        assert_eq!(
            linux.archive,
            Archive::Executable {
                file_name: "chrome.AppImage".into()
            }
        );

        // macOS: the arch-matching disk image.
        let mac = resolve_ungoogled(
            &fixture_macos_releases(),
            &VersionSpec::Latest,
            Platform::MacArm64,
            "repo",
        )
        .unwrap();
        assert_eq!(mac.url, "https://example.com/150-arm64.dmg");
        assert_eq!(
            mac.archive,
            Archive::AppBundleDmg {
                app_dir: "Chromium.app".into()
            }
        );
        assert_eq!(
            resolve_ungoogled(
                &fixture_macos_releases(),
                &VersionSpec::Latest,
                Platform::MacX64,
                "repo",
            )
            .unwrap()
            .url,
            "https://example.com/150-x86_64.dmg"
        );
    }

    /// Availability genuinely differs per platform. Asking macOS for the
    /// version Windows has must report *that*, not fall back to something
    /// else.
    #[test]
    fn a_version_present_on_windows_but_not_macos_reports_not_found() {
        let err = resolve_ungoogled(
            &fixture_macos_releases(),
            &VersionSpec::Explicit("151.0.7922.71".into()),
            Platform::MacArm64,
            "ungoogled-software/ungoogled-chromium-macos",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, FetcherError::VersionNotFound(_)));
        assert!(msg.contains("151.0.7922.71"), "{msg}");
        assert!(msg.contains("ungoogled-chromium-macos"), "{msg}");
    }

    /// A release that exists but carries no asset for this arch is a distinct
    /// failure from "no such version", and says which suffix it looked for.
    #[test]
    fn release_without_an_asset_for_the_platform_reports_asset_not_found() {
        let releases: Vec<GitHubRelease> = serde_json::from_str(
            r#"[{
                "tag_name": "151.0.7922.71-1.1",
                "draft": false,
                "prerelease": false,
                "assets": [
                    {"name": "ungoogled-chromium_151.0.7922.71-1.1_windows_x64.zip",
                     "browser_download_url": "https://example.com/x64.zip"}
                ]
            }]"#,
        )
        .unwrap();

        let err = resolve_ungoogled(
            &releases,
            &VersionSpec::Explicit("151.0.7922.71".into()),
            Platform::Win32,
            "ungoogled-software/ungoogled-chromium-windows",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, FetcherError::AssetNotFound { .. }));
        assert!(msg.contains("_windows_x86.zip"), "{msg}");
    }

    #[test]
    fn ungoogled_skips_drafts_and_prereleases() {
        let releases: Vec<GitHubRelease> = serde_json::from_str(
            r#"[
                {"tag_name": "152.0.0.1-1.1", "draft": true, "prerelease": false,
                 "assets": [{"name": "ungoogled-chromium_152.0.0.1-1.1_windows_x64.zip",
                             "browser_download_url": "https://example.com/draft.zip"}]},
                {"tag_name": "152.0.0.0-1.1", "draft": false, "prerelease": true,
                 "assets": [{"name": "ungoogled-chromium_152.0.0.0-1.1_windows_x64.zip",
                             "browser_download_url": "https://example.com/pre.zip"}]},
                {"tag_name": "151.0.7922.71-1.1", "draft": false, "prerelease": false,
                 "assets": [{"name": "ungoogled-chromium_151.0.7922.71-1.1_windows_x64.zip",
                             "browser_download_url": "https://example.com/real.zip"}]}
            ]"#,
        )
        .unwrap();

        let r =
            resolve_ungoogled(&releases, &VersionSpec::Latest, Platform::Win64, "repo").unwrap();
        assert_eq!(r.url, "https://example.com/real.zip");
        assert_eq!(list_ungoogled(&releases, Platform::Win64).len(), 1);
    }

    /// `Latest` must skip past a newer release that has no build for this
    /// platform rather than failing or returning the wrong asset.
    #[test]
    fn latest_skips_releases_lacking_this_platforms_asset() {
        let releases: Vec<GitHubRelease> = serde_json::from_str(
            r#"[
                {"tag_name": "152.0.0.0-1.1", "draft": false, "prerelease": false,
                 "assets": [{"name": "ungoogled-chromium_152.0.0.0-1.1_windows_x64.zip",
                             "browser_download_url": "https://example.com/152-x64.zip"}]},
                {"tag_name": "151.0.7922.71-1.1", "draft": false, "prerelease": false,
                 "assets": [{"name": "ungoogled-chromium_151.0.7922.71-1.1_windows_x86.zip",
                             "browser_download_url": "https://example.com/151-x86.zip"}]}
            ]"#,
        )
        .unwrap();

        let r =
            resolve_ungoogled(&releases, &VersionSpec::Latest, Platform::Win32, "repo").unwrap();
        assert_eq!(r.build_id, "151.0.7922.71");
        assert_eq!(r.url, "https://example.com/151-x86.zip");
    }

    #[test]
    fn ungoogled_refuses_channels_and_revisions() {
        let releases = fixture_windows_releases();

        let err = resolve_ungoogled(
            &releases,
            &VersionSpec::Channel(Channel::Beta),
            Platform::Win64,
            "repo",
        )
        .unwrap_err();
        assert!(matches!(err, FetcherError::UnsupportedSelector { .. }));
        assert!(err.to_string().contains("no channels"), "{err}");

        let err = resolve_ungoogled(
            &releases,
            &VersionSpec::Revision(1674890),
            Platform::Win64,
            "repo",
        )
        .unwrap_err();
        assert!(matches!(err, FetcherError::UnsupportedSelector { .. }));
    }

    #[test]
    fn list_ungoogled_labels_carry_both_version_and_tag() {
        let builds = list_ungoogled(&fixture_linux_releases(), Platform::LinuxX64);
        assert_eq!(builds.len(), 1);
        assert_eq!(
            builds[0].spec,
            VersionSpec::Explicit("151.0.7922.71".into())
        );
        assert!(builds[0].label.contains("151.0.7922.71"));
        assert!(builds[0].label.contains("tag 151.0.7922.71-1"));
    }

    // -- snapshots ---------------------------------------------------------

    /// The load-bearing snapshot behaviour: an explicit version is refused,
    /// with the reason and the alternative in the message. Silently returning
    /// the newest snapshot would hand back a different browser.
    #[test]
    fn snapshot_refuses_an_explicit_version_and_says_why() {
        let err =
            snapshot_revision_for(&VersionSpec::Explicit("151.0.7922.76".into())).unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, FetcherError::UnsupportedSelector { .. }));
        assert!(msg.contains("151.0.7922.76"), "{msg}");
        assert!(msg.contains("keyed by revision"), "{msg}");
        assert!(msg.contains("--revision"), "{msg}");
    }

    #[test]
    fn snapshot_refuses_channels() {
        let err = snapshot_revision_for(&VersionSpec::Channel(Channel::Canary)).unwrap_err();
        assert!(matches!(err, FetcherError::UnsupportedSelector { .. }));
        assert!(err.to_string().contains("no channels"), "{err}");
    }

    #[test]
    fn snapshot_latest_defers_to_last_change_and_revision_pins() {
        assert_eq!(snapshot_revision_for(&VersionSpec::Latest).unwrap(), None);
        assert_eq!(snapshot_revision_for(&VersionSpec::Stable).unwrap(), None);
        assert_eq!(
            snapshot_revision_for(&VersionSpec::Channel(Channel::Stable)).unwrap(),
            None
        );
        assert_eq!(
            snapshot_revision_for(&VersionSpec::Revision(1674890)).unwrap(),
            Some(1674890)
        );
    }

    /// Snapshot URLs use the bucket's own platform spelling, which differs
    /// from CfT's on every platform.
    #[test]
    fn snapshot_urls_use_the_bucket_platform_names() {
        let r = resolve_snapshot(DEFAULT_SNAPSHOT_BASE, 1674890, Platform::MacArm64);
        assert_eq!(r.build_id, "r1674890");
        assert_eq!(
            r.url,
            "https://commondatastorage.googleapis.com/chromium-browser-snapshots/Mac_Arm/1674890/chrome-mac.zip"
        );
        assert_eq!(
            r.archive,
            Archive::Zip {
                top_dir: "chrome-mac".into()
            }
        );

        assert_eq!(
            resolve_snapshot(DEFAULT_SNAPSHOT_BASE, 1674856, Platform::Win64).url,
            "https://commondatastorage.googleapis.com/chromium-browser-snapshots/Win_x64/1674856/chrome-win.zip"
        );
        assert_eq!(
            resolve_snapshot(DEFAULT_SNAPSHOT_BASE, 1674883, Platform::LinuxX64).url,
            "https://commondatastorage.googleapis.com/chromium-browser-snapshots/Linux_x64/1674883/chrome-linux.zip"
        );
    }

    /// A snapshot build id can never collide with a CfT/ungoogled version
    /// directory, which is what lets them share a cache root.
    #[test]
    fn snapshot_build_ids_are_revision_prefixed() {
        let r = resolve_snapshot(DEFAULT_SNAPSHOT_BASE, 1674890, Platform::LinuxX64);
        assert!(r.build_id.starts_with('r'));
        assert!(!r.build_id.contains('.'));
    }
}
