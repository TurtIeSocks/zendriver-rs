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

/// Parameters the tier tables serve: static device capabilities.
///
/// `getParameter` answers two categories, and the probe
/// (`crates/zendriver/examples/probe_gpu.rs`) captures both indiscriminately
/// because it walks the context prototype. Only this first category may be
/// table-served:
///
/// - **Static device capability** — implementation-fixed for the life of the
///   context and unchangeable by any GL call or context attribute. These
///   differ per device and carry the fingerprint entropy. Listed here.
/// - **Per-context mutable state** — the current blend/stencil/scissor/pixel-
///   store/viewport settings. Every real Chrome reports the same defaults, so
///   they carry no entropy, and they change as the page draws. Listed in
///   [`DELEGATED_PARAMS`] and emitted nowhere: serving them freezes state the
///   page just set (`gl.enable(gl.BLEND); gl.getParameter(gl.BLEND)` → `false`
///   forever) and breaks state-caching renderers that save and restore through
///   `getParameter`.
///
/// The rule, applied against the WebGL 1.0 / 2.0 specifications' state tables:
/// a parameter is served **only** if it is implementation-fixed and cannot be
/// changed by any GL call or context attribute. When uncertain, delegate —
/// delegating loses a little entropy, freezing a mutable value is a live
/// detector.
///
/// Delegation needs no support in `webgl.js`: a name absent from the table
/// already falls through to the real backend, exactly as an unknown enum does.
///
/// Measured against the committed captures, this partition keeps **every**
/// parameter whose value differs between the tiers (10 of 10 in WebGL1, 29 of
/// 31 in WebGL2). The two exceptions are `DRAW_BUFFER6`/`7`, which differ only
/// in *presence* — and presence is a property of the **host backend**, not of
/// the number this table serves, so delegating them does cost something the
/// served `MAX_DRAW_BUFFERS` cannot buy back. `webgl.js` closes that one gap
/// itself (see `drawBufferGap` there), answering what a real device of the
/// claimed size reports — which follows the bound framebuffer — for any index
/// below the served cap that the backend has no constant for. Everything
/// else delegated is byte-identical across every measured backend, so the
/// entropy cost of delegating it is zero.
pub const SERVED_CAPS: &[&str] = &[
    // Implementation-dependent ranges and limits (ES 2.0 Table 6.20 /
    // ES 3.0 Table 6.35, "Implementation Dependent Values"). Nothing in the
    // API writes to any of them.
    "ALIASED_LINE_WIDTH_RANGE",
    "ALIASED_POINT_SIZE_RANGE",
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
    "MAX_TEXTURE_LOD_BIAS",
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
    "MAX_VIEWPORT_DIMS",
    "MIN_PROGRAM_TEXEL_OFFSET",
    "SUBPIXEL_BITS",
    "UNIFORM_BUFFER_OFFSET_ALIGNMENT",
    // Identity strings. Fixed per implementation; `RENDERER`/`VENDOR` are the
    // masked pair ("WebKit WebGL" / "WebKit"), and the unmasked pair reaches
    // the page separately (see `gpu::profile_to_js`).
    "RENDERER",
    "SHADING_LANGUAGE_VERSION",
    "VENDOR",
    "VERSION",
];

/// Parameters the tier tables must **never** serve, with the reason each one
/// is disqualified. See [`SERVED_CAPS`] for the rule.
///
/// This list exists so the coverage guard can tell "classified as delegated"
/// apart from "nobody has looked at it yet": a capture introducing an
/// unclassified name fails the build rather than being silently dropped.
pub const DELEGATED_PARAMS: &[&str] = &[
    // --- Per-context mutable state -----------------------------------------
    // Each is written by an ordinary GL call, so a frozen answer contradicts
    // the call the page just made. The naming makes the writer obvious:
    // `blendFunc`/`blendEquation`/`blendColor`, `stencil*`, `pixelStorei` (the
    // PACK_*/UNPACK_* family), `enable`/`disable` (the GLboolean toggles),
    // `viewport`, `scissor`, `depthRange`, `clearColor`/`clearDepth`/
    // `clearStencil`, `hint`, `activeTexture`, `drawBuffers`, `readBuffer`,
    // `lineWidth`, `polygonOffset`, `sampleCoverage`, `cullFace`, `frontFace`,
    // `depthFunc`/`depthMask`, and the transform-feedback begin/pause pair.
    "ACTIVE_TEXTURE",
    "BLEND",
    "BLEND_COLOR",
    "BLEND_DST_ALPHA",
    "BLEND_DST_RGB",
    "BLEND_EQUATION",
    "BLEND_EQUATION_ALPHA",
    "BLEND_EQUATION_RGB",
    "BLEND_SRC_ALPHA",
    "BLEND_SRC_RGB",
    "COLOR_CLEAR_VALUE",
    "CULL_FACE",
    "CULL_FACE_MODE",
    "DEPTH_CLEAR_VALUE",
    "DEPTH_FUNC",
    "DEPTH_RANGE",
    "DEPTH_TEST",
    "DEPTH_WRITEMASK",
    "DITHER",
    // DRAW_BUFFERn is written by `drawBuffers()` and is per-framebuffer, so
    // freezing the value would contradict every multiple-render-target page on
    // its first `drawBuffers` call. Its *presence* does carry entropy, and the
    // served MAX_DRAW_BUFFERS does not supply it: presence comes from the host
    // backend, which can have fewer buffers than the table claims (SwiftShader
    // 6 under a persona serving 8 is the default pairing). `webgl.js` fills
    // exactly that gap — see `drawBufferGap` — rather than serving the value
    // from here, so the page reads its own `drawBuffers` writes back for every
    // index the backend really has.
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
    "LINE_WIDTH",
    "PACK_ALIGNMENT",
    "PACK_ROW_LENGTH",
    "PACK_SKIP_PIXELS",
    "PACK_SKIP_ROWS",
    "POLYGON_OFFSET_FACTOR",
    "POLYGON_OFFSET_FILL",
    "POLYGON_OFFSET_UNITS",
    "RASTERIZER_DISCARD",
    "READ_BUFFER",
    "SAMPLE_ALPHA_TO_COVERAGE",
    "SAMPLE_COVERAGE",
    "SAMPLE_COVERAGE_INVERT",
    "SAMPLE_COVERAGE_VALUE",
    "SCISSOR_BOX",
    "SCISSOR_TEST",
    "STENCIL_BACK_FAIL",
    "STENCIL_BACK_FUNC",
    "STENCIL_BACK_PASS_DEPTH_FAIL",
    "STENCIL_BACK_PASS_DEPTH_PASS",
    "STENCIL_BACK_REF",
    "STENCIL_BACK_VALUE_MASK",
    "STENCIL_BACK_WRITEMASK",
    "STENCIL_CLEAR_VALUE",
    "STENCIL_FAIL",
    "STENCIL_FUNC",
    "STENCIL_PASS_DEPTH_FAIL",
    "STENCIL_PASS_DEPTH_PASS",
    "STENCIL_REF",
    "STENCIL_TEST",
    "STENCIL_VALUE_MASK",
    "STENCIL_WRITEMASK",
    "TRANSFORM_FEEDBACK_ACTIVE",
    "TRANSFORM_FEEDBACK_PAUSED",
    "UNPACK_ALIGNMENT",
    "UNPACK_COLORSPACE_CONVERSION_WEBGL",
    "UNPACK_FLIP_Y_WEBGL",
    "UNPACK_IMAGE_HEIGHT",
    "UNPACK_PREMULTIPLY_ALPHA_WEBGL",
    "UNPACK_ROW_LENGTH",
    "UNPACK_SKIP_IMAGES",
    "UNPACK_SKIP_PIXELS",
    "UNPACK_SKIP_ROWS",
    // VIEWPORT also tracks the canvas: it is reset to the drawing-buffer size
    // when the context is created and after a resize, so a frozen
    // `[0, 0, 300, 150]` contradicts `gl.drawingBufferWidth` on any page that
    // sizes its canvas.
    "VIEWPORT",
    // --- Determined by the context attributes the page asked for ------------
    // `getContext('webgl', {...})` decides these, so no table value can be
    // right for every caller: `{stencil: true}` makes STENCIL_BITS 8 where the
    // captures say 0, `{alpha: false}` makes ALPHA_BITS 0 where they say 8,
    // and `{antialias: false}` makes SAMPLES 0 and SAMPLE_BUFFERS 0 where they
    // say 4 and 1. In WebGL2 they additionally track whatever framebuffer is
    // bound. Never promote these.
    "ALPHA_BITS",
    "BLUE_BITS",
    "DEPTH_BITS",
    "GREEN_BITS",
    "RED_BITS",
    "SAMPLES",
    "SAMPLE_BUFFERS",
    "STENCIL_BITS",
    // --- Grows as extensions are enabled ------------------------------------
    // The probe reads parameters before enabling any extension, so every
    // capture records an empty list. In real Chrome the array grows with each
    // compressed-texture extension the page enables, so a table value pins it
    // empty forever: `getExtension('WEBGL_compressed_texture_s3tc')` succeeds,
    // `compressedTexImage2D` works, and the format list stays empty — a
    // three-line contradiction.
    "COMPRESSED_TEXTURE_FORMATS",
    // --- Determined by the bound read framebuffer ---------------------------
    // Judgment call, resolved toward delegating. The spec's state tables file
    // these under "Implementation Dependent Values", but both ES 2.0 (via
    // OES_read_format) and ES 3.0 define them against the *current read
    // surface*: bind an FBO backed by an RGBA8UI texture and real WebGL2
    // answers RGBA_INTEGER/UNSIGNED_INT instead of RGBA/UNSIGNED_BYTE. Every
    // committed capture also records the identical 6408/5121 pair, so serving
    // them adds no entropy at all while adding a mutable value to freeze.
    "IMPLEMENTATION_COLOR_READ_FORMAT",
    "IMPLEMENTATION_COLOR_READ_TYPE",
];

