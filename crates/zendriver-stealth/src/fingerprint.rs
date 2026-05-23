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
            Brand {
                brand: "Not_A Brand".into(),
                version: "8".into(),
            },
            Brand {
                brand: "Chromium".into(),
                version: chrome_major.to_string(),
            },
            Brand {
                brand: "Google Chrome".into(),
                version: chrome_major.to_string(),
            },
        ];
        let full_version_list = vec![
            Brand {
                brand: "Not_A Brand".into(),
                version: "8.0.0.0".into(),
            },
            Brand {
                brand: "Chromium".into(),
                version: chrome_full.to_string(),
            },
            Brand {
                brand: "Google Chrome".into(),
                version: chrome_full.to_string(),
            },
        ];
        let (platform_version, architecture, bitness) = match platform {
            Platform::Win32 => ("15.0.0", "x86", "64"),
            Platform::MacIntel => ("10.15.7", "x86", "64"),
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

use std::path::Path;
use std::process::Command;

use crate::error::StealthError;

/// Default Chrome version used when `chrome --version` probe fails.
/// Bump on each release of zendriver-rs.
const FALLBACK_CHROME_FULL: &str = "120.0.6099.234";
const FALLBACK_CHROME_MAJOR: u32 = 120;

/// Probed system + Chrome facts used to compose stealth values.
#[derive(Debug, Clone, Serialize)]
pub struct Fingerprint {
    pub platform: Platform,
    pub chrome_major: u32,
    pub chrome_full: String,
    pub cpu_count: u32,
    pub memory_gb: u32,
    pub ua_string: String,
    pub ua_metadata: UserAgentMetadata,
    pub timezone: Option<String>,
    pub locale: Option<String>,
}

impl Fingerprint {
    /// Probe host system + installed Chrome to build a realistic fingerprint.
    // `StealthError` is large because `PatchFailed` wraps `CallError` (~152B).
    // Boxing it would cross the Task 5 file scope; bypass per-fn instead.
    #[allow(clippy::result_large_err)]
    pub fn auto_detect(chrome_executable: &Path) -> Result<Self, StealthError> {
        let platform = detect_platform();
        let (chrome_major, chrome_full) =
            probe_chrome_version(chrome_executable).unwrap_or_else(|e| {
                tracing::warn!("chrome version probe failed: {e}; using fallback");
                (FALLBACK_CHROME_MAJOR, FALLBACK_CHROME_FULL.to_string())
            });
        let cpu_count = clamp_cpu_count(num_cpus::get() as u32);
        let memory_gb = detect_memory_gb()?;
        let ua_string = crate::ua::compose_ua_string(platform, &chrome_full);
        let ua_metadata = UserAgentMetadata::realistic(platform, chrome_major, &chrome_full);
        Ok(Self {
            platform,
            chrome_major,
            chrome_full,
            cpu_count,
            memory_gb,
            ua_string,
            ua_metadata,
            timezone: None,
            locale: None,
        })
    }

    /// Recompose UA string + UAM after platform/version overrides.
    pub fn recompose(&mut self) {
        self.ua_string = crate::ua::compose_ua_string(self.platform, &self.chrome_full);
        self.ua_metadata =
            UserAgentMetadata::realistic(self.platform, self.chrome_major, &self.chrome_full);
    }
}

fn detect_platform() -> Platform {
    #[cfg(target_os = "windows")]
    {
        Platform::Win32
    }
    #[cfg(target_os = "macos")]
    {
        Platform::MacIntel
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
    {
        Platform::LinuxX86_64
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd"
    )))]
    {
        Platform::LinuxX86_64 // unknown unix-likes -> linux is the safest plausibility
    }
}

