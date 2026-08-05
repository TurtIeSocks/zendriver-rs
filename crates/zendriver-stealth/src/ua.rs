//! User-Agent string composition.

use crate::Platform;

/// Build a Chrome desktop UA string for the given platform + version.
///
/// Format: `Mozilla/5.0 ({platform-token}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36`
///
/// `chrome_full` is the full version (e.g. `"120.0.6099.234"`); only its MAJOR
/// reaches the string. Chrome has frozen the UA's minor/build/patch at `0.0.0`
/// since User-Agent reduction completed in Chrome 110, so a real desktop Chrome
/// sends `Chrome/120.0.0.0` and never `Chrome/120.0.6099.234`. Emitting the full
/// version is a one-line fingerprint tell: it is checkable against a static
/// pattern with no device knowledge at all, and no real browser matches it.
///
/// The full version still belongs in high-entropy UA-CH — `fullVersionList` and
/// `uaFullVersion` — which is where [`UaMetadata`](crate::UaMetadata) puts it.
/// That asymmetry IS the real shape: reduced in the UA string, complete in UA-CH.
///
/// A `chrome_full` with no leading integer is passed through verbatim rather than
/// being turned into `0.0.0`, so a caller that hands over an already-composed or
/// non-standard version is not silently rewritten into a worse one.
#[must_use]
pub fn compose_ua_string(platform: Platform, chrome_full: &str) -> String {
    let reduced = match chrome_full.split('.').next().map(str::trim) {
        Some(major) if !major.is_empty() && major.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{major}.0.0.0")
        }
        _ => chrome_full.to_string(),
    };
    format!(
        "Mozilla/5.0 ({platform_token}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{reduced} Safari/537.36",
        platform_token = platform.ua_token(),
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn compose_macintel_chrome_120_matches_snapshot() {
        let ua = compose_ua_string(Platform::MacIntel, "120.0.6099.234");
        insta::assert_snapshot!("ua_macintel_chrome_120", ua);
    }

    /// The regression this module exists to prevent. Every real desktop Chrome
    /// since v110 sends `Chrome/<major>.0.0.0`; the full build number appears
    /// only in UA-CH. Asserting the exact token rather than a snapshot, because
    /// a snapshot silently re-blesses whatever it is shown.
    #[test]
    fn ua_carries_only_the_reduced_version_never_the_build() {
        for (full, want) in [
            ("120.0.6099.234", "Chrome/120.0.0.0"),
            ("146.0.7680.153", "Chrome/146.0.0.0"),
            ("151.0.7922.76", "Chrome/151.0.0.0"),
            ("120.0.0.0", "Chrome/120.0.0.0"),
            ("120", "Chrome/120.0.0.0"),
        ] {
            let ua = compose_ua_string(Platform::MacIntel, full);
            assert!(ua.contains(want), "{full} should compose {want}, got: {ua}");
            assert!(
                !ua.contains(&format!("Chrome/{full} ")) || full == "120.0.0.0",
                "the full build number must not reach the UA string: {ua}"
            );
        }
    }

    /// A version we cannot parse is passed through rather than mangled into
    /// `0.0.0`, which would be a worse string than the one the caller supplied.
    #[test]
    fn an_unparseable_version_is_left_alone() {
        let ua = compose_ua_string(Platform::Win32, "not-a-version");
        assert!(ua.contains("Chrome/not-a-version "), "{ua}");
    }

    #[test]
    fn compose_win32_chrome_120_matches_snapshot() {
        let ua = compose_ua_string(Platform::Win32, "120.0.6099.234");
        insta::assert_snapshot!("ua_win32_chrome_120", ua);
    }

    #[test]
    fn compose_linux_chrome_120_matches_snapshot() {
        let ua = compose_ua_string(Platform::LinuxX86_64, "120.0.6099.234");
        insta::assert_snapshot!("ua_linux_chrome_120", ua);
    }

    #[test]
    fn composed_ua_never_contains_headless_substring() {
        for p in [Platform::Win32, Platform::MacIntel, Platform::LinuxX86_64] {
            let ua = compose_ua_string(p, "120.0.6099.234");
            assert!(!ua.contains("Headless"), "got: {ua}");
        }
    }
}
