//! Coherent per-GPU value tables and the profile resolved from them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub(crate) mod catalogue;
pub mod device_select;
pub(crate) mod devices;
pub(crate) mod invariants;
pub(crate) mod tiers;
pub(crate) mod types;

pub use types::{GlParam, Provenance, ShaderPrecision};
use types::{GlParamRef, Tier};

/// One GPU's identity and static capabilities, fully resolved.
///
/// Produced by flattening the shared base table, the tier's overrides, the
/// device row, and any caller-supplied spec. Callers only ever see this
/// flattened form, so the internal base/override split can change without
/// breaking anyone.
///
/// The parameter maps hold *device capabilities* only — the values that are
/// implementation-fixed and differ per GPU. Per-context mutable state
/// (`VIEWPORT`, `BLEND`, the `STENCIL_*`/`PACK_*`/`UNPACK_*` families, ...)
/// is deliberately absent so it delegates to the real backend; freezing it
/// would contradict the page's own GL calls. A caller may still pin such a
/// key through [`overlay`](Self::overlay) — nothing here forbids it — but the
/// shipped tiers never do.
///
/// Derives `Serialize`/`Deserialize` because [`Persona`](crate::Persona) does
/// (it round-trips through JSON as `browser_fingerprint_generate`'s return
/// value and in `Persona::overlay`'s own tests) — every field type here
/// already supported serde before this field existed, so this is just wiring
/// the derive through, not adding new capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        Tier::MetalMacos => "metal-macos",
        Tier::D3d11Fl11 => "d3d11-fl11",
        Tier::D3d11Fl11Nvidia => "d3d11-fl11-nvidia",
        Tier::VulkanMesaIntelIrisPro580 => "vulkan-mesa-intel-iris-pro-580",
    }
}

fn lookup<'a, V>(table: &'a [(&str, V)], key: &str) -> Option<&'a V> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// Merge one context version's base table with a tier's overrides.
///
/// The two context versions are kept apart deliberately: a WebGL1 context
/// serves 18 capabilities and a WebGL2 context 47, and serving the WebGL2 set
/// to a WebGL1 context would answer enums that context has no constant for,
/// where real Chrome returns `null` and raises `INVALID_ENUM`.
///
/// Those counts are the *static device capabilities* out of the 82 and up-to-132
/// parameters the captures enumerate. The remainder is per-context mutable
/// state and is deliberately absent from the tables, so it delegates to the
/// real backend — see `gpu-tier-gen`'s `SERVED_CAPS` for the rule and
/// `DELEGATED_PARAMS` for why each excluded name is excluded.
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

/// One tier's measured WebGPU adapter capabilities, owned so a caller's
/// [`WebgpuSpec`](crate::WebgpuSpec) can overlay onto them.
///
/// Deliberately **not** a field on [`GpuProfile`]. That type is public with
/// public fields, and `WebgpuSpec` already owns the caller-facing WebGPU
/// override path — a second one would give the same value two places to be set
/// from, and would make the shape of an internal table part of the public API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WebgpuAdapter {
    /// `GPUSupportedLimits`, keyed as the WebGPU IDL spells them
    /// (`maxTextureDimension2D`, `maxBufferSize`, ...).
    pub limits: BTreeMap<String, u64>,
    /// `GPUSupportedFeatures`, in Chrome's own iteration order.
    pub features: Vec<String>,
}

/// The WebGPU adapter capabilities a tier measured, or `None` when that tier's
/// machine has no adapter at all.
///
/// The sibling of [`profile_for_tier`] for the other GPU API: both read one
/// tier key out of the generated tables, so a renderer that resolves a tier
/// gets that tier's answer on both surfaces and cannot describe two devices.
///
/// **`None` is data, not a gap.** Real SwiftShader Chrome resolves
/// `requestAdapter()` to null, so that tier has no adapter to describe, and the
/// capture records exactly that. Callers must leave the host's adapter alone
/// rather than substituting a neighbouring tier's numbers — a persona claiming
/// a software rasterizer to WebGL while reporting a hardware adapter's limits
/// to WebGPU is the cross-API contradiction this resolution exists to prevent.
pub(crate) fn webgpu_for_tier(tier: Tier) -> Option<WebgpuAdapter> {
    let measured = lookup(tiers::WEBGPU_ADAPTERS, tier_key(tier))?.as_ref()?;
    Some(WebgpuAdapter {
        limits: measured
            .limits
            .iter()
            .map(|(k, v)| ((*k).to_string(), *v))
            .collect(),
        features: measured.features.iter().map(|f| (*f).to_string()).collect(),
    })
}

