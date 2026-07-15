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
#[cfg(not(windows))]
use std::process::Command;

use crate::error::StealthError;

/// Default Chrome version used when the version probe fails.
/// Bump on each release of zendriver-rs.
const FALLBACK_CHROME_FULL: &str = "148.0.7778.181";
const FALLBACK_CHROME_MAJOR: u32 = 148;

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
    pub languages: Option<Vec<String>>,
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
            languages: None,
        })
    }

    /// Recompose UA string + UAM after platform/version overrides.
    pub fn recompose(&mut self) {
        self.ua_string = crate::ua::compose_ua_string(self.platform, &self.chrome_full);
        self.ua_metadata =
            UserAgentMetadata::realistic(self.platform, self.chrome_major, &self.chrome_full);
    }
}

pub(crate) fn detect_platform() -> Platform {
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

/// Parse `Google Chrome 120.0.6099.234` / `Chromium 120.0.6099.0` into
/// `(major, full)`. Shared by the Unix probe and its tests.
#[cfg(not(windows))]
#[allow(clippy::result_large_err)]
fn parse_version_banner(stdout: &str) -> Result<(u32, String), StealthError> {
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

/// Probe the Chrome version by running `chrome --version`.
///
/// Unix-only *by design*. Chrome on Windows does not implement `--version` as a
/// print-and-exit: with no existing browser session it starts a whole browser
/// and never exits, so reading its output blocks forever. See the `cfg(windows)`
/// twin below for what Windows does instead, and why it must not exec.
///
/// Bounded even here. `--version` returns in milliseconds on a healthy Unix
/// Chrome, but this runs *synchronously inside* `Browser::launch()`'s future:
/// an unbounded wait would wedge the poll, and a future that never yields
/// starves the tokio timer driver — which silently disables every
/// `tokio::time::timeout` in the process, including the ones meant to catch
/// exactly this. A blocking call on the async path must carry its own deadline,
/// because no outer timeout can rescue it.
#[cfg(not(windows))]
#[allow(clippy::result_large_err)]
pub(crate) fn probe_chrome_version(exe: &Path) -> Result<(u32, String), StealthError> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    /// Generous: a healthy `chrome --version` answers in ~50ms. This is a
    /// backstop against a wedged binary, not a performance budget.
    const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
    const POLL_INTERVAL: Duration = Duration::from_millis(20);

    let mut child = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| StealthError::ChromeVersionDetect(format!("spawn failed: {e}")))?;

    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    // Reap rather than leak: kill_on_drop is not in play here.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(StealthError::ChromeVersionDetect(format!(
                        "`--version` did not exit within {PROBE_TIMEOUT:?}"
                    )));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => {
                return Err(StealthError::ChromeVersionDetect(format!(
                    "wait failed: {e}"
                )));
            }
        }
    };
    if !status.success() {
        return Err(StealthError::ChromeVersionDetect(format!(
            "exit {:?}",
            status.code()
        )));
    }

    let mut stdout = String::new();
    child
        .stdout
        .take()
        .ok_or_else(|| StealthError::ChromeVersionDetect("no stdout pipe".to_string()))?
        .read_to_string(&mut stdout)
        .map_err(|e| StealthError::ChromeVersionDetect(format!("read stdout: {e}")))?;
    parse_version_banner(&stdout)
}

/// Probe the Chrome version by reading the binary's own PE version resource.
///
/// **Never exec Chrome to ask its version on Windows.** `chrome.exe --version`
/// is not the Unix print-and-exit: with no existing browser session to hand off
/// to, it launches a full browser (GPU + network service + crashpad children)
/// and never exits. `Command::output()` waits for the child to exit *and* for
/// its stdout/stderr to hit EOF, so it blocks forever.
///
/// That block is what made this pathological rather than merely slow. The call
/// sits synchronously inside `Browser::launch()`'s future, so the task never
/// yields; `tokio::time::timeout` can only cancel at an await point, leaving
/// every guard in the launch path — `WS_ENDPOINT_TIMEOUT` (15s),
/// `HANDSHAKE_TIMEOUT` (30s) — structurally unable to fire. The result was an
/// infinite, *silent* hang: on windows-latest CI all three
/// `prerelease_verification` tests timed out at 360s having emitted nothing.
///
/// It hides on dev machines because a developer usually has Chrome already
/// open, and `--version` then hands off to the running instance
/// ("Opening in existing browser session.") and exits in milliseconds. It only
/// bites where no session exists — i.e. exactly on CI.
///
/// A version is a static property of the file, so read it from the file. This
/// parses the documented `VS_FIXEDFILEINFO` block, which keeps the crate's
/// `unsafe_code = "deny"` intact and adds no dependency.
#[cfg(windows)]
#[allow(clippy::result_large_err)]
pub(crate) fn probe_chrome_version(exe: &Path) -> Result<(u32, String), StealthError> {
    let bytes = std::fs::read(exe)
        .map_err(|e| StealthError::ChromeVersionDetect(format!("read {}: {e}", exe.display())))?;
    parse_pe_file_version(&bytes).ok_or_else(|| {
        StealthError::ChromeVersionDetect(format!(
            "no VS_FIXEDFILEINFO version resource in {}",
            exe.display()
        ))
    })
}

