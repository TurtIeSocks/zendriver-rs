//! Fingerprint: composed UA + Sec-CH-UA metadata + system facts.

use serde::Serialize;

use crate::Platform;

#[derive(Debug, Clone, Serialize)]
pub struct Brand {
    pub brand: String,
    pub version: String,
}

/// Sent to CDP as `Emulation.setUserAgentOverride.userAgentMetadata`.
/// Mirrors the [W3C UA-CH spec](https://wicg.github.io/ua-client-hints/).
#[derive(Debug, Clone, Serialize)]
pub struct UserAgentMetadata {
    pub brands: Vec<Brand>,
    #[serde(rename = "fullVersionList")]
    pub full_version_list: Vec<Brand>,
    pub platform: String,
    #[serde(rename = "platformVersion")]
    pub platform_version: String,
    pub architecture: String,
    pub bitness: String,
    pub wow64: bool,
    pub mobile: bool,
    pub model: String,
}

impl UserAgentMetadata {
    /// Build a realistic UAM for the given platform + Chrome major version.
    /// Uses the Chrome convention of three brands: "Not_A Brand;v=8",
    /// "Chromium;v=N", "Google Chrome;v=N".
    pub fn realistic(platform: Platform, chrome_major: u32, chrome_full: &str) -> Self {
        let brands = vec![
            Brand { brand: "Not_A Brand".into(),  version: "8".into() },
            Brand { brand: "Chromium".into(),     version: chrome_major.to_string() },
            Brand { brand: "Google Chrome".into(), version: chrome_major.to_string() },
        ];
        let full_version_list = vec![
            Brand { brand: "Not_A Brand".into(),  version: "8.0.0.0".into() },
            Brand { brand: "Chromium".into(),     version: chrome_full.to_string() },
            Brand { brand: "Google Chrome".into(), version: chrome_full.to_string() },
        ];
        let (platform_version, architecture, bitness) = match platform {
            Platform::Win32       => ("15.0.0", "x86", "64"),
            Platform::MacIntel    => ("10.15.7", "x86", "64"),
            Platform::LinuxX86_64 => ("5.15.0", "x86", "64"),
        };
        Self {
            brands,
            full_version_list,
            platform: platform.ch_platform().to_string(),
            platform_version: platform_version.to_string(),
            architecture: architecture.to_string(),
            bitness: bitness.to_string(),
            wow64: false,
            mobile: false,
            model: String::new(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn realistic_uam_macintel_chrome_120_matches_snapshot() {
        let uam = UserAgentMetadata::realistic(Platform::MacIntel, 120, "120.0.6099.234");
        insta::assert_json_snapshot!("uam_macintel_chrome_120", uam);
    }

    #[test]
    fn realistic_uam_win32_chrome_120_matches_snapshot() {
        let uam = UserAgentMetadata::realistic(Platform::Win32, 120, "120.0.6099.234");
        insta::assert_json_snapshot!("uam_win32_chrome_120", uam);
    }

    #[test]
    fn realistic_uam_linux_chrome_120_matches_snapshot() {
        let uam = UserAgentMetadata::realistic(Platform::LinuxX86_64, 120, "120.0.6099.234");
        insta::assert_json_snapshot!("uam_linux_chrome_120", uam);
    }
}
