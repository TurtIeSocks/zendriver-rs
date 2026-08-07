//! Enumerate the builds a distribution actually publishes for a platform.
//!
//! Exists because "what can I download?" has a genuinely different answer per
//! platform, and guessing is how you end up asking macOS for a version only
//! Windows has. The CLI's interactive picker is built on this; so is any
//! caller that wants to show a menu rather than hardcode a version.
//!
//! [`Distribution::ChromiumSnapshot`] is the thin one: the bucket has no
//! index, so the only enumerable build is the tip named by `LAST_CHANGE`.
//! Older snapshots are reachable, but only if you already know the revision.

use crate::distribution::Distribution;
use crate::error::FetcherError;
use crate::manifest::{fetch_github_releases, fetch_last_change, fetch_manifest_from};
use crate::platform::Platform;
use crate::resolver::{
    Build, DEFAULT_GITHUB_API_BASE, DEFAULT_SNAPSHOT_BASE, list_cft, list_ungoogled,
};
use crate::version::VersionSpec;

/// Builds `distribution` publishes for `platform`, newest first.
///
/// # Errors
///
/// Whatever the distribution's index fetch returns — including
/// [`FetcherError::GitHubRateLimited`] for ungoogled-chromium on an
/// unauthenticated host that has used its 60 requests.
///
/// ```no_run
/// # async fn ex() -> Result<(), zendriver_fetcher::FetcherError> {
/// use zendriver_fetcher::{Distribution, Platform, list_builds};
///
/// for build in list_builds(Distribution::UngoogledChromium, Platform::MacArm64).await? {
///     println!("{}", build.label);
/// }
/// # Ok(()) }
/// ```
pub async fn list_builds(
    distribution: Distribution,
    platform: Platform,
) -> Result<Vec<Build>, FetcherError> {
    list_builds_from(distribution, platform, None).await
}

/// [`list_builds`] with an overridable index endpoint — the same
/// `#[doc(hidden)]` test seam as [`Fetcher::manifest_url`](crate::Fetcher::manifest_url).
#[doc(hidden)]
pub async fn list_builds_from(
    distribution: Distribution,
    platform: Platform,
    index_base: Option<&str>,
) -> Result<Vec<Build>, FetcherError> {
    match distribution {
        Distribution::ChromeForTesting => {
            let url = index_base.unwrap_or(crate::fetcher::DEFAULT_CFT_URL);
            let manifest = fetch_manifest_from(url).await?;
            Ok(list_cft(&manifest, platform))
        }
        Distribution::UngoogledChromium => {
            let api_base = index_base.unwrap_or(DEFAULT_GITHUB_API_BASE);
            let repo = Distribution::ungoogled_repo(platform);
            let releases = fetch_github_releases(api_base, repo).await?;
            Ok(list_ungoogled(&releases, platform))
        }
        Distribution::ChromiumSnapshot => {
            let base = index_base.unwrap_or(DEFAULT_SNAPSHOT_BASE);
            let revision = fetch_last_change(base, platform.as_snapshot_str()).await?;
            Ok(vec![Build {
                spec: VersionSpec::Revision(revision),
                label: format!(
                    "r{revision}  (newest snapshot for {}; older builds need an explicit revision)",
                    platform.as_snapshot_str()
                ),
            }])
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn lists_chrome_for_testing_versions_newest_first() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/manifest.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"versions":[
                    {"version":"119.0.6045.105","revision":"1230","downloads":{"chrome":[
                        {"platform":"linux64","url":"https://example.com/119.zip"}]}},
                    {"version":"120.0.6099.234","revision":"1234","downloads":{"chrome":[
                        {"platform":"linux64","url":"https://example.com/120.zip"}]}}
                ]}"#,
            ))
            .mount(&server)
            .await;

        let url = format!("{}/manifest.json", server.uri());
        let builds = list_builds_from(
            Distribution::ChromeForTesting,
            Platform::LinuxX64,
            Some(&url),
        )
        .await
        .unwrap();

        assert_eq!(builds.len(), 2);
        assert_eq!(builds[0].label, "120.0.6099.234");
    }

    /// The listing must go to the repo for the *requested* platform, which is
    /// what makes per-platform availability visible instead of assumed.
    #[tokio::test]
    async fn lists_ungoogled_releases_from_the_platforms_own_repo() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/repos/ungoogled-software/ungoogled-chromium-macos/releases",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"[{"tag_name":"150.0.7871.46-1.1","draft":false,"prerelease":false,"assets":[
                    {"name":"ungoogled-chromium_150.0.7871.46-1.1_arm64-macos.dmg",
                     "browser_download_url":"https://example.com/150.dmg"}]}]"#,
            ))
            .mount(&server)
            .await;

        let builds = list_builds_from(
            Distribution::UngoogledChromium,
            Platform::MacArm64,
            Some(&server.uri()),
        )
        .await
        .unwrap();

        assert_eq!(builds.len(), 1);
        assert_eq!(
            builds[0].spec,
            VersionSpec::Explicit("150.0.7871.46".into())
        );
    }

    /// Snapshots can only offer the tip, and the label has to say so rather
    /// than imply the list is complete.
    #[tokio::test]
    async fn snapshot_listing_offers_only_the_tip_and_says_so() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Mac_Arm/LAST_CHANGE"))
            .respond_with(ResponseTemplate::new(200).set_body_string("1674890"))
            .mount(&server)
            .await;

        let builds = list_builds_from(
            Distribution::ChromiumSnapshot,
            Platform::MacArm64,
            Some(&server.uri()),
        )
        .await
        .unwrap();

        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].spec, VersionSpec::Revision(1674890));
        assert!(builds[0].label.contains("r1674890"), "{}", builds[0].label);
        assert!(
            builds[0].label.contains("explicit revision"),
            "{}",
            builds[0].label
        );
    }

    #[tokio::test]
    async fn last_change_that_is_not_a_number_is_an_error_not_a_panic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Linux_x64/LAST_CHANGE"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>nope</html>"))
            .mount(&server)
            .await;

        let err = list_builds_from(
            Distribution::ChromiumSnapshot,
            Platform::LinuxX64,
            Some(&server.uri()),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FetcherError::VersionNotFound(_)), "{err:?}");
    }
}
