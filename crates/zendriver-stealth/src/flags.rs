//! Chrome launch flags for stealth profiles.
//!
//! Ported from zendriver Python (`zendriver/core/config.py:119-137`) plus
//! chaser-oxide additions.

use crate::ProfileKind;

/// Flags ALL stealth profiles share (Native + Spoofed + Off-when-not-Off).
/// Off profile uses an empty list (truly stock launch).
fn shared_stealth_flags() -> Vec<String> {
    vec![
        "--no-first-run".into(),
        "--no-service-autorun".into(),
        "--no-default-browser-check".into(),
        "--homepage=about:blank".into(),
        "--no-pings".into(),
        "--password-store=basic".into(),
        "--disable-infobars".into(),
        "--disable-breakpad".into(),
        "--disable-component-update".into(),
        "--disable-backgrounding-occluded-windows".into(),
        "--disable-renderer-backgrounding".into(),
        "--disable-background-networking".into(),
        "--disable-dev-shm-usage".into(),
        "--disable-features=IsolateOrigins,DisableLoadExtensionCommandLineSwitch,site-per-process"
            .into(),
        "--disable-session-crashed-bubble".into(),
        "--disable-search-engine-choice-screen".into(),
        "--remote-allow-origins=*".into(),
        // WebRTC IP-leak prevention (zendriver Python disable_webrtc=True default)
        "--webrtc-ip-handling-policy=disable_non_proxied_udp".into(),
        "--force-webrtc-ip-handling-policy".into(),
    ]
}

/// Build the full flag list for a profile.
#[must_use]
pub fn flags_for_profile(kind: ProfileKind) -> Vec<String> {
    match kind {
        ProfileKind::Off => Vec::new(),
        ProfileKind::Native => shared_stealth_flags(),
        ProfileKind::Spoofed => {
            let mut v = shared_stealth_flags();
            // Stop Blink from injecting `navigator.webdriver` (a native
            // getter that returns `true`). Without this flag Chrome's
            // own AutomationControlled hook overwrites whichever
            // `Object.defineProperty(Navigator.prototype, 'webdriver',
            // ...)` patch we install via
            // `Page.addScriptToEvaluateOnNewDocument`, because the
            // hook runs at context-start AFTER our bootstrap script.
            // Disabling the feature removes the native injection and
            // lets our shim's prototype getter stick.
            v.push("--disable-blink-features=AutomationControlled".into());
            v
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn off_profile_emits_no_flags() {
        assert!(flags_for_profile(ProfileKind::Off).is_empty());
    }

    #[test]
    fn native_profile_includes_webrtc_disable() {
        let flags = flags_for_profile(ProfileKind::Native);
        assert!(
            flags
                .iter()
                .any(|f| f.contains("webrtc-ip-handling-policy"))
        );
    }

    #[test]
    fn spoofed_profile_includes_isolate_origins_disable() {
        let flags = flags_for_profile(ProfileKind::Spoofed);
        assert!(flags.iter().any(|f| f.contains("IsolateOrigins")));
    }

    #[test]
    fn shared_flags_snapshot_native() {
        let flags = flags_for_profile(ProfileKind::Native);
        insta::assert_yaml_snapshot!("native_profile_flags", flags);
    }

    #[test]
    fn shared_flags_snapshot_spoofed() {
        let flags = flags_for_profile(ProfileKind::Spoofed);
        insta::assert_yaml_snapshot!("spoofed_profile_flags", flags);
    }

    #[test]
    fn shared_flags_snapshot_off() {
        let flags = flags_for_profile(ProfileKind::Off);
        insta::assert_yaml_snapshot!("off_profile_flags", flags);
    }
}
