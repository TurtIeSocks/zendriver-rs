//! Device rows: tie a WebGL renderer string to a capability [`Tier`] and to a
//! coherent WebGPU adapter, so one renderer drives both surfaces.
//!
//! `adapter_for_renderer` derives a WebGPU adapter (vendor + architecture)
//! from the spoofed WebGL renderer string. Dataset-mapped, deterministic —
//! NEVER randomized (WAFs hash the WebGPU fingerprint; a random/unknown value
//! reads as a bot).
//!
//! Architecture tokens come from Dawn's `gpu_info.json`, normalized
//! (lowercase, spaces→hyphens) — the scheme Chrome's WebGPU backend uses for
//! `GPUAdapterInfo.architecture`. Validated: Apple M4 Pro → "metal-3";
//! NVIDIA Turing → "turing" (MDN). Only confident model→µarch mappings emit a
//! token; unrecognized models get "" — Chrome legitimately returns "" for
//! unclassified GPUs, so empty is coherent and safe. A WRONG token reads as an
//! unknown device to a fingerprinting WAF.

use crate::Platform;
use crate::gpu::types::Tier;

/// One device's identity. Only what genuinely varies per device lives here;
/// the capability values come from the device's [`Tier`].
///
/// `unmasked_renderer` is read both by this module's tests and by
/// [`default_renderer`], which is what `push_webgl` falls back to when the
/// caller pins no renderer of its own. The two `webgpu_*` fields are reference
/// data only: `push_webgpu` derives its adapter through
/// [`adapter_for_renderer`] rather than from a row.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceRow {
    pub unmasked_vendor: &'static str,
    pub unmasked_renderer: &'static str,
    pub tier: Tier,
    pub webgpu_vendor: &'static str,
    pub webgpu_architecture: &'static str,
    /// Lowercase substring that identifies this device in a renderer string.
    /// Explicit and independent of `unmasked_renderer` / `adapter_for_renderer`
    /// — matching must never be derived from the row's own reference string.
    pub match_token: &'static str,
}

/// Known devices, keyed by [`DeviceRow::match_token`].
const DEVICES: &[DeviceRow] = &[
    DeviceRow {
        unmasked_vendor: "Google Inc. (Google)",
        unmasked_renderer: "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (LLVM 10.0.0) (0x0000C0DE)), SwiftShader driver)",
        tier: Tier::SwiftShader,
        webgpu_vendor: "",
        webgpu_architecture: "",
        match_token: "swiftshader",
    },
    DeviceRow {
        unmasked_vendor: "Google Inc. (Apple)",
        unmasked_renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)",
        tier: Tier::MetalAppleFamily3,
        webgpu_vendor: "apple",
        webgpu_architecture: "metal-3",
        match_token: "apple",
    },
];

/// The device row assumed for a platform, used both when a persona pins no
/// WebGL renderer at all and when the renderer it pins matches no shipped
/// tier.
///
/// **The rule: pick a row whose renderer string real Chrome could report on
/// that platform.** A renderer string is read beside `navigator.platform`, and
/// the two have to be a pair Chrome can produce. Choosing the string needs no
/// new hardware capture — only choosing new *capability values* would.
///
/// - macOS gets the Apple Metal row, which is both coherent and real hardware.
/// - Every other platform gets the SwiftShader row. Chrome's software
///   rasterizer runs on every OS and its renderer string names no
///   platform-specific API, so the pairing is one real Chrome does produce
///   (a GPU-blocklisted machine, a VM, a headless container), and the
///   capability values served beside it are genuinely SwiftShader's.
///
/// The rejected alternative was a D3D11 Intel string, which is what shipped
/// before the tier tables existed. It looks like ordinary hardware, but no
/// captured tier matches it: [`device_for_renderer`] returns `None`, so an
/// Intel name would be served above *Apple Metal's* numbers. That is a
/// plausible-looking mismatch rather than a coherent identity — and the
/// previous default was worse still, reporting the Apple Metal string itself
/// under a Win32 or Linux `navigator.platform`, a pair Chrome cannot produce
/// and which [`platform_skew`](crate::gpu::invariants::platform_skew) flagged
/// on every launch.
///
/// The cost is honest and worth naming: SwiftShader means "this machine has no
/// usable GPU", which some fingerprinters weight on its own. That is a real
/// configuration rather than an impossible one, so it is the better trade —
/// but the actual fix is capturing a D3D11 tier on Windows hardware, after
/// which this should return that row for `Win32`. See the `capture-gpu-tier`
/// skill.
pub(crate) fn default_device(platform: Platform) -> DeviceRow {
    match platform {
        Platform::MacIntel => METAL_APPLE,
        Platform::Win32 | Platform::LinuxX86_64 => SWIFTSHADER,
    }
}

