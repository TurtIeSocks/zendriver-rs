//! Target platforms, and how each distribution spells them.
//!
//! One [`Platform`] value, three naming schemes. Chrome for Testing uses
//! lowercase hyphenated keys (`mac-arm64`); Chromium's snapshot bucket uses
//! capitalised directory names (`Mac_Arm`); ungoogled-chromium encodes the
//! platform in an asset *filename suffix* (`_arm64-macos.dmg`) inside a
//! per-OS repo. Keeping all three spellings beside the enum makes that
//! divergence visible instead of scattering it through the resolvers.

/// Supported host platforms.
///
/// Variants correspond 1:1 with the platform keys in the
/// [Chrome for Testing manifest](https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    /// Linux on x86_64.
    LinuxX64,
    /// macOS on Intel.
    MacX64,
    /// macOS on Apple Silicon (M1/M2/...).
    MacArm64,
    /// 32-bit Windows.
    Win32,
    /// 64-bit Windows.
    Win64,
}

impl Platform {
    /// Every supported platform. Used by the CLI's `--platform` help text and
    /// by tests that assert exhaustive coverage.
    pub const ALL: &'static [Platform] = &[
        Platform::LinuxX64,
        Platform::MacX64,
        Platform::MacArm64,
        Platform::Win32,
        Platform::Win64,
    ];

    /// Parse a Chrome for Testing platform key (`"mac-arm64"`, `"linux64"`, …).
    ///
    /// The CfT spelling is the user-facing one — it is the shortest and the
    /// one that appears in Google's own docs. The snapshot bucket's
    /// `Mac_Arm`-style names are an implementation detail of that index and
    /// are translated internally by [`Platform::as_snapshot_str`].
    ///
    /// ```
    /// use zendriver_fetcher::Platform;
    /// assert_eq!(Platform::parse("mac-arm64"), Some(Platform::MacArm64));
    /// assert_eq!(Platform::parse("solaris"), None);
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Platform::ALL.iter().copied().find(|p| p.as_cft_str() == s)
    }

    /// Detect the current host platform, if supported.
    ///
    /// Returns `None` for platforms not covered by Chrome for Testing
    /// (e.g. Linux on aarch64, BSDs).
    ///
    /// ```
    /// use zendriver_fetcher::Platform;
    /// // On any supported CI host, this resolves.
    /// assert!(Platform::auto_detect().is_some());
    /// ```
    pub fn auto_detect() -> Option<Self> {
        if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
            Some(Platform::LinuxX64)
        } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
            Some(Platform::MacX64)
        } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
            Some(Platform::MacArm64)
        } else if cfg!(target_os = "windows") && cfg!(target_pointer_width = "32") {
            Some(Platform::Win32)
        } else if cfg!(target_os = "windows") && cfg!(target_pointer_width = "64") {
            Some(Platform::Win64)
        } else {
            None
        }
    }

    /// Platform key used in the Chrome for Testing manifest JSON
    /// (e.g. `"linux64"`, `"mac-arm64"`, `"win64"`).
    ///
    /// ```
    /// use zendriver_fetcher::Platform;
    /// assert_eq!(Platform::MacArm64.as_cft_str(), "mac-arm64");
    /// ```
    pub fn as_cft_str(&self) -> &'static str {
        match self {
            Platform::LinuxX64 => "linux64",
            Platform::MacX64 => "mac-x64",
            Platform::MacArm64 => "mac-arm64",
            Platform::Win32 => "win32",
            Platform::Win64 => "win64",
        }
    }

    /// Top-level directory inside a CfT zip for this platform — e.g.
    /// `"chrome-linux64"`, `"chrome-mac-arm64"`. Every entry in a
    /// well-formed CfT archive lives under this directory; the extractor
    /// rejects archives that violate the layout as a tamper guard.
    pub(crate) fn cft_top_dir(&self) -> &'static str {
        match self {
            Platform::LinuxX64 => "chrome-linux64",
            Platform::MacX64 => "chrome-mac-x64",
            Platform::MacArm64 => "chrome-mac-arm64",
            Platform::Win32 => "chrome-win32",
            Platform::Win64 => "chrome-win64",
        }
    }

    /// Directory name for this platform in Chromium's snapshot bucket
    /// (e.g. `"Mac_Arm"`, `"Win_x64"`, `"Linux_x64"`).
    ///
    /// Deliberately *not* the Chrome for Testing spelling — the two indexes
    /// disagree on every single platform, so mixing them up produces 404s
    /// rather than type errors.
    ///
    /// ```
    /// use zendriver_fetcher::Platform;
    /// assert_eq!(Platform::MacArm64.as_cft_str(), "mac-arm64");
    /// assert_eq!(Platform::MacArm64.as_snapshot_str(), "Mac_Arm");
    /// ```
    pub fn as_snapshot_str(&self) -> &'static str {
        match self {
            Platform::LinuxX64 => "Linux_x64",
            Platform::MacX64 => "Mac",
            Platform::MacArm64 => "Mac_Arm",
            Platform::Win32 => "Win",
            Platform::Win64 => "Win_x64",
        }
    }

    /// Archive stem inside a snapshot revision directory: the zip is
    /// `<stem>.zip` and every entry inside it lives under `<stem>/`.
    ///
    /// Snapshots name the archive after the *OS*, not the arch — both
    /// `Mac` and `Mac_Arm` publish `chrome-mac.zip`.
    pub(crate) fn snapshot_archive_stem(&self) -> &'static str {
        match self {
            Platform::LinuxX64 => "chrome-linux",
            Platform::MacX64 | Platform::MacArm64 => "chrome-mac",
            Platform::Win32 | Platform::Win64 => "chrome-win",
        }
    }

    /// Filename suffix identifying this platform's ungoogled-chromium release
    /// asset, e.g. `"_arm64-macos.dmg"`.
    ///
    /// Each per-OS repo attaches several assets per release (installers,
    /// `.zsync` sidecars, multiple arches); matching on the full suffix is
    /// what picks exactly one. Note in particular that `.AppImage.zsync`
    /// does not end in `.AppImage`, so suffix matching excludes it for free.
    pub(crate) fn ungoogled_asset_suffix(&self) -> &'static str {
        match self {
            // Portable-Linux publishes both an AppImage and a `.tar.xz`. The
            // AppImage is a single self-contained executable, so it needs no
            // decompressor dependency at all — see `Archive::Executable`.
            Platform::LinuxX64 => "-x86_64.AppImage",
            Platform::MacX64 => "_x86_64-macos.dmg",
            Platform::MacArm64 => "_arm64-macos.dmg",
            Platform::Win32 => "_windows_x86.zip",
            Platform::Win64 => "_windows_x64.zip",
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn auto_detect_returns_some_on_host() {
        // Any supported CI host should resolve to a known Platform.
        assert!(Platform::auto_detect().is_some());
    }

    #[test]
    fn cft_str_round_trip() {
        assert_eq!(Platform::LinuxX64.as_cft_str(), "linux64");
        assert_eq!(Platform::MacX64.as_cft_str(), "mac-x64");
        assert_eq!(Platform::MacArm64.as_cft_str(), "mac-arm64");
        assert_eq!(Platform::Win32.as_cft_str(), "win32");
        assert_eq!(Platform::Win64.as_cft_str(), "win64");
    }

    const ALL: &[Platform] = Platform::ALL;

    #[test]
    fn parse_round_trips_every_platform() {
        for &p in ALL {
            assert_eq!(Platform::parse(p.as_cft_str()), Some(p));
        }
        assert_eq!(Platform::parse("  MAC-ARM64 "), Some(Platform::MacArm64));
        assert_eq!(Platform::parse("Mac_Arm"), None);
        assert_eq!(Platform::parse(""), None);
    }

    #[test]
    fn snapshot_str_matches_the_bucket_directory_names() {
        assert_eq!(Platform::LinuxX64.as_snapshot_str(), "Linux_x64");
        assert_eq!(Platform::MacX64.as_snapshot_str(), "Mac");
        assert_eq!(Platform::MacArm64.as_snapshot_str(), "Mac_Arm");
        assert_eq!(Platform::Win32.as_snapshot_str(), "Win");
        assert_eq!(Platform::Win64.as_snapshot_str(), "Win_x64");
    }

    /// The two indexes spell every platform differently. Pinning that here
    /// means a future edit that "unifies" the two names fails loudly instead
    /// of producing 404s at download time.
    #[test]
    fn cft_and_snapshot_names_differ_for_every_platform() {
        for &p in ALL {
            assert_ne!(
                p.as_cft_str(),
                p.as_snapshot_str(),
                "{p:?}: CfT and snapshot names must not be conflated"
            );
        }
    }

    #[test]
    fn snapshot_archive_stem_is_per_os_not_per_arch() {
        // Both macOS arches publish the same `chrome-mac.zip` name, under
        // different bucket directories.
        assert_eq!(
            Platform::MacArm64.snapshot_archive_stem(),
            Platform::MacX64.snapshot_archive_stem()
        );
        assert_eq!(Platform::MacArm64.snapshot_archive_stem(), "chrome-mac");
        assert_eq!(Platform::Win64.snapshot_archive_stem(), "chrome-win");
        assert_eq!(Platform::LinuxX64.snapshot_archive_stem(), "chrome-linux");
    }

    #[test]
    fn ungoogled_asset_suffixes_are_unique_per_platform() {
        for (i, &a) in ALL.iter().enumerate() {
            for &b in &ALL[i + 1..] {
                assert_ne!(
                    a.ungoogled_asset_suffix(),
                    b.ungoogled_asset_suffix(),
                    "{a:?} and {b:?} share an asset suffix"
                );
            }
        }
    }

    /// The `.zsync` sidecars sit next to the AppImage in every portablelinux
    /// release; suffix matching must not pick one up.
    #[test]
    fn linux_asset_suffix_excludes_the_zsync_sidecar() {
        let suffix = Platform::LinuxX64.ungoogled_asset_suffix();
        assert!("ungoogled-chromium-151.0.7922.71-1-x86_64.AppImage".ends_with(suffix));
        assert!(!"ungoogled-chromium-151.0.7922.71-1-x86_64.AppImage.zsync".ends_with(suffix));
    }
}
