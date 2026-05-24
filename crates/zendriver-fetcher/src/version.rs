//! Version selectors for Chrome for Testing.

/// Release channel.
///
/// Currently only `Stable` is wired end-to-end; `Beta`/`Dev`/`Canary`
/// require a separate CFT endpoint and return
/// [`FetcherError::UnsupportedPlatform`](crate::FetcherError::UnsupportedPlatform)
/// at resolve time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Stable,
    Beta,
    Dev,
    Canary,
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