/// Renderer string assumed for a platform. See [`default_device`].
pub(crate) fn default_renderer(platform: Platform) -> &'static str {
    default_device(platform).unmasked_renderer
}

/// Named row aliases, so the platform mapping above reads as intent rather
/// than as an index into [`DEVICES`].
const SWIFTSHADER: DeviceRow = DEVICES[0];
const METAL_APPLE: DeviceRow = DEVICES[1];

/// Pick the device row a renderer string belongs to, by explicit
/// [`DeviceRow::match_token`] — never by a key derived from a row's own
/// reference string.
///
/// Only two tiers ship today (SwiftShader and Apple Metal), so `None` is
/// common and expected for D3D11-family renderers (Intel/NVIDIA/AMD) that
/// have no captured tier yet. Callers that need a value regardless should
/// reach for [`default_device`] explicitly rather than have one guessed
/// here. Adding a tier means capturing it on that hardware, not inventing
/// values for an existing row.
pub(crate) fn device_for_renderer(renderer: &str) -> Option<DeviceRow> {
    let r = renderer.to_ascii_lowercase();
    DEVICES.iter().copied().find(|d| r.contains(d.match_token))
}

/// Extensions that may be claimed even where the backend lacks them, because
/// a synthesized stub is indistinguishable from the real object.
///
/// The bar is narrow, and both halves of it matter: the extension object must
/// carry constants **and nothing else in the API may consume them**. An
/// extension whose constants feed some other call is not inert, because the
/// stub satisfies `getExtension` while that other call still fails — a
/// contradiction the page reaches in one line.
///
/// [`WEBGL_debug_renderer_info`] qualifies in the strict sense that matters
/// here: its two constants are consumed by `getParameter`, and the patch
/// serves both of them from the tier profile, so the stub is backed all the
/// way through.
///
/// `EXT_texture_filter_anisotropic` deliberately does **not** qualify, and was
/// removed after being listed here: its constants are consumed by
/// `getParameter` **and** `texParameterf`, and neither is table-served. On a
/// backend without it, the claimed list named it, `getExtension` handed over a
/// stub, and `getParameter(ext.MAX_TEXTURE_MAX_ANISOTROPY_EXT)` answered
/// `null` with `INVALID_ENUM` — exactly the contradiction this rule exists to
/// prevent. It is now claimed only where the backend really provides it, like
/// every other functional extension.
///
/// [`WEBGL_debug_renderer_info`]: https://registry.khronos.org/webgl/extensions/WEBGL_debug_renderer_info/
pub(crate) fn inert_stubs() -> serde_json::Value {
    serde_json::json!({
        "WEBGL_debug_renderer_info": {
            "UNMASKED_VENDOR_WEBGL": 37445,
            "UNMASKED_RENDERER_WEBGL": 37446
        }
    })
}

/// vendor + architecture for the spoofed WebGPU adapter. `device` and
/// `description` are always emitted empty by the patch (Chrome masks them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuAdapterInfo {
    pub vendor: String,
    pub architecture: String,
}