#[allow(clippy::result_large_err)]
fn probe_chrome_version(exe: &Path) -> Result<(u32, String), StealthError> {
    let output = Command::new(exe)
        .arg("--version")
        .output()
        .map_err(|e| StealthError::ChromeVersionDetect(format!("spawn failed: {e}")))?;
    if !output.status.success() {
        return Err(StealthError::ChromeVersionDetect(format!(
            "exit {:?}",
            output.status.code()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Format: "Google Chrome 120.0.6099.234" (sometimes "Chromium 120.0.6099.0")
    let full = stdout
        .split_whitespace()
        .find(|tok| tok.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .ok_or_else(|| StealthError::ChromeVersionDetect(format!("no version token in: {stdout}")))?
        .to_string();
    let major: u32 = full
        .split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| StealthError::ChromeVersionDetect(format!("bad major in: {full}")))?;
    Ok((major, full))
}

fn clamp_cpu_count(n: u32) -> u32 {
    n.clamp(2, 32)
}

/// Detect total RAM in GB, clamped to the spec-compliant values
/// for `navigator.deviceMemory` (capped at 8 per W3C; floor at 4 for plausibility).
#[allow(clippy::result_large_err)]
fn detect_memory_gb() -> Result<u32, StealthError> {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    // sysinfo 0.32: total_memory() returns BYTES, not KiB. Verified against
    // sysinfo-0.32.1/src/common/system.rs::total_memory doc comment
    // ("Returns the RAM size in bytes.").
    let total_bytes = sys.total_memory();
    if total_bytes == 0 {
        return Err(StealthError::SystemInfo("total_memory returned 0".into()));
    }
    let total_gb = (total_bytes / 1_073_741_824) as u32;
    Ok(round_to_navigator_memory(total_gb))
}

fn round_to_navigator_memory(gb: u32) -> u32 {
    // navigator.deviceMemory spec values: 0.25, 0.5, 1, 2, 4, 8. Cap at 8.
    // We floor at 4 for plausibility (sub-4GB consumer desktops are extinct).
    if gb >= 8 {
        8
    } else {
        4
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

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod fingerprint_tests {
    use super::*;

    #[test]
    fn clamp_cpu_count_floors_at_two() {
        assert_eq!(clamp_cpu_count(1), 2);
        assert_eq!(clamp_cpu_count(0), 2);
    }

    #[test]
    fn clamp_cpu_count_caps_at_thirty_two() {
        assert_eq!(clamp_cpu_count(64), 32);
        assert_eq!(clamp_cpu_count(128), 32);
    }

    #[test]
    fn clamp_cpu_count_preserves_normal_values() {
        assert_eq!(clamp_cpu_count(8), 8);
        assert_eq!(clamp_cpu_count(16), 16);
    }

    #[test]
    fn round_navigator_memory_caps_at_eight() {
        assert_eq!(round_to_navigator_memory(16), 8);
        assert_eq!(round_to_navigator_memory(64), 8);
    }

    #[test]
    fn round_navigator_memory_floors_at_four() {
        assert_eq!(round_to_navigator_memory(1), 4);
        assert_eq!(round_to_navigator_memory(3), 4);
    }

    #[test]
    fn round_navigator_memory_eight_stays_eight() {
        assert_eq!(round_to_navigator_memory(8), 8);
    }

    #[test]
    fn detect_memory_gb_works_on_real_system() {
        let gb = detect_memory_gb().expect("real system should have RAM");
        assert!(gb == 4 || gb == 8, "got {gb}");
    }

    #[test]
    fn detect_platform_returns_expected_for_host() {
        let p = detect_platform();
        #[cfg(target_os = "macos")]
        assert_eq!(p, Platform::MacIntel);
        #[cfg(target_os = "linux")]
        assert_eq!(p, Platform::LinuxX86_64);
        #[cfg(target_os = "windows")]
        assert_eq!(p, Platform::Win32);
    }

    #[test]
    fn fingerprint_recompose_updates_ua_and_uam() {
        let mut fp = Fingerprint {
            platform: Platform::Win32,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 8,
            memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: UserAgentMetadata::realistic(Platform::Win32, 120, "120.0.6099.234"),
            timezone: None,
            locale: None,
        };
        fp.recompose();
        assert!(fp.ua_string.contains("Windows NT 10.0"));
        assert!(fp.ua_string.contains("Chrome/120.0.6099.234"));
    }
}
