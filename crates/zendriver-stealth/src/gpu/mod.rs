//! Coherent per-GPU value tables and the profile resolved from them.

use std::collections::BTreeMap;

pub(crate) mod devices;
pub(crate) mod invariants;
pub(crate) mod tiers;
pub(crate) mod types;

pub use types::{GlParam, Provenance, ShaderPrecision};
use types::{GlParamRef, Tier};

/// Everything a page can read about one GPU, fully resolved.
///
/// Produced by flattening the shared base table, the tier's overrides, the
/// device row, and any caller-supplied spec. Callers only ever see this
/// flattened form, so the internal base/override split can change without
/// breaking anyone.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuProfile {
    pub provenance: Provenance,
    pub params_webgl1: BTreeMap<String, GlParam>,
    pub params_webgl2: BTreeMap<String, GlParam>,
    pub precision: BTreeMap<String, ShaderPrecision>,
    pub extensions_webgl1: Vec<String>,
    pub extensions_webgl2: Vec<String>,
    pub unmasked_vendor: String,
    pub unmasked_renderer: String,
}

impl GpuProfile {
    /// An all-empty profile, used as an overlay base.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            provenance: Provenance::Derived {
                source: "empty".into(),
            },
            params_webgl1: BTreeMap::new(),
            params_webgl2: BTreeMap::new(),
            precision: BTreeMap::new(),
            extensions_webgl1: Vec::new(),
            extensions_webgl2: Vec::new(),
            unmasked_vendor: String::new(),
            unmasked_renderer: String::new(),
        }
    }

    /// Merge `over` on top of `self`.
    ///
    /// The maps merge key-wise rather than whole-field, so a caller supplying a
    /// partial profile overrides only the keys it sets and cannot silently wipe
    /// entries it said nothing about. The lists and strings replace wholesale,
    /// but only when non-empty, so an empty `over` is a no-op rather than a
    /// clear.
    #[must_use]
    pub fn overlay(mut self, over: GpuProfile) -> GpuProfile {
        self.params_webgl1.extend(over.params_webgl1);
        self.params_webgl2.extend(over.params_webgl2);
        self.precision.extend(over.precision);
        if !over.extensions_webgl1.is_empty() {
            self.extensions_webgl1 = over.extensions_webgl1;
        }
        if !over.extensions_webgl2.is_empty() {
            self.extensions_webgl2 = over.extensions_webgl2;
        }
        if !over.unmasked_vendor.is_empty() {
            self.unmasked_vendor = over.unmasked_vendor;
        }
        if !over.unmasked_renderer.is_empty() {
            self.unmasked_renderer = over.unmasked_renderer;
        }
        self
    }
}

fn tier_key(tier: Tier) -> &'static str {
    match tier {
        Tier::SwiftShader => "swiftshader",
        Tier::MetalAppleFamily3 => "metal-apple-family3",
    }
}

fn lookup<'a, V>(table: &'a [(&str, V)], key: &str) -> Option<&'a V> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// Merge one context version's base table with a tier's overrides.
///
/// The two context versions are kept apart deliberately. WebGL1 exposes 82
/// parameters and WebGL2 exposes up to 132 (132 on Metal, 130 on SwiftShader,
/// which lacks `DRAW_BUFFER6`/`7` because its `MAX_DRAW_BUFFERS` is 6);
/// serving the WebGL2 set to a WebGL1
/// context would answer enums that context has no constant for, where real
/// Chrome returns `null` and raises `INVALID_ENUM`.
fn flatten(
    base: &[(&str, GlParamRef)],
    overrides: &[(&str, &[(&str, GlParamRef)])],
    key: &str,
) -> BTreeMap<String, GlParam> {
    let mut out: BTreeMap<String, GlParam> = base
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.to_owned_param()))
        .collect();
    if let Some(over) = lookup(overrides, key) {
        for (k, v) in *over {
            out.insert((*k).to_string(), v.to_owned_param());
        }
    }
    out
}

