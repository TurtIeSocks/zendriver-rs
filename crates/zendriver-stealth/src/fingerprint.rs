//! Fingerprint: composed UA + Sec-CH-UA metadata + system facts.

use serde::{Deserialize, Serialize};

use crate::Platform;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Brand {
    pub brand: String,
    pub version: String,
}

/// Sent to CDP as `Emulation.setUserAgentOverride.userAgentMetadata`.
/// Mirrors the [W3C UA-CH spec](https://wicg.github.io/ua-client-hints/).
#[derive(Debug, Clone, PartialEq, Serialize)]
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

/// Chromium's GREASE brand, derived from the major version.
///
/// Chromium builds the "fake" brand from a fixed 11-character alphabet indexed by the major
/// version, and picks the version from a 3-element table the same way — see
/// `GenerateBrandVersionList` in `components/embedder_support/user_agent_utils.cc`. It is
/// deterministic per major, NOT random per boot, so a client that hardcodes one pair is
/// identifiable the moment its major moves on.
///
/// Anchored on real captures: 142 -> `Not_A Brand`/`99`, 146 -> `Not-A.Brand`/`24`.
fn grease_brand(chrome_major: u32) -> (String, &'static str) {
    const CHARS: [&str; 11] = [" ", "(", ":", "-", ".", "/", ")", ";", "=", "?", "_"];
    const VERSIONS: [&str; 3] = ["8", "99", "24"];
    let c1 = CHARS[(chrome_major % 11) as usize];
    let c2 = CHARS[((chrome_major + 1) % 11) as usize];
    (
        format!("Not{c1}A{c2}Brand"),
        VERSIONS[(chrome_major % 3) as usize],
    )
}

/// Chromium's brand-list permutation, also derived from the major version.
///
/// `GenerateBrandVersionList` shuffles the three brands through a fixed table of the six
/// permutations of three elements, selected by `major % 6`, and it SCATTERS: the i-th input
/// brand is written to slot `order[i]`. That direction matters — for the two 3-cycles
/// (`major % 6` of 3 or 4) scattering and gathering give different results, and a real
/// major-142 capture (`major % 6 == 4`) matches the scatter.
///
/// Returns the three brands in wire order given `[grease, chromium, branded]`.
fn permute_brands<T>(chrome_major: u32, brands: [T; 3]) -> Vec<T> {
    const ORDERS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let order = ORDERS[(chrome_major % 6) as usize];
    let mut out: Vec<Option<T>> = vec![None, None, None];
    for (brand, slot) in brands.into_iter().zip(order) {
        out[slot] = Some(brand);
    }
    // Every slot is written exactly once: `order` is a permutation of 0..3.
    out.into_iter()
        .map(|b| b.expect("permutation covers every slot"))
        .collect()
}

