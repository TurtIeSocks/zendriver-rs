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
        // Stop Blink injecting `navigator.webdriver` (native getter → `true`).
        // py-zendriver sets this by default (config.py); ALL non-Off profiles
        // need it. Previously it lived only in the Spoofed branch, so the
        // Native profile leaked `navigator.webdriver === true` — an instant
        // bot tell. The Spoofed JS webdriver shim also relies on this flag so
        // Chrome's AutomationControlled hook can't re-inject over the shim.
        "--disable-blink-features=AutomationControlled".into(),
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
            // (`--disable-blink-features=AutomationControlled` lives in
            // `shared_stealth_flags` — both Native and Spoofed need it.)
            // SwiftShader supplies a WebGL CONTEXT in headless. Chrome runs
            // headless with `--headless=new --disable-gpu` (browser.rs), so with
            // no software backend `canvas.getContext('webgl')` returns null —
            // itself a bot tell (real browsers always have WebGL).
            // `--enable-unsafe-swiftshader` re-enables SwiftShader on Chrome >=116.
            v.push("--use-gl=angle".into());
            v.push("--use-angle=swiftshader".into());
            v.push("--enable-unsafe-swiftshader".into());
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
