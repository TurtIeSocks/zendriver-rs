//! Chrome launch flags for stealth profiles.
//!
//! Ported from zendriver Python (`zendriver/core/config.py:119-137`) plus
//! chaser-oxide additions.

use crate::ProfileKind;
use serde::{Deserialize, Serialize};

/// Which GPU backend Chrome should render WebGL / WebGPU with.
///
/// Defaults to [`Disabled`](Self::Disabled), which reproduces zendriver's
/// historical launch flags exactly. This is an explicit opt-in: zendriver
/// never probes the host for a GPU and never switches backends on its own.
///
/// # Why the backend must be named explicitly
///
/// Removing `--disable-gpu` without also naming an ANGLE backend **hangs**
/// headless Chrome (measured on darwin; see the design spec's Measurements
/// section). The two decisions are therefore coupled and owned together here,
/// rather than left to the caller to combine correctly.
///
/// ```
/// use zendriver_stealth::GpuBackend;
/// // Selecting a backend never probes the host — the caller decides.
/// let flags = GpuBackend::Native.angle_flags();
/// assert_eq!(flags[0], "--use-gl=angle");
/// assert!(!GpuBackend::Native.allows_disable_gpu());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuBackend {
    /// Today's behavior: `--disable-gpu` under headless and no ANGLE backend
    /// forced. Chrome picks its own fallback.
    #[default]
    Disabled,
    /// Force SwiftShader's CPU rasterizer. Guarantees a working WebGL context
    /// on a host with no GPU, at the cost of a software-rasterizer
    /// fingerprint that no real device produces.
    SwiftShader,
    /// Render on the host GPU through the platform's ANGLE backend (macOS →
    /// Metal, Windows → D3D11, otherwise Vulkan).
    ///
    /// Gives a fully coherent GPU surface — real capabilities, real pixels,
    /// real timings, a working `requestDevice()` — but reports **the host's**
    /// GPU, not a chosen one. A fleet sharing one host shares one fingerprint.
    ///
    /// Requires a usable GPU. There is no automatic fallback: if the GPU
    /// process cannot start, the launch fails with an actionable error rather
    /// than silently degrading to software rendering.
    Native,
}

/// ANGLE backend token for an OS name as reported by [`std::env::consts::OS`].
///
/// Unknown platforms take the Vulkan path rather than returning nothing —
/// an absent backend is what causes the launch hang described on
/// [`GpuBackend`].
fn angle_backend_for_os(os: &str) -> &'static str {
    match os {
        "macos" => "metal",
        "windows" => "d3d11",
        _ => "vulkan",
    }
}

impl GpuBackend {
    /// Launch flags selecting this backend. Empty for
    /// [`Disabled`](Self::Disabled).
    #[must_use]
    pub fn angle_flags(self) -> Vec<String> {
        match self {
            Self::Disabled => Vec::new(),
            Self::SwiftShader => vec![
                "--use-gl=angle".into(),
                "--use-angle=swiftshader".into(),
                // Chrome >= 116 refuses the SwiftShader fallback without this.
                "--enable-unsafe-swiftshader".into(),
            ],
            Self::Native => vec![
                "--use-gl=angle".into(),
                format!("--use-angle={}", angle_backend_for_os(std::env::consts::OS)),
            ],
        }
    }

