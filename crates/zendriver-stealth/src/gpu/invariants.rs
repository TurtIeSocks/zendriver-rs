//! Relations a real GPU's parameters always satisfy. Fingerprinters check
//! several of these, so a table edit that breaks one must fail the build
//! rather than ship an impossible device.

use crate::Platform;
use crate::gpu::types::Tier;
use crate::gpu::{GlParam, GpuProfile};

fn int(p: &GpuProfile, k: &str) -> Option<i64> {
    match p.params_webgl2.get(k) {
        Some(GlParam::Int(i)) => Some(*i),
        _ => None,
    }
}

fn pair_min(p: &GpuProfile, k: &str) -> Option<i64> {
    match p.params_webgl2.get(k) {
        Some(GlParam::IntPair([a, b])) => Some(i64::from(*a.min(b))),
        _ => None,
    }
}

/// Check the relations a real device always satisfies.
///
/// Each check only fires when both operands are present, so a partial
/// profile (an overlay, a caller-supplied spec that only sets a few fields)
/// is not penalized for what it left unset — [`GpuProfile::empty`] must stay
/// vacuously coherent. Returns the first violation as a human-readable
/// string rather than a bool, so a failing table edit names what it broke
/// instead of just failing.
///
/// The three relations, and why a real GPU always satisfies them:
///
/// - `MAX_VIEWPORT_DIMS >= MAX_TEXTURE_SIZE`: a viewport *larger* than the
///   texture max is normal — D3D11 reports a 32767 viewport bound
///   (`D3D11_VIEWPORT_BOUNDS_MAX`) beside a 16384 texture max
///   (`D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION`) for feature level 11_0 and
///   above (ANGLE `GetMaximumViewportSize` / `GetMaximum2DTextureSize`,
///   `src/libANGLE/renderer/d3d/d3d11/renderer11_utils.cpp:422-434`), and the
///   `d3d11-fl11` capture measures exactly that pair. What never happens on
///   any backend measured here — SwiftShader 8192/8192, Metal 16384/16384,
///   D3D11 32767/16384 — is the viewport
///   coming in *below* the texture max, so that direction is still a real
///   defect: a profile reporting a smaller viewport than texture max is
///   malformed. This check does not (and must not) catch the shipped bug —
///   a 32767 viewport beside an 8192 texture max is exactly the real D3D11
///   pairing above with a different texture-max source, so `vp < tex` is
///   false and the check correctly passes it. That bug's defect was
///   provenance (viewport and texture max drawn from two different
///   backends in one profile), not an arithmetic relation between the two
///   values; [`profile_for_tier`](crate::gpu::profile_for_tier) resolving
///   every value from a single tier is what prevents it structurally. See
///   `the_historical_mixed_tier_pair_is_not_an_arithmetic_violation` below.
/// - `MAX_COMBINED_TEXTURE_IMAGE_UNITS >= MAX_TEXTURE_IMAGE_UNITS +
///   MAX_VERTEX_TEXTURE_IMAGE_UNITS`: "combined" is literally the shared pool
///   the fragment and vertex stages draw texture units from, per the WebGL
///   spec's definition of the enum. It cannot be smaller than a single stage's
///   share of it.
/// - Every `DRAW_BUFFERn` enum for `n >= MAX_DRAW_BUFFERS` must be absent: the
///   spec only defines `DRAW_BUFFER0` through `DRAW_BUFFER{MAX_DRAW_BUFFERS -
///   1}`; a driver has no backing constant for the rest, so a real
///   `getParameter` call for one throws `INVALID_ENUM` rather than answering.
///   The tier tables no longer carry `DRAW_BUFFERn` at all — it is written by
///   `drawBuffers()`, so it is delegated to the real backend — which leaves
///   this check guarding the one place the pair can still be set
///   incoherently: a caller-supplied [`GpuProfile`] overlay.
pub(crate) fn check_coherence(p: &GpuProfile) -> Result<(), String> {
    if let (Some(tex), Some(vp)) = (int(p, "MAX_TEXTURE_SIZE"), pair_min(p, "MAX_VIEWPORT_DIMS")) {
        if vp < tex {
            return Err(format!(
                "MAX_VIEWPORT_DIMS ({vp}) is below MAX_TEXTURE_SIZE ({tex}); no real GPU reports this"
            ));
        }
    }
    if let (Some(combined), Some(frag), Some(vert)) = (
        int(p, "MAX_COMBINED_TEXTURE_IMAGE_UNITS"),
        int(p, "MAX_TEXTURE_IMAGE_UNITS"),
        int(p, "MAX_VERTEX_TEXTURE_IMAGE_UNITS"),
    ) {
        if combined < frag + vert {
            return Err(format!(
                "MAX_COMBINED_TEXTURE_IMAGE_UNITS ({combined}) is below \
                 MAX_TEXTURE_IMAGE_UNITS ({frag}) + MAX_VERTEX_TEXTURE_IMAGE_UNITS ({vert})"
            ));
        }
    }
    if let Some(max_draw) = int(p, "MAX_DRAW_BUFFERS") {
        for i in 0..32 {
            if p.params_webgl2.contains_key(&format!("DRAW_BUFFER{i}")) && i64::from(i) >= max_draw
            {
                return Err(format!(
                    "DRAW_BUFFER{i} is present but MAX_DRAW_BUFFERS is {max_draw}"
                ));
            }
        }
    }
    Ok(())
}

