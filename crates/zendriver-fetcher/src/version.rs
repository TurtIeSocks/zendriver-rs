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

/// How to resolve a Chrome for Testing version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VersionSpec {
    /// Last entry in the manifest (effectively the newest known good version).
    Latest,
    /// Alias for the stable channel; for now identical to [`VersionSpec::Latest`].
    Stable,
    /// Pick a specific release channel.
    Channel(Channel),
    /// Exact version string, e.g. `"126.0.6478.182"`.
    Explicit(String),
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