/// Enums the patch must resolve that the generated [`tiers::ENUM_NAMES`] table
/// does not carry, because the probe only walks the *core* `getParameter`
/// enums:
///
/// - the two `WEBGL_debug_renderer_info` names, which exist only once that
///   extension has been fetched, and
/// - the shader-type and precision-type arguments `getShaderPrecisionFormat`
///   takes, which are never passed to `getParameter` at all.
///
/// Both reach the page through the same number-to-name lookup as every other
/// value, so leaving them out does not merely omit them — it makes the JS fall
/// through to the real backend, serving the host's own unmasked pair and its
/// own shader precision. All ten are fixed by the WebGL spec, so they are
/// constants here rather than measurements.
const EXTRA_ENUMS: &[(u32, &str)] = &[
    (35632, "FRAGMENT_SHADER"),
    (35633, "VERTEX_SHADER"),
    (36336, "LOW_FLOAT"),
    (36337, "MEDIUM_FLOAT"),
    (36338, "HIGH_FLOAT"),
    (36339, "LOW_INT"),
    (36340, "MEDIUM_INT"),
    (36341, "HIGH_INT"),
    (37445, "UNMASKED_VENDOR_WEBGL"),
    (37446, "UNMASKED_RENDERER_WEBGL"),
];

/// `DRAW_BUFFER0`, which ES 3.0 fixes at `0x8825`, with `DRAW_BUFFER1` through
/// `DRAW_BUFFER15` following contiguously.
const DRAW_BUFFER0: u32 = 34853;

/// How many `DRAW_BUFFERn` enums the ES 3.0 spec defines.
const DRAW_BUFFER_COUNT: u32 = 16;

/// Name every `DRAW_BUFFERn` enum, whether or not a capture recorded it.
///
/// A capture only records the indices its own backend answered, so the
/// SwiftShader capture stops at `DRAW_BUFFER5` (that backend has 6). Taking the
/// names from captures alone would then leave a SwiftShader persona unable to
/// recognise `DRAW_BUFFER6`, and `webgl.js` cannot suppress an enum it cannot
/// name: the read would fall through to a host that has 8 and answer beside a
/// cap saying 6 exist.
///
/// These are spec constants rather than measurements, the same category as
/// [`EXTRA_ENUMS`] above, so the full range is declared here instead of being
/// inferred from whatever hardware happened to be probed.
fn draw_buffer_enums() -> impl Iterator<Item = (u32, String)> {
    (0..DRAW_BUFFER_COUNT).map(|i| (DRAW_BUFFER0 + i, format!("DRAW_BUFFER{i}")))
}

