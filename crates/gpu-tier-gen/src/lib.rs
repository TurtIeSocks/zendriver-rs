//! Offline generator for the vendored GPU capability tier tables.
//! Run via `cargo run -p gpu-tier-gen`. NOT published.

use std::collections::BTreeMap;

use serde_json::Value;

/// How a captured JSON value should be typed in the emitted table.
///
/// `FromJson` means the capture's own shape is faithful. The named variants
/// exist for params whose GL type JSON cannot represent: `JSON.stringify`
/// writes `1.0` as `1`, so every `GLfloat` param would otherwise be emitted
/// as an integer and produce an `Int32Array` where Chrome returns a
/// `Float32Array`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlType {
    FromJson,
    Float,
    FloatPair,
    FloatQuad,
    IntPair,
    IntQuad,
    IntList,
}

/// Spec-declared GL type for params JSON cannot round-trip faithfully.
///
/// Sourced from the WebGL 1.0 and 2.0 specifications' `getParameter` tables,
/// not from any capture. Anything absent takes the capture's own shape.
pub fn gl_type_for(name: &str) -> GlType {
    match name {
        "ALIASED_LINE_WIDTH_RANGE" | "ALIASED_POINT_SIZE_RANGE" | "DEPTH_RANGE" => {
            GlType::FloatPair
        }
        "BLEND_COLOR" | "COLOR_CLEAR_VALUE" => GlType::FloatQuad,
        "DEPTH_CLEAR_VALUE"
        | "LINE_WIDTH"
        | "POLYGON_OFFSET_FACTOR"
        | "POLYGON_OFFSET_UNITS"
        | "SAMPLE_COVERAGE_VALUE"
        | "MAX_TEXTURE_LOD_BIAS"
        | "MAX_TEXTURE_MAX_ANISOTROPY_EXT" => GlType::Float,
        "MAX_VIEWPORT_DIMS" => GlType::IntPair,
        "VIEWPORT" | "SCISSOR_BOX" => GlType::IntQuad,
        "COMPRESSED_TEXTURE_FORMATS" => GlType::IntList,
        _ => GlType::FromJson,
    }
}

/// One emitted parameter value.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    IntPair([i32; 2]),
    FloatPair([f32; 2]),
    FloatQuad([f32; 4]),
    IntQuad([i32; 4]),
    IntList(Vec<u32>),
    Str(String),
}

/// Everything one tier contributes.
#[derive(Debug, Clone)]
pub struct TierData {
    pub name: String,
    pub provenance: String,
    pub params_webgl1: BTreeMap<String, ParamValue>,
    pub params_webgl2: BTreeMap<String, ParamValue>,
    pub precision: BTreeMap<String, [i32; 3]>,
    pub extensions_webgl1: Vec<String>,
    pub extensions_webgl2: Vec<String>,
}

/// Read exactly `count` numbers out of a JSON array, or explain why the
/// capture's shape doesn't match what the spec-declared GL type needs.
///
/// `name` is the parameter/precision key, folded into the error so a failure
/// names what broke rather than just vanishing.
fn exact_nums(name: &str, v: &Value, count: usize) -> Result<Vec<f64>, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| format!("{name}: expected a {count}-element array, got {v}"))?;
    if arr.len() != count {
        return Err(format!(
            "{name}: expected {count} elements, got {} in {v}",
            arr.len()
        ));
    }
    arr.iter()
        .enumerate()
        .map(|(i, x)| {
            x.as_f64()
                .ok_or_else(|| format!("{name}: element {i} is not a number in {v}"))
        })
        .collect()
}

/// Read every number out of a JSON array of unknown length (used for
/// `COMPRESSED_TEXTURE_FORMATS`, which has no fixed arity).
fn all_nums(name: &str, v: &Value) -> Result<Vec<f64>, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| format!("{name}: expected an array, got {v}"))?;
    arr.iter()
        .enumerate()
        .map(|(i, x)| {
            x.as_f64()
                .ok_or_else(|| format!("{name}: element {i} is not a number in {v}"))
        })
        .collect()
}

/// Narrow an `f64` to `i32`, failing rather than silently truncating a value
/// the `as` cast can't represent exactly.
fn checked_i32(name: &str, f: f64) -> Result<i32, String> {
    let out = f as i32;
    (out as f64 == f)
        .then_some(out)
        .ok_or_else(|| format!("{name}: value {f} does not fit in i32"))
}

/// Narrow an `f64` to `u32`; see [`checked_i32`].
fn checked_u32(name: &str, f: f64) -> Result<u32, String> {
    let out = f as u32;
    (out as f64 == f)
        .then_some(out)
        .ok_or_else(|| format!("{name}: value {f} does not fit in u32"))
}

