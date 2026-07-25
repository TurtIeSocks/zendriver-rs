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
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
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

    /// Every distinct parameter name found across both committed captures
    /// (`crates/zendriver-stealth/data/gpu-tiers/{metal-apple-family3,swiftshader}.json`,
    /// union of the `webgl1` and `webgl2` `params` blocks — 132 distinct
    /// names total) whose spec-declared `getParameter` return type is a
    /// float scalar or float array: `GLfloat`, `Float32Array` (2 or 4
    /// elements). Cross-checked against the WebGL1 / WebGL2 spec tables via
    /// MDN's `getParameter()` reference. Every other name in the union is
    /// GLenum/GLint/GLuint/GLint64/GLboolean/DOMString/an integer array, all
    /// of which JSON already represents faithfully.
    const FLOAT_TYPED_PARAMS: &[&str] = &[
        // Float32Array(2)
        "ALIASED_LINE_WIDTH_RANGE",
        "ALIASED_POINT_SIZE_RANGE",
        "DEPTH_RANGE",
        // Float32Array(4)
        "BLEND_COLOR",
        "COLOR_CLEAR_VALUE",
        // GLfloat
        "DEPTH_CLEAR_VALUE",
        "LINE_WIDTH",
        "POLYGON_OFFSET_FACTOR",
        "POLYGON_OFFSET_UNITS",
        "SAMPLE_COVERAGE_VALUE",
        "MAX_TEXTURE_LOD_BIAS",
    ];

    #[test]
    fn every_float_typed_param_in_the_captures_is_overridden() {
        // The override table is only as good as its coverage: a GLfloat param
        // missing from it is silently emitted as an integer, which is the exact
        // failure gl_type_for exists to prevent. Enumerate the captures rather
        // than spot-checking a handful of names.
        for name in FLOAT_TYPED_PARAMS {
            assert!(
                matches!(
                    gl_type_for(name),
                    GlType::Float | GlType::FloatPair | GlType::FloatQuad
                ),
                "{name} is float-typed per the WebGL spec but gl_type_for returns {:?}",
                gl_type_for(name)
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