/// Whether a parameter is a static device capability the tables may serve.
#[must_use]
pub fn is_served_cap(name: &str) -> bool {
    SERVED_CAPS.contains(&name)
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

/// One tier's measured WebGPU adapter capabilities.
///
/// Both fields come from the same probe run as that tier's WebGL blocks, on
/// the same machine, so what a page cross-checks between the two APIs is a
/// pairing Chrome really produced rather than one assembled here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebgpuAdapter {
    /// `GPUSupportedLimits`, every one a non-negative integer per the WebGPU
    /// IDL (`GPUSize64` / `GPUSize32`).
    pub limits: BTreeMap<String, u64>,
    /// `GPUSupportedFeatures`, in the capture's order — see [`strings_of`] for
    /// why an order is preserved rather than sorted.
    pub features: Vec<String>,
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
    /// Numeric GL enum per parameter name, read from the WebGL2 context's
    /// `enums` block. GL enum numbers are spec-fixed constants, not
    /// context-dependent, so one context's block covers both.
    pub enums: BTreeMap<String, u32>,
    /// The tier's WebGPU adapter, or `None` when the probed machine had none.
    ///
    /// `None` is a measurement, not a hole: Chrome on SwiftShader resolves
    /// `requestAdapter()` to null, and the `swiftshader` capture records
    /// exactly that. Keeping the two apart in the type is what stops the
    /// emitter from papering over an absent adapter with an empty one.
    pub webgpu: Option<WebgpuAdapter>,
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

/// Borrow a required object block out of a capture, failing loudly when it is
/// absent.
///
/// Every block the capture format defines is mandatory. A missing one used to
/// yield an empty map and a silently hollow tier — the exact failure mode the
/// rest of this file is written to avoid, and one that would ship a tier
/// serving nothing rather than stopping the build. Two more captures are
/// planned, so a truncated or hand-edited one must stop the generator.
fn object_of<'a>(ctx: &'a Value, key: &str) -> &'a serde_json::Map<String, Value> {
    fail_loud(
        ctx[key]
            .as_object()
            .ok_or_else(|| format!("capture block `{key}` is missing or not an object")),
    )
}

fn params_of(ctx: &Value) -> BTreeMap<String, ParamValue> {
    object_of(ctx, "params")
        .iter()
        .map(|(k, v)| (k.clone(), fail_loud(param_from_json(k, v))))
        .collect()
}

/// Parse one `enums` entry: a GL enum number, always representable as `u32`.
fn enum_num(name: &str, v: &Value) -> Result<u32, String> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("{name}: expected a u32 enum value, got {v}"))
}

fn enums_of(ctx: &Value) -> BTreeMap<String, u32> {
    object_of(ctx, "enums")
        .iter()
        .map(|(k, v)| (k.clone(), fail_loud(enum_num(k, v))))
        .collect()
}

/// Parse one `limits` entry. Every WebGPU limit is a `GPUSize64`/`GPUSize32`
/// in the IDL, so a negative or fractional value means a malformed capture
/// rather than a limit worth serving.
fn limit_num(name: &str, v: &Value) -> Result<u64, String> {
    v.as_u64()
        .ok_or_else(|| format!("{name}: expected a non-negative integer WebGPU limit, got {v}"))
}

