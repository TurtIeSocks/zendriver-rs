//! Cache directory layout + path resolution.
//!
//! The on-disk layout matches the Chrome for Testing zip layout:
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
//! Per-version dirs are written atomically: download + extract land under
//! a `<version>.tmp/` sibling, then a single rename promotes it.

use std::env;
use std::path::{Path, PathBuf};

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

/// Path to the Chrome binary inside the extracted, cached version dir.
///
/// Matches the Chrome for Testing zip layout — see module docs.
pub(crate) fn binary_path(cache_dir: &Path, version: &str, platform: Platform) -> PathBuf {
    let version_dir = cache_dir.join(version);
    match platform {
        Platform::LinuxX64 => version_dir.join("chrome-linux64").join("chrome"),
        Platform::MacX64 => version_dir
            .join("chrome-mac-x64")
            .join("Google Chrome for Testing.app")
            .join("Contents")
            .join("MacOS")
            .join("Google Chrome for Testing"),
        Platform::MacArm64 => version_dir
            .join("chrome-mac-arm64")
            .join("Google Chrome for Testing.app")
            .join("Contents")
            .join("MacOS")
            .join("Google Chrome for Testing"),
        Platform::Win32 => version_dir.join("chrome-win32").join("chrome.exe"),
        Platform::Win64 => version_dir.join("chrome-win64").join("chrome.exe"),
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
    fn binary_path_linux_layout() {
        let root = Path::new("/tmp/cache");
        let p = binary_path(root, "120.0.6099.234", Platform::LinuxX64);
        assert_eq!(
            p,
            Path::new("/tmp/cache/120.0.6099.234/chrome-linux64/chrome")
        );
    }

    #[test]
    fn binary_path_mac_arm64_layout() {
        let root = Path::new("/tmp/cache");
        let p = binary_path(root, "120.0.6099.234", Platform::MacArm64);
        assert_eq!(
            p,
            Path::new(
                "/tmp/cache/120.0.6099.234/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
            )
        );
    }

    #[test]
    fn binary_path_win64_layout() {
        let root = Path::new("/tmp/cache");
        let p = binary_path(root, "120.0.6099.234", Platform::Win64);
        assert_eq!(
            p,
            Path::new("/tmp/cache/120.0.6099.234/chrome-win64/chrome.exe")
        );
    }
}