/// Serialize a profile into the JSON object `webgl.js` consumes.
///
/// Each value carries a `t` tag naming its GL type so the JS side builds the
/// right typed array; JSON alone cannot distinguish `Int32Array` from
/// `Float32Array`.
///
/// The unmasked vendor/renderer strings are folded into both parameter maps
/// (and their enums into `enumNames`, via [`EXTRA_ENUMS`]) rather than shipped
/// as their own fields: a page reads them through `getParameter` exactly like
/// every other value, so giving them a second path through the JS would buy
/// nothing.
pub(crate) fn profile_to_js(p: &GpuProfile) -> String {
    use serde_json::{Map, Value, json};

    fn val(v: &GlParam) -> Value {
        match v {
            GlParam::Int(i) => json!({"t": "i", "v": i}),
            GlParam::Float(f) => json!({"t": "f", "v": f}),
            GlParam::Bool(b) => json!({"t": "b", "v": b}),
            GlParam::Str(s) => json!({"t": "s", "v": s}),
            GlParam::IntPair(a) => json!({"t": "i32pair", "v": a}),
            GlParam::IntQuad(a) => json!({"t": "i32quad", "v": a}),
            GlParam::FloatPair(a) => json!({"t": "f32pair", "v": a}),
            GlParam::FloatQuad(a) => json!({"t": "f32quad", "v": a}),
            GlParam::IntList(a) => json!({"t": "u32list", "v": a}),
        }
    }
    let conv = |m: &BTreeMap<String, GlParam>| -> Map<String, Value> {
        let mut out: Map<String, Value> = m.iter().map(|(k, v)| (k.clone(), val(v))).collect();
        out.insert(
            "UNMASKED_VENDOR_WEBGL".to_string(),
            json!({"t": "s", "v": p.unmasked_vendor}),
        );
        out.insert(
            "UNMASKED_RENDERER_WEBGL".to_string(),
            json!({"t": "s", "v": p.unmasked_renderer}),
        );
        out
    };

    let mut enums = enum_names();
    for (num, name) in EXTRA_ENUMS {
        enums.insert(num.to_string(), Value::from(*name));
    }
    for (num, name) in draw_buffer_enums() {
        enums.insert(num.to_string(), Value::from(name));
    }

    json!({
        "params1": conv(&p.params_webgl1),
        "params2": conv(&p.params_webgl2),
        "precision": p.precision.iter().map(|(k, v)| {
            (k.clone(), json!([v.range_min, v.range_max, v.precision]))
        }).collect::<Map<_, _>>(),
        "extensions1": p.extensions_webgl1,
        "extensions2": p.extensions_webgl2,
        "enumNames": enums,
        "inertStubs": devices::inert_stubs(),
    })
    .to_string()
}

