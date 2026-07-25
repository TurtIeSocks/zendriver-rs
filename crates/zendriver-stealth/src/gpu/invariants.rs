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
/// - `MAX_VIEWPORT_DIMS >= MAX_TEXTURE_SIZE`: the viewport is backed by a
///   framebuffer, and a framebuffer attachment is itself a texture, so a
///   driver can never offer a rendering surface larger than the textures it
///   can allocate. This is the shipped bug this whole effort exists to fix:
///   the old patch reported a 32767 viewport beside an 8192 texture max —
///   one `getParameter` pair that outed the browser as patched.
/// - `MAX_COMBINED_TEXTURE_IMAGE_UNITS >= MAX_TEXTURE_IMAGE_UNITS +
///   MAX_VERTEX_TEXTURE_IMAGE_UNITS`: "combined" is literally the shared pool
///   the fragment and vertex stages draw texture units from, per the WebGL
///   spec's definition of the enum. It cannot be smaller than a single stage's
///   share of it.
/// - Every `DRAW_BUFFERn` enum for `n >= MAX_DRAW_BUFFERS` must be absent: the
///   spec only defines `DRAW_BUFFER0` through `DRAW_BUFFER{MAX_DRAW_BUFFERS -
///   1}`; a driver has no backing constant for the rest, so a real
///   `getParameter` call for one throws `INVALID_ENUM` rather than answering.
#[allow(dead_code)] // called from push_webgl (Task 8), alongside platform_skew
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
#[allow(dead_code)] // called from push_webgl (Task 8), alongside check_coherence
pub(crate) fn platform_skew(platform: Platform, tier: Tier) -> Option<String> {
    // SwiftShader is a software rasterizer, available on every platform, so it
    // never conflicts with a claimed OS.
    if tier == Tier::SwiftShader {
        return None;
    }
    let ok = matches!(
        (platform, tier),
        (Platform::MacIntel, Tier::MetalAppleFamily3)
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
        for tier in [Tier::SwiftShader, Tier::MetalAppleFamily3] {
            let p = profile_for_tier(tier);
            assert_eq!(check_coherence(&p), Ok(()), "tier {tier:?} is incoherent");
        }
    }

    #[test]
    fn viewport_smaller_than_texture_is_rejected() {
        // This is exactly the shipped bug this whole effort exists to fix:
        // the old patch reported a 32767 viewport beside an 8192 texture max.
        let mut p = profile_for_tier(Tier::SwiftShader);
        p.params_webgl2
            .insert("MAX_TEXTURE_SIZE".into(), GlParam::Int(16384));
        p.params_webgl2
            .insert("MAX_VIEWPORT_DIMS".into(), GlParam::IntPair([8192, 8192]));
        assert!(check_coherence(&p).is_err());
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
        p.params_webgl2
            .insert("DRAW_BUFFER6".into(), GlParam::Int(0));
        assert!(check_coherence(&p).is_err());
    }

    #[test]
    fn an_empty_profile_is_vacuously_coherent() {
        assert_eq!(check_coherence(&GpuProfile::empty()), Ok(()));
    }

    // --- completeness (spec invariant 1) ------------------------------------

    #[test]
    fn every_tier_covers_the_whole_measured_parameter_surface() {
        // The captures enumerated 82 WebGL1 and ~132 WebGL2 params. A tier
        // that resolves materially fewer means the base/override split dropped
        // entries, and every dropped param falls through to the real backend
        // — which is exactly the leak this work exists to close.
        for tier in [Tier::SwiftShader, Tier::MetalAppleFamily3] {
            let p = profile_for_tier(tier);
            assert!(
                p.params_webgl2.len() >= 130,
                "tier {tier:?} resolves only {} WebGL2 params; expected >= 130",
                p.params_webgl2.len()
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
        // SwiftShader is platform-neutral: it is software, available anywhere.
        assert!(platform_skew(Platform::Win32, Tier::SwiftShader).is_none());
    }
}
