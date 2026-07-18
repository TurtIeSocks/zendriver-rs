//! Chrome launch flags for stealth profiles.
//!
//! Ported from zendriver Python (`zendriver/core/config.py:119-137`) plus
//! chaser-oxide additions.

use crate::ProfileKind;

/// Flags ALL stealth profiles share (Native + Spoofed + Off-when-not-Off).
/// Off profile uses an empty list (truly stock launch).
///
/// `native_isolation` selects the `--disable-features=...` entry: `false`
/// (the default path) disables Chrome's render-process site isolation
/// (`IsolateOrigins`/`site-per-process`) as today; `true` leaves those two
/// features enabled — Chrome's stock site isolation stays on — while still
/// disabling the unrelated `DisableLoadExtensionCommandLineSwitch` feature
/// (needed for `--load-extension` regardless of the isolation choice). See
/// [`StealthProfile::native_isolation`](crate::StealthProfile::native_isolation)
/// for the caller-facing opt-in and its trade-off.
fn shared_stealth_flags(native_isolation: bool) -> Vec<String> {
    let disable_features = if native_isolation {
        "--disable-features=DisableLoadExtensionCommandLineSwitch".to_string()
    } else {
        "--disable-features=IsolateOrigins,DisableLoadExtensionCommandLineSwitch,site-per-process"
            .to_string()
    };
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
        disable_features,
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
///
/// `native_isolation` is the opt-in from
/// [`StealthProfile::native_isolation`](crate::StealthProfile::native_isolation):
/// `false` (the default) is today's behavior, byte-identical for every
/// [`ProfileKind`]. `true` omits the site-isolation-disabling feature names
/// from `--disable-features=...`, leaving Chrome's real
/// `IsolateOrigins`/`site-per-process` behavior in place.
#[must_use]
pub fn flags_for_profile(kind: ProfileKind, native_isolation: bool) -> Vec<String> {
    match kind {
        ProfileKind::Off => Vec::new(),
        ProfileKind::Native => shared_stealth_flags(native_isolation),
        ProfileKind::Spoofed => {
            let mut v = shared_stealth_flags(native_isolation);
            // (`--disable-blink-features=AutomationControlled` lives in
            // `shared_stealth_flags` — both Native and Spoofed need it.)
            // SwiftShader supplies a WebGL CONTEXT in headless. Chrome runs
            // headless with `--headless=new --disable-gpu` (browser.rs), so with
            // no software backend `canvas.getContext('webgl')` returns null —
            // itself a bot tell (real browsers always have WebGL). This is
            // about the WebGL CONTEXT existing at all, not the vendor/renderer
            // *identity* it reports, so it stays regardless of
            // `native_isolation` — the opt-in only skips the identity patch
            // in `patches.rs`, not this context-enabling launch flag.
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
        assert!(flags_for_profile(ProfileKind::Off, false).is_empty());
    }

    #[test]
    fn native_profile_includes_webrtc_disable() {
        let flags = flags_for_profile(ProfileKind::Native, false);
        assert!(
            flags
                .iter()
                .any(|f| f.contains("webrtc-ip-handling-policy"))
        );
    }

    #[test]
    fn spoofed_profile_includes_isolate_origins_disable() {
        let flags = flags_for_profile(ProfileKind::Spoofed, false);
        assert!(flags.iter().any(|f| f.contains("IsolateOrigins")));
    }

    #[test]
    fn shared_flags_snapshot_native() {
        let flags = flags_for_profile(ProfileKind::Native, false);
        insta::assert_yaml_snapshot!("native_profile_flags", flags);
    }

    #[test]
    fn shared_flags_snapshot_spoofed() {
        let flags = flags_for_profile(ProfileKind::Spoofed, false);
        insta::assert_yaml_snapshot!("spoofed_profile_flags", flags);
    }

    #[test]
    fn shared_flags_snapshot_off() {
        let flags = flags_for_profile(ProfileKind::Off, false);
        insta::assert_yaml_snapshot!("off_profile_flags", flags);
    }

    // --- opt-in native_isolation path (Task 10) -----------------------------

    #[test]
    fn native_isolation_flags_omit_isolate_origins_and_site_per_process() {
        let flags = flags_for_profile(ProfileKind::Native, true);
        assert!(
            !flags
                .iter()
                .any(|f| f.contains("IsolateOrigins") || f.contains("site-per-process")),
            "native_isolation=true must not disable Chrome's real site isolation, got: {flags:?}"
        );
    }

    #[test]
    fn native_isolation_flags_keep_unrelated_disable_load_extension_feature() {
        // `DisableLoadExtensionCommandLineSwitch` is unrelated to site
        // isolation (it controls whether `--load-extension` works) — it must
        // stay disabled regardless of the native_isolation opt-in.
        let flags = flags_for_profile(ProfileKind::Native, true);
        assert!(
            flags
                .iter()
                .any(|f| f.contains("DisableLoadExtensionCommandLineSwitch")),
            "got: {flags:?}"
        );
    }

    #[test]
    fn native_isolation_spoofed_still_carries_swiftshader_context_flags() {
        // The SwiftShader launch flags exist so headless has a *working*
        // WebGL context at all — unrelated to the vendor/renderer identity
        // patch that native_isolation skips in patches.rs. They must stay.
        let flags = flags_for_profile(ProfileKind::Spoofed, true);
        assert!(flags.iter().any(|f| f == "--enable-unsafe-swiftshader"));
    }

    #[test]
    fn native_isolation_false_keeps_isolation_disabled_default_unchanged() {
        // Regression guard: native_isolation=false (today's default, used by
        // every call site that doesn't opt in) still disables site
        // isolation — anchored byte-for-byte by the pre-existing
        // `native_profile_flags`/`spoofed_profile_flags` snapshots above.
        assert!(
            flags_for_profile(ProfileKind::Native, false)
                .iter()
                .any(|f| f.contains("IsolateOrigins"))
        );
    }

    #[test]
    fn shared_flags_snapshot_native_isolation_native() {
        let flags = flags_for_profile(ProfileKind::Native, true);
        insta::assert_yaml_snapshot!("native_isolation_native_profile_flags", flags);
    }

    #[test]
    fn shared_flags_snapshot_native_isolation_spoofed() {
        let flags = flags_for_profile(ProfileKind::Spoofed, true);
        insta::assert_yaml_snapshot!("native_isolation_spoofed_profile_flags", flags);
    }
}
