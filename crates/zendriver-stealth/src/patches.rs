//! Bundles individual patches into a single IIFE called with a serialized
//! Fingerprint. Only one `Page.addScriptToEvaluateOnNewDocument` round-trip
//! per nav instead of nine.

use serde_json::json;

use crate::Fingerprint;

const WEBDRIVER: &str = include_str!("patches/webdriver.js");
const PLUGINS: &str = include_str!("patches/plugins.js");
const CHROME: &str = include_str!("patches/chrome.js");
const WEBGL: &str = include_str!("patches/webgl.js");
const PERMISSIONS: &str = include_str!("patches/permissions.js");
const CODECS: &str = include_str!("patches/codecs.js");
const NAVIGATOR_PROPS: &str = include_str!("patches/navigator_props.js");
const USER_AGENT_DATA: &str = include_str!("patches/user_agent_data.js");
const BROKEN_IMAGE: &str = include_str!("patches/broken_image.js");

/// Build the bootstrap script for the spoofed profile.
/// Order: webdriver first (most-probed), navigator_props last (touches most fields).
#[must_use]
pub fn bootstrap_script(fp: &Fingerprint) -> String {
    let fp_json = json!({
        "platformJs":      fp.platform.js_string(),
        "chPlatform":      fp.platform.ch_platform(),
        "platformVersion": fp.ua_metadata.platform_version,
        "cpuCount":        fp.cpu_count,
        "memoryGb":        fp.memory_gb,
        "languages":       fp.locale.as_deref().map_or_else(
            || vec!["en-US".to_string(), "en".to_string()],
            |l| vec![l.to_string(), "en".to_string()],
        ),
        "architecture":    fp.ua_metadata.architecture,
        "bitness":         fp.ua_metadata.bitness,
        "brands":          fp.ua_metadata.brands,
        "fullVersionList": fp.ua_metadata.full_version_list,
    });

    format!(
        "(function(fp){{\n{WEBDRIVER}\n{PLUGINS}\n{CHROME}\n{WEBGL}\n{PERMISSIONS}\n{CODECS}\n{NAVIGATOR_PROPS}\n{USER_AGENT_DATA}\n{BROKEN_IMAGE}\n}})({fp_json});",
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{Platform, UserAgentMetadata};

    fn mock_fp() -> Fingerprint {
        Fingerprint {
            platform: Platform::MacIntel,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 10,
            memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: UserAgentMetadata::realistic(Platform::MacIntel, 120, "120.0.6099.234"),
            timezone: None,
            locale: Some("en-US".into()),
        }
    }

    #[test]
    fn bootstrap_includes_all_nine_patches() {
        let s = bootstrap_script(&mock_fp());
        assert!(s.contains("webdriver"), "webdriver patch missing");
        assert!(s.contains("PluginArray"), "plugins patch missing");
        assert!(s.contains("window.chrome"), "chrome patch missing");
        assert!(
            s.contains("UNMASKED_VENDOR_WEBGL") || s.contains("37445"),
            "webgl patch missing"
        );
        assert!(
            s.contains("Notification.permission"),
            "permissions patch missing"
        );
        assert!(s.contains("canPlayType"), "codecs patch missing");
        assert!(
            s.contains("hardwareConcurrency"),
            "navigator_props patch missing"
        );
        assert!(s.contains("userAgentData"), "user_agent_data patch missing");
        assert!(s.contains("naturalWidth"), "broken_image patch missing");
    }

    #[test]
    fn bootstrap_is_an_iife_taking_fp() {
        let s = bootstrap_script(&mock_fp());
        assert!(s.starts_with("(function(fp){"));
        assert!(s.contains("})({"), "fp arg JSON should follow");
    }

    #[test]
    fn bootstrap_substitutes_platform_js_string() {
        let s = bootstrap_script(&mock_fp());
        assert!(s.contains("\"MacIntel\""));
    }
}
