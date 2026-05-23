//! User-Agent string composition.

use crate::Platform;

/// Build a Chrome desktop UA string for the given platform + version.
///
/// Format: `Mozilla/5.0 ({platform-token}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{full-version} Safari/537.36`
#[must_use]
pub fn compose_ua_string(platform: Platform, chrome_full: &str) -> String {
    format!(
        "Mozilla/5.0 ({platform_token}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{chrome_full} Safari/537.36",
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