impl UserAgentMetadata {
    /// Build a realistic UAM for the given platform + Chrome major version.
    ///
    /// Three brands, as Chrome sends them: a GREASE brand, `Chromium;v=N` and
    /// `Google Chrome;v=N`. Both the GREASE pair and the list ORDER are derived from
    /// `chrome_major` — see [`grease_brand`] and [`permute_brands`].
    ///
    /// This used to hardcode `Not_A Brand;v=8` in a fixed GREASE-first order. That pair and
    /// that order are what Chrome **120** sends (120 % 11 == 10, 120 % 3 == 0, 120 % 6 == 0),
    /// so every other major presented a `sec-ch-ua` no Chrome has ever sent — checkable
    /// against a static table with no device knowledge at all.
    pub fn realistic(platform: Platform, chrome_major: u32, chrome_full: &str) -> Self {
        let (grease, grease_version) = grease_brand(chrome_major);
        let brands = permute_brands(
            chrome_major,
            [
                Brand {
                    brand: grease.clone(),
                    version: grease_version.into(),
                },
                Brand {
                    brand: "Chromium".into(),
                    version: chrome_major.to_string(),
                },
                Brand {
                    brand: "Google Chrome".into(),
                    version: chrome_major.to_string(),
                },
            ],
        );
        let full_version_list = permute_brands(
            chrome_major,
            [
                Brand {
                    brand: grease,
                    version: format!("{grease_version}.0.0.0"),
                },
                Brand {
                    brand: "Chromium".into(),
                    version: chrome_full.to_string(),
                },
                Brand {
                    brand: "Google Chrome".into(),
                    version: chrome_full.to_string(),
                },
            ],
        );
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
    /// IANA timezone driving `Emulation.setTimezoneOverride`, from
    /// [`StealthProfile::timezone`](crate::StealthProfile::timezone) or from a
    /// [`Persona`](crate::Persona), which wins and is folded in when the
    /// observer is built. `None` sends no override.
    pub timezone: Option<String>,
    /// Locale driving `Emulation.setLocaleOverride` when
    /// [`languages`](Self::languages) is unset. Same two sources as
    /// [`timezone`](Self::timezone).
    pub locale: Option<String>,
    /// Ordered language list driving `Accept-Language`, `navigator.languages`
    /// and — from its first entry — `Emulation.setLocaleOverride`, which is
    /// what keeps the JS-visible locale inside the list the header advertises.
    /// Falls back to [`locale`](Self::locale) when unset, and the header falls
    /// back further to `["en-US", "en"]` when that is unset too (the locale
    /// override is then simply not sent).
    pub languages: Option<Vec<String>>,
    /// Screen / device-metrics override resolved from
    /// [`StealthProfile::screen`](crate::StealthProfile::screen), or from a
    /// [`Persona`](crate::Persona)'s own `screen`, which wins. `None` by
    /// default (`auto_detect` never probes a screen size) — the observer's
    /// fixed 1920x1080 default is untouched until this is explicitly set.
    pub screen: Option<crate::persona::specs::ScreenSpec>,
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
            screen: None,
        })
    }

    /// Fold a [`Persona`](crate::Persona)'s explicitly-set fields into this
    /// fingerprint.
    ///
    /// Four axes exist on both types — `timezone`, `locale`, `languages`,
    /// `screen` — and three separate consumers read them: the CDP
    /// `Emulation.set*Override` calls, the `Accept-Language` header, and the
    /// JS patches. Merging once, at the single point where a persona and a
    /// fingerprint meet
    /// ([`StealthObserver::with_persona`](crate::StealthObserver::with_persona)),
    /// is what keeps those consumers from disagreeing: each reads the merged
    /// value rather than re-deriving the precedence for itself.
    ///
    /// Precedence: an explicitly-set persona field wins; `None` inherits
    /// whatever the fingerprint already resolved — from
    /// [`StealthProfile`](crate::StealthProfile)'s per-field setters, or from
    /// the host probe. A [`Persona::default`](crate::Persona::default) is a
    /// no-op, and the merge is idempotent.
    ///
    /// An empty `languages` list counts as unset, matching how every consumer
    /// in [`lang`](crate::lang) already reads one: `Some(vec![])` describes a
    /// persona that pins no languages, not one that advertises none, and
    /// letting it overwrite would make an empty persona field destroy a
    /// configured [`StealthProfile::languages`](crate::StealthProfile::languages).
    pub(crate) fn overlay_persona(&mut self, persona: &crate::Persona) {
        if let Some(timezone) = &persona.timezone {
            self.timezone = Some(timezone.clone());
        }
        if let Some(locale) = &persona.locale {
            self.locale = Some(locale.clone());
        }
        if let Some(languages) = persona.languages.as_ref().filter(|v| !v.is_empty()) {
            self.languages = Some(languages.clone());
        }
        if let Some(screen) = persona.screen {
            self.screen = Some(screen);
        }
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

/// Bound the *probed* host CPU count to what browsers commonly report.
///
/// This is the probe path, where the input is a measurement of whatever host
/// happens to be running — a 128-thread build machine is a real number and a
/// terrible thing to advertise. An explicit
/// [`StealthProfile::cpu_count`](crate::StealthProfile::cpu_count) is a
/// different kind of input: it is a statement of intent, is not clamped, and
/// only warns. Keep the two apart.
pub(crate) fn clamp_cpu_count(n: u32) -> u32 {
    n.clamp(2, 32)
}

/// Detect total RAM in GB, clamped to the spec-compliant values
/// for `navigator.deviceMemory` (capped at 8 per W3C; floor at 4 for
/// plausibility).
///
/// Probe path, same division as [`clamp_cpu_count`]: rounding a measurement
/// is not the same act as overriding
/// [`StealthProfile::memory_gb`](crate::StealthProfile::memory_gb), which is
/// reported exactly as the caller stated it.
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

    /// Real captures, not derivations: both pairs come from device fingerprints recorded off
    /// genuine Chrome installs. 146 is the anchor that proves the alphabet indexing, since its
    /// two characters differ (`-` and `.`).
    #[test]
    fn grease_brand_matches_real_captures() {
        assert_eq!(grease_brand(146), ("Not-A.Brand".to_string(), "24"));
        assert_eq!(grease_brand(142), ("Not_A Brand".to_string(), "99"));
    }

    /// The value that was hardcoded before this was derived. `Not_A Brand;v=8` is precisely
    /// Chrome 120's GREASE pair, which is why the constant looked plausible for years: it was
    /// correct for exactly one major and wrong for every other.
    #[test]
    fn the_old_hardcoded_pair_was_chrome_120s() {
        assert_eq!(grease_brand(120), ("Not_A Brand".to_string(), "8"));
        // ...and 120 also takes the identity permutation, which is why GREASE-first looked
        // right too.
        assert_eq!(
            permute_brands(120, ["grease", "chromium", "branded"]),
            vec!["grease", "chromium", "branded"]
        );
    }

    /// A real major-146 capture puts Chromium first, GREASE second, the branded entry last.
    #[test]
    fn brand_order_matches_a_real_146_capture() {
        assert_eq!(
            permute_brands(146, ["grease", "chromium", "branded"]),
            vec!["chromium", "grease", "branded"]
        );
    }

    /// 142 is the discriminating case. `142 % 6 == 4` selects `[2, 0, 1]`, one of the two
    /// 3-cycles, so scatter and gather disagree: scatter yields
    /// `[chromium, branded, grease]`, gather would yield `[branded, grease, chromium]`.
    /// The real capture is the former, which is what fixes the direction.
    #[test]
    fn brand_order_scatters_rather_than_gathers() {
        assert_eq!(
            permute_brands(142, ["grease", "chromium", "branded"]),
            vec!["chromium", "branded", "grease"]
        );
    }

    /// Whatever the major, all three brands must survive exactly once — a permutation bug that
    /// dropped or duplicated one would otherwise only show up as a weird header in the wild.
    #[test]
    fn every_major_permutes_without_loss() {
        for major in 100u32..200 {
            let out = permute_brands(major, ["grease", "chromium", "branded"]);
            assert_eq!(out.len(), 3, "major {major}");
            for want in ["grease", "chromium", "branded"] {
                assert_eq!(
                    out.iter().filter(|b| **b == want).count(),
                    1,
                    "major {major} lost or duplicated {want}: {out:?}"
                );
            }
        }
    }

    /// `brands` and `full_version_list` are the same three brands at two precisions, so they
    /// must agree on order — a client whose two lists disagreed would contradict itself.
    #[test]
    fn brands_and_full_version_list_agree_on_order() {
        for major in [120u32, 142, 146, 149, 150] {
            let uam = UserAgentMetadata::realistic(Platform::Win32, major, "146.0.7680.165");
            let a: Vec<&str> = uam.brands.iter().map(|b| b.brand.as_str()).collect();
            let b: Vec<&str> = uam
                .full_version_list
                .iter()
                .map(|b| b.brand.as_str())
                .collect();
            assert_eq!(a, b, "major {major}");
        }
    }
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
            screen: None,
        };
        fp.recompose();
        assert!(fp.ua_string.contains("Windows NT 10.0"));
        // The UA string carries the REDUCED version and the full build number
        // never reaches it — Chrome froze the minor/build/patch at `0.0.0` in
        // v110. This assertion used to demand the opposite.
        assert!(
            fp.ua_string.contains("Chrome/120.0.0.0"),
            "recompose must reduce the UA version: {}",
            fp.ua_string
        );
        assert!(
            !fp.ua_string.contains("120.0.6099.234"),
            "the full build number must not reach the UA string: {}",
            fp.ua_string
        );
        // ...while UA-CH keeps it, which is the asymmetry a real Chrome shows.
        assert!(
            fp.ua_metadata
                .full_version_list
                .iter()
                .any(|b| b.version == "120.0.6099.234"),
            "fullVersionList must still carry the complete version"
        );
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
