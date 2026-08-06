//! Cache directory layout + path resolution.
//!
//! Chrome for Testing keeps the layout it has always had, straight under the
//! cache root:
//!
//! ```text
//! <cache_dir>/
//!   <version>/
//!     chrome-<platform_cft>/
//!       chrome                                            (Linux)
//!       chrome.exe                                        (Windows)
//!       Google Chrome for Testing.app/Contents/MacOS/...  (macOS)
//! ```
//!
//! The distributions added later namespace themselves under their
//! [`Distribution::slug`]:
//!
//! ```text
//! <cache_dir>/
//!   ungoogled/151.0.7922.71/...
//!   snapshot/r1674890/...
//! ```
//!
//! The asymmetry is deliberate and load-bearing in both directions. Chrome
//! for Testing stays un-prefixed so caches populated by earlier versions of
//! this crate keep hitting. The others are prefixed because a Chrome version
//! number is *not* unique across distributions — CfT 151.0.7922.71 and
//! ungoogled 151.0.7922.71 are different binaries, and sharing a directory
//! would serve whichever landed first.
//!
//! Per-build dirs are written atomically: download + unpack land under a
//! `<build>.tmp/` sibling, then a single rename promotes it.

use std::env;
use std::path::{Path, PathBuf};

use crate::distribution::Distribution;
use crate::platform::Platform;

/// Default cache root: OS cache dir if available, otherwise the temp dir.
///
/// Always suffixed with `zendriver/chrome` so multiple consumers don't
/// collide at the top level.
pub(crate) fn default_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(env::temp_dir)
        .join("zendriver/chrome")
}

/// Directory holding one unpacked build.
///
/// See the module docs for why Chrome for Testing is the un-prefixed case.
pub(crate) fn build_dir(cache_dir: &Path, distribution: Distribution, build_id: &str) -> PathBuf {
    match distribution {
        Distribution::ChromeForTesting => cache_dir.join(build_id),
        other => cache_dir.join(other.slug()).join(build_id),
    }
}

/// Path to the Chrome for Testing binary *relative* to its build directory.
///
/// Used against the published `<cache>/<version>/` layout and against the
/// in-progress `<cache>/<version>.tmp/` staging dir, so the binary can be
/// made executable *before* it is atomically renamed into place.
pub(crate) fn cft_binary_subpath(platform: Platform) -> PathBuf {
    match platform {
        Platform::LinuxX64 => PathBuf::from("chrome-linux64").join("chrome"),
        Platform::MacX64 => PathBuf::from("chrome-mac-x64")
            .join("Google Chrome for Testing.app")
            .join("Contents")
            .join("MacOS")
            .join("Google Chrome for Testing"),
        Platform::MacArm64 => PathBuf::from("chrome-mac-arm64")
            .join("Google Chrome for Testing.app")
            .join("Contents")
            .join("MacOS")
            .join("Google Chrome for Testing"),
        Platform::Win32 => PathBuf::from("chrome-win32").join("chrome.exe"),
        Platform::Win64 => PathBuf::from("chrome-win64").join("chrome.exe"),
    }
}

/// Path to the binary inside an unpacked Chromium snapshot.
///
/// Snapshot archives are plain Chromium: the macOS bundle is `Chromium.app`
/// and the Linux/Windows binary keeps the `chrome` name, all under the
/// archive's own per-OS top directory (`chrome-mac`, not `chrome-mac-arm64`).
pub(crate) fn snapshot_binary_subpath(platform: Platform) -> PathBuf {
    let top = PathBuf::from(platform.snapshot_archive_stem());
    match platform {
        Platform::LinuxX64 => top.join("chrome"),
        Platform::MacX64 | Platform::MacArm64 => top
            .join("Chromium.app")
            .join("Contents")
            .join("MacOS")
            .join("Chromium"),
        Platform::Win32 | Platform::Win64 => top.join("chrome.exe"),
    }
}

/// Filename an ungoogled AppImage is stored under inside its build dir.
///
/// An AppImage is not unpacked, so this is both the stored name and the
/// executable path.
pub(crate) const UNGOOGLED_APPIMAGE_NAME: &str = "chrome.AppImage";

/// Bundle directory inside ungoogled-chromium's macOS disk image.
pub(crate) const UNGOOGLED_APP_BUNDLE: &str = "Chromium.app";

