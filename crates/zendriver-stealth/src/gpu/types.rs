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
///
/// Every shape `getParameter` can return is represented, even though the
/// currently generated tables only construct `Int`/`Float`/`Str`/`IntPair`/
/// `FloatPair`. The served set is static device capabilities only, and today
/// none of those is a boolean, a quad, or a list — but the generator's
/// emitter covers all nine, so a future capture that promotes such a
/// capability must not need a type change here to be emittable.
#[allow(dead_code)]
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

/// One tier's measured WebGPU adapter capabilities, in the `'static` form the
/// generated tables store — the same split [`GlParamRef`] makes against
/// [`GlParam`], and for the same reason: a `static` cannot own its `String`s.
/// Widens to [`WebgpuAdapter`](crate::gpu::WebgpuAdapter) through
/// [`webgpu_for_tier`](crate::gpu::webgpu_for_tier).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WebgpuAdapterRef {
    /// `GPUSupportedLimits` entries, sorted by name.
    pub limits: &'static [(&'static str, u64)],
    /// `GPUSupportedFeatures` names, in the order Chrome itself iterates them.
    pub features: &'static [&'static str],
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
pub(crate) enum Tier {
    SwiftShader,
    /// ANGLE's Metal backend on **macOS** — every Mac, Intel and Apple silicon
    /// alike. Probed on an Apple M4 Pro, but the values are not that machine's:
    /// `DisplayMtl.mm:727-731` sets `max2DTextureSize = 16384`,
    /// `maxVaryingVectors = 31 - 1` and `maxVertexOutputComponents = 124 - 4`
    /// as compile-time constants under `#if TARGET_OS_OSX ||
    /// TARGET_OS_MACCATALYST`, with no device query anywhere in that arm.
    ///
    /// It was called `MetalAppleFamily3` until the `#if` was read properly. The
    /// `supportsAppleGPUFamily(3)` test that picks 16384 over 8192 lives in the
    /// `#else` arm, which is **iOS**, so no Mac ever reaches it and the old name
    /// pointed at a branch macOS cannot take. Renaming it is what stops someone
    /// capturing an Intel Mac in the expectation of a second, lower Metal tier —
    /// it would reproduce this one exactly.
    MetalMacos,
    /// Direct3D 11 at feature level 11_0 or above. Named for the backend and
    /// feature level rather than the card it was probed on (an RTX 4090):
    /// ANGLE's D3D11 renderer derives every one of these values from
    /// `D3D11_REQ_*` constants branched on the feature level, so an Intel UHD,
    /// an AMD RX, and an NVIDIA RTX at FL11+ all report the same numbers.
    D3d11Fl11,
    /// ANGLE's **Vulkan** backend on Linux, on an Intel Iris Pro Graphics 580
    /// (Skylake GT4e) under Mesa 25.2.8. Probed on Linux Mint with Chrome
    /// 150.0.7871.186 in a NUC6i7KYK.
    ///
    /// **Named for the device and the driver, because a Vulkan tier does not
    /// generalize.** The other two hardware tiers do, and that is why they are
    /// named for a backend: `renderer11_utils.cpp` branches on the
    /// `D3D_FEATURE_LEVEL`, and `DisplayMtl.mm`'s `TARGET_OS_OSX` arm uses
    /// plain compile-time constants — neither asks the device anything. ANGLE's
    /// Vulkan backend does the opposite: `vk_caps_utils.cpp` fills its caps
    /// straight from `VkPhysicalDeviceLimits` (`max2DTextureSize` is
    /// `min(limitsVk.maxFramebufferWidth, limitsVk.maxImageDimension2D)`, the
    /// viewport bounds come from `limitsVk.maxViewportDimensions`, and that one
    /// file reads `limitsVk.` around 99 times). The limits it reads are the
    /// driver's answer for this physical device, so the **Mesa version is part
    /// of what determines these numbers** as much as the silicon is: a
    /// different Intel part, or the same part on a different Mesa release, is a
    /// different tier and needs its own capture. Nothing here may be reused for
    /// "Linux" or for "Vulkan" in general.
    ///
    /// The measurement bears that out. This capture is closest to
    /// [`SwiftShader`](Self::SwiftShader) — 7 of 82 WebGL1 parameters differ
    /// and 21 of 132 WebGL2, against 10/26 for [`D3d11Fl11`](Self::D3d11Fl11)
    /// and 9/23 for [`MetalMacos`](Self::MetalMacos) — because SwiftShader's
    /// renderer string says `Vulkan 1.3.0` and it runs through the same ANGLE
    /// backend. Those 21 remaining WebGL2 differences between two
    /// Vulkan-backed captures on one Chrome build are exactly the
    /// device-derived limits, which is the empirical form of what the source
    /// predicts.
    ///
    /// **No WebGPU adapter**, and that is a measurement rather than a hole:
    /// Chrome does not enable WebGPU by default on Linux, so `navigator.gpu`
    /// exists and `requestAdapter()` resolves null. The capture records the
    /// explicit null and the tier serves no limits or features, exactly as the
    /// SwiftShader tier does.
    VulkanMesaIntelIrisPro580,
}

impl Tier {
    /// Every shipped tier, in one place: the invariant checks and the tests
    /// that sweep "all tiers" iterate this, so adding a tier cannot quietly
    /// leave one of them behind.
    pub(crate) const ALL: &'static [Tier] = &[
        Tier::SwiftShader,
        Tier::MetalMacos,
        Tier::D3d11Fl11,
        Tier::VulkanMesaIntelIrisPro580,
    ];
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