/// Parse the capture's `adapter` block into the tier's WebGPU capabilities.
///
/// An explicit `null` is a valid, meaningful answer — the machine had no
/// WebGPU adapter, which is what Chrome on SwiftShader reports — and yields
/// `None`. A *missing* key is not: it means the probe never wrote the field,
/// so it fails loudly like every other absent block, rather than silently
/// downgrading a hardware tier to "no adapter".
fn webgpu_of(capture: &Value) -> Option<WebgpuAdapter> {
    let adapter = fail_loud(capture.get("adapter").ok_or_else(|| {
        "capture block `adapter` is missing; a tier with no WebGPU adapter must record an \
         explicit null"
            .to_string()
    }));
    if adapter.is_null() {
        return None;
    }
    let limits = object_of(adapter, "limits")
        .iter()
        .map(|(k, v)| (k.clone(), fail_loud(limit_num(k, v))))
        .collect();
    Some(WebgpuAdapter {
        limits,
        features: strings_of(adapter, "features"),
    })
}

/// Read a capture's string array **in capture order**.
///
/// Deliberately not sorted. This reads the extension lists, and
/// `getSupportedExtensions()` order is a standard order-sensitive fingerprint
/// input: Chrome emits `EXT_shader_texture_lod` before `EXT_sRGB`, which no
/// sort produces. Blink's order is already deterministic, so preserving the
/// capture's order is both more faithful than sorting and equally reproducible
/// for the drift guard. Sorting stays where order genuinely carries no
/// meaning — the parameter and enum tables, which are `BTreeMap`s keyed for
/// lookup.
fn strings_of(ctx: &Value, key: &str) -> Vec<String> {
    let arr = fail_loud(
        ctx[key]
            .as_array()
            .ok_or_else(|| format!("capture block `{key}` is missing or not an array")),
    );
    fail_loud(
        arr.iter()
            .enumerate()
            .map(|(i, s)| {
                s.as_str()
                    .map(String::from)
                    .ok_or_else(|| format!("{key}[{i}]: expected a string, got {s}"))
            })
            .collect(),
    )
}

/// The committed probe captures, embedded rather than read from disk.
///
/// `include_str!` resolves against this source file, so the generator and its
/// drift guard provably read the same bytes and neither depends on the
/// working directory it is invoked from.
const CAPTURES: &[(&str, &str)] = &[
    (
        "swiftshader",
        include_str!("../../zendriver-stealth/data/gpu-tiers/swiftshader.json"),
    ),
    (
        "metal-macos",
        include_str!("../../zendriver-stealth/data/gpu-tiers/metal-macos.json"),
    ),
    (
        "d3d11-fl11",
        include_str!("../../zendriver-stealth/data/gpu-tiers/d3d11-fl11.json"),
    ),
];

/// Parse every committed capture into the emitter's input.
///
/// # Panics
///
/// Panics, loudly and by name, if a committed capture is not valid JSON or is
/// missing a block — a hollow tier must stop the generator, not ship.
#[must_use]
pub fn committed_tiers() -> Vec<TierData> {
    CAPTURES
        .iter()
        .map(|(name, raw)| {
            let doc: Value = fail_loud(
                serde_json::from_str(raw)
                    .map_err(|e| format!("{name}: capture is not valid JSON: {e}")),
            );
            let provenance = doc["provenance"].as_str().unwrap_or("unknown").to_string();
            tier_from_capture(name, &provenance, &doc["capture"])
        })
        .collect()
}

/// Parse one probe capture into the emitter's input.
pub fn tier_from_capture(name: &str, provenance: &str, capture: &Value) -> TierData {
    let w1 = &capture["webgl1"];
    let w2 = &capture["webgl2"];
    let precision = object_of(w2, "precision")
        .iter()
        .map(|(k, v)| (k.clone(), fail_loud(precision_triple(k, v))))
        .collect();
    TierData {
        name: name.to_string(),
        provenance: provenance.to_string(),
        params_webgl1: params_of(w1),
        params_webgl2: params_of(w2),
        precision,
        extensions_webgl1: strings_of(w1, "extensions"),
        extensions_webgl2: strings_of(w2, "extensions"),
        enums: enums_of(w2),
        webgpu: webgpu_of(capture),
    }
}

/// Split tiers into the values every tier agrees on and the per-tier
/// exceptions.
///
/// A param goes to base only when **every** tier has it and all values match.
/// A param absent from any tier stays an override for the tiers that do have
/// it, so a tier never inherits a parameter it must not report.
pub fn split_base_and_overrides(
    tiers: &[TierData],
    pick: fn(&TierData) -> &BTreeMap<String, ParamValue>,
) -> (
    BTreeMap<String, ParamValue>,
    BTreeMap<String, BTreeMap<String, ParamValue>>,
) {
    let mut base = BTreeMap::new();
    let mut overrides: BTreeMap<String, BTreeMap<String, ParamValue>> = tiers
        .iter()
        .map(|t| (t.name.clone(), BTreeMap::new()))
        .collect();

    let all_names: std::collections::BTreeSet<&String> =
        tiers.iter().flat_map(|t| pick(t).keys()).collect();

    for name in all_names {
        let present: Vec<&ParamValue> = tiers.iter().filter_map(|t| pick(t).get(name)).collect();
        let universal = present.len() == tiers.len();
        let identical = present.windows(2).all(|w| w[0] == w[1]);
        if universal && identical {
            base.insert(name.clone(), present[0].clone());
        } else {
            for t in tiers {
                if let Some(v) = pick(t).get(name) {
                    overrides
                        .get_mut(&t.name)
                        .expect("tier key inserted above")
                        .insert(name.clone(), v.clone());
                }
            }
        }
    }
    (base, overrides)
}

fn lit(v: &ParamValue) -> String {
    match v {
        ParamValue::Int(i) => format!("GlParam::Int({i})"),
        ParamValue::Float(f) => format!("GlParam::Float({f:?})"),
        ParamValue::Bool(b) => format!("GlParam::Bool({b})"),
        ParamValue::IntPair([a, b]) => format!("GlParam::IntPair([{a}, {b}])"),
        ParamValue::FloatPair([a, b]) => format!("GlParam::FloatPair([{a:?}, {b:?}])"),
        ParamValue::FloatQuad([a, b, c, d]) => {
            format!("GlParam::FloatQuad([{a:?}, {b:?}, {c:?}, {d:?}])")
        }
        ParamValue::IntQuad([a, b, c, d]) => format!("GlParam::IntQuad([{a}, {b}, {c}, {d}])"),
        ParamValue::IntList(v) => format!(
            "GlParam::IntList(&[{}])",
            v.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
        ),
        ParamValue::Str(s) => format!("GlParam::Str({s:?})"),
    }
}