/// Numeric GL enum to parameter name, as the JS side indexes it.
///
/// A JS object is keyed by number, so aliases collapse: `BLEND_EQUATION` and
/// `BLEND_EQUATION_RGB` are both enum `32777`, and only one name survives into
/// the emitted object. That is safe **only while aliased names carry equal
/// values**, which is asserted below rather than assumed — if a future capture
/// ever gives an aliased pair different values, silently keeping one would
/// serve the wrong number for the other.
///
/// The comparison runs over each tier's *resolved* maps, both context
/// versions, because that is what the page actually reads. Consulting the
/// shared WebGL2 base alone would pass a tier override that moved one spelling
/// of a collapsed pair and not the other, and would never look at WebGL1 at
/// all. A present/absent split fails too: whichever spelling survives has to
/// be the one the table carries, or the lookup misses and the enum falls
/// through to the real backend.
fn enum_names() -> serde_json::Map<String, serde_json::Value> {
    let resolved: Vec<(Tier, GpuProfile)> = Tier::ALL
        .iter()
        .map(|tier| (*tier, profile_for_tier(*tier)))
        .collect();
    let mut out = serde_json::Map::new();
    let mut chosen: BTreeMap<u32, &str> = BTreeMap::new();
    for (num, name) in tiers::ENUM_NAMES {
        if let Some(prev) = chosen.insert(*num, *name) {
            // Both spellings must resolve to the same value, or collapsing
            // them changes what the page reads.
            for (tier, p) in &resolved {
                for (version, params) in
                    [("WebGL1", &p.params_webgl1), ("WebGL2", &p.params_webgl2)]
                {
                    assert_eq!(
                        params.get(prev),
                        params.get(*name),
                        "GL enum {num} aliases {prev} and {name}, which hold different \
                         {version} values on {tier:?}; collapsing them would serve the \
                         wrong one"
                    );
                }
            }
            continue;
        }
        out.insert(num.to_string(), serde_json::Value::from(*name));
    }
    out
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
        let p = profile_for_tier(types::Tier::MetalMacos);
        assert_eq!(p.params_webgl2["MAX_TEXTURE_SIZE"], GlParam::Int(16384));
        assert_eq!(
            p.params_webgl2["MAX_VIEWPORT_DIMS"],
            GlParam::IntPair([16384, 16384])
        );
    }

    #[test]
    fn d3d11_tier_resolves_its_own_measured_values() {
        // The D3D11 pair ANGLE derives from the feature-level constants:
        // D3D11_VIEWPORT_BOUNDS_MAX (32767) beside
        // D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION (16384). This is the pairing
        // neither other tier produces — SwiftShader is 8192/8192 and Metal
        // 16384/16384 — so it is what proves the tier is wired end to end
        // rather than silently inheriting a neighbour's numbers.
        let p = profile_for_tier(types::Tier::D3d11Fl11);
        assert_eq!(p.params_webgl2["MAX_TEXTURE_SIZE"], GlParam::Int(16384));
        assert_eq!(
            p.params_webgl2["MAX_VIEWPORT_DIMS"],
            GlParam::IntPair([32767, 32767])
        );
        // Measured 8 where both other tiers measured 4 — the value that moved
        // MAX_SAMPLES out of the shared base table when this tier landed.
        assert_eq!(p.params_webgl2["MAX_SAMPLES"], GlParam::Int(8));
    }

    #[test]
    fn the_vulkan_tier_resolves_its_own_device_derived_values() {
        // ANGLE's Vulkan backend reads its caps off `VkPhysicalDeviceLimits`,
        // so this tier's numbers are one Intel Iris Pro 580's under Mesa 25.2.8
        // rather than any backend's constants. These four are the ones no other
        // shipped tier reports, which is what proves the tier is wired end to
        // end instead of silently inheriting a neighbour's values: every other
        // capture measured 4 samples (8 on D3D11), 4 subpixel bits, and a
        // one-pixel line-width range.
        let p = profile_for_tier(types::Tier::VulkanMesaIntelIrisPro580);
        assert_eq!(p.params_webgl2["MAX_SAMPLES"], GlParam::Int(16));
        assert_eq!(p.params_webgl2["SUBPIXEL_BITS"], GlParam::Int(8));
        assert_eq!(p.params_webgl1["SUBPIXEL_BITS"], GlParam::Int(8));
        assert_eq!(
            p.params_webgl2["ALIASED_LINE_WIDTH_RANGE"],
            GlParam::FloatPair([1.0, 8.0])
        );
        assert_eq!(
            p.params_webgl2["ALIASED_POINT_SIZE_RANGE"],
            GlParam::FloatPair([1.0, 255.875])
        );
        // Closest to SwiftShader of the three tiers that preceded it — both run
        // through ANGLE's Vulkan backend — but not equal to it, which is the
        // whole reason it is a fourth tier rather than an identity row.
        let sw = profile_for_tier(types::Tier::SwiftShader);
        assert_ne!(p.params_webgl2, sw.params_webgl2);
        assert_ne!(
            p.params_webgl2,
            profile_for_tier(types::Tier::MetalMacos).params_webgl2
        );
        assert_ne!(
            p.params_webgl2,
            profile_for_tier(types::Tier::D3d11Fl11).params_webgl2
        );
    }

    #[test]
    fn base_values_reach_every_tier() {
        // A capability every tier agrees on lives only in base; resolution
        // must still surface it, or the ~19 shared WebGL2 capabilities would
        // silently vanish and fall through to the real backend.
        for tier in types::Tier::ALL {
            let p = profile_for_tier(*tier);
            // MAX_3D_TEXTURE_SIZE and MAX_ARRAY_TEXTURE_LAYERS are identical on
            // every shipped tier, so they exist only in the base table.
            // (MAX_SAMPLES used to be the second example and no longer is:
            // D3D11 measured 8 where SwiftShader and Metal both measured 4, so
            // it moved to the per-tier overrides.)
            assert_eq!(p.params_webgl2["MAX_3D_TEXTURE_SIZE"], GlParam::Int(2048));
            assert_eq!(
                p.params_webgl2["MAX_ARRAY_TEXTURE_LAYERS"],
                GlParam::Int(2048)
            );
        }
    }

    #[test]
    fn per_context_mutable_state_is_never_table_served() {
        // The C1 regression guard at the Rust layer: `getParameter` answers
        // both static device capabilities and per-context mutable state, and
        // only the first may come from a table. A frozen `BLEND` contradicts
        // the `gl.enable(gl.BLEND)` the page just called, and a frozen
        // `VIEWPORT` contradicts `gl.drawingBufferWidth` after a canvas
        // resize. Absent from the map is all it takes: `webgl.js` falls
        // through to the real backend for any name the table does not carry.
        for tier in types::Tier::ALL {
            let p = profile_for_tier(*tier);
            for name in [
                "BLEND",                      // gl.enable / gl.disable
                "VIEWPORT",                   // gl.viewport, and canvas resize
                "SCISSOR_BOX",                // gl.scissor
                "COLOR_CLEAR_VALUE",          // gl.clearColor
                "STENCIL_REF",                // gl.stencilFunc
                "UNPACK_FLIP_Y_WEBGL",        // gl.pixelStorei
                "ACTIVE_TEXTURE",             // gl.activeTexture
                "DRAW_BUFFER0",               // gl.drawBuffers
                "STENCIL_BITS",               // getContext({stencil: true})
                "SAMPLES",                    // getContext({antialias: false})
                "COMPRESSED_TEXTURE_FORMATS", // grows as extensions enable
            ] {
                for (version, params) in
                    [("WebGL1", &p.params_webgl1), ("WebGL2", &p.params_webgl2)]
                {
                    assert!(
                        !params.contains_key(name),
                        "{tier:?} serves {name} to a {version} context; it is mutable state, \
                         not a device capability, and must be delegated"
                    );
                }
            }
        }
    }

    #[test]
    fn precision_differs_where_it_was_measured_to_differ() {
        let sw = profile_for_tier(types::Tier::SwiftShader);
        let mt = profile_for_tier(types::Tier::MetalMacos);
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
        let p = profile_for_tier(types::Tier::MetalMacos);
        assert!(p.extensions_webgl1.iter().any(|e| e == "OES_texture_float"));
        assert!(
            !p.extensions_webgl2.iter().any(|e| e == "OES_texture_float"),
            "OES_texture_float is core in WebGL2; claiming it is a tell"
        );
    }

    #[test]
    fn serialized_profile_tags_every_value_with_its_gl_type() {
        // JSON cannot tell an Int32Array from a Float32Array, so the tag is
        // what stops the page from seeing the wrong typed array — one
        // instanceof check away from a tell.
        let p = profile_for_tier(types::Tier::MetalMacos);
        let js: serde_json::Value = serde_json::from_str(&profile_to_js(&p)).unwrap();
        assert_eq!(js["params2"]["MAX_VIEWPORT_DIMS"]["t"], "i32pair");
        assert_eq!(js["params2"]["ALIASED_POINT_SIZE_RANGE"]["t"], "f32pair");
        assert_eq!(js["params2"]["MAX_TEXTURE_LOD_BIAS"]["t"], "f");
        assert_eq!(js["params2"]["MAX_TEXTURE_SIZE"]["t"], "i");
        assert_eq!(js["params2"]["VERSION"]["t"], "s");
    }

    #[test]
    fn serialized_profile_tags_the_shapes_only_a_caller_can_supply() {
        // The served capabilities are all scalars, strings, and int/float
        // pairs — every boolean, quad, and list `getParameter` answers is
        // mutable state, and therefore delegated. Those shapes still have to
        // serialize correctly, because a caller can pin any of them through a
        // `GpuProfile` overlay.
        let p = GpuProfile::empty().overlay(GpuProfile {
            params_webgl2: [
                ("BLEND".to_string(), GlParam::Bool(true)),
                ("VIEWPORT".to_string(), GlParam::IntQuad([0, 0, 800, 600])),
                (
                    "COLOR_CLEAR_VALUE".to_string(),
                    GlParam::FloatQuad([0.0, 0.0, 0.0, 1.0]),
                ),
                (
                    "COMPRESSED_TEXTURE_FORMATS".to_string(),
                    GlParam::IntList(vec![33776]),
                ),
            ]
            .into_iter()
            .collect(),
            ..GpuProfile::empty()
        });
        let js: serde_json::Value = serde_json::from_str(&profile_to_js(&p)).unwrap();
        assert_eq!(js["params2"]["BLEND"]["t"], "b");
        assert_eq!(js["params2"]["VIEWPORT"]["t"], "i32quad");
        assert_eq!(js["params2"]["COLOR_CLEAR_VALUE"]["t"], "f32quad");
        assert_eq!(js["params2"]["COMPRESSED_TEXTURE_FORMATS"]["t"], "u32list");
    }

    #[test]
    fn serialized_profile_answers_the_unmasked_pair_through_get_parameter() {
        // The two strings a fingerprinter reads first are served like any
        // other parameter, so both halves have to be there: the value in the
        // params map, and 37445/37446 in enumNames to reach it.
        let mut p = profile_for_tier(types::Tier::MetalMacos);
        p.unmasked_vendor = "Google Inc. (Apple)".into();
        p.unmasked_renderer = "ANGLE (Apple, ...)".into();
        let js: serde_json::Value = serde_json::from_str(&profile_to_js(&p)).unwrap();
        assert_eq!(js["enumNames"]["37445"], "UNMASKED_VENDOR_WEBGL");
        assert_eq!(js["enumNames"]["37446"], "UNMASKED_RENDERER_WEBGL");
        for ctx in ["params1", "params2"] {
            assert_eq!(
                js[ctx]["UNMASKED_VENDOR_WEBGL"]["v"], "Google Inc. (Apple)",
                "{ctx} must carry the vendor"
            );
            assert_eq!(
                js[ctx]["UNMASKED_RENDERER_WEBGL"]["v"], "ANGLE (Apple, ...)",
                "{ctx} must carry the renderer"
            );
        }
    }

    #[test]
    fn every_profile_can_name_draw_buffers_past_its_own_cap() {
        // `webgl.js` cannot suppress an enum it cannot name, and the profile
        // that most needs the suppression is the one whose capture carries the
        // fewest of these names.
        //
        // SwiftShader has 6 draw buffers, so its capture records DRAW_BUFFER0
        // through DRAW_BUFFER5 and nothing else. A SwiftShader persona on an
        // 8-buffer host is asked for DRAW_BUFFER6; without a name for 34859 the
        // read falls through and the host answers 1029 beside a served cap of
        // 6, which is two lines for a page to check.
        for tier in types::Tier::ALL {
            let p = profile_for_tier(*tier);
            let js: serde_json::Value = serde_json::from_str(&profile_to_js(&p)).unwrap();
            for i in 0..16 {
                let num = (34853 + i).to_string();
                assert_eq!(
                    js["enumNames"][&num],
                    serde_json::json!(format!("DRAW_BUFFER{i}")),
                    "{tier:?} cannot name enum {num} (DRAW_BUFFER{i})"
                );
            }
        }
    }

    #[test]
    fn serialized_profile_can_name_both_halves_of_every_precision_key() {
        // The JS builds its precision key as
        // `enumNames[shaderType] + '/' + enumNames[precisionType]`, and none of
        // those eight enums is a getParameter enum, so the generated table does
        // not carry them. Without EXTRA_ENUMS the key is "undefined/undefined",
        // every lookup misses, and getShaderPrecisionFormat quietly serves the
        // real backend's precision instead of the tier's.
        let p = profile_for_tier(types::Tier::MetalMacos);
        let js: serde_json::Value = serde_json::from_str(&profile_to_js(&p)).unwrap();
        let named: std::collections::BTreeSet<&str> = js["enumNames"]
            .as_object()
            .unwrap()
            .values()
            .filter_map(serde_json::Value::as_str)
            .collect();
        for key in p.precision.keys() {
            let (shader, precision) = key.split_once('/').expect("precision keys are A/B");
            assert!(named.contains(shader), "no enum number names {shader}");
            assert!(
                named.contains(precision),
                "no enum number names {precision}"
            );
        }
    }

    #[test]
    fn every_draw_buffer_below_the_served_cap_is_named_in_the_enum_table() {
        // `webgl.js` fills the DRAW_BUFFERn gap — the indices the served
        // MAX_DRAW_BUFFERS claims but the host backend does not have — by
        // walking `enumNames` for names matching DRAW_BUFFERn. A name missing
        // from that table is an index the fill can never reach, so it would
        // keep answering null beside a cap that says it exists.
        for tier in types::Tier::ALL {
            let p = profile_for_tier(*tier);
            let GlParam::Int(max_draw) = p.params_webgl2["MAX_DRAW_BUFFERS"] else {
                panic!("{tier:?} does not serve MAX_DRAW_BUFFERS as an integer");
            };
            let js: serde_json::Value = serde_json::from_str(&profile_to_js(&p)).unwrap();
            let named: std::collections::BTreeSet<&str> = js["enumNames"]
                .as_object()
                .unwrap()
                .values()
                .filter_map(serde_json::Value::as_str)
                .collect();
            for i in 0..max_draw {
                let name = format!("DRAW_BUFFER{i}");
                assert!(
                    named.contains(name.as_str()),
                    "{tier:?} serves MAX_DRAW_BUFFERS {max_draw} but no enum number names \
                     {name}; webgl.js could not fill it if the backend lacks it"
                );
            }
        }
    }

    #[test]
    fn extra_enums_do_not_shadow_the_generated_table() {
        // EXTRA_ENUMS is inserted after the generated names, so a number
        // appearing in both would silently win here. That can only happen if a
        // regenerated table started carrying one of these — at which point the
        // hand-written entry is the one to delete.
        let generated: std::collections::BTreeSet<u32> =
            tiers::ENUM_NAMES.iter().map(|(n, _)| *n).collect();
        for (num, name) in EXTRA_ENUMS {
            assert!(
                !generated.contains(num),
                "{name} ({num}) is now in the generated table; drop it from EXTRA_ENUMS"
            );
        }
    }

    #[test]
    fn each_hardware_tier_resolves_its_own_measured_webgpu_adapter() {
        let metal = webgpu_for_tier(types::Tier::MetalMacos).expect("Metal has an adapter");
        let d3d11 = webgpu_for_tier(types::Tier::D3d11Fl11).expect("D3D11 has an adapter");

        // Both captures enumerate the same 36 limits; what identifies the
        // device is the values, so the assertion is that they differ rather
        // than that the maps do.
        assert_eq!(metal.limits.len(), 36);
        assert_eq!(d3d11.limits.len(), 36);
        assert_ne!(
            metal.limits, d3d11.limits,
            "two tiers resolving identical limits would mean one is being served the other's"
        );
        // The pair a script reads first, and the widest gap between the two: a
        // Metal buffer caps at 4 GiB - 4, D3D12's at exactly 2 GiB.
        assert_eq!(metal.limits["maxBufferSize"], 4_294_967_292);
        assert_eq!(d3d11.limits["maxBufferSize"], 2_147_483_648);
        // Backend-shaped rather than card-shaped, like the WebGL values:
        // Metal's argument-buffer tier allows 10 storage buffers per stage
        // where D3D12's binding tier allows 16.
        assert_eq!(metal.limits["maxStorageBuffersPerShaderStage"], 10);
        assert_eq!(d3d11.limits["maxStorageBuffersPerShaderStage"], 16);

        assert_eq!(metal.features.len(), 22);
        assert_eq!(d3d11.features.len(), 19);
        // The three Metal has and D3D11 does not — Apple GPUs support the
        // mobile-lineage compressed formats, desktop D3D12 does not.
        for only_metal in [
            "texture-compression-astc",
            "texture-compression-astc-sliced-3d",
            "texture-compression-etc2",
        ] {
            assert!(metal.features.iter().any(|f| f == only_metal));
            assert!(!d3d11.features.iter().any(|f| f == only_metal));
        }
    }

    #[test]
    fn the_tiers_probed_without_an_adapter_resolve_none_at_all() {
        // Not an oversight in either capture. Chrome on SwiftShader resolves
        // `requestAdapter()` to null, and Chrome on Linux does not enable
        // WebGPU by default, so neither machine had an adapter to describe.
        // Serving a neighbour's limits here would hand a persona that just told
        // WebGL it has a software rasterizer — or a machine with WebGPU off —
        // a hardware adapter's capabilities.
        assert_eq!(webgpu_for_tier(types::Tier::SwiftShader), None);
        assert_eq!(
            webgpu_for_tier(types::Tier::VulkanMesaIntelIrisPro580),
            None
        );
    }

    #[test]
    fn webgpu_features_keep_chromes_order_rather_than_being_sorted() {
        // `GPUSupportedFeatures` is setlike, so `Array.from(adapter.features)`
        // iterates in Chrome's insertion order — an order-sensitive
        // fingerprint input exactly like `getSupportedExtensions()`. A sorted
        // list is a different fingerprint from the one that was measured.
        let metal = webgpu_for_tier(types::Tier::MetalMacos).expect("Metal has an adapter");
        let mut sorted = metal.features.clone();
        sorted.sort();
        assert_ne!(
            metal.features, sorted,
            "the committed capture is genuinely unsorted, so this guard is not vacuous"
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
