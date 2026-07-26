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
/// [`adapter_for_renderer`] rather than from a row. They are still the row's
/// declaration of what that device's adapter *is*, so the tests assert the
/// derivation agrees with them — the two must never describe different GPUs.
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
    /// Extra lowercase substring that must *also* be present, or `None` when
    /// `match_token` alone decides the row.
    ///
    /// Exists because one row needs two facts that are not adjacent in the
    /// string. ANGLE puts the vendor first and the backend tag last —
    /// `ANGLE (NVIDIA, … Direct3D11 …, D3D11)` — and the NVIDIA D3D11 tier is
    /// selected by exactly that pair, so no single contiguous substring can
    /// express it. Splitting the D3D11 rows on `Some("nvidia")` also keeps the
    /// vendor test out of `device_for_renderer`, which stays a table sweep.
    pub vendor_token: Option<&'static str>,
}

/// Known devices, keyed by [`DeviceRow::match_token`].
///
/// Order is load-bearing: [`device_for_renderer`] takes the *first* row whose
/// tokens the renderer matches, so a specific row must precede the general one
/// it refines. That holds twice here — the Subzero SwiftShader row before the
/// generic one, and the NVIDIA D3D11 row before the generic D3D11 row. The
/// second pair matters more: the SwiftShader rows share a tier, so their order
/// decides only an identity string, while the D3D11 rows carry *different
/// tiers* and getting them backwards serves the wrong capability values.
///
/// The two SwiftShader rows are one capability tier under two identity
/// strings. SwiftShader picks its JIT backend at build time — Subzero on
/// Linux, LLVM on macOS — and Chrome prints the chosen one inside the
/// renderer string, so the string is platform-specific even though the
/// capabilities behind it are not (measured; see [`default_device`]).
const DEVICES: &[DeviceRow] = &[
    DeviceRow {
        unmasked_vendor: "Google Inc. (Google)",
        unmasked_renderer: "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)",
        tier: Tier::SwiftShader,
        webgpu_vendor: "",
        webgpu_architecture: "",
        // Specific enough to beat the generic row below, which is why it is
        // listed first.
        match_token: "subzero",
        vendor_token: None,
    },
    DeviceRow {
        unmasked_vendor: "Google Inc. (Google)",
        unmasked_renderer: "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (LLVM 10.0.0) (0x0000C0DE)), SwiftShader driver)",
        tier: Tier::SwiftShader,
        webgpu_vendor: "",
        webgpu_architecture: "",
        // The catch-all for the tier: any SwiftShader string that is not
        // Subzero's lands here. Both rows carry the same tier, so a miss costs
        // only the identity string, never the capability values.
        match_token: "swiftshader",
        vendor_token: None,
    },
    DeviceRow {
        unmasked_vendor: "Google Inc. (Apple)",
        unmasked_renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)",
        tier: Tier::MetalMacos,
        webgpu_vendor: "apple",
        webgpu_architecture: "metal-3",
        match_token: "apple",
        vendor_token: None,
    },
    // The two D3D11 rows are one backend and feature level under two capability
    // tiers, and the NVIDIA row must be listed first because its token is a
    // strict refinement of the generic one below.
    //
    // Splitting them is measured, not defensive. An RTX 4090 and an AMD Radeon
    // probed on the same machine and the same Chrome build differ in
    // `MAX_VERTEX_UNIFORM_VECTORS` (4095 against 4096) and in the two values
    // derived from it. ANGLE's cause is a vendor-conditional workaround —
    // `ANGLE_FEATURE_CONDITION(features, skipVSConstantRegisterZero, isNvidia)`,
    // which then does `caps->maxVertexUniformVectors -= 1` — so the condition
    // is the vendor and nothing else. See [`Tier::D3d11Fl11Nvidia`].
    DeviceRow {
        unmasked_vendor: "Google Inc. (NVIDIA)",
        unmasked_renderer: "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 (0x00002684) Direct3D11 vs_5_0 ps_5_0, D3D11)",
        tier: Tier::D3d11Fl11Nvidia,
        webgpu_vendor: "nvidia",
        webgpu_architecture: "lovelace",
        match_token: "d3d11",
        vendor_token: Some("nvidia"),
    },
    DeviceRow {
        unmasked_vendor: "Google Inc. (AMD)",
        unmasked_renderer: "ANGLE (AMD, AMD Radeon(TM) Graphics (0x0000164E) Direct3D11 vs_5_0 ps_5_0, D3D11)",
        tier: Tier::D3d11Fl11,
        webgpu_vendor: "amd",
        webgpu_architecture: "rdna-2",
        // ANGLE's D3D11 renderer string always ends in the backend tag
        // ", D3D11)", whatever the card — which is exactly the granularity the
        // tier has. `direct3d11` would be wrong twice over: it misses the AMD
        // form ("ANGLE (AMD, AMD Radeon RX 7900 XT, D3D11)"), which names no
        // shader model, and it would tie a backend-and-feature-level tier to a
        // spelling only some vendors emit.
        //
        // No vendor token: this row is every non-NVIDIA D3D11 device, AMD and
        // Intel alike, which is what `skipVSConstantRegisterZero` being keyed
        // on `isNvidia` means.
        match_token: "d3d11",
        vendor_token: None,
    },
    DeviceRow {
        unmasked_vendor: "Google Inc. (Intel)",
        unmasked_renderer: "ANGLE (Intel, Vulkan 1.4.318 (Intel(R) Iris(R) Pro Graphics 580 (SKL GT4) (0x0000193B)), Intel open-source Mesa driver)",
        tier: Tier::VulkanMesaIntelIrisPro580,
        // The probed machine has no WebGPU adapter at all (Chrome does not
        // enable WebGPU by default on Linux), so the tier serves no limits and
        // no features. What this pair still declares is the adapter *identity*
        // `adapter_for_renderer` derives from the string above, which is what
        // decorates whatever adapter a host does resolve: an Intel renderer
        // must not be paired with any other vendor's name. The architecture
        // stays empty because nothing measured this part's Dawn token — and
        // Chrome legitimately answers "" for a device it does not classify,
        // where a wrong token reads as an unknown device.
        webgpu_vendor: "intel",
        webgpu_architecture: "",
        // Names the *device as Mesa spells it*, not the backend, because a
        // Vulkan tier's numbers are read off one physical device under one
        // driver (see [`Tier::VulkanMesaIntelIrisPro580`]). `"vulkan"` would be
        // wrong twice over: SwiftShader's own renderer string carries
        // `Vulkan 1.3.0`, and every other Linux GPU would then be served this
        // Iris Pro's device-derived limits. `"(SKL GT4)"` is Mesa's naming, so
        // the same card on Windows — `Intel(R) Iris(R) Pro Graphics 580
        // Direct3D11 vs_5_0 ps_5_0, D3D11` — does not match this row and keeps
        // resolving the D3D11 tier it belongs to.
        match_token: "iris(r) pro graphics 580 (skl gt4)",
        vendor_token: None,
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
/// - Windows gets the D3D11 row. ANGLE's D3D11 backend is what Chrome uses on
///   Windows by default, so the renderer string, `navigator.platform`, and the
///   capability values beside them all describe one ordinary Windows machine.
/// - Linux gets the Intel Iris Pro 580 Vulkan row. ANGLE's Vulkan backend is
///   what Chrome uses on Linux, so this is real hardware under the API real
///   Linux Chrome runs — the same standing macOS and Windows already had.
///   Linux used to get a SwiftShader row instead, which was honest but said
///   "this machine has no usable GPU", something some fingerprinters weight on
///   its own; capturing the tier is what retired that last fallback. What the
///   Vulkan row cannot claim is generality: its numbers are this Iris Pro's
///   under this Mesa build, not Linux's or Vulkan's (see
///   [`Tier::VulkanMesaIntelIrisPro580`]).
///
/// **Which SwiftShader row, and why there are two.** SwiftShader's
/// *capability values* are platform-independent; its *renderer string* is
/// not. Measured on 2026-07-25: the same flag set probed on Ubuntu 24
/// (Chrome 150.0.7871.114, GPU-less VM) reproduced the macOS capture exactly
/// — 0 of 82 WebGL1 parameters differed, 0 of 130 WebGL2, extension lists
/// identical in content and order, all 12 precision entries identical — while
/// the renderer string differed in one token, because SwiftShader selects its
/// JIT backend at build time and Chrome prints the choice:
///
/// - Linux: `SwiftShader Device (Subzero)`
/// - Windows: `SwiftShader Device (Subzero)`
/// - macOS: `SwiftShader Device (LLVM 10.0.0)`
///
/// That is one capability tier and two identity rows, which is why the split
/// lives in [`DEVICES`] rather than in [`Tier`], and why it needed no capture
/// and no table regeneration. Neither row decides a *default* any more, now
/// that Linux resolves the captured Vulkan row above — but Subzero is still
/// what a Linux persona lands on when it pins a SwiftShader renderer itself,
/// so it has to stay the build real Linux Chrome prints.
///
/// Windows' entry there was previously **inferred** from the Linux
/// measurement; it is now measured too. `cargo run -p zendriver --example
/// probe_gpu -- swiftshader` on Windows 10.0.21996 (Chrome 150.0.7871.186)
/// prints `SwiftShader Device (Subzero)`, confirming the guess. It no longer
/// decides a *default* — `Win32` resolves the D3D11 row above — but it is what
/// a Win32 persona gets if it pins a SwiftShader renderer, so the row it lands
/// on still has to be the right build.
///
/// The rejected alternative for Windows was a D3D11 *string* with no D3D11
/// *tier*, which is what shipped before the tier tables existed: it looks like
/// ordinary hardware, but [`device_for_renderer`] returned `None` for it, so an
/// Intel name was served above *Apple Metal's* numbers. Capturing the tier is
/// what makes the D3D11 row honest rather than plausible-looking — the row's
/// name and its numbers now come from the same probe. The earlier default was
/// worse still, reporting the Apple Metal string itself under a Win32 or Linux
/// `navigator.platform`, a pair Chrome cannot produce and which
/// [`platform_skew`](crate::gpu::invariants::platform_skew) flagged on every
/// launch.
pub(crate) fn default_device(platform: Platform) -> DeviceRow {
    match platform {
        Platform::MacIntel => METAL_APPLE,
        Platform::Win32 => NVIDIA_D3D11,
        Platform::LinuxX86_64 => VULKAN_INTEL_IRIS_PRO_580,
    }
}

/// Renderer string assumed for a platform. See [`default_device`].
pub(crate) fn default_renderer(platform: Platform) -> &'static str {
    default_device(platform).unmasked_renderer
}