/// Narrow an `f64` to `f32`; see [`checked_i32`].
fn checked_f32(name: &str, f: f64) -> Result<f32, String> {
    let out = f as f32;
    (out as f64 == f)
        .then_some(out)
        .ok_or_else(|| format!("{name}: value {f} does not fit in f32 without loss"))
}

/// Convert one captured value using its spec-declared GL type.
///
/// Returns `Err` naming the parameter and the raw capture value whenever the
/// shape doesn't match — a param this can't convert must be surfaced, not
/// dropped from the table.
fn param_from_json(name: &str, v: &Value) -> Result<ParamValue, String> {
    Ok(match gl_type_for(name) {
        GlType::Float => ParamValue::Float(
            v.as_f64()
                .ok_or_else(|| format!("{name}: expected a number, got {v}"))?,
        ),
        GlType::FloatPair => {
            let n = exact_nums(name, v, 2)?;
            ParamValue::FloatPair([checked_f32(name, n[0])?, checked_f32(name, n[1])?])
        }
        GlType::FloatQuad => {
            let n = exact_nums(name, v, 4)?;
            ParamValue::FloatQuad([
                checked_f32(name, n[0])?,
                checked_f32(name, n[1])?,
                checked_f32(name, n[2])?,
                checked_f32(name, n[3])?,
            ])
        }
        GlType::IntPair => {
            let n = exact_nums(name, v, 2)?;
            ParamValue::IntPair([checked_i32(name, n[0])?, checked_i32(name, n[1])?])
        }
        GlType::IntQuad => {
            let n = exact_nums(name, v, 4)?;
            ParamValue::IntQuad([
                checked_i32(name, n[0])?,
                checked_i32(name, n[1])?,
                checked_i32(name, n[2])?,
                checked_i32(name, n[3])?,
            ])
        }
        GlType::IntList => {
            let n = all_nums(name, v)?;
            ParamValue::IntList(
                n.iter()
                    .map(|f| checked_u32(name, *f))
                    .collect::<Result<_, _>>()?,
            )
        }
        GlType::FromJson => match v {
            Value::Bool(b) => ParamValue::Bool(*b),
            Value::String(s) => ParamValue::Str(s.clone()),
            Value::Number(num) => ParamValue::Int(
                num.as_i64()
                    .ok_or_else(|| format!("{name}: number {num} does not fit in i64"))?,
            ),
            other => return Err(format!("{name}: unsupported JSON shape {other}")),
        },
    })
}

/// Read the three-element `[range_min, range_max, precision]` triple the
/// WebGL spec returns for a `getShaderPrecisionFormat` query.
fn precision_triple(name: &str, v: &Value) -> Result<[i32; 3], String> {
    let n = exact_nums(name, v, 3)?;
    Ok([
        checked_i32(name, n[0])?,
        checked_i32(name, n[1])?,
        checked_i32(name, n[2])?,
    ])
}

/// Panic with a message naming the offending key and raw value. Acceptable
/// here: this is a build-time-only generator (`publish = false`, never
/// shipped), and a capture entry this can't convert must stop the generator
/// loudly rather than silently vanish from the emitted table.
#[allow(clippy::panic)]
fn fail_loud<T>(result: Result<T, String>) -> T {
    result.unwrap_or_else(|e| panic!("gpu-tier-gen: {e}"))
}

fn params_of(ctx: &Value) -> BTreeMap<String, ParamValue> {
    ctx["params"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), fail_loud(param_from_json(k, v))))
                .collect()
        })
        .unwrap_or_default()
}

