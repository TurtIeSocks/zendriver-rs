//! Derive a coherent WebGPU adapter (vendor + architecture) from the spoofed
//! WebGL renderer string. Dataset-mapped, deterministic — NEVER randomized
//! (WAFs hash the WebGPU fingerprint; a random/unknown value reads as a bot).
//!
//! Architecture tokens come from Dawn's `gpu_info.json`, normalized
//! (lowercase, spaces→hyphens) — the scheme Chrome's WebGPU backend uses for
//! `GPUAdapterInfo.architecture`. Validated: Apple M4 Pro → "metal-3";
//! NVIDIA Turing → "turing" (MDN). Only confident model→µarch mappings emit a
//! token; unrecognized models get "" — Chrome legitimately returns "" for
//! unclassified GPUs, so empty is coherent and safe. A WRONG token reads as an
//! unknown device to a fingerprinting WAF.

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
}