/// Report a mismatch between the persona's claimed OS and the tier supplying
/// its capability values, or `None` when they are compatible.
///
/// Deliberately a warning rather than an error: a caller may pair them on
/// purpose, and refusing to launch over a fingerprint detail is a worse
/// failure than reporting one. Same stance as the header-coherence check
/// (#43).
///
/// Called by [`push_webgl`](crate::patches) with the platform the page claims
/// (the persona's, else the probed host's). Every platform's *default* row is
/// now coherent, so this stays silent on the common path and fires only where
/// a caller pinned a renderer from another OS's backend.
pub(crate) fn platform_skew(platform: Platform, tier: Tier) -> Option<String> {
    // SwiftShader is a software rasterizer, available on every platform, so it
    // never conflicts with a claimed OS.
    if tier == Tier::SwiftShader {
        return None;
    }
    // Each remaining tier is tied to the one OS its backend exists on: Metal
    // to macOS, D3D11 to Windows. A page claiming either OS beside the other's
    // numbers is a pair Chrome cannot produce.
    let ok = matches!(
        (platform, tier),
        (Platform::MacIntel, Tier::MetalAppleFamily3) | (Platform::Win32, Tier::D3d11Fl11)
    );
    (!ok).then(|| format!("persona claims {platform:?} but its GPU values come from {tier:?}"))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::gpu::{GlParam, GpuProfile, profile_for_tier, types::Tier};

    #[test]
    fn shipped_tiers_are_all_coherent() {
        for &tier in Tier::ALL {
            let p = profile_for_tier(tier);
            assert_eq!(check_coherence(&p), Ok(()), "tier {tier:?} is incoherent");
        }
    }

    #[test]
    fn viewport_smaller_than_texture_is_rejected() {
        // No measured or documented backend ever reports a viewport bound
        // below its own texture max (SwiftShader 8192/8192, Metal
        // 16384/16384, D3D11 32767/16384) — that direction is still a real
        // defect, so it must stay an error. This is NOT the shipped bug:
        // see `the_historical_mixed_tier_pair_is_not_an_arithmetic_violation`
        // for why that pair (32767 viewport, 8192 texture) is legitimate and
        // must NOT be rejected by this check.
        let mut p = profile_for_tier(Tier::SwiftShader);
        p.params_webgl2
            .insert("MAX_TEXTURE_SIZE".into(), GlParam::Int(16384));
        p.params_webgl2
            .insert("MAX_VIEWPORT_DIMS".into(), GlParam::IntPair([8192, 8192]));
        assert!(check_coherence(&p).is_err());
    }

    #[test]
    fn the_historical_mixed_tier_pair_is_not_an_arithmetic_violation() {
        // The shipped bug was MAX_VIEWPORT_DIMS [32767,32767] beside
        // MAX_TEXTURE_SIZE 8192. Both values are real: 32767 is D3D11's
        // viewport bound (ANGLE GetMaximumViewportSize) and 8192 is
        // SwiftShader's texture max. The defect was that they came from two
        // different backends, which no arithmetic relation can detect —
        // resolving every value from a single tier is what prevents it.
        // This test exists so nobody "fixes" check_coherence by inventing a
        // ratio bound that would reject real D3D11 hardware.
        let mut p = profile_for_tier(Tier::SwiftShader);
        p.params_webgl2
            .insert("MAX_VIEWPORT_DIMS".into(), GlParam::IntPair([32767, 32767]));
        assert_eq!(check_coherence(&p), Ok(()));
    }

    #[test]
    fn combined_texture_units_below_the_sum_of_its_parts_is_rejected() {
        let mut p = profile_for_tier(Tier::MetalAppleFamily3);
        p.params_webgl2
            .insert("MAX_COMBINED_TEXTURE_IMAGE_UNITS".into(), GlParam::Int(1));
        assert!(check_coherence(&p).is_err());
    }

    #[test]
    fn draw_buffer_params_beyond_max_draw_buffers_are_rejected() {
        let mut p = profile_for_tier(Tier::SwiftShader);
        // SwiftShader has MAX_DRAW_BUFFERS = 6, so DRAW_BUFFER6 must not exist.
        // The tier tables never produce this pairing (DRAW_BUFFERn is
        // delegated), so the insert models the case that can still reach it:
        // a caller pinning DRAW_BUFFER6 through a GpuProfile overlay.
        p.params_webgl2
            .insert("DRAW_BUFFER6".into(), GlParam::Int(0));
        assert!(check_coherence(&p).is_err());
    }

    #[test]
    fn an_empty_profile_is_vacuously_coherent() {
        assert_eq!(check_coherence(&GpuProfile::empty()), Ok(()));
    }

    // --- completeness (spec invariant 1) ------------------------------------

    /// Served capabilities per context version, from `gpu-tier-gen`'s
    /// `SERVED_CAPS` intersected with what each context reports: 18 of the 82
    /// parameters a WebGL1 context exposes, 47 of the up-to-132 a WebGL2 one
    /// does. The rest is per-context mutable state, which the tables must not
    /// carry — see `SERVED_CAPS` for the rule and `DELEGATED_PARAMS` for the
    /// reason each one is disqualified.
    const SERVED_WEBGL1: usize = 18;
    const SERVED_WEBGL2: usize = 47;

    #[test]
    fn every_tier_covers_the_whole_served_capability_surface() {
        // Both directions matter. Resolving *fewer* means the base/override
        // split dropped entries, and a dropped capability falls through to the
        // real backend — the leak this work exists to close. Resolving *more*
        // means mutable state crept back into the tables, which freezes values
        // the page just wrote — the regression C1 was reported against.
        for &tier in Tier::ALL {
            let p = profile_for_tier(tier);
            assert_eq!(
                p.params_webgl2.len(),
                SERVED_WEBGL2,
                "tier {tier:?} resolves {} WebGL2 params, expected {SERVED_WEBGL2}",
                p.params_webgl2.len()
            );
            assert_eq!(
                p.params_webgl1.len(),
                SERVED_WEBGL1,
                "tier {tier:?} resolves {} WebGL1 params, expected {SERVED_WEBGL1}",
                p.params_webgl1.len()
            );
            assert_eq!(
                p.precision.len(),
                12,
                "tier {tier:?} lost precision entries"
            );
            assert!(
                p.params_webgl1.len() < p.params_webgl2.len(),
                "tier {tier:?} serves WebGL1 the WebGL2 set ({} vs {}); a WebGL1 context \
                 must not answer WebGL2-only enums",
                p.params_webgl1.len(),
                p.params_webgl2.len()
            );
        }
    }

    #[test]
    fn every_resolved_param_has_a_known_enum_number() {
        // A param with no enum number can never be served: the JS looks up
        // profile.enumNames[param] and falls through when it misses.
        let p = profile_for_tier(Tier::MetalAppleFamily3);
        let known: std::collections::BTreeSet<&str> = crate::gpu::tiers::ENUM_NAMES
            .iter()
            .map(|(_, n)| *n)
            .collect();
        let orphans: Vec<&String> = p
            .params_webgl2
            .keys()
            .filter(|k| !known.contains(k.as_str()))
            .collect();
        assert!(
            orphans.is_empty(),
            "params with no enum number: {orphans:?}"
        );
    }

    // --- platform coherence (spec invariant 3) ------------------------------

    #[test]
    fn platform_skew_between_claimed_os_and_tier_is_reported() {
        // A Windows persona resolving Metal's values is incoherent. This is a
        // warning, not an error, matching the header-coherence precedent from
        // #43 — the caller may be doing it deliberately.
        assert!(platform_skew(Platform::Win32, Tier::MetalAppleFamily3).is_some());
        assert!(platform_skew(Platform::MacIntel, Tier::MetalAppleFamily3).is_none());
        // D3D11 is the mirror image: coherent on Windows, impossible anywhere
        // else. A Linux persona on it is as wrong as a Windows one on Metal.
        assert!(platform_skew(Platform::Win32, Tier::D3d11Fl11).is_none());
        assert!(platform_skew(Platform::MacIntel, Tier::D3d11Fl11).is_some());
        assert!(platform_skew(Platform::LinuxX86_64, Tier::D3d11Fl11).is_some());
        // SwiftShader is platform-neutral: it is software, available anywhere.
        assert!(platform_skew(Platform::Win32, Tier::SwiftShader).is_none());
    }
}