fn strings_of(ctx: &Value, key: &str) -> Vec<String> {
    let mut v: Vec<String> = ctx[key]
        .as_array()
        .map(|a| {
            fail_loud(
                a.iter()
                    .enumerate()
                    .map(|(i, s)| {
                        s.as_str()
                            .map(String::from)
                            .ok_or_else(|| format!("{key}[{i}]: expected a string, got {s}"))
                    })
                    .collect(),
            )
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Parse one probe capture into the emitter's input.
pub fn tier_from_capture(name: &str, provenance: &str, capture: &Value) -> TierData {
    let w1 = &capture["webgl1"];
    let w2 = &capture["webgl2"];
    let precision = w2["precision"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), fail_loud(precision_triple(k, v))))
                .collect()
        })
        .unwrap_or_default();
    TierData {
        name: name.to_string(),
        provenance: provenance.to_string(),
        params_webgl1: params_of(w1),
        params_webgl2: params_of(w2),
        precision,
        extensions_webgl1: strings_of(w1, "extensions"),
        extensions_webgl2: strings_of(w2, "extensions"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_typed_params_are_not_inferred_from_json() {
        // The capture shows `[1, 1023]` because JSON.stringify collapses 1.0.
        // The WebGL spec declares GLfloat[2], and that is what must win.
        assert_eq!(gl_type_for("ALIASED_POINT_SIZE_RANGE"), GlType::FloatPair);
        assert_eq!(gl_type_for("ALIASED_LINE_WIDTH_RANGE"), GlType::FloatPair);
        assert_eq!(gl_type_for("DEPTH_RANGE"), GlType::FloatPair);
        assert_eq!(gl_type_for("BLEND_COLOR"), GlType::FloatQuad);
        assert_eq!(gl_type_for("COLOR_CLEAR_VALUE"), GlType::FloatQuad);
        assert_eq!(gl_type_for("LINE_WIDTH"), GlType::Float);
        assert_eq!(gl_type_for("MAX_TEXTURE_MAX_ANISOTROPY_EXT"), GlType::Float);
        // WebGL2 / OpenGL ES 3.0 spec: glGetFloatv(GL_MAX_TEXTURE_LOD_BIAS),
        // enum 0x84FD. Present in both committed captures as the integer 15,
        // which is exactly the JSON-collapse case gl_type_for exists to catch.
        assert_eq!(gl_type_for("MAX_TEXTURE_LOD_BIAS"), GlType::Float);
    }

    #[test]
    fn int_typed_arrays_stay_integer() {
        assert_eq!(gl_type_for("MAX_VIEWPORT_DIMS"), GlType::IntPair);
        assert_eq!(gl_type_for("VIEWPORT"), GlType::IntQuad);
        assert_eq!(gl_type_for("SCISSOR_BOX"), GlType::IntQuad);
        assert_eq!(gl_type_for("COMPRESSED_TEXTURE_FORMATS"), GlType::IntList);
    }

    #[test]
    fn unlisted_params_fall_back_to_the_json_shape() {
        // Most params are plain GLint/GLboolean/DOMString; the override table
        // only needs to name the ones JSON cannot represent faithfully.
        assert_eq!(gl_type_for("MAX_TEXTURE_SIZE"), GlType::FromJson);
        assert_eq!(gl_type_for("CULL_FACE"), GlType::FromJson);
    }

    #[test]
    fn capture_parses_into_tier_data() {
        let capture = serde_json::json!({
            "webgl1": {
                "params": {"MAX_TEXTURE_SIZE": 8192, "ALIASED_POINT_SIZE_RANGE": [1, 1023]},
                "precision": {"VERTEX_SHADER/MEDIUM_FLOAT": [15, 15, 10]},
                "extensions": ["OES_texture_float"]
            },
            "webgl2": {
                "params": {"MAX_TEXTURE_SIZE": 8192},
                "precision": {"VERTEX_SHADER/MEDIUM_FLOAT": [15, 15, 10]},
                "extensions": []
            }
        });
        let t = tier_from_capture("swiftshader", "probed: test", &capture);
        assert_eq!(t.name, "swiftshader");
        assert_eq!(t.params_webgl1["MAX_TEXTURE_SIZE"], ParamValue::Int(8192));
        // The spec-declared float type wins over the JSON integer shape.
        assert_eq!(
            t.params_webgl1["ALIASED_POINT_SIZE_RANGE"],
            ParamValue::FloatPair([1.0, 1023.0])
        );
        assert_eq!(t.precision["VERTEX_SHADER/MEDIUM_FLOAT"], [15, 15, 10]);
        assert_eq!(t.extensions_webgl1, vec!["OES_texture_float".to_string()]);
    }

    /// Params confirmed against the WebGL 1.0 / 2.0 spec's `getParameter`
    /// table (cross-checked via MDN's `getParameter()` reference) to return a
    /// plain `GLenum`/`GLint`/`GLuint`/`GLint64`/`GLboolean`/`DOMString` —
    /// i.e. a shape JSON already represents faithfully, so `gl_type_for`'s
    /// `FromJson` default is correct for them.
    ///
    /// Derived mechanically from the committed captures, not hand-guessed:
    /// this is every name in the two captures' `params` union minus every
    /// name `gl_type_for` already classifies with a non-`FromJson` override
    /// arm. See `every_captured_param_is_classified` below, which is what
    /// actually enforces that this list (plus the override arms) stays
    /// exhaustive as captures change.
    const VERIFIED_PLAIN_PARAMS: &[&str] = &[
        "ACTIVE_TEXTURE",
        "ALPHA_BITS",
        "BLEND",
        "BLEND_DST_ALPHA",
        "BLEND_DST_RGB",
        "BLEND_EQUATION",
        "BLEND_EQUATION_ALPHA",
        "BLEND_EQUATION_RGB",
        "BLEND_SRC_ALPHA",
        "BLEND_SRC_RGB",
        "BLUE_BITS",
        "CULL_FACE",
        "CULL_FACE_MODE",
        "DEPTH_BITS",
        "DEPTH_FUNC",
        "DEPTH_TEST",
        "DEPTH_WRITEMASK",
        "DITHER",
        "DRAW_BUFFER0",
        "DRAW_BUFFER1",
        "DRAW_BUFFER2",
        "DRAW_BUFFER3",
        "DRAW_BUFFER4",
        "DRAW_BUFFER5",
        "DRAW_BUFFER6",
        "DRAW_BUFFER7",
        "FRAGMENT_SHADER_DERIVATIVE_HINT",
        "FRONT_FACE",
        "GENERATE_MIPMAP_HINT",
        "GREEN_BITS",
        "IMPLEMENTATION_COLOR_READ_FORMAT",
        "IMPLEMENTATION_COLOR_READ_TYPE",
        "MAX_3D_TEXTURE_SIZE",
        "MAX_ARRAY_TEXTURE_LAYERS",
        "MAX_CLIENT_WAIT_TIMEOUT_WEBGL",
        "MAX_COLOR_ATTACHMENTS",
        "MAX_COMBINED_FRAGMENT_UNIFORM_COMPONENTS",
        "MAX_COMBINED_TEXTURE_IMAGE_UNITS",
        "MAX_COMBINED_UNIFORM_BLOCKS",
        "MAX_COMBINED_VERTEX_UNIFORM_COMPONENTS",
        "MAX_CUBE_MAP_TEXTURE_SIZE",
        "MAX_DRAW_BUFFERS",
        "MAX_ELEMENTS_INDICES",
        "MAX_ELEMENTS_VERTICES",
        "MAX_ELEMENT_INDEX",
        "MAX_FRAGMENT_INPUT_COMPONENTS",
        "MAX_FRAGMENT_UNIFORM_BLOCKS",
        "MAX_FRAGMENT_UNIFORM_COMPONENTS",
        "MAX_FRAGMENT_UNIFORM_VECTORS",
        "MAX_PROGRAM_TEXEL_OFFSET",
        "MAX_RENDERBUFFER_SIZE",
        "MAX_SAMPLES",
        "MAX_SERVER_WAIT_TIMEOUT",
        "MAX_TEXTURE_IMAGE_UNITS",
        "MAX_TEXTURE_SIZE",
        "MAX_TRANSFORM_FEEDBACK_INTERLEAVED_COMPONENTS",
        "MAX_TRANSFORM_FEEDBACK_SEPARATE_ATTRIBS",
        "MAX_TRANSFORM_FEEDBACK_SEPARATE_COMPONENTS",
        "MAX_UNIFORM_BLOCK_SIZE",
        "MAX_UNIFORM_BUFFER_BINDINGS",
        "MAX_VARYING_COMPONENTS",
        "MAX_VARYING_VECTORS",
        "MAX_VERTEX_ATTRIBS",
        "MAX_VERTEX_OUTPUT_COMPONENTS",
        "MAX_VERTEX_TEXTURE_IMAGE_UNITS",
        "MAX_VERTEX_UNIFORM_BLOCKS",
        "MAX_VERTEX_UNIFORM_COMPONENTS",
        "MAX_VERTEX_UNIFORM_VECTORS",
        "MIN_PROGRAM_TEXEL_OFFSET",
        "PACK_ALIGNMENT",
        "PACK_ROW_LENGTH",
        "PACK_SKIP_PIXELS",
        "PACK_SKIP_ROWS",
        "POLYGON_OFFSET_FILL",
        "RASTERIZER_DISCARD",
        "READ_BUFFER",
        "RED_BITS",
        "RENDERER",
        "SAMPLES",
        "SAMPLE_ALPHA_TO_COVERAGE",
        "SAMPLE_BUFFERS",
        "SAMPLE_COVERAGE",
        "SAMPLE_COVERAGE_INVERT",
        "SCISSOR_TEST",
        "SHADING_LANGUAGE_VERSION",
        "STENCIL_BACK_FAIL",
        "STENCIL_BACK_FUNC",
        "STENCIL_BACK_PASS_DEPTH_FAIL",
        "STENCIL_BACK_PASS_DEPTH_PASS",
        "STENCIL_BACK_REF",
        "STENCIL_BACK_VALUE_MASK",
        "STENCIL_BACK_WRITEMASK",
        "STENCIL_BITS",
        "STENCIL_CLEAR_VALUE",
        "STENCIL_FAIL",
        "STENCIL_FUNC",
        "STENCIL_PASS_DEPTH_FAIL",
        "STENCIL_PASS_DEPTH_PASS",
        "STENCIL_REF",
        "STENCIL_TEST",
        "STENCIL_VALUE_MASK",
        "STENCIL_WRITEMASK",
        "SUBPIXEL_BITS",
        "TRANSFORM_FEEDBACK_ACTIVE",
        "TRANSFORM_FEEDBACK_PAUSED",
        "UNIFORM_BUFFER_OFFSET_ALIGNMENT",
        "UNPACK_ALIGNMENT",
        "UNPACK_COLORSPACE_CONVERSION_WEBGL",
        "UNPACK_FLIP_Y_WEBGL",
        "UNPACK_IMAGE_HEIGHT",
        "UNPACK_PREMULTIPLY_ALPHA_WEBGL",
        "UNPACK_ROW_LENGTH",
        "UNPACK_SKIP_IMAGES",
        "UNPACK_SKIP_PIXELS",
        "UNPACK_SKIP_ROWS",
        "VENDOR",
        "VERSION",
    ];

    /// Non-circular replacement for a prior version of this guard that
    /// hand-copied `gl_type_for`'s own float arms into a list and checked the
    /// list against itself — which can never catch a capture containing a
    /// spec-float parameter nobody classified, precisely the failure
    /// `gl_type_for` exists to prevent.
    ///
    /// This test instead reads the *committed captures* (the ground truth
    /// that changes independently of this file) and requires every parameter
    /// name found in them to be accounted for by one of two
    /// independently-maintained, human-authored sources: `gl_type_for`'s
    /// override arms, or `VERIFIED_PLAIN_PARAMS`. A capture introducing an
    /// unclassified parameter now fails the build until a human looks up its
    /// spec type — it cannot be silently absorbed by either list because
    /// neither list is derived from "whatever the capture happens to
    /// contain."
    #[test]
    fn every_captured_param_is_classified() {
        const SWIFTSHADER: &str =
            include_str!("../../zendriver-stealth/data/gpu-tiers/swiftshader.json");
        const METAL_APPLE_FAMILY3: &str =
            include_str!("../../zendriver-stealth/data/gpu-tiers/metal-apple-family3.json");

        let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for raw in [SWIFTSHADER, METAL_APPLE_FAMILY3] {
            let doc: Value =
                serde_json::from_str(raw).expect("committed capture must be valid JSON");
            for ctx in ["webgl1", "webgl2"] {
                if let Some(params) = doc["capture"][ctx]["params"].as_object() {
                    names.extend(params.keys().cloned());
                }
            }
        }
        assert!(
            !names.is_empty(),
            "no parameter names found in the committed captures — the include_str! path or \
             the capture's JSON shape probably changed out from under this test"
        );

        for name in names {
            let overridden = gl_type_for(&name) != GlType::FromJson;
            let verified_plain = VERIFIED_PLAIN_PARAMS.contains(&name.as_str());
            assert!(
                overridden || verified_plain,
                "parameter `{name}` appears in a committed capture but is classified by \
                 neither gl_type_for nor VERIFIED_PLAIN_PARAMS. Look up its return type in the \
                 WebGL spec's getParameter table and add it to the override arms (if it is a \
                 float or an array JSON cannot round-trip) or to VERIFIED_PLAIN_PARAMS (if it \
                 is a plain GLint/GLboolean/DOMString)."
            );
            assert!(
                !(overridden && verified_plain),
                "parameter `{name}` is listed in both gl_type_for's override arms and \
                 VERIFIED_PLAIN_PARAMS — it can only be one or the other, pick the correct one \
                 and remove it from the other list"
            );
        }
    }

    #[test]
    fn an_unconvertible_capture_value_fails_loudly() {
        let capture = serde_json::json!({
            "webgl1": {
                "params": {"MAX_VIEWPORT_DIMS": "not-an-array"},
                "precision": {}, "extensions": [], "enums": {}
            },
            "webgl2": {"params": {}, "precision": {}, "extensions": [], "enums": {}}
        });
        let err =
            std::panic::catch_unwind(|| tier_from_capture("swiftshader", "probed: test", &capture));
        assert!(
            err.is_err(),
            "a malformed capture value must not be silently dropped"
        );
    }
}