/// Find the first occurrence of `needle` in `haystack`.
#[cfg(windows)]
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Extract `(major, "a.b.c.d")` from a PE image's `VS_FIXEDFILEINFO`.
///
/// Anchors on the UTF-16LE `VS_VERSION_INFO` key, then reads the `0xFEEF04BD`
/// signature that opens the fixed-info block a few bytes later. Layout
/// (little-endian): `+0` signature, `+8` dwFileVersionMS, `+12` dwFileVersionLS,
/// each packing two 16-bit fields as `high.low`.
///
/// Both halves of that search are load-bearing, and real `chrome.exe` punishes
/// getting either wrong:
///
/// - **Anchor on the key, not the signature.** The signature is only four bytes
///   and does occur by chance in a multi-megabyte image: Chrome 150 carries one
///   at file offset ~2.79M, ~475KB *before* the resource, which decodes to
///   nonsense (`9340.36168.8075.18720`).
/// - **Try every key match, not just the first.** Chrome's `.rdata` contains the
///   literal text `VS_VERSION_INFO` — error strings from Chrome's *own* resource
///   parser ("unexpected VS_VERSIONINFO in ") — ~645KB before the genuine
///   resource. The first match is therefore a decoy with no signature after it;
///   only a later one is real. Scanning all matches and keeping the first that
///   is actually followed by a signature tolerates any number of such decoys.
#[cfg(windows)]
fn parse_pe_file_version(bytes: &[u8]) -> Option<(u32, String)> {
    const SIGNATURE: [u8; 4] = 0xFEEF_04BDu32.to_le_bytes();
    /// The signature sits just past the key + NUL + 32-bit alignment padding
    /// (34 bytes in practice). Bounding the search is what makes a decoy match
    /// fail fast and fall through to the next candidate.
    const SIG_SEARCH_WINDOW: usize = 64;

    let key: Vec<u8> = "VS_VERSION_INFO"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();

    let mut search_from = 0usize;
    while let Some(rel) = find_bytes(bytes.get(search_from..)?, &key) {
        let anchor = search_from + rel + key.len();
        let window_end = anchor.saturating_add(SIG_SEARCH_WINDOW).min(bytes.len());

        if let Some(sig) = bytes
            .get(anchor..window_end)
            .and_then(|w| find_bytes(w, &SIGNATURE))
            .map(|sig_rel| anchor + sig_rel)
        {
            let word = |off: usize| -> Option<u32> {
                Some(u32::from_le_bytes(
                    bytes.get(sig + off..sig + off + 4)?.try_into().ok()?,
                ))
            };
            let ms = word(8)?;
            let ls = word(12)?;
            let major = ms >> 16;
            // A zero major means we locked onto something that is not a version
            // block; keep looking rather than reporting `0.x.y.z`.
            if major != 0 {
                let full = format!("{}.{}.{}.{}", major, ms & 0xFFFF, ls >> 16, ls & 0xFFFF);
                return Some((major, full));
            }
        }
        search_from += rel + 1;
    }
    None
}

pub(crate) fn clamp_cpu_count(n: u32) -> u32 {
    n.clamp(2, 32)
}