/// Named row aliases, so the platform mapping above reads as intent rather
/// than as an index into [`DEVICES`].
/// No platform defaults to either SwiftShader row any more: macOS gets Metal,
/// Windows D3D11, and Linux the captured Vulkan row. Both are still reached
/// when a caller pins a SwiftShader string itself — Subzero on Linux and
/// Windows, LLVM on macOS — and are named here so the tests can assert the two
/// never drift apart.
#[allow(dead_code)]
const SWIFTSHADER_SUBZERO: DeviceRow = DEVICES[0];
#[allow(dead_code)]
const SWIFTSHADER_LLVM: DeviceRow = DEVICES[1];
const METAL_APPLE: DeviceRow = DEVICES[2];
const NVIDIA_D3D11: DeviceRow = DEVICES[3];
#[allow(dead_code)]
const AMD_D3D11: DeviceRow = DEVICES[4];
const VULKAN_INTEL_IRIS_PRO_580: DeviceRow = DEVICES[5];

/// Pick the device row a renderer string belongs to, by explicit
/// [`DeviceRow::match_token`] — never by a key derived from a row's own
/// reference string.
///
/// A row matches when the renderer contains its `match_token` and, where it
/// declares one, its [`vendor_token`](DeviceRow::vendor_token) as well.
///
/// First match in [`DEVICES`] order wins, so a specific row must be listed
/// ahead of one it is a special case of. Two pairs depend on that:
///
/// - `"subzero"` before `"swiftshader"`, or every Subzero string would resolve
///   to the LLVM row's identity. Both carry the same tier, so ordering decides
///   the identity string alone — but that string is the whole reason the rows
///   are separate.
/// - The NVIDIA D3D11 row before the generic one. Here ordering decides the
///   **capability values**, not just a name: the two rows carry different
///   tiers, and the generic row's `match_token` alone would swallow every
///   NVIDIA string. Getting this backwards serves an RTX its 4096 instead of
///   its measured 4095, which is one `getParameter` call from being read.
///
/// The remaining tokens (`"apple"` and the Iris Pro's Mesa device name) are
/// disjoint from those and from each other, so their position is not
/// load-bearing.
///
/// Five tiers ship today (SwiftShader, Apple Metal, D3D11 FL11+ in its NVIDIA
/// and non-NVIDIA forms, and the Intel Iris Pro 580 under Mesa), so `None` is
/// still expected for the backends none of them covers — a desktop-GL renderer, a D3D9 string, or **any Vulkan
/// device other than that one Iris Pro**. That last one is not a gap waiting
/// to be filled by widening the token: ANGLE's Vulkan caps come off
/// `VkPhysicalDeviceLimits`, so serving another Linux GPU this row's numbers
/// would be exactly the wrong-backend pairing the tiers exist to prevent (see
/// [`Tier::VulkanMesaIntelIrisPro580`]). A **feature-level-10** D3D11
/// renderer does *not* answer `None`: ANGLE emits the same trailing `, D3D11)`
/// tag at every feature level, so an FL10 string matches the D3D11 row's
/// `match_token` and is served the FL11 numbers silently. Telling the two
/// apart needs an FL10 capture to compare against, which nobody has taken.
/// Callers that need a value
/// regardless should reach for [`default_device`] explicitly rather than have
/// one guessed here. Adding a tier means capturing it on that hardware, not
/// inventing values for an existing row.
pub(crate) fn device_for_renderer(renderer: &str) -> Option<DeviceRow> {
    let r = renderer.to_ascii_lowercase();
    DEVICES.iter().copied().find(|d| {
        r.contains(d.match_token) && d.vendor_token.is_none_or(|vendor| r.contains(vendor))
    })
}

