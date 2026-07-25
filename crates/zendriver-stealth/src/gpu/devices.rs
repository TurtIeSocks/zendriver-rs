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
}

/// Known devices, keyed by [`DeviceRow::match_token`].
///
/// Order is load-bearing: [`device_for_renderer`] takes the *first* row whose
/// token the renderer contains, so the specific SwiftShader row must precede
/// the generic one.
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
    },
    DeviceRow {
        unmasked_vendor: "Google Inc. (Apple)",
        unmasked_renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)",
        tier: Tier::MetalAppleFamily3,
        webgpu_vendor: "apple",
        webgpu_architecture: "metal-3",
        match_token: "apple",
    },
    DeviceRow {
        unmasked_vendor: "Google Inc. (NVIDIA)",
        unmasked_renderer: "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 (0x00002684) Direct3D11 vs_5_0 ps_5_0, D3D11)",
        tier: Tier::D3d11Fl11,
        webgpu_vendor: "nvidia",
        webgpu_architecture: "lovelace",
        // ANGLE's D3D11 renderer string always ends in the backend tag
        // ", D3D11)", whatever the card — which is exactly the granularity the
        // tier has. `direct3d11` would be wrong twice over: it misses the AMD
        // form ("ANGLE (AMD, AMD Radeon RX 7900 XT, D3D11)"), which names no
        // shader model, and it would tie a backend-and-feature-level tier to a
        // spelling only some vendors emit.
        match_token: "d3d11",
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
/// - Linux gets a SwiftShader row. Chrome's software rasterizer runs on every
///   OS and its renderer string names no platform-specific API, so the pairing
///   is one real Chrome does produce (a GPU-blocklisted machine, a VM, a
///   headless container), and the capability values served beside it are
///   genuinely SwiftShader's. Linux keeps it until a Vulkan or desktop-GL tier
///   is captured — the cost is honest and worth naming: SwiftShader means "this
///   machine has no usable GPU", which some fingerprinters weight on its own.
///   That is a real configuration rather than an impossible one, so it is the
///   better trade until that capture exists. See the `capture-gpu-tier` skill.
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
/// and no table regeneration.
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
        Platform::LinuxX86_64 => SWIFTSHADER_SUBZERO,
    }
}

/// Renderer string assumed for a platform. See [`default_device`].
pub(crate) fn default_renderer(platform: Platform) -> &'static str {
    default_device(platform).unmasked_renderer
}

/// Named row aliases, so the platform mapping above reads as intent rather
/// than as an index into [`DEVICES`].
const SWIFTSHADER_SUBZERO: DeviceRow = DEVICES[0];
/// No platform defaults to this row: macOS gets Metal, Windows gets D3D11, and
/// Linux gets Subzero. It is the tier's catch-all, reached when a caller pins a
/// macOS-flavored SwiftShader string — and named here so the tests can assert
/// the two rows never drift apart.
#[allow(dead_code)]
const SWIFTSHADER_LLVM: DeviceRow = DEVICES[1];
const METAL_APPLE: DeviceRow = DEVICES[2];
const NVIDIA_D3D11: DeviceRow = DEVICES[3];