    /// Whether `--disable-gpu` may still be emitted under headless.
    ///
    /// False only for [`Native`](Self::Native), where the flag would defeat
    /// the entire point of selecting a hardware backend.
    #[must_use]
    pub fn allows_disable_gpu(self) -> bool {
        !matches!(self, Self::Native)
    }
}

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
///
/// `gpu_backend` is the opt-in from
/// [`StealthProfile::gpu_backend`](crate::StealthProfile::gpu_backend).
/// [`GpuBackend::Disabled`] (the default) reproduces today's behavior exactly:
/// no ANGLE flags on `Native`, and the unconditional SwiftShader flags on
/// `Spoofed` (see the `Spoofed` arm below for why those stay). An explicit
/// backend adds/replaces the ANGLE flags on any non-Off profile; `Off` stays
/// stock under every backend.
#[must_use]
pub fn flags_for_profile(
    kind: ProfileKind,
    native_isolation: bool,
    gpu_backend: GpuBackend,
) -> Vec<String> {
    match kind {
        // Off stays a truly stock launch under every backend — selecting a
        // GPU backend on an Off profile is a no-op by design.
        ProfileKind::Off => Vec::new(),
        ProfileKind::Native => {
            let mut v = shared_stealth_flags(native_isolation);
            v.extend(gpu_backend.angle_flags());
            v
        }
        ProfileKind::Spoofed => {
            let mut v = shared_stealth_flags(native_isolation);
            // (`--disable-blink-features=AutomationControlled` lives in
            // `shared_stealth_flags` — both Native and Spoofed need it.)
            // A WebGL *context* must exist at all in headless — Chrome runs
            // headless with `--headless=new --disable-gpu` (browser.rs), so
            // with no software backend `canvas.getContext('webgl')` returns
            // null, itself a bot tell (real browsers always have WebGL).
            // This is about the context existing at all, not the
            // vendor/renderer *identity* it reports (that's `patches.rs`'s
            // job, gated by `native_webgl`), so it stays regardless of
            // `native_isolation`. Historically that was guaranteed by
            // unconditionally forcing SwiftShader here; that default is now
            // expressed as `GpuBackend::Disabled` keeping the SwiftShader
            // flags, while an explicit backend replaces them.
            match gpu_backend {
                GpuBackend::Disabled => {
                    v.extend(GpuBackend::SwiftShader.angle_flags());
                }
                explicit => v.extend(explicit.angle_flags()),
            }
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
        assert!(flags_for_profile(ProfileKind::Off, false, GpuBackend::Disabled).is_empty());
    }

    #[test]
    fn native_profile_includes_webrtc_disable() {
        let flags = flags_for_profile(ProfileKind::Native, false, GpuBackend::Disabled);
        assert!(
            flags
                .iter()
                .any(|f| f.contains("webrtc-ip-handling-policy"))
        );
    }

    #[test]
    fn spoofed_profile_includes_isolate_origins_disable() {
        let flags = flags_for_profile(ProfileKind::Spoofed, false, GpuBackend::Disabled);
        assert!(flags.iter().any(|f| f.contains("IsolateOrigins")));
    }

    #[test]
    fn shared_flags_snapshot_native() {
        let flags = flags_for_profile(ProfileKind::Native, false, GpuBackend::Disabled);
        insta::assert_yaml_snapshot!("native_profile_flags", flags);
    }

    #[test]
    fn shared_flags_snapshot_spoofed() {
        let flags = flags_for_profile(ProfileKind::Spoofed, false, GpuBackend::Disabled);
        insta::assert_yaml_snapshot!("spoofed_profile_flags", flags);
    }

    #[test]
    fn shared_flags_snapshot_off() {
        let flags = flags_for_profile(ProfileKind::Off, false, GpuBackend::Disabled);
        insta::assert_yaml_snapshot!("off_profile_flags", flags);
    }

    // --- opt-in native_isolation path (Task 10) -----------------------------

    #[test]
    fn native_isolation_flags_omit_isolate_origins_and_site_per_process() {
        let flags = flags_for_profile(ProfileKind::Native, true, GpuBackend::Disabled);
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
        let flags = flags_for_profile(ProfileKind::Native, true, GpuBackend::Disabled);
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
        let flags = flags_for_profile(ProfileKind::Spoofed, true, GpuBackend::Disabled);
        assert!(flags.iter().any(|f| f == "--enable-unsafe-swiftshader"));
    }

    #[test]
    fn native_isolation_false_keeps_isolation_disabled_default_unchanged() {
        // Regression guard: native_isolation=false (today's default, used by
        // every call site that doesn't opt in) still disables site
        // isolation — anchored byte-for-byte by the pre-existing
        // `native_profile_flags`/`spoofed_profile_flags` snapshots above.
        assert!(
            flags_for_profile(ProfileKind::Native, false, GpuBackend::Disabled)
                .iter()
                .any(|f| f.contains("IsolateOrigins"))
        );
    }

    #[test]
    fn shared_flags_snapshot_native_isolation_native() {
        let flags = flags_for_profile(ProfileKind::Native, true, GpuBackend::Disabled);
        insta::assert_yaml_snapshot!("native_isolation_native_profile_flags", flags);
    }

    #[test]
    fn shared_flags_snapshot_native_isolation_spoofed() {
        let flags = flags_for_profile(ProfileKind::Spoofed, true, GpuBackend::Disabled);
        insta::assert_yaml_snapshot!("native_isolation_spoofed_profile_flags", flags);
    }

    // --- GpuBackend ---------------------------------------------------------

    #[test]
    fn gpu_backend_default_is_disabled() {
        assert_eq!(GpuBackend::default(), GpuBackend::Disabled);
    }

    #[test]
    fn disabled_backend_emits_no_angle_flags() {
        assert!(GpuBackend::Disabled.angle_flags().is_empty());
    }

    #[test]
    fn swiftshader_backend_emits_todays_three_flags() {
        assert_eq!(
            GpuBackend::SwiftShader.angle_flags(),
            vec![
                "--use-gl=angle".to_string(),
                "--use-angle=swiftshader".to_string(),
                "--enable-unsafe-swiftshader".to_string(),
            ]
        );
    }

    #[test]
    fn native_backend_maps_os_to_angle_backend() {
        assert_eq!(angle_backend_for_os("macos"), "metal");
        assert_eq!(angle_backend_for_os("windows"), "d3d11");
        assert_eq!(angle_backend_for_os("linux"), "vulkan");
        // Unknown platforms take the Linux path rather than emitting nothing —
        // an empty backend would silently fall back to Chrome's default pick.
        assert_eq!(angle_backend_for_os("freebsd"), "vulkan");
    }

    #[test]
    fn native_backend_emits_angle_flags_for_current_os() {
        let flags = GpuBackend::Native.angle_flags();
        assert_eq!(flags[0], "--use-gl=angle");
        assert!(
            flags[1].starts_with("--use-angle="),
            "expected an explicit backend, got: {flags:?}"
        );
        // Never SwiftShader under Native — that is the whole point.
        assert!(
            !flags.iter().any(|f| f.contains("swiftshader")),
            "got: {flags:?}"
        );
    }

    #[test]
    fn only_native_forbids_disable_gpu() {
        // Measured: dropping --disable-gpu without naming a backend hangs
        // Chrome, and keeping it with a backend suppresses the GPU entirely.
        // So the two decisions are coupled and Native owns both.
        assert!(GpuBackend::Disabled.allows_disable_gpu());
        assert!(GpuBackend::SwiftShader.allows_disable_gpu());
        assert!(!GpuBackend::Native.allows_disable_gpu());
    }

    #[test]
    fn gpu_backend_round_trips_json_as_snake_case() {
        let json = serde_json::to_string(&GpuBackend::SwiftShader).unwrap();
        assert_eq!(json, "\"swift_shader\"");
        let back: GpuBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(back, GpuBackend::SwiftShader);
    }

    // --- GpuBackend threaded through flags_for_profile (Task 2) -------------

    #[test]
    fn spoofed_default_backend_still_emits_swiftshader_flags() {
        // Regression guard: the default path must be byte-for-byte unchanged.
        let flags = flags_for_profile(ProfileKind::Spoofed, false, GpuBackend::Disabled);
        assert!(flags.iter().any(|f| f == "--use-angle=swiftshader"));
        assert!(flags.iter().any(|f| f == "--enable-unsafe-swiftshader"));
    }

    #[test]
    fn spoofed_native_backend_replaces_swiftshader_flags() {
        let flags = flags_for_profile(ProfileKind::Spoofed, false, GpuBackend::Native);
        assert!(
            !flags.iter().any(|f| f.contains("swiftshader")),
            "Native must not carry SwiftShader flags, got: {flags:?}"
        );
        assert!(flags.iter().any(|f| f.starts_with("--use-angle=")));
    }

    #[test]
    fn native_profile_gains_angle_flags_only_when_backend_selected() {
        // The Native *profile* emits no GPU flags today; selecting a backend
        // is what adds them, for every non-Off profile kind.
        let default = flags_for_profile(ProfileKind::Native, false, GpuBackend::Disabled);
        assert!(!default.iter().any(|f| f.starts_with("--use-angle=")));

        let native_gpu = flags_for_profile(ProfileKind::Native, false, GpuBackend::Native);
        assert!(native_gpu.iter().any(|f| f.starts_with("--use-angle=")));
    }

    #[test]
    fn off_profile_stays_empty_under_every_backend() {
        // `off()` is documented as a truly stock launch and its doctest
        // asserts `build_flags().is_empty()`. A GPU backend must not break it.
        for backend in [
            GpuBackend::Disabled,
            GpuBackend::SwiftShader,
            GpuBackend::Native,
        ] {
            assert!(
                flags_for_profile(ProfileKind::Off, false, backend).is_empty(),
                "Off profile must stay stock under {backend:?}"
            );
        }
    }
}
