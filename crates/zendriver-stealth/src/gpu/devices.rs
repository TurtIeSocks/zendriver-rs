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

use crate::gpu::types::Tier;

/// One device's identity. Only what genuinely varies per device lives here;
/// the capability values come from the device's [`Tier`].
///
/// `device_for_renderer` / `DEFAULT_RENDERER` are not wired into a patch yet
/// (that starts with the persona/fingerprint wiring, a later task), so the
/// struct and its lookup are currently reachable only from this module's own
/// tests.
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

/// Renderer assumed when a persona pins no WebGL renderer of its own.
///
/// The Apple Metal row rather than the software one: a persona that says
/// nothing should look like ordinary hardware, and SwiftShader's renderer
/// string is itself a bot signal.
#[allow(dead_code)] // consumed starting with the persona/fingerprint wiring (a later task)
pub(crate) const DEFAULT_RENDERER: &str =
    "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)";

/// Row used when a renderer matches no shipped tier.
///
/// Deliberately the hardware row, not the software one: a persona that
/// pins nothing should look like ordinary hardware, and SwiftShader's
/// renderer string is itself a bot signal. Callers must reach for this
/// explicitly, because using it for a renderer that named a *different*
/// vendor means serving that vendor's name above Apple's capability
/// values — incoherent, and only acceptable until that vendor's tier is
/// actually captured.
#[allow(dead_code)] // consumed starting with the persona/fingerprint wiring (a later task)
pub(crate) const FALLBACK_DEVICE: DeviceRow = DEVICES[1];

/// Pick the device row a renderer string belongs to, by explicit
/// [`DeviceRow::match_token`] — never by a key derived from a row's own
/// reference string.
///
/// Only two tiers ship today (SwiftShader and Apple Metal), so `None` is
/// common and expected for D3D11-family renderers (Intel/NVIDIA/AMD) that
/// have no captured tier yet. Callers that need a value regardless should
/// reach for [`FALLBACK_DEVICE`] explicitly rather than have one guessed
/// here. Adding a tier means capturing it on that hardware, not inventing
/// values for an existing row.
#[allow(dead_code)] // consumed starting with the persona/fingerprint wiring (a later task)
pub(crate) fn device_for_renderer(renderer: &str) -> Option<DeviceRow> {
    let r = renderer.to_ascii_lowercase();
    DEVICES.iter().copied().find(|d| r.contains(d.match_token))
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