/// Pick the device row a renderer string belongs to, by explicit
/// [`DeviceRow::match_token`] — never by a key derived from a row's own
/// reference string.
///
/// First match in [`DEVICES`] order wins, so a specific token must be listed
/// ahead of a token it is a special case of: `"subzero"` before
/// `"swiftshader"`, or every Subzero string would resolve to the LLVM row's
/// identity. Both carry the same tier, so ordering decides the identity
/// string alone — but that string is the whole reason the rows are separate.
/// The other two tokens (`"apple"`, `"d3d11"`) are disjoint from those and
/// from each other, so their position is not load-bearing.
///
/// Three tiers ship today (SwiftShader, Apple Metal, D3D11 FL11+), so `None`
/// is still expected for the backends none of them covers — a Linux Vulkan or
/// desktop-GL renderer, or a D3D9 string. A **feature-level-10** D3D11
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
    DEVICES.iter().copied().find(|d| r.contains(d.match_token))
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
pub(crate) fn vendor_for_renderer(renderer: &str) -> Option<String> {
    let vendor = renderer.strip_prefix("ANGLE (")?.split(',').next()?.trim();
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
    // did reach it, which is the split this arm closes: a SwiftShader row is
    // Linux's default and was Windows' too (see `default_device`), so such a
    // persona served SwiftShader's renderer to WebGL and an *Intel* adapter to
    // WebGPU. Both JIT-backend rows go through this arm — the token is
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
        // A SwiftShader row is Linux's default and was Windows' too, so this
        // is the adapter a default Linux persona serves — and the one any
        // persona gets when it pins a SwiftShader renderer itself, `Win32`
        // included, now that Windows defaults to the D3D11 row. It used to
        // reach the Intel catch-all: an Intel WebGPU adapter beside a
        // SwiftShader WebGL renderer, a pairing Chrome does not produce. Both
        // JIT-backend rows, since either can be served depending on the
        // renderer resolved.
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
        // The D3D11 row is deliberately shared across vendors — its token
        // matches every FL11+ renderer because the capability values are the
        // same. The identity strings are not shared, so an Intel or AMD
        // renderer must carry an Intel or AMD vendor even though the row that
        // supplied its numbers declares NVIDIA. Measured before the derivation
        // existed: `Google Inc. (NVIDIA)` beside an Intel renderer, silently.
        for (renderer, expected) in [
            (
                "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)",
                "Google Inc. (Intel)",
            ),
            (
                "ANGLE (AMD, AMD Radeon RX 7900 XT, D3D11)",
                "Google Inc. (AMD)",
            ),
            (
                "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 (0x00002684) Direct3D11 vs_5_0 ps_5_0, D3D11)",
                "Google Inc. (NVIDIA)",
            ),
        ] {
            // All three share one tier...
            assert_eq!(
                device_for_renderer(renderer).map(|d| d.tier),
                Some(Tier::D3d11Fl11),
                "{renderer} must resolve the shared D3D11 tier"
            );
            // ...and none of them shares its identity.
            assert_eq!(
                vendor_for_renderer(renderer).as_deref(),
                Some(expected),
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
    fn a_linux_persona_defaults_to_swiftshaders_subzero_string() {
        // The bug this guards: every non-Mac platform pointed at the single
        // SwiftShader row, which carries the *macOS* build's string. A Linux
        // persona then reported `LLVM 10.0.0` where real Linux Chrome reports
        // `Subzero` — one token off against any corpus of real Linux
        // fingerprints.
        let linux = default_renderer(Platform::LinuxX86_64);
        assert!(
            linux.contains("Subzero"),
            "a Linux persona must report SwiftShader's Subzero build: {linux}"
        );
        assert!(
            !linux.contains("LLVM"),
            "a Linux persona must not report macOS's LLVM build: {linux}"
        );
        // Win32 no longer defaults to any SwiftShader row — it resolves the
        // captured D3D11 tier, real hardware rather than a software rasterizer.
        let win = default_renderer(Platform::Win32);
        assert!(
            win.contains("D3D11") && !win.contains("SwiftShader"),
            "Win32 must default to the captured D3D11 row, not a software one: {win}"
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
        assert_eq!(
            default_device(Platform::MacIntel).tier,
            Tier::MetalAppleFamily3
        );
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
    fn every_d3d11_vendor_selects_the_one_d3d11_tier() {
        // The tier is per backend and feature level, not per card: ANGLE's
        // D3D11 renderer derives these values from `D3D11_REQ_*` constants
        // branched on the feature level, so Intel, NVIDIA and AMD at FL11+ all
        // report the same numbers and all resolve here. The match token is the
        // trailing ", D3D11)" backend tag, which is why the AMD form — naming
        // no shader model at all — matches like the other two.
        for r in [
            "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)",
            "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 Direct3D11 vs_5_0 ps_5_0, D3D11)",
            "ANGLE (AMD, AMD Radeon RX 7900 XT, D3D11)",
        ] {
            assert_eq!(
                device_for_renderer(r).map(|d| d.tier),
                Some(Tier::D3d11Fl11),
                "expected the D3D11 tier for {r}"
            );
        }
    }

    #[test]
    fn renderers_with_no_shipped_tier_return_none() {
        // Three tiers ship (SwiftShader, Apple Metal, D3D11 FL11+). A backend
        // none of them covers still has no measured tier, and guessing one
        // would pair its name with another backend's numbers. These are the
        // Linux desktop-GL and Vulkan renderer strings, which is why Linux
        // still defaults to the SwiftShader row — see `default_device`.
        for r in [
            "ANGLE (Mesa, llvmpipe (LLVM 15.0.7, 256 bits), OpenGL ES 3.2)",
            "ANGLE (Intel, Mesa Intel(R) UHD Graphics (TGL GT1), OpenGL ES 3.2)",
            "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 (0x00002684), Vulkan 1.3.277)",
        ] {
            assert_eq!(device_for_renderer(r), None, "unexpected tier for {r}");
        }
    }
}
