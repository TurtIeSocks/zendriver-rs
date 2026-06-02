//! Derive a coherent WebGPU `GPUAdapterInfo` from the spoofed WebGL renderer
//! string. Dataset-mapped, deterministic — NEVER randomized (DataDome and
//! other WAFs hash the WebGPU fingerprint and compare against a device dataset;
//! a random adapter reads as an unknown device).

/// Minimal `GPUAdapterInfo` triple the patch substitutes into `navigator.gpu`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuAdapterInfo {
    pub vendor: String,
    pub architecture: String,
    pub description: String,
}

/// Map a WebGL `UNMASKED_RENDERER` string to a coherent adapter. Falls back to
/// a stable Intel integrated-GPU adapter for unrecognized renderers.
// NOTE: `vendor` is the load-bearing coherence field for the #20 fix (it must
// agree with the WebGL renderer's vendor). The `architecture` tokens
// (ada-lovelace / ampere / turing / rdna-3 / gen-12lp / common-3) are
// best-effort, dataset-style values; their exact match to what Chrome's Dawn
// backend reports is validated by the nightly `webgpu_adapter_coheres_with_webgl_renderer`
// integration test (Phase 9) and may be refined in a fast-follow. They are
// never randomized — a random value reads as an unknown device to a WAF.
pub(crate) fn adapter_for_renderer(renderer: &str) -> GpuAdapterInfo {
    let r = renderer.to_ascii_lowercase();
    // Order matters: check discrete vendors before the Intel fallback.
    if r.contains("nvidia") || r.contains("geforce") || r.contains("rtx") || r.contains("gtx") {
        let arch = if r.contains("rtx 40") || r.contains("ada") {
            "ada-lovelace"
        } else if r.contains("rtx 30") {
            "ampere"
        } else if r.contains("rtx 20") {
            "turing"
        } else {
            "ampere"
        };
        return GpuAdapterInfo {
            vendor: "nvidia".into(),
            architecture: arch.into(),
            description: extract_model(renderer, "NVIDIA"),
        };
    }
    if r.contains("amd") || r.contains("radeon") {
        return GpuAdapterInfo {
            vendor: "amd".into(),
            architecture: "rdna-3".into(),
            description: extract_model(renderer, "AMD"),
        };
    }
    if r.contains("apple") {
        return GpuAdapterInfo {
            vendor: "apple".into(),
            architecture: "common-3".into(),
            description: extract_model(renderer, "Apple"),
        };
    }
    // Intel + everything unrecognized → coherent Intel integrated default.
    GpuAdapterInfo {
        vendor: "intel".into(),
        architecture: "gen-12lp".into(),
        description: if r.contains("intel") {
            extract_model(renderer, "Intel")
        } else {
            "Intel(R) UHD Graphics 630".into()
        },
    }
}

/// Pull a human-readable model substring out of the ANGLE renderer string,
/// or fall back to a vendor-generic description.
fn extract_model(renderer: &str, vendor: &str) -> String {
    // ANGLE strings look like "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 Direct3D11 ...)".
    // Take the middle segment after the first comma, trimmed of the D3D suffix.
    if let Some(inner) = renderer.split('(').nth(1).and_then(|s| s.split(')').next()) {
        if let Some(mid) = inner.split(',').nth(1) {
            let model = mid
                .split(" Direct3D")
                .next()
                .unwrap_or(mid)
                .split(" vs_")
                .next()
                .unwrap_or(mid)
                .trim();
            if !model.is_empty() {
                return model.to_string();
            }
        }
    }
    format!("{vendor} Graphics")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_renderer_maps_to_nvidia_adapter() {
        let a = adapter_for_renderer(
            "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 Direct3D11 vs_5_0 ps_5_0, D3D11)",
        );
        assert_eq!(a.vendor, "nvidia");
        assert_eq!(a.architecture, "ada-lovelace");
        assert!(a.description.contains("RTX 4090"));
    }

    #[test]
    fn intel_renderer_maps_to_intel_adapter() {
        let a = adapter_for_renderer(
            "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)",
        );
        assert_eq!(a.vendor, "intel");
        assert_eq!(a.architecture, "gen-12lp");
    }

    #[test]
    fn unknown_renderer_falls_back_to_coherent_intel() {
        let a = adapter_for_renderer("Mesa OffScreen");
        // Never panics, never randomizes: a stable coherent default.
        assert_eq!(a.vendor, "intel");
    }
}