/// Map a WebGL `UNMASKED_RENDERER` string to a coherent WebGPU adapter
/// (vendor + architecture). Architecture tokens are Dawn's `gpu_info.json`
/// names normalized (lowercase, spaces→hyphens) — the scheme Chrome's WebGPU
/// backend uses for `GPUAdapterInfo.architecture` (validated: Apple M4 Pro →
/// "metal-3"; NVIDIA Turing → "turing" per MDN). Only confident model→µarch
/// mappings emit a token; unrecognized models get "" (Chrome legitimately
/// returns "" for unclassified GPUs, so empty is coherent and safe — a WRONG
/// token reads as an unknown device to a fingerprinting WAF). Add families
/// here as they're confirmed.
pub(crate) fn adapter_for_renderer(renderer: &str) -> GpuAdapterInfo {
    let r = renderer.to_ascii_lowercase();

    if r.contains("nvidia") || r.contains("geforce") || r.contains("rtx") || r.contains("gtx") {
        let arch = if r.contains("rtx 50") || r.contains("rtx50") {
            "blackwell"
        } else if r.contains("rtx 40") || r.contains("rtx40") {
            "lovelace"
        } else if r.contains("rtx 30") || r.contains("rtx30") {
            "ampere"
        } else if r.contains("rtx 20")
            || r.contains("rtx20")
            || r.contains("rtx 16")
            || r.contains("gtx 16")
            || r.contains("gtx16")
        {
            "turing"
        } else if r.contains("gtx 10") || r.contains("gtx10") {
            "pascal"
        } else if r.contains("titan v") {
            "volta"
        } else {
            ""
        };
        return GpuAdapterInfo {
            vendor: "nvidia".into(),
            architecture: arch.into(),
        };
    }

    if r.contains("amd") || r.contains("radeon") {
        let arch = if r.contains("rx 9") || r.contains("rx9") {
            "rdna-4"
        } else if r.contains("rx 7") || r.contains("rx7") {
            "rdna-3"
        } else if r.contains("rx 6") || r.contains("rx6") {
            "rdna-2"
        } else if r.contains("rx 5700") || r.contains("rx 5600") || r.contains("rx 5500") {
            "rdna-1"
        } else {
            ""
        };
        return GpuAdapterInfo {
            vendor: "amd".into(),
            architecture: arch.into(),
        };
    }

    if r.contains("apple") {
        return GpuAdapterInfo {
            vendor: "apple".into(),
            architecture: "metal-3".into(),
        };
    }

    // Intel + everything unrecognized → Intel (the common integrated default).
    let arch = if r.contains("iris xe") || r.contains("xe graphics") {
        "gen-12-lp"
    } else if r.contains("uhd graphics 6")
        || r.contains("hd graphics 6")
        || r.contains("uhd graphics 5")
        || r.contains("hd graphics 5")
    {
        "gen-9"
    } else {
        ""
    };
    GpuAdapterInfo {
        vendor: "intel".into(),
        architecture: arch.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_rtx_generations() {
        assert_eq!(
            adapter_for_renderer(
                "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 Direct3D11 vs_5_0 ps_5_0, D3D11)"
            )
            .architecture,
            "lovelace"
        );
        assert_eq!(
            adapter_for_renderer("NVIDIA GeForce RTX 3080").architecture,
            "ampere"
        );
        assert_eq!(
            adapter_for_renderer("NVIDIA GeForce RTX 2070 SUPER").architecture,
            "turing"
        );
        assert_eq!(
            adapter_for_renderer("NVIDIA GeForce GTX 1080 Ti").architecture,
            "pascal"
        );
        let a = adapter_for_renderer("NVIDIA GeForce RTX 4090");
        assert_eq!(a.vendor, "nvidia");
    }

    #[test]
    fn nvidia_unknown_generation_is_empty() {
        // Vendor still nvidia, but an unrecognized model → empty arch (safe).
        assert_eq!(adapter_for_renderer("NVIDIA Quadro K2200").architecture, "");
    }

    #[test]
    fn amd_rx_generations() {
        assert_eq!(
            adapter_for_renderer("ANGLE (AMD, AMD Radeon RX 7900 XT, D3D11)").architecture,
            "rdna-3"
        );
        assert_eq!(
            adapter_for_renderer("AMD Radeon RX 6800 XT").architecture,
            "rdna-2"
        );
        assert_eq!(
            adapter_for_renderer("AMD Radeon RX 5700 XT").architecture,
            "rdna-1"
        );
        assert_eq!(adapter_for_renderer("AMD Radeon RX 7900 XT").vendor, "amd");
    }

    #[test]
    fn apple_is_metal3() {
        let a = adapter_for_renderer(
            "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)",
        );
        assert_eq!(a.vendor, "apple");
        assert_eq!(a.architecture, "metal-3");
    }

    #[test]
    fn intel_integrated_generations() {
        assert_eq!(
            adapter_for_renderer(
                "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)"
            )
            .architecture,
            "gen-9"
        );
        assert_eq!(
            adapter_for_renderer("ANGLE (Intel, Intel(R) Iris(R) Xe Graphics, D3D11)").architecture,
            "gen-12-lp"
        );
        assert_eq!(
            adapter_for_renderer("Intel(R) UHD Graphics 630").vendor,
            "intel"
        );
    }

    #[test]
    fn unknown_renderer_falls_back_to_intel_empty_arch() {
        let a = adapter_for_renderer("Mesa OffScreen");
        assert_eq!(a.vendor, "intel");
        assert_eq!(a.architecture, "");
    }

    #[test]
    fn a_known_renderer_selects_its_tier() {
        let d = device_for_renderer(
            "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)",
        )
        .expect("apple renderer must match the metal row");
        assert_eq!(d.tier, Tier::MetalAppleFamily3);
        assert_eq!(d.webgpu_vendor, "apple");
    }

    #[test]
    fn a_software_renderer_selects_the_swiftshader_tier() {
        let d = device_for_renderer(
            "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (LLVM 10.0.0) (0x0000C0DE)), SwiftShader driver)",
        )
        .expect("swiftshader renderer must match the software row");
        assert_eq!(d.tier, Tier::SwiftShader);
    }

    #[test]
    fn an_unknown_renderer_returns_none() {
        // Unknown hardware matches no row; guessing a tier would be more
        // detectable than admitting there is none.
        assert_eq!(device_for_renderer("Some Unreleased GPU"), None);
    }

    #[test]
    fn a_hardware_renderer_never_selects_the_software_tier() {
        // The bug this replaces: the SwiftShader row's match key was derived
        // from its own reference string, which fell through to "intel", so
        // every Intel renderer selected the software tier.
        let intel = "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)";
        assert_ne!(
            device_for_renderer(intel).map(|d| d.tier),
            Some(Tier::SwiftShader),
            "a real Intel GPU must never resolve to the software rasterizer's values"
        );
    }

    #[test]
    fn the_default_renderer_is_coherent_with_every_platform() {
        // A renderer string is read beside navigator.platform, so the default
        // has to be a pair Chrome can produce. The regression this guards: the
        // default was the Apple Metal string unconditionally, so a Win32 or
        // Linux persona reported an Apple GPU.
        for platform in [Platform::Win32, Platform::MacIntel, Platform::LinuxX86_64] {
            let row = default_device(platform);
            // Whatever is chosen must be a shipped row, so its renderer name
            // and its capability values come from the same backend.
            assert!(
                DEVICES.contains(&row),
                "{platform:?} defaults to a row that is not in DEVICES"
            );
            assert_eq!(
                device_for_renderer(row.unmasked_renderer).map(|d| d.tier),
                Some(row.tier),
                "{platform:?}'s default renderer must resolve back to its own tier"
            );
            // An Apple renderer is only coherent on macOS.
            if platform != Platform::MacIntel {
                assert!(
                    !row.unmasked_renderer.contains("Apple"),
                    "{platform:?} must not default to an Apple renderer: {}",
                    row.unmasked_renderer
                );
            }
            // And the default must never trip the skew check, which is what
            // fired on every non-Mac launch before.
            assert_eq!(
                crate::gpu::invariants::platform_skew(platform, row.tier),
                None,
                "{platform:?}'s default tier must not skew against it"
            );
        }
        assert_eq!(
            default_device(Platform::MacIntel).tier,
            Tier::MetalAppleFamily3
        );
    }

    #[test]
    fn only_extensions_nothing_else_consumes_are_claimed_unconditionally() {
        let stubs = inert_stubs();
        let stubs = stubs.as_object().expect("inert stubs is an object");
        // The unmasked pair is served from the tier profile, so the stub is
        // backed all the way through.
        assert!(stubs.contains_key("WEBGL_debug_renderer_info"));
        // Not inert: getParameter and texParameterf both consume its
        // constants, and neither is table-served. Claiming it on a backend
        // that lacks it yields getExtension -> stub but
        // getParameter(MAX_TEXTURE_MAX_ANISOTROPY_EXT) -> null + INVALID_ENUM.
        assert!(
            !stubs.contains_key("EXT_texture_filter_anisotropic"),
            "EXT_texture_filter_anisotropic is not inert; it must be claimed only where the \
             backend really provides it"
        );
    }

    #[test]
    fn renderers_with_no_shipped_tier_return_none() {
        // Only SwiftShader and Apple Metal tiers ship. A D3D11-family renderer
        // has no measured tier, and guessing one would pair its name with
        // another backend's numbers.
        for r in [
            "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)",
            "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 Direct3D11 vs_5_0 ps_5_0, D3D11)",
            "ANGLE (AMD, AMD Radeon RX 7900 XT, D3D11)",
        ] {
            assert_eq!(device_for_renderer(r), None, "unexpected tier for {r}");
        }
    }
}
