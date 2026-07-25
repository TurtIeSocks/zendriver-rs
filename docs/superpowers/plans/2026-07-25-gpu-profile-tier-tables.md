# GPU Profile Tier Tables Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every readable WebGL value coherent with one claimed GPU, replacing today's six-parameter spoof that leaks the real backend through the other ~126.

**Architecture:** Probe captures are committed as provenance-tagged JSON. A `publish = false` generator crate (mirroring `locale-gen`) turns them into a committed `tiers.rs` of `pub(crate) static` tables: one shared base plus small per-tier override sets. A resolver flattens base + tier + device row + caller specs into a `GpuProfile`, which `patches.rs` substitutes into a rewritten, table-driven `webgl.js`.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), `serde_json`, `insta`, Chrome DevTools Protocol.

**Source spec:** `docs/superpowers/specs/2026-07-24-gpu-spoofing-design.md` (phases 3–4). Phases 1–2 merged in PR #124.

## Global Constraints

- Edition 2024, MSRV 1.85. Adding one workspace member (`crates/gpu-tier-gen`).
- **The generated `tiers.rs` is never hand-edited.** It carries a `DO NOT EDIT` header naming the generator, exactly like `crates/zendriver-stealth/src/geo/table.rs:1`. A CI test regenerates and asserts no diff.
- **GL types come from the WebGL spec, never inferred from the captures.** `JSON.stringify` collapses `1.0` to `1`, so `ALIASED_POINT_SIZE_RANGE` looks like an int pair in the capture but must emit `Float32Array`. Getting this wrong is caught by a one-line `instanceof` check.
- No auto behavior. Nothing probes the host to pick values; every value is table-derived or caller-supplied.
- Before any push: `cargo fmt --all`, then `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- MCP: any input/output type change needs `cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked` then `cargo insta accept --all`. Any public-API change needs a `mcp-coverage-ledger.toml` entry and a regenerated `public-api-baseline.txt`.
- **Rebase-friendliness.** A release-plz PR will open and automerge while this branch is open. Do **not** touch `CHANGELOG.md` or any `version = ` field in a `Cargo.toml` — those are release-plz's. Adding `crates/gpu-tier-gen` to the root `Cargo.toml` `members` list is fine; release-plz edits `[workspace.dependencies]` versions, a different region of the file. Expect to `git fetch && git rebase origin/main` before merge.

## Measured baseline

Captured on this host through `cargo run -p zendriver --example probe_gpu`, Chrome 150.0.7871.186. These numbers drive the design; do not re-derive them from the spec, which predates the measurement and understates several.

| | measured |
|---|---|
| WebGL1 params | 82 |
| WebGL2 params | 132 (SwiftShader reports 130) |
| WebGL2 params differing SwiftShader vs Metal | **28 of 132** |
| Extensions core-promoted in WebGL2 (WebGL1-only) | 16 |
| `getShaderPrecisionFormat` entries | 12, of which **8 discriminate** |

The 104 identical params are why the table is base-plus-override rather than one full table per tier.

Value shapes actually observed (WebGL2, Metal): 103 scalar ints, 16 bools, 4 string, 4 two-element arrays, 4 four-element arrays, 1 empty array (`COMPRESSED_TEXTURE_FORMATS`).

Param presence is **derived, not arbitrary**: the only presence delta is `DRAW_BUFFER6`/`DRAW_BUFFER7`, present on Metal because `MAX_DRAW_BUFFERS` is 8 there versus 6 on SwiftShader. `DRAW_BUFFERi` exists for `i < MAX_DRAW_BUFFERS`.

## File structure

```
crates/gpu-tier-gen/                              # NEW, publish = false, mirrors locale-gen
  Cargo.toml
  src/lib.rs        # capture JSON -> TierData; gl_type_for(); emit_rust()
  src/main.rs       # read the committed captures, write tiers.rs
crates/zendriver-stealth/
  data/gpu-tiers/swiftshader.json                 # committed captures, provenance-tagged
  data/gpu-tiers/metal-apple-family3.json
  src/gpu/mod.rs        # GpuProfile, resolve(), re-exports
  src/gpu/types.rs      # GlParam, ShaderPrecision, Provenance, Tier
  src/gpu/tiers.rs      # GENERATED — base + per-tier overrides
  src/gpu/devices.rs    # DeviceRow + renderer lookup (absorbs webgpu_adapter.rs)
  src/gpu/invariants.rs # the three coherence invariants
  src/patches/webgl.js  # rewritten table-driven
  src/patches.rs        # push_webgl rewritten (currently line 234)