/// Flatten the base tables plus one tier's overrides into a profile.
#[allow(dead_code)] // consumed starting with the persona/fingerprint wiring (a later task)
pub(crate) fn profile_for_tier(tier: Tier) -> GpuProfile {
    let key = tier_key(tier);
    let params = flatten(
        tiers::BASE_PARAMS_WEBGL2,
        tiers::PARAM_OVERRIDES_WEBGL2,
        key,
    );
    let precision = lookup(tiers::PRECISION, key)
        .map(|rows| {
            rows.iter()
                .map(|(k, [a, b, c])| {
                    (
                        (*k).to_string(),
                        ShaderPrecision {
                            range_min: *a,
                            range_max: *b,
                            precision: *c,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let exts = |t: &[(&str, &[&str])]| -> Vec<String> {
        lookup(t, key)
            .map(|l| l.iter().map(|s| (*s).to_string()).collect())
            .unwrap_or_default()
    };
    GpuProfile {
        provenance: Provenance::Probed {
            chrome: "see data/gpu-tiers".into(),
            os: "see data/gpu-tiers".into(),
        },
        params_webgl1: flatten(
            tiers::BASE_PARAMS_WEBGL1,
            tiers::PARAM_OVERRIDES_WEBGL1,
            key,
        ),
        params_webgl2: params,
        precision,
        extensions_webgl1: exts(tiers::EXTENSIONS_WEBGL1),
        extensions_webgl2: exts(tiers::EXTENSIONS_WEBGL2),
        unmasked_vendor: String::new(),
        unmasked_renderer: String::new(),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn swiftshader_tier_resolves_its_measured_values() {
        let p = profile_for_tier(types::Tier::SwiftShader);
        assert_eq!(p.params_webgl2["MAX_TEXTURE_SIZE"], GlParam::Int(8192));
        assert_eq!(
            p.params_webgl2["MAX_VIEWPORT_DIMS"],
            GlParam::IntPair([8192, 8192])
        );
    }

    #[test]
    fn metal_tier_resolves_its_own_values_not_swiftshaders() {
        let p = profile_for_tier(types::Tier::MetalAppleFamily3);
        assert_eq!(p.params_webgl2["MAX_TEXTURE_SIZE"], GlParam::Int(16384));
        assert_eq!(
            p.params_webgl2["MAX_VIEWPORT_DIMS"],
            GlParam::IntPair([16384, 16384])
        );
    }

    #[test]
    fn base_values_reach_every_tier() {
        // A param both tiers agreed on lives only in base; resolution must
        // still surface it, or ~104 params would silently vanish.
        let sw = profile_for_tier(types::Tier::SwiftShader);
        let mt = profile_for_tier(types::Tier::MetalAppleFamily3);
        assert!(
            sw.params_webgl2.len() > 100,
            "got {}",
            sw.params_webgl2.len()
        );
        assert!(
            mt.params_webgl2.len() > 100,
            "got {}",
            mt.params_webgl2.len()
        );
    }

    #[test]
    fn draw_buffer_params_do_not_leak_across_tiers() {
        // DRAW_BUFFER6/7 exist only where MAX_DRAW_BUFFERS allows.
        let sw = profile_for_tier(types::Tier::SwiftShader);
        let mt = profile_for_tier(types::Tier::MetalAppleFamily3);
        assert!(!sw.params_webgl2.contains_key("DRAW_BUFFER6"));
        assert!(mt.params_webgl2.contains_key("DRAW_BUFFER6"));
    }

    #[test]
    fn precision_differs_where_it_was_measured_to_differ() {
        let sw = profile_for_tier(types::Tier::SwiftShader);
        let mt = profile_for_tier(types::Tier::MetalAppleFamily3);
        assert_ne!(
            sw.precision["VERTEX_SHADER/MEDIUM_FLOAT"],
            mt.precision["VERTEX_SHADER/MEDIUM_FLOAT"]
        );
        // HIGH_FLOAT was measured identical on both; it carries no entropy.
        assert_eq!(
            sw.precision["FRAGMENT_SHADER/HIGH_FLOAT"],
            mt.precision["FRAGMENT_SHADER/HIGH_FLOAT"]
        );
    }

    #[test]
    fn webgl2_extension_list_drops_the_core_promoted_entries() {
        let p = profile_for_tier(types::Tier::MetalAppleFamily3);
        assert!(p.extensions_webgl1.iter().any(|e| e == "OES_texture_float"));
        assert!(
            !p.extensions_webgl2.iter().any(|e| e == "OES_texture_float"),
            "OES_texture_float is core in WebGL2; claiming it is a tell"
        );
    }

    #[test]
    fn overlay_lets_the_caller_win_field_by_field() {
        let base = profile_for_tier(types::Tier::SwiftShader);
        let mut over = GpuProfile::empty();
        over.unmasked_renderer = "ANGLE (NVIDIA, ...)".into();
        let merged = base.clone().overlay(over);
        assert_eq!(merged.unmasked_renderer, "ANGLE (NVIDIA, ...)");
        // Untouched fields survive.
        assert_eq!(merged.params_webgl2["MAX_TEXTURE_SIZE"], GlParam::Int(8192));
    }
}