/// Detect total RAM in GB, clamped to the spec-compliant values
/// for `navigator.deviceMemory` (capped at 8 per W3C; floor at 4 for plausibility).
#[allow(clippy::result_large_err)]
pub(crate) fn detect_memory_gb() -> Result<u32, StealthError> {
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
    if gb >= 8 { 8 } else { 4 }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The version probe must never *execute* Chrome on Windows: with no
    /// existing browser session `chrome.exe --version` starts a browser that
    /// never exits, and because the probe is called synchronously from inside
    /// `Browser::launch()`'s future it wedges the poll — starving the tokio
    /// timer so that no `timeout` anywhere in the launch path can fire. That
    /// shipped as a silent, infinite hang on windows-latest CI.
    ///
    /// Reading a real on-disk PE (this test binary itself) proves the parser
    /// works against a genuine version resource and needs no Chrome installed.
    #[cfg(windows)]
    #[test]
    fn probes_windows_version_from_pe_resource_without_executing() {
        let exe = std::env::current_exe().unwrap();
        let bytes = std::fs::read(&exe).unwrap();
        // Rust test binaries carry a version resource only when built with one,
        // so tolerate absence; what must never happen is a *wrong* parse.
        if let Some((major, full)) = parse_pe_file_version(&bytes) {
            assert!(
                full.split('.').count() == 4,
                "expected a 4-part version, got {full:?}"
            );
            assert_eq!(
                major,
                full.split('.').next().unwrap().parse::<u32>().unwrap(),
                "major must be the first component of {full:?}"
            );
        }
    }

    /// Locks the `VS_FIXEDFILEINFO` field packing (`high.low` per dword) against
    /// a synthetic resource, so the bit-twiddling is verified without depending
    /// on whatever Chrome happens to be installed on the test host.
    #[cfg(windows)]
    #[test]
    fn parses_fixed_file_info_field_packing() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&[0xAA; 128]); // leading noise
        buf.extend("VS_VERSION_INFO".encode_utf16().flat_map(u16::to_le_bytes));
        buf.extend_from_slice(&[0x00, 0x00]); // NUL + padding to the signature
        buf.extend_from_slice(&0xFEEF_04BDu32.to_le_bytes()); // dwSignature
        buf.extend_from_slice(&0x0001_0000u32.to_le_bytes()); // dwStrucVersion
        // Each dword packs `high.low`: MS = 150.0, LS = 7871.114.
        buf.extend_from_slice(&(150u32 << 16).to_le_bytes());
        buf.extend_from_slice(&((7871u32 << 16) | 114).to_le_bytes());

        assert_eq!(
            parse_pe_file_version(&buf),
            Some((150, "150.0.7871.114".to_string()))
        );
    }

    #[cfg(windows)]
    #[test]
    fn pe_version_parse_rejects_image_without_version_resource() {
        assert_eq!(parse_pe_file_version(&[0x00; 512]), None);
    }

    /// Real `chrome.exe` carries the literal `VS_VERSION_INFO` in `.rdata`
    /// (error text from Chrome's own resource parser) ~645KB before the genuine
    /// resource. Anchoring on the *first* match alone finds no signature and
    /// silently falls back to the baked-in version — which is precisely the
    /// wrong-version bug this probe exists to fix. Every match must be tried.
    #[cfg(windows)]
    #[test]
    fn pe_version_parse_skips_decoy_key_without_signature() {
        let key: Vec<u8> = "VS_VERSION_INFO"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();

        let mut buf: Vec<u8> = Vec::new();
        // Decoy #1: the key as plain string data, followed by prose rather than
        // a signature (mirrors Chrome's "unexpected VS_VERSIONINFO in " text).
        buf.extend_from_slice(&key);
        buf.extend_from_slice(b"\0\0unexpected VS_VERSIONINFO in \0");
        buf.extend_from_slice(&[0xCC; 96]);
        // The genuine resource.
        buf.extend_from_slice(&key);
        buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // NUL + alignment padding
        buf.extend_from_slice(&0xFEEF_04BDu32.to_le_bytes());
        buf.extend_from_slice(&0x0001_0000u32.to_le_bytes());
        buf.extend_from_slice(&(150u32 << 16).to_le_bytes());
        buf.extend_from_slice(&((7871u32 << 16) | 114).to_le_bytes());

        assert_eq!(
            parse_pe_file_version(&buf),
            Some((150, "150.0.7871.114".to_string())),
            "must skip the decoy key and find the real resource behind it"
        );
    }

    /// A bare `0xFEEF04BD` occurring in code/data must never be mistaken for the
    /// version block. Chrome 150 really does contain one ~475KB before the
    /// resource, decoding to `9340.36168.8075.18720`.
    #[cfg(windows)]
    #[test]
    fn pe_version_parse_ignores_stray_signature_without_key() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&0xFEEF_04BDu32.to_le_bytes());
        buf.extend_from_slice(&[0xEF; 32]);
        assert_eq!(parse_pe_file_version(&buf), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn parses_chrome_and_chromium_version_banners() {
        assert_eq!(
            parse_version_banner("Google Chrome 120.0.6099.234\n").unwrap(),
            (120, "120.0.6099.234".to_string())
        );
        assert_eq!(
            parse_version_banner("Chromium 120.0.6099.0\n").unwrap(),
            (120, "120.0.6099.0".to_string())
        );
        assert!(parse_version_banner("Opening in existing browser session.\n").is_err());
    }

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
            languages: None,
        };
        fp.recompose();
        assert!(fp.ua_string.contains("Windows NT 10.0"));
        assert!(fp.ua_string.contains("Chrome/120.0.6099.234"));
    }

    #[test]
    fn fallback_chrome_is_not_ancient() {
        // Tripwire: forces a conscious bump when Chrome moves well past this floor.
        // Floor is 4 majors below the probed version (148) at the time of writing.
        const {
            assert!(
                FALLBACK_CHROME_MAJOR >= 144,
                "FALLBACK_CHROME_MAJOR is stale; bump it (and this floor) to current stable"
            )
        };
    }
}
