//! Version selectors for Chrome for Testing.

/// Release channel.
///
/// `Stable` resolves through the flat `known-good-versions-with-downloads.json`
/// manifest (same as [`VersionSpec::Latest`]); `Beta`/`Dev`/`Canary` resolve
/// through Chrome for Testing's per-channel
/// `last-known-good-versions-with-downloads.json` endpoint instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Chrome's stable channel.
    Stable,
    /// Beta channel.
    Beta,
    /// Dev channel.
    Dev,
    /// Canary channel.
    Canary,
}

impl Channel {
    /// Channel name as used by Chrome for Testing's per-channel manifest
    /// (`last-known-good-versions-with-downloads.json`'s `channels` map
    /// keys: `"Stable"`, `"Beta"`, `"Dev"`, `"Canary"`).
    pub(crate) fn as_cft_str(self) -> &'static str {
        match self {
            Channel::Stable => "Stable",
            Channel::Beta => "Beta",
            Channel::Dev => "Dev",
            Channel::Canary => "Canary",
        }
    }
}

/// Which build to resolve.
///
/// Not every selector is meaningful for every
/// [`Distribution`](crate::Distribution): a snapshot bucket has no channels
/// and no version index, and a version-keyed distribution has no revisions.
/// The combinations that cannot be honoured return
/// [`FetcherError::UnsupportedSelector`](crate::FetcherError::UnsupportedSelector)
/// naming the alternative, rather than quietly resolving something else.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VersionSpec {
    /// Newest build the distribution publishes for the target platform.
    Latest,
    /// Alias for the stable channel; for now identical to [`VersionSpec::Latest`].
    Stable,
    /// Pick a specific release channel. Chrome for Testing only — neither
    /// ungoogled-chromium nor the snapshot bucket publishes channels.
    Channel(Channel),
    /// Exact version string, e.g. `"126.0.6478.182"`.
    ///
    /// Matched exactly against Chrome for Testing's manifest, and as a **tag
    /// prefix** for ungoogled-chromium, whose tags append a packaging suffix
    /// (`151.0.7922.71-1.1`). Refused for
    /// [`Distribution::ChromiumSnapshot`](crate::Distribution::ChromiumSnapshot),
    /// which has no version index — use [`VersionSpec::Revision`] there.
    Explicit(String),
    /// Exact Chromium revision (commit position), e.g. `1674890`.
    ///
    /// The only way to pin a specific
    /// [`Distribution::ChromiumSnapshot`](crate::Distribution::ChromiumSnapshot)
    /// build, since that bucket is keyed by revision. Refused for the
    /// version-keyed distributions.
    Revision(u64),
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn channel_variants_construct() {
        let _ = Channel::Stable;
        let _ = Channel::Beta;
        let _ = Channel::Dev;
        let _ = Channel::Canary;
    }

    #[test]
    fn channel_as_cft_str_matches_manifest_keys() {
        assert_eq!(Channel::Stable.as_cft_str(), "Stable");
        assert_eq!(Channel::Beta.as_cft_str(), "Beta");
        assert_eq!(Channel::Dev.as_cft_str(), "Dev");
        assert_eq!(Channel::Canary.as_cft_str(), "Canary");
    }

    #[test]
    fn version_spec_variants_construct() {
        let _ = VersionSpec::Latest;
        let _ = VersionSpec::Stable;
        let _ = VersionSpec::Channel(Channel::Stable);
        let _ = VersionSpec::Explicit("126.0.6478.182".into());
    }

    #[test]
    fn version_spec_is_clone() {
        let v = VersionSpec::Explicit("126.0.6478.182".into());
        let cloned = v.clone();
        assert_eq!(v, cloned);
    }
}
