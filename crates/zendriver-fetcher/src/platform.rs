//! Target platforms understood by the Chrome for Testing manifest.

/// Supported host platforms.
///
/// Variants correspond 1:1 with the platform keys in the
/// [Chrome for Testing manifest](https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    LinuxX64,
    MacX64,
    MacArm64,
    Win32,
    Win64,
}

impl Platform {
    /// Detect the current host platform, if supported.
    ///
    /// Returns `None` for platforms not covered by Chrome for Testing
    /// (e.g. Linux on aarch64, BSDs).
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

    /// Platform key used in the Chrome for Testing manifest JSON.
    pub fn as_cft_str(&self) -> &'static str {
        match self {
            Platform::LinuxX64 => "linux64",
            Platform::MacX64 => "mac-x64",
            Platform::MacArm64 => "mac-arm64",
            Platform::Win32 => "win32",
            Platform::Win64 => "win64",
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
}
