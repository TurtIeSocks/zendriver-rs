//! Which *build* of Chromium to fetch.
//!
//! The crate started life as a Chrome for Testing downloader, and Chrome for
//! Testing remains the default — [`Distribution::default()`] is
//! [`Distribution::ChromeForTesting`], so a caller that never mentions a
//! distribution gets exactly the behaviour it always had.
//!
//! The three distributions differ **only** in how a
//! [`VersionSpec`](crate::VersionSpec) + [`Platform`] pair turns into a
//! download URL. Everything downstream — streaming, SHA256 verification,
//! extraction, the atomic per-build cache — is shared. Resolution is the
//! whole distribution axis:
//!
//! | Distribution | Index | Keyed by |
//! |---|---|---|
//! | [`ChromeForTesting`](Distribution::ChromeForTesting) | one JSON manifest for every platform | version |
//! | [`UngoogledChromium`](Distribution::UngoogledChromium) | three GitHub repos, one per OS | version (tag *prefix*) |
//! | [`ChromiumSnapshot`](Distribution::ChromiumSnapshot) | a GCS bucket per platform | **revision** |
//!
//! That last row is the awkward one and the reason
//! [`VersionSpec::Revision`](crate::VersionSpec::Revision) exists — see
//! [`Distribution::ChromiumSnapshot`].

use crate::platform::Platform;

/// Which Chromium build to download.
///
/// Defaults to [`Distribution::ChromeForTesting`].
///
/// ```
/// use zendriver_fetcher::Distribution;
/// assert_eq!(Distribution::default(), Distribution::ChromeForTesting);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Distribution {
    /// Google's [Chrome for Testing][cft] builds: one JSON manifest listing
    /// every version on every platform, with a stable URL scheme and no
    /// auto-update. The default, and the only distribution that is really
    /// *Chrome* rather than Chromium.
    ///
    /// [cft]: https://googlechromelabs.github.io/chrome-for-testing/
    #[default]
    ChromeForTesting,

    /// [ungoogled-chromium][ug]: Chromium with Google integration stripped out.
    ///
    /// There is no single manifest. Binaries are built by **three separate
    /// per-OS repos** under the `ungoogled-software` org, each cutting its own
    /// releases on its own cadence, so version availability genuinely differs
    /// per platform — as of 2026-08-06 Windows and Linux were on
    /// `151.0.7922.71` while macOS was still on `150.0.7871.46`. Resolution
    /// reports what the *requested* platform actually has; it never assumes
    /// parity.
    ///
    /// The org's main `ungoogled-chromium` repo also tags releases, but they
    /// carry **zero assets** — it is the source/patches repo. It is
    /// deliberately not used as a binary source.
    ///
    /// [ug]: https://github.com/ungoogled-software/ungoogled-chromium
    UngoogledChromium,

    /// Per-commit [Chromium continuous-build snapshots][snap] from Google's
    /// GCS bucket.
    ///
    /// **Snapshots are keyed by revision, not by version.** The bucket is laid
    /// out `<platform>/<revision>/chrome-<os>.zip` and publishes no
    /// version→revision index, so
    /// [`VersionSpec::Explicit`](crate::VersionSpec::Explicit) cannot be
    /// resolved here and is refused rather than silently downgraded to "the
    /// newest snapshot" — handing back a different browser than the one asked
    /// for is worse than an error. Pin a build with
    /// [`VersionSpec::Revision`](crate::VersionSpec::Revision) instead, or take
    /// the tip with [`VersionSpec::Latest`](crate::VersionSpec::Latest).
    ///
    /// [snap]: https://commondatastorage.googleapis.com/chromium-browser-snapshots/index.html
    ChromiumSnapshot,
}

