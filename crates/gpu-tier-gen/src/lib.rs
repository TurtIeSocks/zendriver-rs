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

fn nums(v: &Value) -> Vec<f64> {
    v.as_array()
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// Convert one captured value using its spec-declared GL type.
fn param_from_json(name: &str, v: &Value) -> Option<ParamValue> {
    let n = nums(v);
    Some(match gl_type_for(name) {
        GlType::Float => ParamValue::Float(v.as_f64()?),
        GlType::FloatPair => ParamValue::FloatPair([*n.first()? as f32, *n.get(1)? as f32]),
        GlType::FloatQuad => ParamValue::FloatQuad([
            *n.first()? as f32,
            *n.get(1)? as f32,
            *n.get(2)? as f32,
            *n.get(3)? as f32,
        ]),
        GlType::IntPair => ParamValue::IntPair([*n.first()? as i32, *n.get(1)? as i32]),
        GlType::IntQuad => ParamValue::IntQuad([
            *n.first()? as i32,
            *n.get(1)? as i32,
            *n.get(2)? as i32,
            *n.get(3)? as i32,
        ]),
        GlType::IntList => ParamValue::IntList(n.iter().map(|f| *f as u32).collect()),
        GlType::FromJson => match v {
            Value::Bool(b) => ParamValue::Bool(*b),
            Value::String(s) => ParamValue::Str(s.clone()),
            Value::Number(num) => ParamValue::Int(num.as_i64()?),
            _ => return None,
        },
    })
}

fn params_of(ctx: &Value) -> BTreeMap<String, ParamValue> {
    ctx["params"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| param_from_json(k, v).map(|p| (k.clone(), p)))
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
                .filter_map(|(k, v)| {
                    let n = nums(v);
                    Some((
                        k.clone(),
                        [*n.first()? as i32, *n.get(1)? as i32, *n.get(2)? as i32],
                    ))
                })
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
}