/// The `UNMASKED_VENDOR_WEBGL` string Chrome reports beside an ANGLE renderer.
///
/// Chrome answers `Google Inc. (<X>)`, where `<X>` is the same vendor token
/// ANGLE puts first in its own renderer string — so the pair is *derivable*,
/// and a renderer the caller pinned can be answered by its own vendor instead
/// of by whichever device row happened to match it.
///
/// That distinction is load-bearing now that one row covers many cards. The
/// D3D11 row declares NVIDIA, but its [`DeviceRow::match_token`] deliberately
/// matches every FL11+ D3D11 renderer, Intel and AMD included — because the
/// capability values really are shared. Reading the *vendor* off the row too
/// would serve `Google Inc. (NVIDIA)` beside an Intel renderer, a contradiction
/// a page reads in one line. Measured before this existed: pinning
/// `ANGLE (Intel, Intel(R) UHD Graphics 630 ... D3D11)` served exactly that
/// pair, and silently, since a matched row raises no warning.
///
/// Returns `None` for anything not in ANGLE's format, leaving the caller to
/// fall back to the device row — which is right for the default renderers,
/// since each row's own string derives that row's own declared vendor (asserted
/// in `every_row_derives_its_own_declared_vendor`).
///
/// The token ends at the first comma **or** closing paren, whichever comes
/// first. ANGLE's usual string has both — `ANGLE (Apple, ANGLE Metal ...,
/// Version ...)` — but the one-field form `ANGLE (Google)` has no comma, and
/// splitting on the comma alone carried the paren into the token and answered
/// `Google Inc. (Google))`.
pub(crate) fn vendor_for_renderer(renderer: &str) -> Option<String> {
    let inner = renderer.strip_prefix("ANGLE (")?;
    let end = inner.find([',', ')']).unwrap_or(inner.len());
    let vendor = inner[..end].trim();
    (!vendor.is_empty()).then(|| format!("Google Inc. ({vendor})"))
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
///
/// A software rasterizer answers empty for *both* fields, and is matched before
/// any hardware family so it cannot fall through to the Intel catch-all — see
/// the arm's own comment.
pub(crate) fn adapter_for_renderer(renderer: &str) -> GpuAdapterInfo {
    let r = renderer.to_ascii_lowercase();

    // Software rasterizer: no vendor to report, and it must be answered before
    // the hardware branches so it cannot reach the Intel catch-all below. It
    // did reach it, which is the split this arm closes: a SwiftShader row was
    // Linux's default and Windows' before either tier was captured (see
    // `default_device`), so such a persona served SwiftShader's renderer to
    // WebGL and an *Intel* adapter to WebGPU. A persona that pins a
    // SwiftShader renderer itself still lands here on every platform, which is
    // what keeps the arm load-bearing.
    // Both JIT-backend rows go through it — the token is
    // `swiftshader`, which each renderer string carries.
    // Empty matches what the SwiftShader rows themselves declare, and is what Chrome
    // answers for an adapter it cannot classify — the honest value for a
    // rasterizer that has no hardware behind it at all. (On a real SwiftShader
    // Chrome `requestAdapter()` resolves null and there is no adapter to read;
    // the patch cannot synthesize that, so it decorates whatever adapter the
    // host does resolve with a vendor that claims nothing.)
    if r.contains("swiftshader") {
        return GpuAdapterInfo {
            vendor: String::new(),
            architecture: String::new(),
        };
    }

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
    fn the_software_rasterizer_reports_no_vendor() {
        // A SwiftShader row was Linux's default and Windows' before either
        // platform had a captured tier. No platform defaults to one now, so
        // this is the adapter a persona gets when it pins a SwiftShader
        // renderer itself — on any OS. It used to reach the Intel catch-all:
        // an Intel WebGPU adapter beside a SwiftShader WebGL renderer, a
        // pairing Chrome does not produce. Both JIT-backend rows, since either
        // can be served depending on the renderer resolved.
        for row in [SWIFTSHADER_SUBZERO, SWIFTSHADER_LLVM] {
            let a = adapter_for_renderer(row.unmasked_renderer);
            assert_eq!(
                (a.vendor.as_str(), a.architecture.as_str()),
                (row.webgpu_vendor, row.webgpu_architecture),
                "the derived adapter must match the device row's own declaration"
            );
            assert_eq!(
                a.vendor, "",
                "a software rasterizer has no vendor to report"
            );
        }
    }

    #[test]
    fn every_row_derives_its_own_declared_vendor() {
        // The self-check that makes `vendor_for_renderer` safe as the primary
        // source: for every shipped row, deriving the vendor from that row's
        // own renderer string must reproduce the vendor the row declares. If
        // this holds, replacing the row lookup with the derivation cannot
        // change any default — it only fixes the renderers a row matches but
        // does not describe.
        for row in DEVICES {
            assert_eq!(
                vendor_for_renderer(row.unmasked_renderer).as_deref(),
                Some(row.unmasked_vendor),
                "{} must derive its own declared vendor",
                row.unmasked_renderer
            );
        }
    }

    #[test]
    fn a_d3d11_renderer_reports_its_own_vendor_not_the_rows() {
        // The non-NVIDIA D3D11 row is deliberately shared across vendors — its
        // token matches every FL11+ renderer that is not NVIDIA's, because
        // their capability values really are the same. The identity strings are
        // not shared, so an Intel renderer must carry an Intel vendor even
        // though the row that supplied its numbers declares AMD. Measured
        // before the derivation existed: `Google Inc. (NVIDIA)` beside an Intel
        // renderer, silently.
        //
        // Vendor now decides the *tier* as well, but only for NVIDIA and only
        // by one cap — see `d3d11_splits_nvidia_from_every_other_vendor`. The
        // vendor string stays derived from the renderer either way, which is
        // what keeps a shared row from lending its name to another card.
        for (renderer, expected_vendor, expected_tier) in [
            (
                "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)",
                "Google Inc. (Intel)",
                Tier::D3d11Fl11,
            ),
            (
                "ANGLE (AMD, AMD Radeon RX 7900 XT, D3D11)",
                "Google Inc. (AMD)",
                Tier::D3d11Fl11,
            ),
            (
                "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 (0x00002684) Direct3D11 vs_5_0 ps_5_0, D3D11)",
                "Google Inc. (NVIDIA)",
                Tier::D3d11Fl11Nvidia,
            ),
        ] {
            assert_eq!(
                device_for_renderer(renderer).map(|d| d.tier),
                Some(expected_tier),
                "{renderer} must resolve {expected_tier:?}"
            );
            // Whichever row supplied the numbers, none of them lends its name.
            assert_eq!(
                vendor_for_renderer(renderer).as_deref(),
                Some(expected_vendor),
                "{renderer} must report its own vendor"
            );
        }
    }

    #[test]
    fn a_non_angle_renderer_derives_no_vendor() {
        // Nothing to parse, so the caller keeps the device row's vendor rather
        // than inventing one from a string that is not in ANGLE's format.
        assert_eq!(vendor_for_renderer("Mesa OffScreen"), None);
        assert_eq!(vendor_for_renderer("ANGLE ("), None);
    }

    #[test]
    fn a_comma_less_renderer_stops_the_vendor_at_the_paren() {
        // ANGLE's usual string is three comma-separated fields, but the
        // one-field form has none — splitting on the comma alone swallowed the
        // closing paren and answered `Google Inc. (Google))`, an unbalanced
        // string no Chrome reports.
        assert_eq!(
            vendor_for_renderer("ANGLE (Google)").as_deref(),
            Some("Google Inc. (Google)")
        );
        assert_eq!(
            vendor_for_renderer("ANGLE (Apple)").as_deref(),
            Some("Google Inc. (Apple)")
        );
        // A paren after the comma is still the comma's job to stop at.
        assert_eq!(
            vendor_for_renderer("ANGLE (Intel, Intel(R) Iris)").as_deref(),
            Some("Google Inc. (Intel)")
        );
        // Nothing before the paren is as empty as nothing at all.
        assert_eq!(vendor_for_renderer("ANGLE ()"), None);
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
        assert_eq!(d.tier, Tier::MetalMacos);
        assert_eq!(d.webgpu_vendor, "apple");
    }

    #[test]
    fn a_software_renderer_selects_the_swiftshader_tier() {
        // Both strings verbatim as real Chrome prints them. SwiftShader picks
        // its JIT backend at build time, so the platform shows through the
        // renderer string even though nothing behind it differs — each string
        // must land on its own row, not merely on the right tier.
        for (renderer, expected) in [
            (
                "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)",
                SWIFTSHADER_SUBZERO,
            ),
            (
                "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (LLVM 10.0.0) (0x0000C0DE)), SwiftShader driver)",
                SWIFTSHADER_LLVM,
            ),
        ] {
            let d = device_for_renderer(renderer)
                .expect("swiftshader renderer must match a software row");
            assert_eq!(d, expected, "{renderer} resolved to the wrong device row");
            assert_eq!(d.tier, Tier::SwiftShader);
        }
    }

    #[test]
    fn both_swiftshader_rows_serve_one_capability_tier() {
        // The rows differ in identity only. Probing Ubuntu 24 (Chrome
        // 150.0.7871.114) against the flags the macOS capture used reproduced
        // it exactly — 0 of 82 WebGL1 parameters differed, 0 of 130 WebGL2,
        // extensions identical in content and order, all 12 precision entries
        // identical — and only the renderer string moved. A split that forked
        // the tier would serve two platforms different numbers off that one
        // measurement.
        assert_eq!(SWIFTSHADER_SUBZERO.tier, Tier::SwiftShader);
        assert_eq!(SWIFTSHADER_LLVM.tier, Tier::SwiftShader);
        assert_eq!(
            SWIFTSHADER_SUBZERO.unmasked_vendor,
            SWIFTSHADER_LLVM.unmasked_vendor
        );
        assert_eq!(
            (
                SWIFTSHADER_SUBZERO.webgpu_vendor,
                SWIFTSHADER_SUBZERO.webgpu_architecture
            ),
            (
                SWIFTSHADER_LLVM.webgpu_vendor,
                SWIFTSHADER_LLVM.webgpu_architecture
            )
        );
        assert_ne!(
            SWIFTSHADER_SUBZERO.unmasked_renderer, SWIFTSHADER_LLVM.unmasked_renderer,
            "two rows exist only because the identity string differs"
        );
    }

    #[test]
    fn no_platform_defaults_to_a_software_rasterizer_any_more() {
        // Every platform now resolves a captured hardware tier: Metal on
        // macOS, D3D11 on Windows, and Mesa/Vulkan on Linux. Linux was the
        // last fallback — it served SwiftShader's renderer, which is a real
        // configuration but announces "this machine has no usable GPU".
        for (platform, expected) in [
            (Platform::Win32, "D3D11"),
            (Platform::MacIntel, "Metal"),
            (Platform::LinuxX86_64, "Vulkan"),
        ] {
            let r = default_renderer(platform);
            assert!(
                r.contains(expected),
                "{platform:?} must default to its captured {expected} row: {r}"
            );
            assert!(
                !r.contains("SwiftShader"),
                "{platform:?} must not default to a software rasterizer: {r}"
            );
        }
        assert_eq!(
            default_device(Platform::LinuxX86_64).tier,
            Tier::VulkanMesaIntelIrisPro580
        );
    }

    #[test]
    fn a_pinned_swiftshader_string_still_resolves_its_platforms_build() {
        // Neither SwiftShader row is a default any more, but a persona may pin
        // one itself — a GPU-blocklisted machine, a VM, a headless container —
        // and it must land on the build that platform's Chrome really prints.
        // The bug this descends from: every non-Mac platform pointed at the
        // single SwiftShader row, which carries the *macOS* build's string, so
        // a Linux persona reported `LLVM 10.0.0` where real Linux Chrome
        // reports `Subzero`.
        let subzero = SWIFTSHADER_SUBZERO.unmasked_renderer;
        assert!(subzero.contains("Subzero") && !subzero.contains("LLVM"));
        assert_eq!(
            device_for_renderer(subzero),
            Some(SWIFTSHADER_SUBZERO),
            "the Linux/Windows SwiftShader build must resolve its own row"
        );
        // And it stays platform-neutral, so pinning it warns on no OS.
        for platform in [Platform::Win32, Platform::MacIntel, Platform::LinuxX86_64] {
            assert_eq!(
                crate::gpu::invariants::platform_skew(platform, Tier::SwiftShader),
                None
            );
        }
    }

    #[test]
    fn a_linux_vulkan_renderer_selects_only_its_own_device() {
        // A Vulkan tier's numbers come off `VkPhysicalDeviceLimits`, so the row
        // describes this Iris Pro under this Mesa build and nothing else. The
        // token has to be narrow enough to say so: the captured string matches,
        // and other Vulkan devices — including the same vendor — must not.
        let d = device_for_renderer(VULKAN_INTEL_IRIS_PRO_580.unmasked_renderer)
            .expect("the captured Vulkan renderer must match its own row");
        assert_eq!(d, VULKAN_INTEL_IRIS_PRO_580);
        assert_eq!(d.tier, Tier::VulkanMesaIntelIrisPro580);
        for other in [
            "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 (0x00002684), Vulkan 1.3.277)",
            "ANGLE (Intel, Vulkan 1.3.289 (Intel(R) UHD Graphics 620 (KBL GT2) (0x00005917)), Intel open-source Mesa driver)",
        ] {
            assert_eq!(
                device_for_renderer(other),
                None,
                "another Vulkan device must not be served this Iris Pro's limits: {other}"
            );
        }
        // The same card on Windows goes through ANGLE's D3D11 backend, whose
        // values come from feature-level constants instead — so it must resolve
        // the D3D11 tier, not this one.
        assert_eq!(
            device_for_renderer(
                "ANGLE (Intel, Intel(R) Iris(R) Pro Graphics 580 Direct3D11 vs_5_0 ps_5_0, D3D11)"
            )
            .map(|d| d.tier),
            Some(Tier::D3d11Fl11)
        );
    }

    #[test]
    fn the_linux_default_row_has_no_measured_webgpu_adapter() {
        // Chrome does not enable WebGPU by default on Linux, so the probe
        // recorded an explicit null and the tier serves neither limits nor
        // features — the same standing SwiftShader has. What the row still
        // declares is the adapter *identity* derived from its renderer, which
        // has to name Intel and not some other vendor.
        assert_eq!(
            crate::gpu::webgpu_for_tier(Tier::VulkanMesaIntelIrisPro580),
            None,
            "no adapter was measured; substituting another tier's would claim a GPU the \
             persona never reported"
        );
        let a = adapter_for_renderer(VULKAN_INTEL_IRIS_PRO_580.unmasked_renderer);
        assert_eq!(
            (a.vendor.as_str(), a.architecture.as_str()),
            (
                VULKAN_INTEL_IRIS_PRO_580.webgpu_vendor,
                VULKAN_INTEL_IRIS_PRO_580.webgpu_architecture
            ),
            "the derived adapter must match the device row's own declaration"
        );
    }

    #[test]
    fn a_windows_pinned_swiftshader_string_resolves_the_subzero_row() {
        // The Windows SwiftShader build used to be *inferred* from the Linux
        // measurement. It is now measured: `probe_gpu -- swiftshader` on
        // Windows 10.0.21996 (Chrome 150.0.7871.186) prints Subzero, matching
        // Linux and confirming the inference.
        //
        // It no longer decides a default — `Win32` resolves the D3D11 row — so
        // what it still governs is this: a Win32 persona that pins a
        // SwiftShader renderer itself must land on the build real Windows
        // Chrome prints, not on macOS's LLVM one.
        let d = device_for_renderer(
            "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)",
        )
        .expect("the measured Windows/Linux SwiftShader string must match a row");
        assert_eq!(d, SWIFTSHADER_SUBZERO);
        // And it is a software rasterizer wherever it is pinned, so the skew
        // check stays silent on it for Win32 exactly as it does for Linux.
        assert_eq!(
            crate::gpu::invariants::platform_skew(Platform::Win32, d.tier),
            None,
            "SwiftShader is platform-neutral; pinning it under Win32 must not warn"
        );
    }

    #[test]
    fn a_mac_persona_defaults_to_the_apple_metal_renderer() {
        // Unchanged by the SwiftShader split: macOS resolves real hardware,
        // never either software row.
        assert_eq!(
            default_renderer(Platform::MacIntel),
            "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)"
        );
        assert_eq!(default_device(Platform::MacIntel).tier, Tier::MetalMacos);
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
        assert_eq!(default_device(Platform::MacIntel).tier, Tier::MetalMacos);
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
    fn d3d11_splits_nvidia_from_every_other_vendor() {
        // The D3D11 tier is per backend and feature level rather than per card
        // — `renderer11_utils.cpp` derives these values from `D3D11_REQ_*`
        // constants branched on the feature level — with one measured
        // exception. ANGLE applies `skipVSConstantRegisterZero` when and only
        // when `isNvidia`, docking `maxVertexUniformVectors` by one, so NVIDIA
        // parts report 4095 where AMD and Intel report 4096.
        //
        // This test used to assert the opposite, that all three vendors shared
        // one tier. Probing an AMD Radeon and an RTX 4090 on the same machine
        // and the same Chrome build disproved it. Vendor is therefore
        // load-bearing in row selection, and the match token alone is not
        // enough to pick a tier.
        for (r, want) in [
            (
                "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)",
                Tier::D3d11Fl11,
            ),
            ("ANGLE (AMD, AMD Radeon RX 7900 XT, D3D11)", Tier::D3d11Fl11),
            (
                "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 Direct3D11 vs_5_0 ps_5_0, D3D11)",
                Tier::D3d11Fl11Nvidia,
            ),
        ] {
            assert_eq!(
                device_for_renderer(r).map(|d| d.tier),
                Some(want),
                "expected {want:?} for {r}"
            );
        }
    }

    #[test]
    fn the_d3d11_tiers_differ_only_in_the_nvidia_reserved_uniform_vector() {
        // Locks the shape of the split, so a future capture that widens it
        // fails here rather than silently making the two tiers diverge.
        // ANGLE's workaround touches exactly one cap; the other two reported
        // values are arithmetic consequences of it.
        use crate::gpu::{GlParam, profile_for_tier};

        let base = profile_for_tier(Tier::D3d11Fl11);
        let nvidia = profile_for_tier(Tier::D3d11Fl11Nvidia);

        let differing: Vec<&str> = base
            .params_webgl2
            .iter()
            .filter(|(name, value)| nvidia.params_webgl2.get(*name) != Some(*value))
            .map(|(name, _)| name.as_str())
            .collect();

        assert_eq!(
            differing,
            [
                "MAX_COMBINED_VERTEX_UNIFORM_COMPONENTS",
                "MAX_VERTEX_UNIFORM_COMPONENTS",
                "MAX_VERTEX_UNIFORM_VECTORS",
            ],
            "the D3D11 tiers must differ only where skipVSConstantRegisterZero reaches"
        );

        // Both sides spelled out, so the measured numbers are visible here and
        // not only in the generated table.
        for (profile, vectors, components, combined) in
            [(&base, 4096, 16384, 212992), (&nvidia, 4095, 16380, 212988)]
        {
            let param = |k: &str| profile.params_webgl2.get(k);
            assert_eq!(
                param("MAX_VERTEX_UNIFORM_VECTORS"),
                Some(&GlParam::Int(vectors))
            );
            assert_eq!(
                param("MAX_VERTEX_UNIFORM_COMPONENTS"),
                Some(&GlParam::Int(components))
            );
            assert_eq!(
                param("MAX_COMBINED_VERTEX_UNIFORM_COMPONENTS"),
                Some(&GlParam::Int(combined))
            );
            // The relations that make these one fact rather than three: the
            // components are the vectors in scalars, and the combined total
            // adds the same 12 * 16384 of uniform-block storage on both tiers.
            assert_eq!(components, vectors * 4);
            assert_eq!(combined, components + 12 * 16384);
        }
    }

    #[test]
    fn renderers_with_no_shipped_tier_return_none() {
        // Five tiers ship (SwiftShader, Apple Metal, D3D11 FL11+ in its NVIDIA
        // and non-NVIDIA forms, and the Intel Iris Pro 580 under Mesa/Vulkan).
        // A backend none of them covers still
        // has no measured tier, and guessing one would pair its name with
        // another backend's numbers. Desktop-GL is such a backend, and so is
        // every Vulkan device but the captured one: the Vulkan tier is
        // device-scoped by construction, not a Linux catch-all.
        for r in [
            "ANGLE (Mesa, llvmpipe (LLVM 15.0.7, 256 bits), OpenGL ES 3.2)",
            "ANGLE (Intel, Mesa Intel(R) UHD Graphics (TGL GT1), OpenGL ES 3.2)",
            "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 (0x00002684), Vulkan 1.3.277)",
        ] {
            assert_eq!(device_for_renderer(r), None, "unexpected tier for {r}");
        }
    }
}