/// Drop every parameter that is not a static device capability.
///
/// See [`SERVED_CAPS`]. Applied at emission rather than at parse time so the
/// capture still parses in full — a malformed value fails loudly, and the
/// coverage guard still sees every captured name.
fn retain_served_caps(t: &TierData) -> TierData {
    let keep = |m: &BTreeMap<String, ParamValue>| -> BTreeMap<String, ParamValue> {
        m.iter()
            .filter(|(k, _)| is_served_cap(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    TierData {
        params_webgl1: keep(&t.params_webgl1),
        params_webgl2: keep(&t.params_webgl2),
        ..t.clone()
    }
}

/// Emit the whole `tiers.rs`. Deterministic: every map is a `BTreeMap`, and
/// the extension lists arrive in the captures' own (deterministic) order.
///
/// Only [`SERVED_CAPS`] reach the emitted tables. Per-context mutable state is
/// filtered out here, which is all the delegation takes: `webgl.js` already
/// falls through to the real backend for any name the table does not carry.
pub fn emit_rust(tiers: &[TierData]) -> String {
    let served: Vec<TierData> = tiers.iter().map(retain_served_caps).collect();
    let tiers: &[TierData] = &served;
    let mut s = String::new();
    s.push_str("// Generated by `cargo run -p gpu-tier-gen`. DO NOT EDIT.\n");
    s.push_str("// Sources:\n");
    for t in tiers {
        s.push_str(&format!("//   {} — {}\n", t.name, t.provenance));
    }
    // Nothing in the crate reads these tables until profile_for_tier (Task
    // 4); allow, rather than fake a consumer just to satisfy the lint. The
    // nested tuple-of-tuple shapes below are inherent to the generated data
    // and not worth a named type alias.
    s.push_str("#![allow(dead_code)]\n");
    s.push_str("#![allow(clippy::type_complexity)]\n");
    s.push_str("\nuse super::types::{GlParamRef as GlParam, WebgpuAdapterRef};\n\n");

    // Both context versions get their own tables. Of the captured parameters
    // a WebGL1 context exposes 82 and a WebGL2 context up to 132, of which 18
    // and 47 respectively are served capabilities; sharing one table would
    // answer WebGL2-only enums on a WebGL1 context, where real Chrome returns
    // null and raises INVALID_ENUM.
    for (suffix, pick) in [
        (
            "WEBGL1",
            (|t: &TierData| &t.params_webgl1) as fn(&TierData) -> &BTreeMap<String, ParamValue>,
        ),
        ("WEBGL2", |t: &TierData| &t.params_webgl2),
    ] {
        let (base, overrides) = split_base_and_overrides(tiers, pick);
        s.push_str(&format!(
            "/// {suffix} values every tier agrees on. Sorted, binary-searchable.\n"
        ));
        s.push_str(&format!(
            "pub(crate) static BASE_PARAMS_{suffix}: &[(&str, GlParam)] = &[\n"
        ));
        for (k, v) in &base {
            s.push_str(&format!("    ({k:?}, {}),\n", lit(v)));
        }
        s.push_str("];\n\n");

        s.push_str(&format!(
            "/// Per-tier {suffix} exceptions to the base, keyed by tier name.\n"
        ));
        s.push_str(&format!(
            "pub(crate) static PARAM_OVERRIDES_{suffix}: &[(&str, &[(&str, GlParam)])] = &[\n"
        ));
        for (tier, params) in &overrides {
            s.push_str(&format!("    ({tier:?}, &[\n"));
            for (k, v) in params {
                s.push_str(&format!("        ({k:?}, {}),\n", lit(v)));
            }
            s.push_str("    ]),\n");
        }
        s.push_str("];\n\n");
    }

    s.push_str("/// `getShaderPrecisionFormat` results per tier.\n");
    s.push_str("pub(crate) static PRECISION: &[(&str, &[(&str, [i32; 3])])] = &[\n");
    for t in tiers {
        s.push_str(&format!("    ({:?}, &[\n", t.name));
        for (k, p) in &t.precision {
            s.push_str(&format!(
                "        ({k:?}, [{}, {}, {}]),\n",
                p[0], p[1], p[2]
            ));
        }
        s.push_str("    ]),\n");
    }
    s.push_str("];\n\n");

    for (label, pick) in [("EXTENSIONS_WEBGL1", true), ("EXTENSIONS_WEBGL2", false)] {
        s.push_str(&format!(
            "/// Extension list per tier for {}.\n",
            if pick { "WebGL1" } else { "WebGL2" }
        ));
        s.push_str(&format!(
            "pub(crate) static {label}: &[(&str, &[&str])] = &[\n"
        ));
        for t in tiers {
            let list = if pick {
                &t.extensions_webgl1
            } else {
                &t.extensions_webgl2
            };
            s.push_str(&format!("    ({:?}, &[\n", t.name));
            for e in list {
                s.push_str(&format!("        {e:?},\n"));
            }
            s.push_str("    ]),\n");
        }
        s.push_str("];\n\n");
    }

    // A parameter *name*'s enum number is fixed by the WebGL spec, so it must
    // agree across every tier that reports it; a mismatch means a capture is
    // malformed and must fail loudly rather than pick a winner. The reverse
    // is not true and must not be asserted: a single enum number can
    // legitimately answer to more than one name (e.g. `BLEND_EQUATION` and
    // `BLEND_EQUATION_RGB` are both 0x8009 per spec), so the table keeps
    // every distinct (number, name) pair rather than deduplicating by number.
    s.push_str("/// Numeric GL enum -> parameter name. Fixed by the WebGL spec.\n");
    s.push_str("///\n");
    s.push_str("/// A single enum number can legitimately answer to more than one name (e.g.\n");
    s.push_str(
        "/// `BLEND_EQUATION` and `BLEND_EQUATION_RGB` are both 0x8009 per spec), so this\n",
    );
    s.push_str("/// is a flat pair-list rather than a map keyed by number. Do not collapse it\n");
    s.push_str("/// into a map (e.g. a JS object keyed by enum number) without first checking\n");
    s.push_str("/// that every pair of aliased names holds an equal value.\n");
    s.push_str("pub(crate) static ENUM_NAMES: &[(u32, &str)] = &[\n");
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    let mut pairs: std::collections::BTreeSet<(u32, &str)> = std::collections::BTreeSet::new();
    for t in tiers {
        for (name, num) in &t.enums {
            if let Some(prev) = seen.insert(name, *num) {
                assert_eq!(
                    prev, *num,
                    "{name} maps to two different enum numbers; a capture is malformed"
                );
            }
            pairs.insert((*num, name));
        }
    }
    for (num, name) in &pairs {
        s.push_str(&format!("    ({num}, {name:?}),\n"));
    }
    s.push_str("];\n\n");

    // Rows sorted by tier name, and the limits within each row sorted by their
    // own (they arrive in a BTreeMap). The feature list is the one thing here
    // that keeps the capture's order — see the emitted doc comment.
    let webgpu: BTreeMap<&str, &Option<WebgpuAdapter>> =
        tiers.iter().map(|t| (t.name.as_str(), &t.webgpu)).collect();
    s.push_str("/// Measured WebGPU adapter capabilities per tier.\n");
    s.push_str("///\n");
    s.push_str("/// `None` is a measurement, not a missing row: that tier's Chrome resolves\n");
    s.push_str("/// `navigator.gpu.requestAdapter()` to null, so it has no adapter to describe.\n");
    s.push_str(
        "/// Filling it from a neighbouring tier would hand a persona claiming a software\n",
    );
    s.push_str(
        "/// rasterizer a hardware adapter's limits — the cross-API contradiction the tier\n",
    );
    s.push_str("/// resolution exists to prevent.\n");
    s.push_str("///\n");
    s.push_str(
        "/// Each row's values were probed in the same run as that tier's WebGL blocks, on\n",
    );
    s.push_str("/// one machine, so the two APIs' answers are a pairing Chrome really produced.\n");
    s.push_str("///\n");
    s.push_str("/// Limits are sorted by name. The feature list keeps the capture's own order,\n");
    s.push_str(
        "/// because `GPUSupportedFeatures` is setlike and iterates in Chrome's insertion\n",
    );
    s.push_str("/// order — the same order-sensitive fingerprint input the extension lists are.\n");
    s.push_str("pub(crate) static WEBGPU_ADAPTERS: &[(&str, Option<WebgpuAdapterRef>)] = &[\n");
    for (name, adapter) in &webgpu {
        match adapter {
            None => s.push_str(&format!("    ({name:?}, None),\n")),
            Some(a) => {
                s.push_str(&format!("    ({name:?}, Some(WebgpuAdapterRef {{\n"));
                s.push_str("        limits: &[\n");
                for (k, v) in &a.limits {
                    s.push_str(&format!("            ({k:?}, {v}),\n"));
                }
                s.push_str("        ],\n        features: &[\n");
                for f in &a.features {
                    s.push_str(&format!("            {f:?},\n"));
                }
                s.push_str("        ],\n    })),\n");
            }
        }
    }
    s.push_str("];\n");
    s
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
        // enum 0x84FD. Present in every committed capture as an integer (15 on
        // SwiftShader and Metal, 2 on D3D11), which is exactly the JSON-collapse
        // case gl_type_for exists to catch.
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
                "extensions": ["OES_texture_float"],
                "enums": {}
            },
            "webgl2": {
                "params": {"MAX_TEXTURE_SIZE": 8192},
                "precision": {"VERTEX_SHADER/MEDIUM_FLOAT": [15, 15, 10]},
                "extensions": [],
                "enums": {}
            },
            "adapter": null
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
        for name in captured_param_names() {
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

    /// Every parameter name found in the committed captures, both context
    /// versions. The ground truth the classification guards are checked
    /// against — it changes when a capture changes, independently of the
    /// hand-authored lists in this file.
    ///
    /// Reads [`CAPTURES`] rather than its own `include_str!` list, so a tier
    /// added to the generator cannot be left out of the guard: the previous
    /// hand-maintained pair would have silently kept passing while a third
    /// capture's parameters went unclassified.
    fn captured_param_names() -> std::collections::BTreeSet<String> {
        let mut names = std::collections::BTreeSet::new();
        for (_, raw) in CAPTURES {
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
        names
    }

    /// The served/delegated partition must be total over the captures.
    ///
    /// Same shape as `every_captured_param_is_classified`: the captures are
    /// the independent ground truth, and both lists are hand-authored from the
    /// spec, so a capture introducing a name nobody has classified fails the
    /// build rather than being silently dropped from the tables (or, worse,
    /// silently served).
    #[test]
    fn every_captured_param_is_served_or_delegated() {
        for name in captured_param_names() {
            let served = SERVED_CAPS.contains(&name.as_str());
            let delegated = DELEGATED_PARAMS.contains(&name.as_str());
            assert!(
                served || delegated,
                "parameter `{name}` appears in a committed capture but is in neither \
                 SERVED_CAPS nor DELEGATED_PARAMS. Look it up in the WebGL 1.0 / 2.0 spec's \
                 state tables and classify it: SERVED_CAPS only if it is implementation-fixed \
                 and no GL call or context attribute can change it, DELEGATED_PARAMS (with the \
                 reason) otherwise. When uncertain, delegate — delegating loses a little \
                 entropy, freezing a mutable value is a live detector."
            );
            assert!(
                !(served && delegated),
                "parameter `{name}` is in both SERVED_CAPS and DELEGATED_PARAMS — it can only \
                 be one, pick the correct one and remove it from the other list"
            );
        }
    }

    /// Mutable state must not survive into the emitted tables, whatever the
    /// capture contains. These are the exact reads C1 was reported against:
    /// `gl.enable(gl.BLEND); gl.getParameter(gl.BLEND)` answering a frozen
    /// `false`, and `VIEWPORT` answering `[0, 0, 300, 150]` beside an
    /// 800-wide drawing buffer.
    #[test]
    fn emitted_tables_carry_no_mutable_state() {
        let capture = serde_json::json!({
            "webgl1": {
                "params": {
                    "MAX_TEXTURE_SIZE": 8192,
                    "BLEND": false,
                    "VIEWPORT": [0, 0, 300, 150],
                    "STENCIL_BITS": 0,
                    "COMPRESSED_TEXTURE_FORMATS": [],
                    "UNPACK_FLIP_Y_WEBGL": false
                },
                "precision": {}, "extensions": [], "enums": {}
            },
            "webgl2": {
                "params": {
                    "MAX_TEXTURE_SIZE": 8192,
                    "BLEND": false,
                    "VIEWPORT": [0, 0, 300, 150],
                    "DRAW_BUFFER0": 1029,
                    "SAMPLES": 4,
                    "IMPLEMENTATION_COLOR_READ_FORMAT": 6408
                },
                "precision": {}, "extensions": [], "enums": {}
            },
            "adapter": null
        });
        let out = emit_rust(&[tier_from_capture("swiftshader", "probed: test", &capture)]);
        assert!(
            out.contains("MAX_TEXTURE_SIZE"),
            "a static capability must still be served"
        );
        for delegated in [
            "BLEND",
            "VIEWPORT",
            "STENCIL_BITS",
            "COMPRESSED_TEXTURE_FORMATS",
            "UNPACK_FLIP_Y_WEBGL",
            "DRAW_BUFFER0",
            "SAMPLES",
            "IMPLEMENTATION_COLOR_READ_FORMAT",
        ] {
            assert!(
                !out.contains(&format!("(\"{delegated}\", GlParam")),
                "`{delegated}` reached the emitted table; delegated params must not appear in \
                 tiers.rs at all, so webgl.js falls through to the real backend"
            );
        }
    }

    /// The committed generated file, for the guards that check it directly.
    const COMMITTED_TIERS_RS: &str = include_str!("../../zendriver-stealth/src/gpu/tiers.rs");

    /// Collapse the only differences `rustfmt` can introduce between what
    /// [`emit_rust`] writes and what lands in the committed file: whitespace,
    /// and the trailing comma rustfmt drops when it collapses a list onto one
    /// line. Everything token-level — every name, number and nesting — still
    /// has to match exactly.
    fn ignoring_rustfmt(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        for c in src.chars().filter(|c| !c.is_whitespace()) {
            if matches!(c, ']' | ')') {
                while out.ends_with(',') {
                    out.pop();
                }
            }
            out.push(c);
        }
        out
    }

    /// The committed `tiers.rs` must be what the generator produces from the
    /// committed captures.
    ///
    /// This replaces a test that carried exactly this claim in its name and
    /// doc while its body only grepped for two header substrings — so
    /// rewriting every number in the table kept it green. It now runs the real
    /// pipeline ([`committed_tiers`] + [`emit_rust`], the same two calls
    /// `main.rs` makes) and compares the whole output.
    #[test]
    fn committed_table_matches_a_fresh_generation() {
        let fresh = emit_rust(&committed_tiers());
        assert_eq!(
            ignoring_rustfmt(&fresh),
            ignoring_rustfmt(COMMITTED_TIERS_RS),
            "crates/zendriver-stealth/src/gpu/tiers.rs is not what the generator produces from \
             the committed captures. Either it was hand-edited, or a capture changed without \
             rerunning `cargo run -p gpu-tier-gen`."
        );
        // The comparison above is only meaningful if it is looking at real
        // content, so pin the two things that would make it vacuous.
        assert!(
            fresh.contains("BASE_PARAMS_WEBGL2") && fresh.contains("MAX_TEXTURE_SIZE"),
            "the generated source must actually contain the tables"
        );
        assert!(
            COMMITTED_TIERS_RS.contains("DO NOT EDIT"),
            "tiers.rs lost its generated-file header"
        );
    }

    /// A changed value must fail the comparison above — the property the test
    /// it replaces did not have.
    #[test]
    fn a_hand_edited_value_fails_the_comparison() {
        let fresh = emit_rust(&committed_tiers());
        let tampered = fresh.replacen("GlParam::Int(16384)", "GlParam::Int(16385)", 1);
        assert_ne!(
            tampered, fresh,
            "the tamper target must exist, or this test proves nothing"
        );
        assert_ne!(
            ignoring_rustfmt(&tampered),
            ignoring_rustfmt(&fresh),
            "a single changed number must not survive normalization"
        );
    }

    /// The whole delegated set, checked against the real captures rather than
    /// a hand-picked sample.
    #[test]
    fn no_delegated_param_survives_into_the_real_tables() {
        let committed = COMMITTED_TIERS_RS.replace('\n', " ");
        for name in DELEGATED_PARAMS {
            assert!(
                !committed.contains(&format!("(\"{name}\", GlParam")),
                "committed tiers.rs serves delegated param `{name}`; rerun \
                 `cargo run -p gpu-tier-gen`"
            );
        }
    }

    /// `getSupportedExtensions()` order is itself a fingerprint input, and
    /// Chrome's order is not alphabetical — it emits `EXT_shader_texture_lod`
    /// before `EXT_sRGB`, which is exactly the pair a sort would swap. Checked
    /// against the committed captures rather than a synthetic list, so the
    /// guard is anchored to what Chrome really emitted.
    #[test]
    fn extension_order_is_chromes_not_alphabetical() {
        const METAL: &str = include_str!("../../zendriver-stealth/data/gpu-tiers/metal-macos.json");
        let doc: Value = serde_json::from_str(METAL).expect("capture json");
        let tier = tier_from_capture("metal-macos", "probed: test", &doc["capture"]);

        let captured: Vec<String> = doc["capture"]["webgl1"]["extensions"]
            .as_array()
            .expect("extensions array")
            .iter()
            .map(|s| s.as_str().expect("string").to_string())
            .collect();
        assert_eq!(
            tier.extensions_webgl1, captured,
            "the extension list must survive in the capture's own order"
        );

        let pos = |name: &str| {
            tier.extensions_webgl1
                .iter()
                .position(|e| e == name)
                .expect("both names are present in the committed capture")
        };
        assert!(
            pos("EXT_shader_texture_lod") < pos("EXT_sRGB"),
            "Chrome emits EXT_shader_texture_lod before EXT_sRGB; a sorted list \
             reverses them and changes an order-sensitive fingerprint"
        );
        assert_ne!(
            tier.extensions_webgl1,
            {
                let mut sorted = tier.extensions_webgl1.clone();
                sorted.sort();
                sorted
            },
            "this capture is genuinely unsorted, so the assertion above is not vacuous"
        );
    }

    #[test]
    fn capture_enums_are_parsed_into_tier_data() {
        // GL enum numbers are fixed by the spec; the generator needs them to
        // emit ENUM_NAMES so the JS side can name a parameter from its enum.
        let capture = serde_json::json!({
            "webgl1": {"params": {}, "precision": {}, "extensions": [], "enums": {}},
            "webgl2": {
                "params": {}, "precision": {}, "extensions": [],
                "enums": {"ACTIVE_TEXTURE": 34016, "BLEND": 3042}
            },
            "adapter": null
        });
        let t = tier_from_capture("swiftshader", "probed: test", &capture);
        assert_eq!(t.enums["ACTIVE_TEXTURE"], 34016);
        assert_eq!(t.enums["BLEND"], 3042);
    }

    /// Build a `TierData` with independent WebGL1 and WebGL2 param sets, so a
    /// test can give a tier content that genuinely differs by context version
    /// (mirroring the real 82-vs-132 param gap between the two).
    fn tier_with_versions(name: &str, webgl1: &[(&str, i64)], webgl2: &[(&str, i64)]) -> TierData {
        let to_map = |params: &[(&str, i64)]| {
            params
                .iter()
                .map(|(k, v)| ((*k).to_string(), ParamValue::Int(*v)))
                .collect()
        };
        TierData {
            name: name.into(),
            provenance: "probed: test".into(),
            params_webgl1: to_map(webgl1),
            params_webgl2: to_map(webgl2),
            precision: BTreeMap::new(),
            extensions_webgl1: vec![],
            extensions_webgl2: vec![],
            enums: BTreeMap::new(),
            // The WebGL-shaped tests below say nothing about WebGPU; the
            // adapter tables get their own tests further down.
            webgpu: None,
        }
    }

    /// Most tests only care about one context version's params; leave WebGL1
    /// empty for those and use `tier_with_versions` where the distinction
    /// between the two matters.
    fn tier(name: &str, params: &[(&str, i64)]) -> TierData {
        tier_with_versions(name, &[], params)
    }

    #[test]
    fn shared_values_go_to_base_and_only_differences_become_overrides() {
        let a = tier("swiftshader", &[("SHARED", 7), ("DIFFERS", 8192)]);
        let b = tier("metal", &[("SHARED", 7), ("DIFFERS", 16384)]);
        let (base, overrides) = split_base_and_overrides(&[a, b], |t| &t.params_webgl2);

        assert_eq!(base["SHARED"], ParamValue::Int(7));
        assert!(
            !base.contains_key("DIFFERS"),
            "a differing param cannot be in base"
        );
        assert_eq!(overrides["swiftshader"]["DIFFERS"], ParamValue::Int(8192));
        assert_eq!(overrides["metal"]["DIFFERS"], ParamValue::Int(16384));
        assert!(!overrides["swiftshader"].contains_key("SHARED"));
    }

    #[test]
    fn a_param_missing_from_one_tier_is_never_promoted_to_base() {
        // DRAW_BUFFER6/7 exist only where MAX_DRAW_BUFFERS is high enough.
        // Putting them in base would hand SwiftShader a param it must not have.
        let a = tier("swiftshader", &[("SHARED", 7)]);
        let b = tier("metal", &[("SHARED", 7), ("DRAW_BUFFER6", 0)]);
        let (base, overrides) = split_base_and_overrides(&[a, b], |t| &t.params_webgl2);
        assert!(!base.contains_key("DRAW_BUFFER6"));
        assert_eq!(overrides["metal"]["DRAW_BUFFER6"], ParamValue::Int(0));
        assert!(!overrides["swiftshader"].contains_key("DRAW_BUFFER6"));
    }

    #[test]
    fn emitted_source_is_deterministic_and_marked_generated() {
        let tiers = vec![
            tier("swiftshader", &[("SHARED", 7), ("DIFFERS", 8192)]),
            tier("metal", &[("SHARED", 7), ("DIFFERS", 16384)]),
        ];
        let a = emit_rust(&tiers);
        let b = emit_rust(&tiers);
        assert_eq!(
            a, b,
            "emission must be deterministic or the drift test flaps"
        );
        assert!(a.contains("DO NOT EDIT"), "generated file must say so");
        assert!(a.contains("cargo run -p gpu-tier-gen"));
        assert!(
            a.contains("probed: test"),
            "provenance must survive into the source"
        );
    }

    #[test]
    fn webgl1_and_webgl2_tables_stay_separate_per_context() {
        // MAX_TEXTURE_SIZE is reported by both contexts (here with different
        // values), and MAX_3D_TEXTURE_SIZE only by WebGL2 — the same shape as
        // the real 18-vs-47 served-param gap. If the WebGL1 and WebGL2 tables
        // were ever merged or crossed, either a WebGL1 context would gain an
        // enum it has no constant for, or it would report the WebGL2
        // capture's value instead of its own. Both names are real served
        // capabilities because `emit_rust` drops everything else.
        let a = tier_with_versions(
            "swiftshader",
            &[("MAX_TEXTURE_SIZE", 111)],
            &[("MAX_TEXTURE_SIZE", 222), ("MAX_3D_TEXTURE_SIZE", 999)],
        );
        let b = tier_with_versions(
            "metal",
            &[("MAX_TEXTURE_SIZE", 111)],
            &[("MAX_TEXTURE_SIZE", 222), ("MAX_3D_TEXTURE_SIZE", 999)],
        );
        let out = emit_rust(&[a, b]);

        // Slice the emitted source into its WebGL1 and WebGL2 table regions
        // so the assertions below check what actually ships in each, not
        // just that a substring appears somewhere in the whole file.
        let webgl1_start = out
            .find("BASE_PARAMS_WEBGL1")
            .expect("WebGL1 base table must be emitted");
        let webgl2_start = out
            .find("BASE_PARAMS_WEBGL2")
            .expect("WebGL2 base table must be emitted");
        let precision_start = out
            .find("static PRECISION")
            .expect("precision table must be emitted");
        assert!(webgl1_start < webgl2_start && webgl2_start < precision_start);
        let webgl1_section = &out[webgl1_start..webgl2_start];
        let webgl2_section = &out[webgl2_start..precision_start];

        // A WebGL2-only param must land in the WebGL2 section and nowhere in
        // the WebGL1 section — a WebGL1 context has no constant for it, and
        // real Chrome returns null + INVALID_ENUM if asked.
        assert!(
            webgl2_section.contains("\"MAX_3D_TEXTURE_SIZE\""),
            "a WebGL2-only param must appear in the WebGL2 tables"
        );
        assert!(
            !webgl1_section.contains("MAX_3D_TEXTURE_SIZE"),
            "a WebGL2-only param must never leak into the WebGL1 tables"
        );

        // A param reported by both contexts must keep each context's own
        // value rather than the two getting crossed.
        assert!(
            webgl1_section.contains("(\"MAX_TEXTURE_SIZE\", GlParam::Int(111))"),
            "the WebGL1 table must use the WebGL1 input's value"
        );
        assert!(
            !webgl1_section.contains("GlParam::Int(222)"),
            "the WebGL1 table must not pick up the WebGL2 input's value"
        );
        assert!(
            webgl2_section.contains("(\"MAX_TEXTURE_SIZE\", GlParam::Int(222))"),
            "the WebGL2 table must use the WebGL2 input's value"
        );
    }

    #[test]
    fn enum_names_union_across_tiers_is_emitted_and_deduplicated() {
        let mut a = tier("swiftshader", &[]);
        a.enums.insert("ACTIVE_TEXTURE".into(), 34016);
        let mut b = tier("metal", &[]);
        b.enums.insert("ACTIVE_TEXTURE".into(), 34016);
        b.enums.insert("BLEND".into(), 3042);
        let out = emit_rust(&[a, b]);
        assert!(out.contains("(3042, \"BLEND\")"));
        assert!(out.contains("(34016, \"ACTIVE_TEXTURE\")"));
        // An enum more than one tier reports must appear exactly once.
        assert_eq!(out.matches("34016").count(), 1);
    }

    #[test]
    #[should_panic(expected = "ACTIVE_TEXTURE maps to two different enum numbers")]
    fn a_name_reported_with_two_different_numbers_panics_rather_than_pick_a_winner() {
        // A parameter name's enum number is fixed by the WebGL spec; two
        // captures disagreeing about ACTIVE_TEXTURE's number means one of
        // them is malformed.
        let mut a = tier("swiftshader", &[]);
        a.enums.insert("ACTIVE_TEXTURE".into(), 34016);
        let mut b = tier("metal", &[]);
        b.enums.insert("ACTIVE_TEXTURE".into(), 99999);
        emit_rust(&[a, b]);
    }

    #[test]
    fn a_number_may_legitimately_answer_to_two_names() {
        // GL_BLEND_EQUATION and GL_BLEND_EQUATION_RGB are literal spec-level
        // aliases for the same enum value (0x8009 / 32777) in both committed
        // captures. This is not a malformed capture, so both names must
        // survive into the table rather than tripping the ENUM_NAMES guard.
        let mut a = tier("swiftshader", &[]);
        a.enums.insert("BLEND_EQUATION".into(), 32777);
        a.enums.insert("BLEND_EQUATION_RGB".into(), 32777);
        let out = emit_rust(&[a]);
        assert!(out.contains("(32777, \"BLEND_EQUATION\")"));
        assert!(out.contains("(32777, \"BLEND_EQUATION_RGB\")"));
    }

    /// A capture missing any block must stop the generator, not silently
    /// produce an empty tier that serves nothing and falls through to the real
    /// backend for everything.
    #[test]
    fn a_capture_missing_a_block_fails_loudly() {
        let full = || {
            serde_json::json!({
                "params": {"MAX_TEXTURE_SIZE": 8192},
                "precision": {"VERTEX_SHADER/MEDIUM_FLOAT": [15, 15, 10]},
                "extensions": ["OES_texture_float"],
                "enums": {"MAX_TEXTURE_SIZE": 3379}
            })
        };
        for block in ["params", "precision", "extensions", "enums"] {
            let mut w1 = full();
            let mut w2 = full();
            // `precision` and `enums` are only read off the WebGL2 block, and
            // `params`/`extensions` off both, so drop the block from both to
            // cover whichever side reads it.
            w1.as_object_mut().expect("object").remove(block);
            w2.as_object_mut().expect("object").remove(block);
            // `adapter` is present and explicitly null so this test keeps
            // failing for the reason it names: without it every iteration would
            // panic on the missing adapter instead, and would pass even if the
            // block being dropped were tolerated.
            let capture = serde_json::json!({"webgl1": w1, "webgl2": w2, "adapter": null});
            let err = std::panic::catch_unwind(|| {
                tier_from_capture("swiftshader", "probed: test", &capture)
            });
            assert!(
                err.is_err(),
                "a capture missing its `{block}` block must fail loudly rather than yield an \
                 empty tier"
            );
        }
    }

    /// A tier with no WebGPU adapter and a tier whose adapter block never got
    /// written look identical once both are read as "no adapter" — so they must
    /// not be read the same way. An explicit `null` is the measurement (Chrome
    /// on SwiftShader really does resolve `requestAdapter()` to null); a
    /// missing key is a truncated capture, and silently accepting it would
    /// downgrade a hardware tier to serving nothing.
    #[test]
    fn an_absent_adapter_and_an_absent_adapter_block_are_not_the_same() {
        let blocks = || {
            serde_json::json!({
                "params": {"MAX_TEXTURE_SIZE": 8192},
                "precision": {}, "extensions": [], "enums": {}
            })
        };
        let probed_no_adapter =
            serde_json::json!({"webgl1": blocks(), "webgl2": blocks(), "adapter": null});
        assert_eq!(
            tier_from_capture("swiftshader", "probed: test", &probed_no_adapter).webgpu,
            None,
            "an explicit null is a measured absence and must parse"
        );

        let truncated = serde_json::json!({"webgl1": blocks(), "webgl2": blocks()});
        let err = std::panic::catch_unwind(|| {
            tier_from_capture("swiftshader", "probed: test", &truncated)
        });
        assert!(
            err.is_err(),
            "a capture with no `adapter` key at all must fail loudly rather than be read as a \
             tier with no adapter"
        );
    }

    #[test]
    fn adapter_limits_and_features_parse_into_tier_data() {
        let blocks = || {
            serde_json::json!({
                "params": {"MAX_TEXTURE_SIZE": 16384},
                "precision": {}, "extensions": [], "enums": {}
            })
        };
        let capture = serde_json::json!({
            "webgl1": blocks(),
            "webgl2": blocks(),
            "adapter": {
                "vendor": "apple", "architecture": "metal-3",
                "device": "", "description": "",
                "limits": {"maxTextureDimension2D": 16384, "maxBufferSize": 4294967292u64},
                // Deliberately not alphabetical: Chrome's own order must survive.
                "features": ["texture-compression-bc", "depth-clip-control"]
            }
        });
        let t = tier_from_capture("metal-macos", "probed: test", &capture);
        let a = t.webgpu.expect("the adapter block must parse");
        assert_eq!(a.limits["maxTextureDimension2D"], 16384);
        // Larger than u32::MAX — a limit narrowed to 32 bits would wrap here.
        assert_eq!(a.limits["maxBufferSize"], 4_294_967_292);
        assert_eq!(
            a.features,
            vec!["texture-compression-bc", "depth-clip-control"]
        );
    }

    #[test]
    fn a_malformed_adapter_limit_fails_loudly() {
        let blocks = || {
            serde_json::json!({
                "params": {}, "precision": {}, "extensions": [], "enums": {}
            })
        };
        let capture = serde_json::json!({
            "webgl1": blocks(), "webgl2": blocks(),
            "adapter": {"limits": {"maxBufferSize": -1}, "features": []}
        });
        let err =
            std::panic::catch_unwind(|| tier_from_capture("metal-macos", "probed: test", &capture));
        assert!(
            err.is_err(),
            "every WebGPU limit is a GPUSize64/32 in the IDL, so a negative one is a malformed \
             capture and must not be silently dropped from the table"
        );
    }

    /// The emitted table must keep "no adapter" and "an adapter" apart, and
    /// must not let one tier's numbers stand in for another's.
    #[test]
    fn the_webgpu_table_distinguishes_no_adapter_from_an_empty_one() {
        let blocks = || {
            serde_json::json!({
                "params": {}, "precision": {}, "extensions": [], "enums": {}
            })
        };
        let with_adapter = serde_json::json!({
            "webgl1": blocks(), "webgl2": blocks(),
            "adapter": {"limits": {"maxBufferSize": 4294967292u64}, "features": ["shader-f16"]}
        });
        let without = serde_json::json!({"webgl1": blocks(), "webgl2": blocks(), "adapter": null});
        let out = emit_rust(&[
            tier_from_capture("metal-macos", "probed: test", &with_adapter),
            tier_from_capture("swiftshader", "probed: test", &without),
        ]);
        assert!(
            out.contains("(\"swiftshader\", None)"),
            "a tier with no adapter must emit None, not an empty adapter: {out}"
        );
        assert!(
            out.contains("(\"maxBufferSize\", 4294967292)") && out.contains("\"shader-f16\""),
            "the measured adapter's values must reach the table: {out}"
        );
    }

    #[test]
    fn an_unconvertible_capture_value_fails_loudly() {
        let capture = serde_json::json!({
            "webgl1": {
                "params": {"MAX_VIEWPORT_DIMS": "not-an-array"},
                "precision": {}, "extensions": [], "enums": {}
            },
            "webgl2": {"params": {}, "precision": {}, "extensions": [], "enums": {}},
            "adapter": null
        });
        let err =
            std::panic::catch_unwind(|| tier_from_capture("swiftshader", "probed: test", &capture));
        assert!(
            err.is_err(),
            "a malformed capture value must not be silently dropped"
        );
    }
}
