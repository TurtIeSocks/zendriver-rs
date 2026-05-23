//! Profile types: ProfileKind enum, Platform enum, PerFieldOverride struct,
//! plus the StealthProfile builder (filled in Task 7).

use std::path::PathBuf;

/// Stealth modes shipped by the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    /// No stealth applied. Browser is launched stock; no JS patches, no UA scrub.
    Off,
    /// Launch flags + UA scrub (HeadlessChrome → Chrome). No JS bootstrap.
    /// Safe against `Function.prototype.toString` detection. Default.
    Native,
    /// Native + Navigator-prototype JS patches. Passes sannysoft. Detectable
    /// by sophisticated bots that probe `toString` on Navigator getters.
    Spoofed,
}

/// JS `navigator.platform` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Platform {
    Win32,
    MacIntel,
    LinuxX86_64,
}

impl Platform {
    /// Map to the `navigator.platform` string Chrome reports for that OS.
    #[must_use]
    pub fn js_string(self) -> &'static str {
        match self {
            Platform::Win32 => "Win32",
            Platform::MacIntel => "MacIntel",
            Platform::LinuxX86_64 => "Linux x86_64",
        }
    }

    /// CDP `userAgentMetadata.platform` value (no version).
    #[must_use]
    pub fn ch_platform(self) -> &'static str {
        match self {
            Platform::Win32 => "Windows",
            Platform::MacIntel => "macOS",
            Platform::LinuxX86_64 => "Linux",
        }
    }

    /// UA-string OS token (the bit inside parentheses).
    #[must_use]
    pub fn ua_token(self) -> &'static str {
        match self {
            Platform::Win32 => "Windows NT 10.0; Win64; x64",
            Platform::MacIntel => "Macintosh; Intel Mac OS X 10_15_7",
            Platform::LinuxX86_64 => "X11; Linux x86_64",
        }
    }
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub(crate) struct PerFieldOverride {
    pub memory_gb: Option<u32>,
    pub cpu_count: Option<u32>,
    pub chrome_major: Option<u32>,
    pub platform: Option<Platform>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub ua_string: Option<String>,
}

/// Placeholder; full StealthProfile lands in Task 7.
#[allow(dead_code)]
pub struct StealthProfile {
    pub(crate) kind: ProfileKind,
    pub(crate) extra_flags: Vec<String>,
    pub(crate) per_field: PerFieldOverride,
    pub(crate) bypass_csp: bool,
    pub(crate) user_data_dir: Option<PathBuf>,
    // fingerprint_override: Option<Fingerprint>,  // added in Task 7
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn platform_js_string_matches_chrome_output() {
        assert_eq!(Platform::Win32.js_string(), "Win32");
        assert_eq!(Platform::MacIntel.js_string(), "MacIntel");
        assert_eq!(Platform::LinuxX86_64.js_string(), "Linux x86_64");
    }

    #[test]
    fn platform_ch_platform_uses_no_version() {
        assert_eq!(Platform::MacIntel.ch_platform(), "macOS");
    }

    #[test]
    fn platform_ua_token_includes_arch() {
        assert!(Platform::Win32.ua_token().contains("Win64; x64"));
    }
}