impl Distribution {
    /// Every distribution, in menu order. Used by the CLI's interactive
    /// picker and by tests that assert exhaustive coverage.
    pub const ALL: &'static [Distribution] = &[
        Distribution::ChromeForTesting,
        Distribution::UngoogledChromium,
        Distribution::ChromiumSnapshot,
    ];

    /// Short lowercase slug — the value accepted by `--distribution` and used
    /// as the cache sub-directory name.
    ///
    /// ```
    /// use zendriver_fetcher::Distribution;
    /// assert_eq!(Distribution::UngoogledChromium.slug(), "ungoogled");
    /// ```
    pub fn slug(self) -> &'static str {
        match self {
            Distribution::ChromeForTesting => "cft",
            Distribution::UngoogledChromium => "ungoogled",
            Distribution::ChromiumSnapshot => "snapshot",
        }
    }

    /// Human-readable name, for CLI menus and error messages.
    pub fn title(self) -> &'static str {
        match self {
            Distribution::ChromeForTesting => "Chrome for Testing",
            Distribution::UngoogledChromium => "ungoogled-chromium",
            Distribution::ChromiumSnapshot => "Chromium snapshot",
        }
    }

    /// Parse a slug. Accepts a few obvious aliases so the CLI is forgiving.
    ///
    /// ```
    /// use zendriver_fetcher::Distribution;
    /// assert_eq!(
    ///     Distribution::parse("chrome-for-testing"),
    ///     Some(Distribution::ChromeForTesting)
    /// );
    /// assert_eq!(Distribution::parse("nope"), None);
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cft" | "chrome-for-testing" | "chrome_for_testing" | "chrome" => {
                Some(Distribution::ChromeForTesting)
            }
            "ungoogled" | "ungoogled-chromium" | "ungoogled_chromium" => {
                Some(Distribution::UngoogledChromium)
            }
            "snapshot" | "snapshots" | "chromium-snapshot" | "chromium_snapshot" => {
                Some(Distribution::ChromiumSnapshot)
            }
            _ => None,
        }
    }

    /// The `owner/name` GitHub repo publishing ungoogled-chromium binaries for
    /// `platform`.
    ///
    /// The three OSes are built by three different repos — that split is the
    /// entire reason ungoogled needs per-platform resolution rather than one
    /// manifest lookup.
    ///
    /// ```
    /// use zendriver_fetcher::{Distribution, Platform};
    /// assert_eq!(
    ///     Distribution::ungoogled_repo(Platform::MacArm64),
    ///     "ungoogled-software/ungoogled-chromium-macos"
    /// );
    /// ```
    pub fn ungoogled_repo(platform: Platform) -> &'static str {
        match platform {
            Platform::MacX64 | Platform::MacArm64 => "ungoogled-software/ungoogled-chromium-macos",
            Platform::Win32 | Platform::Win64 => "ungoogled-software/ungoogled-chromium-windows",
            // "portablelinux" ships relocatable AppImage/tarball builds; the
            // sibling `ungoogled-chromium-archlinux` / `-debian` repos publish
            // distro packages, which are not usable as a drop-in binary.
            Platform::LinuxX64 => "ungoogled-software/ungoogled-chromium-portablelinux",
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn default_is_chrome_for_testing_so_existing_callers_are_untouched() {
        assert_eq!(Distribution::default(), Distribution::ChromeForTesting);
    }

    #[test]
    fn slug_round_trips_through_parse_for_every_distribution() {
        for &d in Distribution::ALL {
            assert_eq!(Distribution::parse(d.slug()), Some(d), "slug {}", d.slug());
        }
    }

    #[test]
    fn parse_is_case_insensitive_and_rejects_unknown() {
        assert_eq!(
            Distribution::parse("  UnGoogled  "),
            Some(Distribution::UngoogledChromium)
        );
        assert_eq!(Distribution::parse("firefox"), None);
    }

    #[test]
    fn ungoogled_maps_the_three_oses_to_three_different_repos() {
        let mac = Distribution::ungoogled_repo(Platform::MacArm64);
        let win = Distribution::ungoogled_repo(Platform::Win64);
        let linux = Distribution::ungoogled_repo(Platform::LinuxX64);

        assert_eq!(mac, "ungoogled-software/ungoogled-chromium-macos");
        assert_eq!(win, "ungoogled-software/ungoogled-chromium-windows");
        assert_eq!(linux, "ungoogled-software/ungoogled-chromium-portablelinux");

        // The point of the mapping: three OSes, three distinct repos.
        assert_ne!(mac, win);
        assert_ne!(win, linux);
        assert_ne!(mac, linux);

        // Both arches of an OS share that OS's repo.
        assert_eq!(mac, Distribution::ungoogled_repo(Platform::MacX64));
        assert_eq!(win, Distribution::ungoogled_repo(Platform::Win32));
    }

    #[test]
    fn ungoogled_never_points_at_the_assetless_source_repo() {
        // `ungoogled-software/ungoogled-chromium` tags releases with zero
        // assets (it is the patch set, not a build). Resolving against it
        // would always fail to find a binary.
        for &p in &[
            Platform::MacX64,
            Platform::MacArm64,
            Platform::Win32,
            Platform::Win64,
            Platform::LinuxX64,
        ] {
            assert_ne!(
                Distribution::ungoogled_repo(p),
                "ungoogled-software/ungoogled-chromium",
                "{p:?} points at the source-only repo"
            );
        }
    }
}