```

---

### Task 1: Value types

Pure types, no data. Everything downstream names these, so they land first.

**Files:**
- Create: `crates/zendriver-stealth/src/gpu/types.rs`
- Create: `crates/zendriver-stealth/src/gpu/mod.rs`
- Modify: `crates/zendriver-stealth/src/lib.rs` (add `mod gpu;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub enum GlParam { Int(i64), Float(f64), Bool(bool), IntPair([i32;2]), FloatPair([f32;2]), FloatQuad([f32;4]), IntQuad([i32;4]), IntList(Vec<u32>), Str(String) }`
  - `pub struct ShaderPrecision { pub range_min: i32, pub range_max: i32, pub precision: i32 }`
  - `pub enum Provenance { Probed { chrome: String, os: String }, Derived { source: String } }`
  - `pub(crate) enum Tier { SwiftShader, MetalAppleFamily3 }`

- [ ] **Step 1: Write the failing test**

Create `crates/zendriver-stealth/src/gpu/types.rs` containing only the test module for now:

```rust
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
        let p = ShaderPrecision { range_min: 127, range_max: 127, precision: 23 };
        let s = serde_json::to_string(&p).unwrap();
        let back: ShaderPrecision = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn provenance_records_where_a_value_came_from() {
        let p = Provenance::Probed { chrome: "150.0.7871.186".into(), os: "macos".into() };
        assert!(serde_json::to_string(&p).unwrap().contains("150.0.7871.186"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p zendriver-stealth --lib gpu::types 2>&1 | tail -20
```

Expected: FAIL — `cannot find type GlParam in this scope`.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/zendriver-stealth/src/gpu/types.rs`:

```rust
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
    MetalAppleFamily3,
}
```

Create `crates/zendriver-stealth/src/gpu/mod.rs`:

```rust
//! Coherent per-GPU value tables and the profile resolved from them.

pub(crate) mod types;

pub use types::{GlParam, Provenance, ShaderPrecision};
```

Add `mod gpu;` to `crates/zendriver-stealth/src/lib.rs` beside the other module declarations, and re-export the public types next to the existing `pub use flags::GpuBackend;` line:

```rust
pub use gpu::{GlParam, Provenance, ShaderPrecision};
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p zendriver-stealth --lib gpu::types 2>&1 | tail -20
```

Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/zendriver-stealth/src/gpu crates/zendriver-stealth/src/lib.rs
git commit -m "feat(stealth): add GPU profile value types"
```

---

### Task 2: Commit the captures and the capture reader

The generator's input. Split from the emitter so the parse is testable without touching generated output.

**Files:**
- Create: `crates/gpu-tier-gen/Cargo.toml`, `crates/gpu-tier-gen/src/lib.rs`
- Create: `crates/zendriver-stealth/data/gpu-tiers/swiftshader.json`
- Create: `crates/zendriver-stealth/data/gpu-tiers/metal-apple-family3.json`
- Modify: `Cargo.toml` (workspace `members`)

**Interfaces:**
- Consumes: nothing (the gen crate does not depend on `zendriver-stealth`).
- Produces:
  - `pub struct TierData { pub name: String, pub provenance: String, pub params_webgl1: BTreeMap<String, ParamValue>, pub params_webgl2: BTreeMap<String, ParamValue>, pub precision: BTreeMap<String, [i32;3]>, pub extensions_webgl1: Vec<String>, pub extensions_webgl2: Vec<String> }`
  - `pub enum ParamValue { Int(i64), Float(f64), Bool(bool), IntPair([i32;2]), FloatPair([f32;2]), FloatQuad([f32;4]), IntQuad([i32;4]), IntList(Vec<u32>), Str(String) }`
  - `pub fn gl_type_for(name: &str) -> GlType`
  - `pub fn tier_from_capture(name: &str, provenance: &str, capture: &serde_json::Value) -> TierData`

- [ ] **Step 1: Teach the probe to record enum numbers**

`webgl.js` receives a GL enum *number* at runtime (`getParameter(3379)`) but the tables are keyed by *name*. The capture currently records only names, so nothing can build the number-to-name map the patch needs. Fix that at the source, in `crates/zendriver/examples/probe_gpu.rs`, inside `readContext` where it already walks the prototype:

```javascript
    r.enums = {};
    for (const name of Object.keys(Object.getPrototypeOf(gl))) {
      const val = gl[name];
      if (typeof val === 'number' && Object.prototype.hasOwnProperty.call(r.params, name)) {
        r.enums[name] = val;
      }
    }
```

Add `enums` to the output-shape list in the module doc. Verify:

```bash
cargo run -q -p zendriver --example probe_gpu -- swiftshader 2>/dev/null \
  | python3 -c "import json,sys; d=json.load(sys.stdin); e=d['webgl2']['enums']; print('enums:', len(e), 'MAX_TEXTURE_SIZE =', e['MAX_TEXTURE_SIZE'])"
```

Expected: a count matching the param count, and `MAX_TEXTURE_SIZE = 3379`.

```bash
cargo fmt --all && git add crates/zendriver/examples/probe_gpu.rs
git commit -m "feat(zendriver): record GL enum numbers in the probe output"
```

- [ ] **Step 2: Generate the capture files**

Run the probe and save both captures with a provenance wrapper:

```bash
mkdir -p crates/zendriver-stealth/data/gpu-tiers
CHROME_VER=$("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --version | tr -d '\n')
for mode in swiftshader native; do
  case $mode in swiftshader) out=swiftshader ;; native) out=metal-apple-family3 ;; esac
  cargo run -q -p zendriver --example probe_gpu -- $mode 2>/dev/null \
    | python3 -c "
import json,sys,os
d=json.load(sys.stdin)
print(json.dumps({'tier': '$out', 'provenance': 'probed: $CHROME_VER on ' + os.uname().sysname, 'capture': d}, indent=2, sort_keys=True))
" > crates/zendriver-stealth/data/gpu-tiers/$out.json
done
wc -l crates/zendriver-stealth/data/gpu-tiers/*.json
```

Expected: two files, a few hundred lines each. Sanity-check that `swiftshader.json` contains `"MAX_TEXTURE_SIZE": 8192` and `metal-apple-family3.json` contains `16384`. If either is missing or the adapter block is null in the metal capture, stop — the probe was not run from a secure context or the backend did not engage, and the tables would be built on bad data.

- [ ] **Step 3: Write the failing test**

Create `crates/gpu-tier-gen/src/lib.rs` with just the tests:

```rust
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
```

- [ ] **Step 4: Run test to verify it fails**

```bash
cargo test -p gpu-tier-gen 2>&1 | tail -20
```

Expected: FAIL — the package does not exist yet.

- [ ] **Step 5: Create the crate**

`crates/gpu-tier-gen/Cargo.toml`, mirroring `crates/locale-gen/Cargo.toml`:

```toml
[package]
name = "gpu-tier-gen"
version = "0.0.0"
edition.workspace = true
publish = false

[lints]
workspace = true

[dependencies]
serde_json.workspace = true
```

Add `"crates/gpu-tier-gen",` to the `members` list in the root `Cargo.toml`, after `"crates/locale-gen",`.

- [ ] **Step 6: Write the implementation**

Prepend to `crates/gpu-tier-gen/src/lib.rs`:

```rust
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
        | "MAX_TEXTURE_MAX_ANISOTROPY_EXT"
        | "MAX_TEXTURE_LOD_BIAS" => GlType::Float,
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
        .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
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
```

- [ ] **Step 7: Run tests to verify they pass**

```bash
cargo test -p gpu-tier-gen 2>&1 | tail -20
```

Expected: PASS, 4 tests.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add Cargo.toml crates/gpu-tier-gen crates/zendriver-stealth/data
git commit -m "feat(gpu-tier-gen): add the capture reader and committed tier captures"
```

---

### Task 3: Emit the tables and guard them against drift

Splits base from overrides, emits `tiers.rs`, and proves regeneration is stable.

**Files:**
- Modify: `crates/gpu-tier-gen/src/lib.rs`
- Create: `crates/gpu-tier-gen/src/main.rs`
- Create: `crates/zendriver-stealth/src/gpu/tiers.rs` (generated)
- Create: `crates/zendriver-stealth/tests/tier_table_is_current.rs`

**Interfaces:**
- Consumes: `TierData`, `tier_from_capture` (Task 2).
- Produces:
  - `pub fn split_base_and_overrides(tiers: &[TierData]) -> (BTreeMap<String, ParamValue>, BTreeMap<String, BTreeMap<String, ParamValue>>)`
  - `pub fn emit_rust(tiers: &[TierData]) -> String`
  - generated `pub(crate) static BASE_PARAMS_WEBGL1`, `BASE_PARAMS_WEBGL2`, `PARAM_OVERRIDES_WEBGL1`, `PARAM_OVERRIDES_WEBGL2`, `PRECISION`, `EXTENSIONS_WEBGL1`, `EXTENSIONS_WEBGL2`, `ENUM_NAMES` in `crates/zendriver-stealth/src/gpu/tiers.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/gpu-tier-gen/src/lib.rs`'s test module:

```rust
    fn tier(name: &str, params: &[(&str, i64)]) -> TierData {
        TierData {
            name: name.into(),
            provenance: "probed: test".into(),
            params_webgl1: BTreeMap::new(),
            params_webgl2: params
                .iter()
                .map(|(k, v)| ((*k).to_string(), ParamValue::Int(*v)))
                .collect(),
            precision: BTreeMap::new(),
            extensions_webgl1: vec![],
            extensions_webgl2: vec![],
        }
    }

    #[test]
    fn shared_values_go_to_base_and_only_differences_become_overrides() {
        let a = tier("swiftshader", &[("SHARED", 7), ("DIFFERS", 8192)]);
        let b = tier("metal", &[("SHARED", 7), ("DIFFERS", 16384)]);
        let (base, overrides) = split_base_and_overrides(&[a, b], |t| &t.params_webgl2);

        assert_eq!(base["SHARED"], ParamValue::Int(7));
        assert!(!base.contains_key("DIFFERS"), "a differing param cannot be in base");
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
        assert_eq!(a, b, "emission must be deterministic or the drift test flaps");
        assert!(a.contains("DO NOT EDIT"), "generated file must say so");
        assert!(a.contains("cargo run -p gpu-tier-gen"));
        assert!(a.contains("probed: test"), "provenance must survive into the source");
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p gpu-tier-gen 2>&1 | tail -20
```

Expected: FAIL — `cannot find function split_base_and_overrides`.

- [ ] **Step 3: Write the implementation**

Append to `crates/gpu-tier-gen/src/lib.rs`:

```rust
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
        let present: Vec<&ParamValue> =
            tiers.iter().filter_map(|t| pick(t).get(name)).collect();
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

/// Emit the whole `tiers.rs`. Deterministic: every map is a `BTreeMap` and
/// every list is sorted before it gets here.
pub fn emit_rust(tiers: &[TierData]) -> String {
    let mut s = String::new();
    s.push_str("// Generated by `cargo run -p gpu-tier-gen`. DO NOT EDIT.\n");
    s.push_str("// Sources:\n");
    for t in tiers {
        s.push_str(&format!("//   {} — {}\n", t.name, t.provenance));
    }
    s.push_str("\nuse super::types::GlParamRef as GlParam;\n\n");

    // Both context versions get their own tables. WebGL1 exposes 82 params and
    // WebGL2 exposes 132; sharing one table would answer WebGL2-only enums on
    // a WebGL1 context, where real Chrome returns null and raises INVALID_ENUM.
    for (suffix, pick) in [
        ("WEBGL1", (|t: &TierData| &t.params_webgl1) as fn(&TierData) -> &BTreeMap<String, ParamValue>),
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
            s.push_str(&format!("        ({k:?}, [{}, {}, {}]),\n", p[0], p[1], p[2]));
        }
        s.push_str("    ]),\n");
    }
    s.push_str("];\n\n");

    for (label, pick) in [
        ("EXTENSIONS_WEBGL1", true),
        ("EXTENSIONS_WEBGL2", false),
    ] {
        s.push_str(&format!(
            "/// Extension list per tier for {}.\n",
            if pick { "WebGL1" } else { "WebGL2" }
        ));
        s.push_str(&format!(
            "pub(crate) static {label}: &[(&str, &[&str])] = &[\n"
        ));
        for t in tiers {
            let list = if pick { &t.extensions_webgl1 } else { &t.extensions_webgl2 };
            s.push_str(&format!("    ({:?}, &[\n", t.name));
            for e in list {
                s.push_str(&format!("        {e:?},\n"));
            }
            s.push_str("    ]),\n");
        }
        s.push_str("];\n\n");
    }
    s
}
```

The emitted source refers to `GlParamRef`, a `'static`-friendly mirror of `GlParam` (owned `String`/`Vec` cannot live in a `static`). Add it to `crates/zendriver-stealth/src/gpu/types.rs`:

```rust
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
```

- [ ] **Step 4: Write the generator binary**

Create `crates/gpu-tier-gen/src/main.rs`, mirroring `crates/locale-gen/src/main.rs`:

```rust
//! Regenerate the stealth crate's GPU capability tier tables from the
//! committed probe captures.

use gpu_tier_gen::{emit_rust, tier_from_capture, TierData};

const CAPTURES: &[(&str, &str)] = &[
    ("swiftshader", "crates/zendriver-stealth/data/gpu-tiers/swiftshader.json"),
    (
        "metal-apple-family3",
        "crates/zendriver-stealth/data/gpu-tiers/metal-apple-family3.json",
    ),
];
const OUT: &str = "crates/zendriver-stealth/src/gpu/tiers.rs";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut tiers: Vec<TierData> = Vec::new();
    for (name, path) in CAPTURES {
        let raw = std::fs::read_to_string(path)?;
        let v: serde_json::Value = serde_json::from_str(&raw)?;
        let prov = v["provenance"].as_str().unwrap_or("unknown");
        tiers.push(tier_from_capture(name, prov, &v["capture"]));
    }
    eprintln!("emitting {} tiers to {OUT}", tiers.len());
    std::fs::write(OUT, emit_rust(&tiers))?;
    Ok(())
}
```

- [ ] **Step 5: Generate and inspect**

```bash
cargo run -p gpu-tier-gen && wc -l crates/zendriver-stealth/src/gpu/tiers.rs && head -12 crates/zendriver-stealth/src/gpu/tiers.rs
```

Expected: a few hundred lines, a `DO NOT EDIT` header, and both tiers named with their provenance. Confirm the base table is substantially larger than either override block, which is the whole premise of the split. Add `pub(crate) mod tiers;` to `crates/zendriver-stealth/src/gpu/mod.rs`.

- [ ] **Step 6: Write the drift test**

Create `crates/zendriver-stealth/tests/tier_table_is_current.rs`:

```rust
//! The committed `tiers.rs` must equal what the generator produces from the
//! committed captures. If this fails, someone hand-edited the generated file
//! or changed a capture without rerunning `cargo run -p gpu-tier-gen`.

#[test]
fn generated_tier_table_matches_the_committed_captures() {
    let committed = std::fs::read_to_string(
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/gpu/tiers.rs"),
    )
    .expect("read committed tiers.rs");
    assert!(
        committed.contains("DO NOT EDIT"),
        "tiers.rs lost its generated-file header"
    );
    assert!(
        committed.contains("cargo run -p gpu-tier-gen"),
        "tiers.rs must name the generator that produces it"
    );
}
```

The byte-level regeneration comparison runs in CI rather than here, because the test crate cannot depend on `gpu-tier-gen` without a dev-dependency cycle. Add to `.github/workflows/ci.yml`, in the existing lint job after the fmt step:

```yaml
      - name: GPU tier table is current
        run: |
          cargo run -p gpu-tier-gen
          git diff --exit-code crates/zendriver-stealth/src/gpu/tiers.rs
```

- [ ] **Step 7: Run tests and the gates**

```bash
cargo test -p gpu-tier-gen && cargo test -p zendriver-stealth --test tier_table_is_current
cargo run -p gpu-tier-gen && git diff --exit-code crates/zendriver-stealth/src/gpu/tiers.rs && echo "regeneration is stable"
```

Expected: all pass, and the regeneration leaves no diff.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add crates/gpu-tier-gen crates/zendriver-stealth/src/gpu crates/zendriver-stealth/tests .github/workflows/ci.yml
git commit -m "feat(gpu-tier-gen): emit the tier tables and guard them against drift"
```

---

### Task 4: Resolve a flat `GpuProfile`

**Files:**
- Modify: `crates/zendriver-stealth/src/gpu/mod.rs`

**Interfaces:**
- Consumes: `GlParam`, `GlParamRef`, `Tier` (Task 1); the generated statics (Task 3).
- Produces:
  - `pub struct GpuProfile { pub provenance: Provenance, pub params_webgl1: BTreeMap<String, GlParam>, pub params_webgl2: BTreeMap<String, GlParam>, pub precision: BTreeMap<String, ShaderPrecision>, pub extensions_webgl1: Vec<String>, pub extensions_webgl2: Vec<String>, pub unmasked_vendor: String, pub unmasked_renderer: String }`
  - `pub(crate) fn profile_for_tier(tier: Tier) -> GpuProfile`
  - `pub fn GpuProfile::overlay(self, over: GpuProfile) -> GpuProfile`

- [ ] **Step 1: Write the failing test**

Add to `crates/zendriver-stealth/src/gpu/mod.rs`:

```rust
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
        assert!(sw.params_webgl2.len() > 100, "got {}", sw.params_webgl2.len());
        assert!(mt.params_webgl2.len() > 100, "got {}", mt.params_webgl2.len());
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
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p zendriver-stealth --lib gpu:: 2>&1 | tail -20
```

Expected: FAIL — `cannot find function profile_for_tier`.

- [ ] **Step 3: Write the implementation**

Add to `crates/zendriver-stealth/src/gpu/mod.rs`:

```rust
use std::collections::BTreeMap;

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
            provenance: Provenance::Derived { source: "empty".into() },
            params_webgl1: BTreeMap::new(),
            params_webgl2: BTreeMap::new(),
            precision: BTreeMap::new(),
            extensions_webgl1: Vec::new(),
            extensions_webgl2: Vec::new(),
            unmasked_vendor: String::new(),
            unmasked_renderer: String::new(),
        }
    }

    /// Field-wise merge: anything set in `over` wins, anything empty inherits.
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
/// parameters and WebGL2 exposes 132; serving the WebGL2 set to a WebGL1
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
pub(crate) fn profile_for_tier(tier: Tier) -> GpuProfile {
    let key = tier_key(tier);
    let params = flatten(tiers::BASE_PARAMS_WEBGL2, tiers::PARAM_OVERRIDES_WEBGL2, key);
    let precision = lookup(tiers::PRECISION, key)
        .map(|rows| {
            rows.iter()
                .map(|(k, [a, b, c])| {
                    (
                        (*k).to_string(),
                        ShaderPrecision { range_min: *a, range_max: *b, precision: *c },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let exts = |t: &[(&str, &[&str])]| -> Vec<String> {
        lookup(t, key).map(|l| l.iter().map(|s| (*s).to_string()).collect()).unwrap_or_default()
    };
    GpuProfile {
        provenance: Provenance::Probed {
            chrome: "see data/gpu-tiers".into(),
            os: "see data/gpu-tiers".into(),
        },
        params_webgl1: flatten(tiers::BASE_PARAMS_WEBGL1, tiers::PARAM_OVERRIDES_WEBGL1, key),
        params_webgl2: params,
        precision,
        extensions_webgl1: exts(tiers::EXTENSIONS_WEBGL1),
        extensions_webgl2: exts(tiers::EXTENSIONS_WEBGL2),
        unmasked_vendor: String::new(),
        unmasked_renderer: String::new(),
    }
}
```

Export `GpuProfile` from `crates/zendriver-stealth/src/lib.rs` beside the other `gpu` re-exports.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p zendriver-stealth --lib gpu:: 2>&1 | tail -20
```

Expected: PASS, 7 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/zendriver-stealth/src
git commit -m "feat(stealth): resolve a flat GpuProfile from the tier tables"
```

---

### Task 5: Coherence invariants

The spec's three invariants, each a test over the shipped tables rather than a comment.

**Files:**
- Create: `crates/zendriver-stealth/src/gpu/invariants.rs`
- Modify: `crates/zendriver-stealth/src/gpu/mod.rs` (add `mod invariants;`)

**Interfaces:**
- Consumes: `GpuProfile`, `profile_for_tier` (Task 4).
- Produces: `pub(crate) fn check_coherence(p: &GpuProfile) -> Result<(), String>`

- [ ] **Step 1: Write the failing test**

Create `crates/zendriver-stealth/src/gpu/invariants.rs`:

```rust
//! Relations a real GPU's parameters always satisfy. Fingerprinters check
//! several of these, so a table edit that breaks one must fail the build
//! rather than ship an impossible device.

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::gpu::{profile_for_tier, types::Tier, GlParam, GpuProfile};

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
            assert_eq!(p.precision.len(), 12, "tier {tier:?} lost precision entries");
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
        let known: std::collections::BTreeSet<&str> =
            crate::gpu::tiers::ENUM_NAMES.iter().map(|(_, n)| *n).collect();
        let orphans: Vec<&String> = p
            .params_webgl2
            .keys()
            .filter(|k| !known.contains(k.as_str()))
            .collect();
        assert!(orphans.is_empty(), "params with no enum number: {orphans:?}");
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
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p zendriver-stealth --lib gpu::invariants 2>&1 | tail -20
```

Expected: FAIL — `cannot find function check_coherence`.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/zendriver-stealth/src/gpu/invariants.rs`:

```rust
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
/// Returns the first violation as a human-readable string rather than a bool,
/// so a failing table edit names what it broke instead of just failing.
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
            if p.params_webgl2.contains_key(&format!("DRAW_BUFFER{i}")) && i64::from(i) >= max_draw {
                return Err(format!(
                    "DRAW_BUFFER{i} is present but MAX_DRAW_BUFFERS is {max_draw}"
                ));
            }
        }
    }
    Ok(())
}
```

Also append the platform-coherence check to the same file:

```rust
use crate::Platform;
use crate::gpu::types::Tier;

/// Report a mismatch between the persona's claimed OS and the tier supplying
/// its capability values, or `None` when they are compatible.
///
/// Deliberately a warning rather than an error: a caller may pair them on
/// purpose, and refusing to launch over a fingerprint detail is a worse
/// failure than reporting one. Same stance as the header-coherence check.
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
    (!ok).then(|| {
        format!("persona claims {platform:?} but its GPU values come from {tier:?}")
    })
}
```

Call it from `push_webgl` (Task 8) alongside `check_coherence`, logging through the same `tracing::warn!`.

Add `pub(crate) mod invariants;` to `crates/zendriver-stealth/src/gpu/mod.rs`.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p zendriver-stealth --lib gpu::invariants 2>&1 | tail -20
```

Expected: PASS, 5 tests. If `shipped_tiers_are_all_coherent` fails, a capture is bad — investigate rather than relaxing the invariant.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/zendriver-stealth/src/gpu
git commit -m "feat(stealth): enforce GPU parameter coherence invariants"
```

---

### Task 6: Device rows and renderer lookup

Folds `webgpu_adapter.rs` in, so one renderer string drives both the WebGL identity and the WebGPU adapter.

**Files:**
- Create: `crates/zendriver-stealth/src/gpu/devices.rs`
- Delete: `crates/zendriver-stealth/src/persona/webgpu_adapter.rs`
- Modify: `crates/zendriver-stealth/src/persona/mod.rs` (drop the `webgpu_adapter` module), `crates/zendriver-stealth/src/patches.rs` (import moves)

**Interfaces:**
- Consumes: `Tier` (Task 1), `profile_for_tier` (Task 4).
- Produces:
  - `pub(crate) struct DeviceRow { pub unmasked_vendor: &'static str, pub unmasked_renderer: &'static str, pub tier: Tier, pub webgpu_vendor: &'static str, pub webgpu_architecture: &'static str }`
  - `pub(crate) fn device_for_renderer(renderer: &str) -> DeviceRow`
  - `pub(crate) fn adapter_for_renderer(renderer: &str) -> GpuAdapterInfo` — same signature as today's, re-homed

- [ ] **Step 1: Move the existing tests**

`crates/zendriver-stealth/src/persona/webgpu_adapter.rs` already has six passing tests covering NVIDIA/AMD/Apple/Intel renderer mapping. Copy that file to `crates/zendriver-stealth/src/gpu/devices.rs` unchanged, then add the new device-row tests to its test module:

```rust
    #[test]
    fn a_known_renderer_selects_its_tier() {
        let d = device_for_renderer(
            "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)",
        );
        assert_eq!(d.tier, Tier::MetalAppleFamily3);
        assert_eq!(d.webgpu_vendor, "apple");
    }

    #[test]
    fn a_software_renderer_selects_the_swiftshader_tier() {
        let d = device_for_renderer(
            "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (LLVM 10.0.0) (0x0000C0DE)), SwiftShader driver)",
        );
        assert_eq!(d.tier, Tier::SwiftShader);
    }

    #[test]
    fn an_unknown_renderer_falls_back_without_inventing_a_tier() {
        // Unknown hardware takes the default desktop tier rather than
        // guessing; a wrong tier is more detectable than a generic one.
        let d = device_for_renderer("Some Unreleased GPU");
        assert_eq!(d.tier, Tier::MetalAppleFamily3);
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p zendriver-stealth --lib gpu::devices 2>&1 | tail -20
```

Expected: FAIL — `cannot find function device_for_renderer`.

- [ ] **Step 3: Write the implementation**

Append to `crates/zendriver-stealth/src/gpu/devices.rs`:

```rust
use crate::gpu::types::Tier;

/// One device's identity. Only what genuinely varies per device lives here;
/// the capability values come from the device's [`Tier`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceRow {
    pub unmasked_vendor: &'static str,
    pub unmasked_renderer: &'static str,
    pub tier: Tier,
    pub webgpu_vendor: &'static str,
    pub webgpu_architecture: &'static str,
}

/// Known devices, keyed by a substring of the unmasked renderer string.
static DEVICES: &[DeviceRow] = &[
    DeviceRow {
        unmasked_vendor: "Google Inc. (Google)",
        unmasked_renderer:
            "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (LLVM 10.0.0) (0x0000C0DE)), SwiftShader driver)",
        tier: Tier::SwiftShader,
        webgpu_vendor: "",
        webgpu_architecture: "",
    },
    DeviceRow {
        unmasked_vendor: "Google Inc. (Apple)",
        unmasked_renderer:
            "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)",
        tier: Tier::MetalAppleFamily3,
        webgpu_vendor: "apple",
        webgpu_architecture: "metal-3",
    },
];

/// Renderer assumed when a persona pins no WebGL renderer of its own.
///
/// The Apple Metal row rather than the software one: a persona that says
/// nothing should look like ordinary hardware, and SwiftShader's renderer
/// string is itself a bot signal.
pub(crate) const DEFAULT_RENDERER: &str =
    "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)";

/// Pick the device row a renderer string belongs to.
///
/// Falls back to the first non-software row rather than inventing a tier:
/// reporting a plausible real desktop GPU beats reporting a tier that matches
/// no device.
pub(crate) fn device_for_renderer(renderer: &str) -> DeviceRow {
    let r = renderer.to_ascii_lowercase();
    if r.contains("swiftshader") {
        return DEVICES[0];
    }
    DEVICES
        .iter()
        .copied()
        .find(|d| {
            let vendor = adapter_for_renderer(d.unmasked_renderer).vendor;
            !vendor.is_empty() && r.contains(&vendor)
        })
        .unwrap_or(DEVICES[1])
}
```

Delete `crates/zendriver-stealth/src/persona/webgpu_adapter.rs`, drop its `mod` line from `crates/zendriver-stealth/src/persona/mod.rs`, and update the `use crate::persona::webgpu_adapter::adapter_for_renderer;` import inside `push_webgpu` (`crates/zendriver-stealth/src/patches.rs`, currently line 274) to `use crate::gpu::devices::adapter_for_renderer;`.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p zendriver-stealth --lib 2>&1 | tail -20
```

Expected: PASS, including the six migrated `adapter_for_renderer` tests and the three new ones. No test should have been deleted in the move.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/zendriver-stealth/src
git commit -m "refactor(stealth): fold webgpu_adapter into the GPU device table"
```

---

### Task 7: Wire `Persona.gpu`

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/mod.rs` (struct + `overlay`)
- Modify: `crates/zendriver-stealth/src/persona/specs.rs` (rustdoc cross-reference)

**Interfaces:**
- Consumes: `GpuProfile` (Task 4).
- Produces: `pub gpu: Option<GpuProfile>` on `Persona`.

- [ ] **Step 1: Write the failing test**

Add to the test module in `crates/zendriver-stealth/src/persona/mod.rs`:

```rust
    #[test]
    fn persona_gpu_defaults_to_none() {
        assert!(Persona::default().gpu.is_none());
    }

    #[test]
    fn persona_overlay_takes_the_higher_priority_gpu_whole() {
        // One device is one coherent artifact, like ScreenSpec: the winning
        // persona's GPU wins outright rather than merging field-wise, which
        // could compose two devices into one that exists nowhere.
        let mut base = Persona::default();
        base.gpu = Some(crate::GpuProfile::empty());
        let mut over = Persona::default();
        let mut p = crate::GpuProfile::empty();
        p.unmasked_renderer = "ANGLE (NVIDIA, ...)".into();
        over.gpu = Some(p);

        let merged = base.overlay(over);
        assert_eq!(
            merged.gpu.expect("gpu survives overlay").unmasked_renderer,
            "ANGLE (NVIDIA, ...)"
        );
    }

    #[test]
    fn persona_overlay_keeps_the_base_gpu_when_the_overlay_has_none() {
        let mut base = Persona::default();
        let mut p = crate::GpuProfile::empty();
        p.unmasked_renderer = "base-renderer".into();
        base.gpu = Some(p);

        let merged = base.overlay(Persona::default());
        assert_eq!(
            merged.gpu.expect("gpu survives").unmasked_renderer,
            "base-renderer"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p zendriver-stealth --lib persona 2>&1 | tail -20
```

Expected: FAIL — `no field gpu on type Persona`.

- [ ] **Step 3: Write the implementation**

Add the field to `Persona` in `crates/zendriver-stealth/src/persona/mod.rs`, beside the existing `webgl` / `webgpu` fields:

```rust
    /// One coherent GPU identity: every readable WebGL value plus the WebGPU
    /// adapter, resolved from the tier tables.
    ///
    /// `None` resolves from the persona's WebGL renderer via the device table.
    /// The finer-grained [`WebglSpec`](specs::WebglSpec) and
    /// [`WebgpuSpec`](specs::WebgpuSpec) still overlay on top of whatever this
    /// produces, so a caller can pin one value without restating a whole device.
    pub gpu: Option<GpuProfile>,
```

In `Persona::overlay`, add the field using whole-value semantics, matching how `screen` is handled:

```rust
            gpu: over.gpu.or(self.gpu),
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p zendriver-stealth --lib persona 2>&1 | tail -20
```

Expected: PASS. Existing persona tests must be unaffected; if a struct-literal construction of `Persona` now fails to compile, add `gpu: None` there.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/zendriver-stealth/src/persona
git commit -m "feat(stealth): carry a resolved GpuProfile on Persona"
```

---

### Task 8: Rewrite `webgl.js` table-driven

The behavior change. Closes the impossible viewport/texture pair, the `getExtension` contradiction, and the shared WebGL1/WebGL2 extension list.

**Files:**
- Rewrite: `crates/zendriver-stealth/src/patches/webgl.js`
- Modify: `crates/zendriver-stealth/src/patches.rs:234` (`push_webgl`)

**Interfaces:**
- Consumes: `GpuProfile` (Task 4), `device_for_renderer` (Task 6).
- Produces: `WEBGL` JS taking one substituted `WEBGL_PROFILE` JSON object.

- [ ] **Step 1: Write the failing test**

Add to the test module in `crates/zendriver-stealth/src/patches.rs`:

```rust
    #[test]
    fn webgl_patch_substitutes_a_complete_profile() {
        let mut out = String::new();
        push_webgl(&mut out, None);
        // The profile arrives as one JSON object, not a ladder of literals.
        assert!(out.contains("\"params2\""), "got: {out}");
        assert!(out.contains("\"precision\""), "got: {out}");
        assert!(out.contains("\"extensions1\""), "got: {out}");
        assert!(out.contains("\"extensions2\""), "got: {out}");
        assert!(!out.contains("WEBGL_PROFILE"), "placeholder was not replaced");
    }

    #[test]
    fn webgl_patch_no_longer_hardcodes_the_impossible_viewport() {
        // The shipped bug: a 32767 viewport beside an unpatched 8192 texture
        // max. The value may legitimately appear if a tier measures it, but
        // never as a bare literal in the JS source.
        assert!(
            !WEBGL.contains("32767"),
            "webgl.js must not hardcode viewport dimensions"
        );
    }

    #[test]
    fn webgl_patch_serves_different_extension_lists_per_context_version() {
        let mut out = String::new();
        push_webgl(&mut out, None);
        let profile = out
            .split_once("\"extensions1\":")
            .and_then(|(_, r)| r.split_once("\"extensions2\":"))
            .map(|(a, _)| a.to_string())
            .expect("both lists present");
        assert!(
            profile.contains("OES_texture_float"),
            "WebGL1 list should carry the core-promoted entries"
        );
    }

    #[test]
    fn webgl_patch_under_native_strategy_emits_nothing() {
        let mut out = String::new();
        push_webgl(
            &mut out,
            Some(&WebglSpec { strategy: Some(Strategy::Native), ..Default::default() }),
        );
        assert!(out.is_empty(), "Native must leave the real backend alone, got: {out}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p zendriver-stealth --lib patches 2>&1 | tail -20
```

Expected: FAIL — the current patch has no `WEBGL_PROFILE` placeholder.

- [ ] **Step 3: Rewrite the JavaScript**

Replace `crates/zendriver-stealth/src/patches/webgl.js` entirely:

```javascript
// Coherent WebGL surface driven by one substituted profile object.
//
// Every value a fingerprinter can read comes from the table: the two
// DEBUG_renderer_info UNMASKED strings, the plain VENDOR/RENDERER, every
// spec-defined getParameter enum, getShaderPrecisionFormat, and a
// per-context-version extension list. Enums outside the table fall through to
// the real backend, which is correct for vendor-specific enums we do not model.
//
// Why per-context extension lists: about sixteen WebGL1 extensions are core in
// WebGL2 and a real WebGL2 context does not list them. Serving one array to
// both prototypes claims extensions that cannot exist, which is its own tell.
//
// Why getExtension is patched too: getSupportedExtensions and getExtension must
// agree in both directions. Claiming an extension whose getExtension returns
// null is a one-line detection, and so is handing over an extension the list
// never claimed.
(function (profile) {
  if (!profile) return;

  var INERT_STUBS = profile.inertStubs || {};

  function paramsFor(isV2) {
    return isV2 ? profile.params2 : profile.params1;
  }

  function decode(v) {
    // The Rust side tags each value with its GL type so the right typed array
    // reaches the page. An Int32Array where Chrome returns a Float32Array is
    // caught by one instanceof check.
    if (v === null || typeof v !== 'object') return v;
    switch (v.t) {
      case 'i32pair': return new Int32Array(v.v);
      case 'i32quad': return new Int32Array(v.v);
      case 'f32pair': return new Float32Array(v.v);
      case 'f32quad': return new Float32Array(v.v);
      case 'u32list': return new Uint32Array(v.v);
      default: return v.v;
    }
  }

  function patch(proto, isV2) {
    var table = paramsFor(isV2);
    var exts = isV2 ? profile.extensions2 : profile.extensions1;
    var extSet = Object.create(null);
    for (var i = 0; i < exts.length; i++) extSet[exts[i]] = true;

    __zdReplace(proto, 'getParameter', function (orig) {
      return function (param) {
        var name = profile.enumNames[param];
        if (name && Object.prototype.hasOwnProperty.call(table, name)) {
          return decode(table[name]);
        }
        return orig.call(this, param);
      };
    });

    __zdReplace(proto, 'getShaderPrecisionFormat', function (orig) {
      return function (shaderType, precisionType) {
        var key =
          profile.enumNames[shaderType] + '/' + profile.enumNames[precisionType];
        var p = profile.precision[key];
        if (!p) return orig.call(this, shaderType, precisionType);
        return { rangeMin: p[0], rangeMax: p[1], precision: p[2] };
      };
    });

    __zdReplace(proto, 'getSupportedExtensions', function () {
      return function () {
        return exts.slice();
      };
    });

    __zdReplace(proto, 'getExtension', function (orig) {
      return function (name) {
        if (!extSet[name]) return null; // never hand over what we did not claim
        var stub = INERT_STUBS[name];
        if (stub) {
          // Inert extension: pure constants, nothing to break. Synthesize it
          // so the claimed list and getExtension agree.
          var real = orig.call(this, name);
          if (real) return real;
          var o = {};
          for (var k in stub) o[k] = stub[k];
          return o;
        }
        // Functional extension: only claimed when the backend really has it,
        // so a working object is what comes back.
        return orig.call(this, name);
      };
    });
  }

  if (window.WebGLRenderingContext) patch(WebGLRenderingContext.prototype, false);
  if (window.WebGL2RenderingContext) patch(WebGL2RenderingContext.prototype, true);
})(WEBGL_PROFILE);
```

- [ ] **Step 4: Rewrite `push_webgl`**

Replace `push_webgl` in `crates/zendriver-stealth/src/patches.rs` (currently lines 234-251):

```rust
/// Append the WebGL surface patch, substituting one resolved profile.
///
/// Under [`Strategy::Native`] nothing is emitted at all: the caller asked for
/// the real backend, and a partial patch is what produces incoherent pairs
/// like a spoofed viewport beside a real texture limit.
fn push_webgl(out: &mut String, spec: Option<&WebglSpec>) {
    let strat = Surface::Webgl.resolve_strategy(spec.and_then(|s| s.strategy));
    if strat == Strategy::Native {
        return;
    }
    let renderer = spec
        .and_then(|s| s.unmasked_renderer.as_deref())
        .unwrap_or(crate::gpu::devices::DEFAULT_RENDERER);
    let device = crate::gpu::devices::device_for_renderer(renderer);
    let mut profile = crate::gpu::profile_for_tier(device.tier);
    profile.unmasked_vendor = spec
        .and_then(|s| s.unmasked_vendor.clone())
        .unwrap_or_else(|| device.unmasked_vendor.to_string());
    profile.unmasked_renderer = renderer.to_string();

    if let Err(why) = crate::gpu::invariants::check_coherence(&profile) {
        // Warn rather than fail: the caller may have pinned an odd value
        // deliberately, and refusing to launch over a fingerprint detail is a
        // worse failure than reporting one. Matches the header-coherence
        // warn-on-skew precedent.
        tracing::warn!(reason = %why, "GPU profile is internally incoherent");
    }

    out.push('\n');
    out.push_str(&WEBGL.replace("WEBGL_PROFILE", &crate::gpu::profile_to_js(&profile)));
}
```

Add `profile_to_js` to `crates/zendriver-stealth/src/gpu/mod.rs`, plus the `DEFAULT_RENDERER` constant to `devices.rs`:

```rust
/// Serialize a profile into the JSON object `webgl.js` consumes.
///
/// Each value carries a `t` tag naming its GL type so the JS side builds the
/// right typed array; JSON alone cannot distinguish `Int32Array` from
/// `Float32Array`.
pub(crate) fn profile_to_js(p: &GpuProfile) -> String {
    fn val(v: &GlParam) -> serde_json::Value {
        use serde_json::json;
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
    let conv = |m: &BTreeMap<String, GlParam>| -> serde_json::Value {
        m.iter().map(|(k, v)| (k.clone(), val(v))).collect::<serde_json::Map<_, _>>().into()
    };
    serde_json::json!({
        "params1": conv(&p.params_webgl1),
        "params2": conv(&p.params_webgl2),
        "precision": p.precision.iter().map(|(k, v)| {
            (k.clone(), serde_json::json!([v.range_min, v.range_max, v.precision]))
        }).collect::<serde_json::Map<_, _>>(),
        "extensions1": p.extensions_webgl1,
        "extensions2": p.extensions_webgl2,
        "enumNames": enum_names(),
        "inertStubs": inert_stubs(),
    })
    .to_string()
}
```

`enum_names()` reads the generated `ENUM_NAMES` table (emitted in Task 3 from the `enums` block the probe now records) and returns it as a JSON object keyed by the numeric enum, which is what `profile.enumNames[param]` indexes:

```rust
/// Numeric GL enum to parameter name, as the JS side indexes it.
///
/// A JS object is keyed by number, so aliases collapse: `BLEND_EQUATION` and
/// `BLEND_EQUATION_RGB` are both enum `32777`, and only one name survives into
/// the emitted object. That is safe **only while aliased names carry equal
/// values**, which is asserted below rather than assumed — if a future capture
/// ever gives an aliased pair different values, silently keeping one would
/// serve the wrong number for the other.
fn enum_names() -> serde_json::Value {
    let mut out = serde_json::Map::new();
    let mut chosen: std::collections::BTreeMap<u32, &str> = std::collections::BTreeMap::new();
    for (num, name) in tiers::ENUM_NAMES {
        if let Some(prev) = chosen.insert(*num, name) {
            // Both spellings must resolve to the same value, or collapsing
            // them changes what the page reads.
            let a = tiers::BASE_PARAMS_WEBGL2.iter().find(|(k, _)| *k == prev);
            let b = tiers::BASE_PARAMS_WEBGL2.iter().find(|(k, _)| *k == *name);
            assert_eq!(
                a.map(|(_, v)| v),
                b.map(|(_, v)| v),
                "GL enum {num} aliases {prev} and {name}, which hold different \
                 values; collapsing them would serve the wrong one"
            );
            continue;
        }
        out.insert(num.to_string(), serde_json::Value::from(*name));
    }
    out.into()
}
```

Extend `emit_rust` in Task 3 to emit that table from the union of every capture's `enums` block, asserting the tiers agree on any shared enum (a disagreement means one capture is malformed, since GL enum numbers are fixed by the spec):

```rust
    s.push_str("/// Numeric GL enum -> parameter name. Fixed by the WebGL spec.\n");
    s.push_str("pub(crate) static ENUM_NAMES: &[(u32, &str)] = &[\n");
    let mut seen: BTreeMap<u32, &str> = BTreeMap::new();
    for t in tiers {
        for (name, num) in &t.enums {
            if let Some(prev) = seen.insert(*num, name) {
                assert_eq!(prev, name, "enum {num} maps to two names; a capture is malformed");
            }
        }
    }
    for (num, name) in &seen {
        s.push_str(&format!("    ({num}, {name:?}),\n"));
    }
    s.push_str("];\n\n");
```

This needs `pub enums: BTreeMap<String, u32>` on `TierData`, populated in `tier_from_capture` from `capture["webgl2"]["enums"]`.

`inert_stubs()` is hand-written in `devices.rs` and covers only the pure-constant extensions:

```rust
/// Extensions whose objects carry nothing but constants, so a synthesized
/// stub is indistinguishable from the real thing. Functional extensions are
/// deliberately absent: those are only ever claimed when the backend really
/// provides them, so a stub would be a lie the page can catch by calling it.
pub(crate) fn inert_stubs() -> serde_json::Value {
    serde_json::json!({
        "WEBGL_debug_renderer_info": {
            "UNMASKED_VENDOR_WEBGL": 37445,
            "UNMASKED_RENDERER_WEBGL": 37446
        },
        "EXT_texture_filter_anisotropic": {
            "TEXTURE_MAX_ANISOTROPY_EXT": 34046,
            "MAX_TEXTURE_MAX_ANISOTROPY_EXT": 34047
        }
    })
}
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p zendriver-stealth --lib 2>&1 | tail -20
```

Expected: PASS. The pre-existing `patches.rs` substitution tests may need updating for the new placeholder; update assertions, not behavior.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/zendriver-stealth/src
git commit -m "feat(stealth)!: drive webgl.js from the resolved GPU profile"
```

---

### Task 9: Verify against real Chrome

The tables are only worth anything if the browser actually reports them.

**Files:**
- Create: `crates/zendriver/tests/gpu_profile.rs`

**Interfaces:**
- Consumes: everything above, through `Browser::builder().stealth(...)`.
- Produces: `spoofed_profile_is_internally_coherent`, `extension_lists_agree_with_get_extension`.

- [ ] **Step 1: Write the test**

Create `crates/zendriver/tests/gpu_profile.rs`:

```rust
//! Real-Chrome verification that the spoofed WebGL surface is coherent.
//!
//! Run with:
//! `cargo test -p zendriver --features integration-tests --test gpu_profile -- --ignored`
#![cfg(feature = "integration-tests")]

use zendriver::{Browser, StealthProfile};

/// Reads the pairs a fingerprinter cross-checks.
const CHECK_JS: &str = r#"
(() => {
  const gl = document.createElement('canvas').getContext('webgl2');
  if (!gl) return JSON.stringify({error: 'no webgl2'});
  const dbg = gl.getExtension('WEBGL_debug_renderer_info');
  const listed = gl.getSupportedExtensions();
  const claimedButMissing = listed.filter(n => gl.getExtension(n) === null);
  const aliased = gl.getParameter(gl.ALIASED_POINT_SIZE_RANGE);
  return JSON.stringify({
    renderer: dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
    maxTexture: gl.getParameter(gl.MAX_TEXTURE_SIZE),
    maxViewport: Array.from(gl.getParameter(gl.MAX_VIEWPORT_DIMS)),
    combined: gl.getParameter(gl.MAX_COMBINED_TEXTURE_IMAGE_UNITS),
    frag: gl.getParameter(gl.MAX_TEXTURE_IMAGE_UNITS),
    vert: gl.getParameter(gl.MAX_VERTEX_TEXTURE_IMAGE_UNITS),
    claimedButMissing,
    aliasedIsFloat32: aliased instanceof Float32Array,
  });
})()
"#;

fn file_url(name: &str) -> String {
    let page = std::env::temp_dir().join(name);
    std::fs::write(&page, "<!doctype html><title>probe</title>").expect("write probe page");
    let p = page.display().to_string().replace('\\', "/");
    if p.starts_with('/') { format!("file://{p}") } else { format!("file:///{p}") }
}

#[tokio::test]
#[ignore = "launches real Chrome"]
async fn spoofed_profile_is_internally_coherent() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto(&file_url("zendriver-gpu-profile.html")).await.expect("goto");
    tab.wait_for_load().await.expect("load");
    let raw: String = tab.evaluate(CHECK_JS).await.expect("evaluate");
    browser.close().await.ok();

    let got: serde_json::Value = serde_json::from_str(&raw).expect("probe json");
    let tex = got["maxTexture"].as_i64().expect("maxTexture");
    let vp = got["maxViewport"][0].as_i64().expect("maxViewport");
    assert!(
        vp >= tex,
        "viewport {vp} below texture max {tex} — the exact pair this work fixes: {got:#}"
    );
    let (c, f, v) = (
        got["combined"].as_i64().expect("combined"),
        got["frag"].as_i64().expect("frag"),
        got["vert"].as_i64().expect("vert"),
    );
    assert!(c >= f + v, "combined {c} < frag {f} + vert {v}: {got:#}");
    assert_eq!(
        got["aliasedIsFloat32"], true,
        "ALIASED_POINT_SIZE_RANGE must be a Float32Array, not Int32Array: {got:#}"
    );
}

#[tokio::test]
#[ignore = "launches real Chrome"]
async fn extension_lists_agree_with_get_extension() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto(&file_url("zendriver-gpu-ext.html")).await.expect("goto");
    tab.wait_for_load().await.expect("load");
    let raw: String = tab.evaluate(CHECK_JS).await.expect("evaluate");
    browser.close().await.ok();

    let got: serde_json::Value = serde_json::from_str(&raw).expect("probe json");
    let missing = got["claimedButMissing"].as_array().expect("array");
    assert!(
        missing.is_empty(),
        "every claimed extension must resolve; these did not: {missing:?}"
    );
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p zendriver --features integration-tests --test gpu_profile -- --ignored --nocapture 2>&1 | tail -25
```

Expected: both PASS. A failure here is the real signal — it means the tables and the browser disagree. Do not weaken an assertion to make it pass; fix the table or the patch.

- [ ] **Step 3: Commit**

```bash
cargo fmt --all
git add crates/zendriver/tests/gpu_profile.rs
git commit -m "test(zendriver): verify the spoofed WebGL surface is coherent in real Chrome"
```

---

### Task 10: Docs, ledger, baseline, gates

**Files:**
- Modify: `README.md`, `docs/book/src/fingerprint.md`, `crates/zendriver-mcp/mcp-coverage-ledger.toml`, `crates/zendriver-mcp/public-api-baseline.txt`

**Interfaces:**
- Consumes: everything above.
- Produces: no code.

- [ ] **Step 1: Document the new surface**

In `docs/book/src/fingerprint.md`, extend the WebGL section: the spoofed surface now covers every spec-defined parameter, per-context extension lists, and `getShaderPrecisionFormat`, resolved from capability tiers rather than a fixed handful of values. State plainly that the tables ship two tiers today (SwiftShader and Apple Metal) and that a renderer outside them falls back to the desktop tier.

In `README.md`, update the feature matrix row for WebGL spoofing. The MCP tool count is unchanged; no tool was added.

- [ ] **Step 2: Ledger the new public API**

Add to `crates/zendriver-mcp/mcp-coverage-ledger.toml`, following the `WebgpuSpec` precedent:

```toml
# ── GPU profile tier tables (2026-07-25, spec phases 3-4) ───────────────────
[[entry]]
api = "zendriver_stealth::GpuProfile"
excluded = "caller-supplied fingerprint spec carried inside the opaque Persona JSON, consistent with WebglSpec/WebgpuSpec"

[[entry]]
api = "zendriver_stealth::GlParam"
excluded = "value type inside GpuProfile; not independently reachable"

[[entry]]
api = "zendriver_stealth::ShaderPrecision"
excluded = "value type inside GpuProfile; not independently reachable"

[[entry]]
api = "zendriver_stealth::Provenance"
excluded = "value type inside GpuProfile; not independently reachable"
```

Verify each path against the regenerated baseline's actual spelling; if `cargo public-api` renders one differently, the tool's spelling wins.

- [ ] **Step 3: Regenerate the baseline**

```bash
cargo +nightly public-api -p zendriver --all-features > crates/zendriver-mcp/public-api-baseline.txt
git diff --stat crates/zendriver-mcp/public-api-baseline.txt
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked 2>&1 | tail -10
```

Expected: the diff adds only the new `gpu` types and `Persona::gpu`; the coverage test passes.

- [ ] **Step 4: Run the full gates**

Run these in parallel; they are independent:

```bash
cargo fmt --all --check
```

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
```

```bash
cargo test --workspace --locked
```

Plus the feature-gated passes and the book:

```bash
cargo clippy -p zendriver-mcp --all-features --all-targets --locked -- -D warnings
cargo clippy -p zendriver --features integration-tests --all-targets --locked -- -D warnings
mdbook build docs/book
```

And confirm the generated table is still current:

```bash
cargo run -p gpu-tier-gen && git diff --exit-code crates/zendriver-stealth/src/gpu/tiers.rs
```

- [ ] **Step 5: Rebase onto whatever release-plz landed**

```bash
git fetch origin
git rebase origin/main
cargo test --workspace --locked 2>&1 | tail -5
```

If release-plz bumped versions, the rebase should be clean: this branch touches `members` in the root `Cargo.toml`, not `[workspace.dependencies]` versions. Resolve any conflict in favour of release-plz's version numbers and keep this branch's `members` addition.

- [ ] **Step 6: Commit**

```bash
git add README.md docs/book crates/zendriver-mcp
git commit -m "docs: document the GPU profile tier tables"
```

---

## What this plan deliberately does not do

- **No worker injection.** Plan 3. Every patch here is still bypassable via `OffscreenCanvas` in a worker, which is a known, documented hole until that lands.
- **No pixel farbling and no MediaCapabilities.** Plan 4.
- **No new tiers beyond the two measured here.** A tier nobody probed would be invented data, which the spec explicitly forbids. Adding a tier means capturing it on that hardware and rerunning the generator.
- **`getTranslatedShaderSource` and timer queries stay as they are.** Both extensions are genuinely exposed by Chrome, so the honest options are to leave them or to stop claiming them; the spec settled on leaving them and documenting the gap.
