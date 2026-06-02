//! Derive a coherent WebGPU adapter (vendor + architecture) from the spoofed
//! WebGL renderer string. Dataset-mapped, deterministic — NEVER randomized
//! (WAFs hash the WebGPU fingerprint; a random/unknown value reads as a bot).
//!
//! Validated against native Chrome (Apple M4 Pro, 2026-06): `requestAdapter().info`
//! = `{ vendor: "apple", architecture: "metal-3", device: "", description: "" }`.
//! Chrome MASKS `device` + `description` to "" (the patch emits them empty);
//! `vendor` + `architecture` are exposed. `vendor` is the load-bearing #20
//! coherence field (must agree with the WebGL renderer). `architecture` is
//! emitted only where validated against real Chrome (Apple → "metal-3");
//! unvalidated vendors get "" — emitting a WRONG token is a worse tell than
//! an empty one. Add NVIDIA/AMD/Intel tokens here as they are confirmed on
//! real hardware.

/// vendor + architecture for the spoofed WebGPU adapter. `device` and
/// `description` are always emitted empty by the patch (Chrome masks them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuAdapterInfo {
    pub vendor: String,
    pub architecture: String,
}

/// Map a WebGL `UNMASKED_RENDERER` string to a coherent WebGPU adapter.
/// Unrecognized renderers fall back to Intel (the common integrated default).
pub(crate) fn adapter_for_renderer(renderer: &str) -> GpuAdapterInfo {
    let r = renderer.to_ascii_lowercase();
    let (vendor, architecture) = if r.contains("nvidia")
        || r.contains("geforce")
        || r.contains("rtx")
        || r.contains("gtx")
    {
        ("nvidia", "")
    } else if r.contains("amd") || r.contains("radeon") {
        ("amd", "")
    } else if r.contains("apple") {
        ("apple", "metal-3")
    } else {
        ("intel", "")
    };
    GpuAdapterInfo {
        vendor: vendor.into(),
        architecture: architecture.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_vendor_arch_empty_until_validated() {
        let a = adapter_for_renderer(
            "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 Direct3D11 vs_5_0 ps_5_0, D3D11)",
        );
        assert_eq!(a.vendor, "nvidia");
        assert_eq!(a.architecture, "");
    }
    #[test]
    fn amd_vendor_arch_empty_until_validated() {
        let a = adapter_for_renderer(
            "ANGLE (AMD, AMD Radeon RX 7900 XT Direct3D11 vs_5_0 ps_5_0, D3D11)",
        );
        assert_eq!(a.vendor, "amd");
        assert_eq!(a.architecture, "");
    }
    #[test]
    fn apple_arch_is_metal3_validated_against_real_chrome() {
        let a = adapter_for_renderer(
            "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)",
        );
        assert_eq!(a.vendor, "apple");
        assert_eq!(a.architecture, "metal-3");
    }
    #[test]
    fn intel_vendor_arch_empty() {
        let a = adapter_for_renderer(
            "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)",
        );
        assert_eq!(a.vendor, "intel");
        assert_eq!(a.architecture, "");
    }
    #[test]
    fn unknown_renderer_falls_back_to_intel() {
        let a = adapter_for_renderer("Mesa OffScreen");
        assert_eq!(a.vendor, "intel");
        assert_eq!(a.architecture, "");
    }
}
