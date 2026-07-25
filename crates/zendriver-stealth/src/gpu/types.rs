//! Value types for the GPU profile tables.
//!
//! `GlParam` mirrors the shapes `WebGLRenderingContext.getParameter` can
//! return. The float variants exist even though the probe captures cannot
//! produce them: `JSON.stringify` collapses `1.0` to `1`, so a capture shows
//! `ALIASED_POINT_SIZE_RANGE` as `[1, 1023]` when the WebGL spec declares it
//! `GLfloat[2]`. The generator applies the spec's declared type; emitting an
//! `Int32Array` where Chrome returns a `Float32Array` is a one-line tell.

use serde::{Deserialize, Serialize};

/// One `getParameter` return value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlParam {
    /// `GLint` / `GLint64` / `GLuint` scalar.
    Int(i64),
    /// `GLfloat` scalar (e.g. `LINE_WIDTH`, `MAX_TEXTURE_MAX_ANISOTROPY_EXT`).
    Float(f64),
    /// `GLboolean`.
    Bool(bool),
    /// `Int32Array(2)` (e.g. `MAX_VIEWPORT_DIMS`).
    IntPair([i32; 2]),
    /// `Float32Array(2)` (e.g. `ALIASED_POINT_SIZE_RANGE`, `DEPTH_RANGE`).
    FloatPair([f32; 2]),
    /// `Float32Array(4)` (e.g. `BLEND_COLOR`, `COLOR_CLEAR_VALUE`).
    FloatQuad([f32; 4]),
    /// `Int32Array(4)` (e.g. `VIEWPORT`, `SCISSOR_BOX`).
    IntQuad([i32; 4]),
    /// `Uint32Array` of variable length (`COMPRESSED_TEXTURE_FORMATS`).
    IntList(Vec<u32>),
    /// `DOMString` (e.g. `VERSION`, `SHADING_LANGUAGE_VERSION`).
    Str(String),
}

/// `'static`-friendly mirror of [`GlParam`] for the generated tables.
///
/// `GlParam` owns its `String`/`Vec` payloads so callers can build one at
/// runtime; a `static` table cannot. The two convert with [`GlParamRef::to_owned_param`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum GlParamRef {
    Int(i64),
    Float(f64),
    Bool(bool),
    IntPair([i32; 2]),
    FloatPair([f32; 2]),
    FloatQuad([f32; 4]),
    IntQuad([i32; 4]),
    IntList(&'static [u32]),
    Str(&'static str),
}

impl GlParamRef {
    /// Widen a table entry into the owned form callers see.
    pub(crate) fn to_owned_param(self) -> GlParam {
        match self {
            Self::Int(i) => GlParam::Int(i),
            Self::Float(f) => GlParam::Float(f),
            Self::Bool(b) => GlParam::Bool(b),
            Self::IntPair(v) => GlParam::IntPair(v),
            Self::FloatPair(v) => GlParam::FloatPair(v),
            Self::FloatQuad(v) => GlParam::FloatQuad(v),
            Self::IntQuad(v) => GlParam::IntQuad(v),
            Self::IntList(v) => GlParam::IntList(v.to_vec()),
            Self::Str(s) => GlParam::Str(s.to_string()),
        }
    }
}

/// One `getShaderPrecisionFormat` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShaderPrecision {
    pub range_min: i32,
    pub range_max: i32,
    pub precision: i32,
}

/// Where a table's values came from. Travels with the data so a reader can
/// tell a measured value from a derived one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Measured on a real browser.
    Probed { chrome: String, os: String },
    /// Derived from a documented source (an ANGLE constant, a spec floor).
    /// `source` cites it precisely enough to re-check.
    Derived { source: String },
}

/// A backend capability tier. Values cluster by tier, not by GPU model:
/// ANGLE computes them from constants branched on backend and feature level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
// `profile_for_tier` (Task 4) matches on `Tier`, but nothing outside tests
// constructs a `Tier` value yet — that starts with the persona/fingerprint
// wiring that calls `profile_for_tier` in production code (a later task).
#[allow(dead_code)]
pub(crate) enum Tier {
    SwiftShader,
    MetalAppleFamily3,
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn gl_param_covers_every_measured_shape() {
        // These are the six shapes the probe actually observed, plus the two
        // float forms the capture cannot distinguish (JSON collapses 1.0 -> 1).
        let _ = GlParam::Int(16384);
        let _ = GlParam::Bool(false);
        let _ = GlParam::Str("WebGL GLSL ES 3.00".into());
        let _ = GlParam::IntPair([16384, 16384]);
        let _ = GlParam::FloatPair([1.0, 511.0]);
        let _ = GlParam::FloatQuad([0.0, 0.0, 0.0, 0.0]);
        let _ = GlParam::IntQuad([0, 0, 300, 150]);
        let _ = GlParam::IntList(vec![]);
        let _ = GlParam::Float(1.0);
    }

    #[test]
    fn gl_param_round_trips_json() {
        let v = GlParam::FloatPair([1.0, 511.0]);
        let s = serde_json::to_string(&v).unwrap();
        let back: GlParam = serde_json::from_str(&s).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn shader_precision_round_trips_json() {
        let p = ShaderPrecision {
            range_min: 127,
            range_max: 127,
            precision: 23,
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: ShaderPrecision = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn provenance_records_where_a_value_came_from() {
        let p = Provenance::Probed {
            chrome: "150.0.7871.186".into(),
            os: "macos".into(),
        };
        assert!(
            serde_json::to_string(&p)
                .unwrap()
                .contains("150.0.7871.186")
        );
    }

    #[test]
    fn gl_param_ref_widens_to_the_owned_param_for_every_shape() {
        // GlParamRef is the 'static mirror the generated tables store; every
        // variant must widen back to the exact owned GlParam callers see.
        assert_eq!(GlParamRef::Int(5).to_owned_param(), GlParam::Int(5));
        assert_eq!(GlParamRef::Float(1.5).to_owned_param(), GlParam::Float(1.5));
        assert_eq!(GlParamRef::Bool(true).to_owned_param(), GlParam::Bool(true));
        assert_eq!(
            GlParamRef::IntPair([1, 2]).to_owned_param(),
            GlParam::IntPair([1, 2])
        );
        assert_eq!(
            GlParamRef::FloatPair([1.0, 2.0]).to_owned_param(),
            GlParam::FloatPair([1.0, 2.0])
        );
        assert_eq!(
            GlParamRef::FloatQuad([1.0, 2.0, 3.0, 4.0]).to_owned_param(),
            GlParam::FloatQuad([1.0, 2.0, 3.0, 4.0])
        );
        assert_eq!(
            GlParamRef::IntQuad([1, 2, 3, 4]).to_owned_param(),
            GlParam::IntQuad([1, 2, 3, 4])
        );
        assert_eq!(
            GlParamRef::IntList(&[1, 2, 3]).to_owned_param(),
            GlParam::IntList(vec![1, 2, 3])
        );
        assert_eq!(
            GlParamRef::Str("hi").to_owned_param(),
            GlParam::Str("hi".to_string())
        );
    }
}