/// Path to the binary inside an unpacked ungoogled-chromium build.
///
/// `archive_top` is the zip's top-level directory on Windows — ungoogled
/// names it after the asset (`ungoogled-chromium_151.0.7922.71-1.1_windows_x64/`),
/// so unlike every other case here it is not a constant and has to be
/// threaded through from the resolved asset name.
pub(crate) fn ungoogled_binary_subpath(platform: Platform, archive_top: &str) -> PathBuf {
    match platform {
        Platform::LinuxX64 => PathBuf::from(UNGOOGLED_APPIMAGE_NAME),
        Platform::MacX64 | Platform::MacArm64 => PathBuf::from(UNGOOGLED_APP_BUNDLE)
            .join("Contents")
            .join("MacOS")
            .join("Chromium"),
        Platform::Win32 | Platform::Win64 => PathBuf::from(archive_top).join("chrome.exe"),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn default_cache_dir_ends_with_zendriver_chrome() {
        let p = default_cache_dir();
        assert!(
            p.ends_with("zendriver/chrome"),
            "expected suffix zendriver/chrome, got {}",
            p.display()
        );
    }

    #[test]
    fn cft_binary_path_linux_layout() {
        let root = Path::new("/tmp/cache");
        let p = build_dir(root, Distribution::ChromeForTesting, "120.0.6099.234")
            .join(cft_binary_subpath(Platform::LinuxX64));
        assert_eq!(
            p,
            Path::new("/tmp/cache/120.0.6099.234/chrome-linux64/chrome")
        );
    }

    #[test]
    fn cft_binary_path_mac_arm64_layout() {
        let root = Path::new("/tmp/cache");
        let p = build_dir(root, Distribution::ChromeForTesting, "120.0.6099.234")
            .join(cft_binary_subpath(Platform::MacArm64));
        assert_eq!(
            p,
            Path::new(
                "/tmp/cache/120.0.6099.234/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
            )
        );
    }

    #[test]
    fn cft_binary_path_win64_layout() {
        let root = Path::new("/tmp/cache");
        let p = build_dir(root, Distribution::ChromeForTesting, "120.0.6099.234")
            .join(cft_binary_subpath(Platform::Win64));
        assert_eq!(
            p,
            Path::new("/tmp/cache/120.0.6099.234/chrome-win64/chrome.exe")
        );
    }

    /// Chrome for Testing must keep the un-prefixed layout, or every cache
    /// populated before the distribution axis existed silently misses.
    #[test]
    fn chrome_for_testing_build_dir_is_unprefixed() {
        let root = Path::new("/tmp/cache");
        assert_eq!(
            build_dir(root, Distribution::ChromeForTesting, "120.0.6099.234"),
            Path::new("/tmp/cache/120.0.6099.234")
        );
    }

    /// ...and the others must not be, or two distributions sharing a Chrome
    /// version would share a directory.
    #[test]
    fn other_distributions_namespace_by_slug() {
        let root = Path::new("/tmp/cache");
        assert_eq!(
            build_dir(root, Distribution::UngoogledChromium, "151.0.7922.71"),
            Path::new("/tmp/cache/ungoogled/151.0.7922.71")
        );
        assert_eq!(
            build_dir(root, Distribution::ChromiumSnapshot, "r1674890"),
            Path::new("/tmp/cache/snapshot/r1674890")
        );
        assert_ne!(
            build_dir(root, Distribution::ChromeForTesting, "151.0.7922.71"),
            build_dir(root, Distribution::UngoogledChromium, "151.0.7922.71"),
        );
    }

    #[test]
    fn snapshot_binary_subpaths_use_the_snapshot_top_dir_not_the_cft_one() {
        assert_eq!(
            snapshot_binary_subpath(Platform::LinuxX64),
            Path::new("chrome-linux/chrome")
        );
        assert_eq!(
            snapshot_binary_subpath(Platform::Win64),
            Path::new("chrome-win/chrome.exe")
        );
        assert_eq!(
            snapshot_binary_subpath(Platform::MacArm64),
            Path::new("chrome-mac/Chromium.app/Contents/MacOS/Chromium")
        );
        // Snapshots are Chromium, not Chrome for Testing — different tree.
        assert_ne!(
            snapshot_binary_subpath(Platform::LinuxX64),
            cft_binary_subpath(Platform::LinuxX64)
        );
    }

    #[test]
    fn ungoogled_binary_subpaths_cover_the_three_packagings() {
        let win_top = "ungoogled-chromium_151.0.7922.71-1.1_windows_x64";
        assert_eq!(
            ungoogled_binary_subpath(Platform::Win64, win_top),
            Path::new(win_top).join("chrome.exe")
        );
        assert_eq!(
            ungoogled_binary_subpath(Platform::LinuxX64, ""),
            Path::new("chrome.AppImage")
        );
        assert_eq!(
            ungoogled_binary_subpath(Platform::MacArm64, ""),
            Path::new("Chromium.app/Contents/MacOS/Chromium")
        );
    }
}
