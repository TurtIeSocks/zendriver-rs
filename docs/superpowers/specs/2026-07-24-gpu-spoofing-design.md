# Full GPU spoofing — design

> **Status:** design approved 2026-07-24. Supersedes the partial WebGL/WebGPU
> spoofing shipped in `webgl.js` / `webgpu.js` / `persona/webgpu_adapter.rs`.

## Problem

zendriver's GPU spoofing today covers 6 of roughly 70 readable WebGL values.
Everything else falls through to whatever backend Chrome is actually running —
SwiftShader, under the default headless flags. The result is not merely
incomplete, it is **impossible**: measured on real Chrome (see
[Measurements](#measurements)), the current patch reports

- `MAX_VIEWPORT_DIMS` = `[32767, 32767]` (spoofed), and
- `MAX_TEXTURE_SIZE` = `8192` (SwiftShader's, unpatched).

No real GPU has a viewport four times its maximum texture dimension. A single
`getParameter` pair identifies the browser as patched. The unmasked renderer
string claims `ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 …)` while the
capability surface underneath describes a CPU rasterizer.

Three further gaps compound it:

1. **`getExtension` is unpatched.** `getSupportedExtensions()` returns a
   hardcoded 35-entry list; `getExtension()` still answers from the real
   backend. SwiftShader offers 30 extensions, so the two contradict each other
   in both directions — we claim extensions that return `null`, and hand over
   extensions we never claimed.
2. **The same extension list is served to WebGL1 and WebGL2.** `webgl.js:64-65`
   applies one 35-entry array to both prototypes. In WebGL2, roughly ten of
   those — `OES_texture_float`, `OES_element_index_uint`,
   `OES_vertex_array_object`, `ANGLE_instanced_arrays`, `WEBGL_depth_texture`,
   `EXT_frag_depth`, `EXT_sRGB` and friends — are **core**, and a real WebGL2
   context does not list them as extensions. Claiming them is a tell on its own.
3. **Workers bypass the spoof entirely.** `observer.rs:99` skips worker targets
   ("workers have no DOM"). True and irrelevant: `new Worker()` →
   `new OffscreenCanvas(1,1).getContext('webgl')` reaches an unpatched context
   in three lines.
4. **WebGL pixel output is never farbled.** `canvas.js` wraps
   `CanvasRenderingContext2D.getImageData` and `HTMLCanvasElement.toDataURL`,
   but `toDataURL`'s path calls `this.getContext('2d')`, which returns `null`
   for a WebGL-backed canvas. The classic WebGL fingerprint — draw, hash the
   pixels — is pure SwiftShader output, identical across every zendriver user.
   That is a shared cluster identifier.

## Measurements

All figures below are from real Chrome on the darwin dev host (Apple M4 Pro),
same probe page, `--dump-dom` with `--virtual-time-budget`. These are the
ground truth this design is built on; assumptions that contradicted them were
discarded (see [Corrections](#corrections-from-measurement)).

| Value | **A:** `--disable-gpu --use-angle=swiftshader` (zendriver default) | **C:** `--headless=new --use-gl=angle --use-angle=metal` |
|---|---|---|
| `'gpu' in navigator` | `true` | `true` |
| `requestAdapter()` | `null` | `{vendor:"apple", architecture:"metal-3", device:"", description:""}` |
| `requestDevice()` | n/a | **succeeds** |
| `MAX_TEXTURE_SIZE` | 8192 | 16384 |
| `MAX_VIEWPORT_DIMS` | `[8192,8192]` | `[16384,16384]` |
| `MAX_VERTEX_UNIFORM_VECTORS` | 4096 | 1024 |
| `getSupportedExtensions().length` | 30 | 36 |
| `WEBGL_debug_shaders` present | yes | yes |
| `EXT_disjoint_timer_query` present | yes | yes |
| `getShaderPrecisionFormat(FRAGMENT, HIGH_FLOAT)` | `[127,127,23]` | `[127,127,23]` |
| `failIfMajorPerformanceCaveat: true` yields a context | yes | yes |
| unmasked renderer | `ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (LLVM 10.0.0) (0x0000C0DE)), SwiftShader driver)` | `ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)` |

The `'gpu' in navigator` row above was measured on the `file://` probe page
this appendix loads via `--dump-dom`, which is a secure context. That
precondition matters: `navigator.gpu` is `[SecureContext]`-gated, so an
opaque-origin page (`about:blank`, `data:`) reports `'gpu' in navigator` as
`false` regardless of these backend flags (see
[Corrections](#corrections-from-measurement)). Re-running this table against
`about:blank` instead of `file://` will not reproduce the `true` values above
— that is expected, not a discrepancy in this table.

Two configurations that did **not** work, recorded so they are not retried:

- `--headless=new` with `--disable-gpu` simply removed: **hangs**. Killed at 30s,
  twice. Dropping the flag without naming a backend is not a viable path.
- Headful with no flags produced no result within the same window on this host.

### Why the capability values cluster the way they do

WebGL's numeric caps are not queried from the GPU. ANGLE computes them from
hardcoded constants branched on a small number of backend capability tiers:

- `src/libANGLE/renderer/d3d/d3d11/renderer11_utils.cpp:342` —
  `GetMaximum2DTextureSize()` returns `D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION`
  (16384) for every feature level ≥ 11_0. Roughly forty sibling
  `GetMaximum*(D3D_FEATURE_LEVEL)` functions follow the same shape.
- `src/libANGLE/renderer/metal/DisplayMtl.mm:718-762` — literals branched on
  `supportsAppleGPUFamily(3)` and Apple-Silicon-vs-Mac. `max2DTextureSize` is
  16384 or 8192; `maxRenderbufferSize` and `maxCubeMapTextureSize` are derived
  from it.

The only device-dependent queries in either file are **format support** and
**driver-bug workarounds**, not the numeric GL caps. Measurement C confirms
this: real Metal reported exactly the `DisplayMtl.mm` constants.

Consequence: desktop WebGL capability surfaces cluster by **(backend, tier)** —
five or six buckets covering all of desktop — not by GPU model. This is what
makes a hand-maintained table tractable, and it is why swapping GPU identity
*within* a tier is nearly free while swapping *across* tiers is not.

A widely-repeated claim that `MAX_TEXTURE_SIZE` "reveals VRAM size and driver
version" is false for Chrome. It is a compile-time constant per feature level.

### No usable public corpus

Searched for an importable dataset of WebGL parameter dumps. None suitable:

- `gpuinfo.org` (OpenGL / OpenGL ES / Vulkan hardware databases) holds **native**
  driver reports, predominantly Android. Native GL is not ANGLE-translated
  WebGL, so the values do not transfer. Its web front-end is AGPL-3.0; terms for
  the data itself are unstated, and the download endpoint returns HTTP 403.
- `zendriver-fingerprints` already vendors Apache-2.0 data from
  `fingerprint-suite`, but that Bayesian network carries only `videoCard`
  (vendor/renderer strings) — no parameter-level capabilities.

ANGLE's source is a better authority than any corpus regardless: it is the code
that produces the numbers.

## Approach

Two complementary layers, sequenced so the larger win lands first.

**L0 — run a real GPU.** Chrome can use the host GPU headless. On this host
that is one flag change away and it delivers, at no fidelity cost, everything
the patch layer strains to fake: all ~70 parameters coherent, real precision
formats, the real extension list, real rasterization, real timings, a working
`requestDevice()`, real hardware decode. What L0 does *not* give is identity
control — you get the host's GPU, and a fleet on one host shares one
fingerprint.

**L1–L3 — the patch layer.** Provides identity control and covers hosts with no
GPU at all (containers, most CI). Because of the tier finding, with L0 underneath
this layer's job shrinks from "manufacture a GPU out of a CPU rasterizer" to
"translate between two real tiers" — both endpoints real, constants known.

They compose. L0 is the foundation; the patch layer is identity control on top
of it and the fallback beneath it.

Rejected alternatives:

- **Flat per-device rows** (one row per GPU model, all parameters inline).
  Every D3D11 device repeats the same forty numbers; that duplication is
  precisely where incoherence enters, and it cannot be seeded without a
  hardware lab.
- **Probe-only, no derivation.** Zero invented values, but ships with one
  profile and leaves out-of-box behavior leaky indefinitely.
- **Heuristic derivation from vendor/model.** Plausible-looking invented
  numbers matching no real device — the failure mode `WebgpuSpec`'s rustdoc
  already warns against.

## Data model

Three layers, flat at the public boundary.

**Tier table** (internal, `&'static`), transcribed from ANGLE source:

```rust
enum Tier { D3d11Fl11, D3d11Fl10, MetalAppleFamily3, MetalMac, LinuxVulkan, SwiftShader }

struct ShaderPrecision { range_min: i32, range_max: i32, precision: i32 }

struct TierCaps {
    params: &'static [(u32, GlParam)],                  // ~40 WebGL1 + ~30 WebGL2 enums
    precision: &'static [(u32, u32, ShaderPrecision)],  // 2 shader types × 6 precision types
    extensions_webgl1: &'static [&'static str],
    extensions_webgl2: &'static [&'static str],         // core-promoted entries removed
}
```

The tier list is provisional. Linux ANGLE may run Vulkan or GL depending on
build and driver, and the exact Linux tier set is settled in Phase 3 from
probes rather than guessed here. Splitting the extension list by context
version is what fixes problem 2 above.

**Device rows** — only what genuinely varies per device:

```rust
struct DeviceRow {
    unmasked_vendor: &'static str,
    unmasked_renderer: &'static str,
    tier: Tier,
    webgpu: WebgpuValues,               // vendor, architecture, limits, features
    ext_add: &'static [&'static str],   // driver-gated additions
    ext_remove: &'static [&'static str],// driver-gated removals
    media: MediaCaps,                   // hardware decode blocks
}
```

**Resolved** — `GpuProfile`: fully concrete, owned, `serde`. The public API only
ever sees this. Tiers stay private, so re-tiering is never a breaking change.

`getParameter` returns six distinct shapes, so parameter values are:

```rust
enum GlParam { Int(i64), Float(f64), Bool(bool), IntPair([i32;2]), FloatPair([f32;2]), Str(String) }
```

Provenance travels with the data:

```rust
enum Provenance {
    Probed  { chrome: String, os: String },
    Derived { source: String },   // e.g. "ANGLE renderer11_utils.cpp GetMaximum* @ <rev>"
}
```

A `Derived` row **omits** any field it cannot source rather than inventing one.

**Resolution order** (later wins): tier caps → device row → caller-supplied
`GpuProfile` → `Persona.webgl` / `Persona.webgpu` spec overrides. The existing
specs remain the finest-grained escape hatch. There is no separate "preset"
concept — looking up a device row by renderer string *is* the preset mechanism,
and `WebgpuValues` / `MediaCaps` are field bundles of that row, not independent
types callers assemble.

### Invariants

Each is a test, not a comment.

1. **Completeness.** The tier table covers every spec-defined WebGL1 and WebGL2
   parameter enum. Enums outside the spec set fall through to the real backend;
   spec'd ones never do. This is what actually closes the leak — today 6 of ~70
   are covered and the other 64 answer honestly.
2. **Self-consistency.** `MAX_VIEWPORT_DIMS ≥ MAX_TEXTURE_SIZE`;
   `MAX_COMBINED_TEXTURE_IMAGE_UNITS ≥ MAX_TEXTURE_IMAGE_UNITS +
   MAX_VERTEX_TEXTURE_IMAGE_UNITS`; precision ranges monotonic. Fingerprinters
   check these relations. A table edit that breaks one fails CI. This invariant
   is exactly what the shipped `[32767,32767]` / `8192` pair violates.
3. **Platform coherence.** The claimed OS must match the backend tier — a
   Windows persona cannot resolve Metal's 8192. Enforced as **warn-on-skew**,
   matching the header-coherence precedent from #43 rather than inventing a new
   failure mode.

## Components

### Layout

No new crate — this is stealth data consumed by stealth patches, and the
workspace is nine crates already.

```
crates/zendriver-stealth/src/gpu/
  mod.rs      // GpuProfile, GlParam, Provenance, resolve()
  tiers.rs    // TierCaps constants transcribed from ANGLE
  devices.rs  // DeviceRow table + renderer-string lookup
  media.rs    // hardware-decode matrix
```

`persona/webgpu_adapter.rs` folds into `devices.rs`: its renderer→vendor/architecture
mapping becomes one column of the device row, with `adapter_for_renderer`
retained as the fallback for renderer strings absent from the table.

Patches: `webgl.js` rewritten table-driven, `webgpu.js` extended, new
`media_caps.js`.

### Persona wiring

```rust
pub struct Persona {
    pub gpu: Option<GpuProfile>,     // new
    pub webgl: Option<WebglSpec>,    // unchanged, overlays gpu
    pub webgpu: Option<WebgpuSpec>,  // unchanged, overlays gpu
    // …
}
```

`gpu` is whole-value in `Persona::overlay` — one device is one coherent
artifact, the same rule `ScreenSpec` follows.

**Default behavior changes deliberately.** With `gpu: None` the resolver looks
up the device row for the existing default renderer and emits the complete
Intel UHD 630 profile instead of today's six parameters. Every existing caller's
fingerprint changes without a code change.

This is the correct default rather than an opt-in: the project's fingerprint
philosophy is coherent defaults, everything overridable, never lock a value —
and this is not detect-and-adjust. Nothing is probed; the default renderer
string is the same one it has always been, merely no longer contradicted by 64
unspoofed parameters. Ships as `feat(stealth)!` with a migration note.

### L0 — GPU backend selection

```rust
enum GpuBackend { Disabled, SwiftShader, Native }
```

`BrowserBuilder::gpu_backend(GpuBackend)`. Default stays `Disabled` (today's
behavior). `Native` resolves per-platform to the appropriate ANGLE backend:
macOS → `metal`, Windows → `d3d11`, Linux → `vulkan`. A named opt-in with
documented deterministic behavior, in the same shape as `geo_auto` — not
detect-and-adjust.

Two requirements, both measured rather than assumed:

- Emit `--use-gl=angle --use-angle=<backend>` **and** drop `--disable-gpu`
  together. Removing `--disable-gpu` alone hangs Chrome.
- `flags_for_profile` currently pushes the SwiftShader flags unconditionally for
  `ProfileKind::Spoofed` (`flags.rs:83-85`). That becomes conditional on the
  selected backend.

**No auto-fallback.** If the GPU process fails to start, surface a launch
timeout and a clear error. Silent fallback to SwiftShader is precisely the
behavior the project's no-auto law forbids, and it would silently reintroduce
the incoherence this design exists to remove.

### L1 — read surface

`webgl.js` becomes table-driven: Rust substitutes one resolved profile object
and the JS performs a lookup, replacing the current ladder of `if (param === …)`.

- **`getParameter`** — complete map over spec'd WebGL1 and WebGL2 enums, plus
  enums defined by claimed extensions. `MAX_TEXTURE_MAX_ANISOTROPY_EXT` is read
  constantly and is 16 on D3D11.
- **`getShaderPrecisionFormat`** — 12-entry table. Note from measurement:
  `FRAGMENT_SHADER`/`HIGH_FLOAT` is `[127,127,23]` on both SwiftShader and real
  Metal, so highp float carries little entropy; `mediump`/`lowp` and the vertex
  stage are where the discrimination lives.
- **`getSupportedExtensions` / `getExtension`** — one source of truth, agreement
  enforced in both directions, and **served per context version**: the WebGL2
  list drops the roughly ten entries promoted to core, which today's shared
  array wrongly claims on a WebGL2 context.
- **WebGL2 extras** — `getIndexedParameter`, and
  `getInternalformatParameter(…, SAMPLES)`, which reports per-format MSAA counts
  and is genuinely backend-dependent.

**Extension claiming rule.** Two unattractive options: intersect our list with
the real backend's (coherent, but the resulting list *is* a backend signature —
SwiftShader's 30 entries are distinctive), or synthesize everything (plausible
list, hollow objects). Split by kind instead:

- **Inert extensions** — pure constants: compression formats,
  `WEBGL_debug_renderer_info`, `EXT_texture_filter_anisotropic`. Claim freely;
  synthesize a stub carrying the correct constants. Nothing can break.
- **Functional extensions** — `ANGLE_instanced_arrays`,
  `OES_vertex_array_object`, `WEBGL_draw_buffers`, `EXT_disjoint_timer_query`.
  Claim **only if the real backend provides them**, so the methods work.
  SwiftShader has most, so the list stays long enough to resemble hardware.

A stub that lies about a capability the page then *uses* is worse than not
claiming it. This rule buys list plausibility without that exposure.

All new overrides route through the existing `__zdReplace` / `__zdMark` helpers
so `toString()` masking holds, consistent with every current patch.

### Worker injection

`Target.setAutoAttach { autoAttach: true, waitForDebuggerOnStart: true,
flatten: true }`. On `Target.attachedToTarget` for worker target types,
`Runtime.evaluate` the patch bundle, then `Runtime.runIfWaitingForDebugger`.
Workers expose no `Page` domain, so evaluate-before-resume is the only ordering
that lands before page script runs.

Requires an audit pass over every existing patch: some assume `window`
(`webgl.js:64`), while `webgpu.js:81` already carries the correct idiom
(`typeof self !== 'undefined' ? self : window`). Standardize on it and mark each
patch worker-applicable or not.

This item touches how **all** stealth patches deploy, not only the GPU ones. Its
regression surface is the whole patch set, and it needs its own verification
pass beyond the GPU tests.

### L2 — render output

Hook points: `readPixels` on both contexts; `toDataURL` / `toBlob` on a
WebGL-backed canvas; `OffscreenCanvas.convertToBlob` /
`transferToImageBitmap`; `createImageBitmap`. The `drawImage(webglCanvas)` →
2D-readback path is already covered by `canvas.js`, with the canvas seed rather
than the WebGL one — still farbled.

Gate:

- Export paths on a WebGL canvas → always farble.
- `readPixels` → farble only when the read rectangle covers ≳90% of the drawing
  buffer **and** the canvas is ≤512×512. Fingerprint probes are tiny full-canvas
  reads; GPU picking is a 1×1 or small sub-rectangle on a large canvas. This is
  a heuristic with a known ceiling; the comment names the ceiling and the
  upgrade path rather than implying precision.
- **`UNSIGNED_BYTE` reads only.** A ±1 LSB perturbation of a `Float32Array`
  readback is meaningless and would corrupt genuine compute or picking work.

**Determinism is mandatory.** Reuse `canvas.js`'s `__zdKeyedRng(seed, data)`,
keyed by (seed, content), so two reads of identical pixels produce identical
noise. Fingerprinters read twice and compare; nondeterministic noise is a
louder signal than no noise at all.

**Strategy, not a default.** Under `GpuBackend::Native`, real GPU pixels are the
most credible artifact available, and farbling trades blending-in for
per-persona uniqueness. Those are opposing goals and the right answer depends on
whether the caller is defending against linkage or against detection. It stays
an explicit `Strategy` on the surface; the rustdoc states the tradeoff rather
than choosing.

**Honest ceiling.** This makes each persona's WebGL hash unique and stable. It
cannot make pixels *match* the claimed GPU — a software rasterizer's output
differs from any real GPU's in ways ±1 LSB does not bridge. Killing the shared
cluster identifier is the achievable win.

### L3 — behavior

Measurement removed most of the originally scoped work here.

- **`getTranslatedShaderSource` / `WEBGL_debug_shaders`** — measured present on
  both SwiftShader and real Metal. MDN's "privileged contexts" note is
  Firefox-specific. Dropping the claim would be *incoherent*, so it stays
  claimed. Under L0 the leak is honest — a real Metal machine emitting real
  Metal shaders. Under the fallback it is a genuine hole with no good answer:
  documented, not coded around.
- **`EXT_disjoint_timer_query`** — likewise present in both configurations. No
  invented timings; documented alongside the above.
- **`MediaCapabilities`** — the one item still worth building.
  `decodingInfo()` returns `powerEfficient` from the profile's decode-block
  matrix; NVDEC / QuickSync / VideoToolbox support is published per GPU family,
  so this is real data. `VideoDecoder.isConfigSupported` (WebCodecs) must agree;
  one table feeds both.
- **`getContextAttributes` / `failIfMajorPerformanceCaveat`** — **cut.**
  Measured: context creation with `failIfMajorPerformanceCaveat: true` succeeds
  under both SwiftShader and real Metal, so it carries no signal.

### Probe tool

`examples/probe_gpu.rs`: launch non-stealth Chrome, evaluate the probe script,
print `GpuProfile` JSON tagged `Provenance::Probed`. The smallest thing that
works — no new crate and no MCP tool. Dual purpose: seeds the dataset, and lets
a caller capture their own real GPU for reuse elsewhere.

## Testing

**Unit** — tier-table completeness across the spec enum set; the three
invariants; platform-coherence warn-on-skew; JS argument substitution (existing
`patches.rs` pattern); `serde` round-trips.

**Real Chrome** (`#[ignore]`, existing integration pattern):

- Probe the live SwiftShader tier and diff it against the shipped `SwiftShader`
  row. **This is the ANGLE-drift canary** — the one tier verifiable on any host,
  including GPU-less CI.
- Under `GpuBackend::Native`, assert a real adapter resolves and
  `requestDevice()` succeeds. Skipped when the runner has no GPU.
- Every spec enum returns profile values; none returns the backend's.
- `getSupportedExtensions` and `getExtension` agree in both directions.
- Two identical `readPixels` reads produce identical noise; a 1×1 picking read
  is untouched.
- **Worker `OffscreenCanvas` WebGL matches the main thread** — the headline test
  for the worker work.

## Phasing

1. **L0 backend selection** — `GpuBackend`, per-platform ANGLE flags, the hang
   guard, conditional `flags_for_profile`, MCP tool option + schema snapshot.
2. **Probe tooling + canaries** — `examples/probe_gpu.rs`, SwiftShader drift
   canary, native-adapter test.
3. **Tier table, `GpuProfile`, resolver** — including the three invariants.
4. **L1 `webgl.js` rewrite** — closes the impossible viewport/texture pair and
   the `getExtension` contradiction.
5. **Worker injection + patch audit.**
6. **L2 farbling** as an explicit `Strategy`.
7. **L3 MediaCapabilities.**
8. **Docs, ledger, public-API baseline.**

## Obligations

- **MCP coverage** — `gpu_backend` is a new `BrowserBuilder` option, so it must
  be reachable through the browser-open tool, with an `insta` schema-snapshot
  regeneration. This is the only wire-shape change in the design. `GpuProfile`'s
  re-export takes an `excluded` ledger entry matching the `WebgpuSpec`
  precedent (caller-supplied spec, opaque inside `Persona` JSON).
- **Docs, all three surfaces** — READMEs feature matrix (MCP tool count
  unchanged, no new tool); rustdoc on `GpuProfile`, `GlParam`, `Provenance`,
  `Persona.gpu`, `GpuBackend`, with `no_run`-compilable examples; mdBook
  `fingerprint.md` gains a GPU-profiles section and an updated WebGPU
  subsection, plus a `datadome.md` cross-link.
- **Public API baseline** regenerated via `cargo +nightly public-api`.

## Risks

- **ANGLE constants drift** between Chrome versions. Mitigated by the
  SwiftShader-tier canary, which is verifiable on any host.
- **Chromium is removing the SwiftShader fallback.** `--enable-unsafe-swiftshader`
  is already required (`flags.rs:82`). The fallback layer therefore depends on a
  flag Chrome is deprecating; L0 is the durable answer, which is one more reason
  it leads the phasing.
- **Worker injection regresses every patch at once**, not only the GPU ones.
- **Extension stubs** are inert-only, which bounds the exposure, but a page
  probing a stub's prototype chain can still distinguish it.
- **L0 gives no identity diversity.** A fleet on one host shares one real GPU
  fingerprint. Real-looking, but still a cluster.
- **Throughput is unfixable at the patch layer.** A page that renders and
  measures frame time observes software rasterization regardless of reported
  values. Only L0 addresses this.

## Corrections from measurement

Recorded because the pre-measurement reasoning was wrong in ways worth not
repeating:

- **`WEBGL_debug_shaders` is exposed by Chrome**, on both SwiftShader and real
  Metal. The plan to stop claiming it would have *introduced* incoherence.
- **`EXT_disjoint_timer_query` is likewise exposed.** Same correction.
- **`failIfMajorPerformanceCaveat: true` yields a context under SwiftShader**, so
  it is not a software-renderer detector and the planned patch was cut.
- **`'gpu' in navigator` measured `true`** under the GPU-relevant flags
  (`--disable-gpu --use-gl=angle --use-angle=swiftshader
  --enable-unsafe-swiftshader`), contradicting the note in
  `docs/superpowers/deferred-backlog.md:103`, which recorded it as `false` in
  both headless and headful. **Re-confirmed 2026-07-25 (Chrome
  150.0.7871.186), and it was never a launch-flags question.**
  `navigator.gpu` is `[SecureContext]`-gated: `about:blank` and `data:` URLs
  are opaque origins where `window.isSecureContext` is `false`, and WebGPU is
  absent there regardless of flags, backend, or headless/headful. The
  `deferred-backlog.md` note almost certainly measured from a non-secure page
  and misattributed the absence to launch flags. `cargo run -p zendriver
  --example probe_gpu -- disabled`, which navigates to a `file://` page
  (secure context) through `BrowserBuilder`'s real launch argv, confirms
  `{"isSecureContext":true,"gpuInNavigator":true,"adapter":null}` under
  zendriver's default flags — the governing variable is the page's
  secure-context status, not the flag set. Either way, `requestAdapter()`
  resolving `null` — not the property being absent — is what the fabrication
  path actually has to handle.
- **`MAX_TEXTURE_SIZE` does not encode VRAM.** It is a compile-time constant per
  feature level.

## Appendix — reproducing the measurements

Saved so the numbers above can be re-checked against a future Chrome rather
than trusted. Write to `probe.html`, then run each configuration:

```bash
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
"$CHROME" --headless=new --use-gl=angle --use-angle=metal --no-sandbox \
  --user-data-dir="$(mktemp -d)" --virtual-time-budget=6000 \
  --dump-dom "file://$PWD/probe.html" 2>/dev/null | grep -o '{"gpuInNavigator.*}'
```

Swap the backend flags for configuration A
(`--disable-gpu --use-gl=angle --use-angle=swiftshader --enable-unsafe-swiftshader`).
Note that `--headless=new` with `--disable-gpu` merely removed will hang; kill
it rather than waiting.

```html
<body><div id=out>pending</div><script>
(async () => {
  const r = {};
  r.gpuInNavigator = ('gpu' in navigator);
  try {
    const a = navigator.gpu ? await navigator.gpu.requestAdapter() : null;
    r.adapter = a ? { info: a.info ? {v:a.info.vendor, arch:a.info.architecture,
                                      d:a.info.device, desc:a.info.description} : null,
                      maxTex: a.limits ? a.limits.maxTextureDimension2D : null,
                      nFeatures: a.features ? a.features.size : null } : null;
    if (a) { try { const dev = await a.requestDevice(); r.deviceOk = !!dev; }
             catch(e) { r.deviceOk = 'reject: '+e.name; } }
  } catch (e) { r.adapterErr = String(e); }
  const c = document.createElement('canvas');
  const gl = c.getContext('webgl2') || c.getContext('webgl');
  if (gl) {
    const dbg = gl.getExtension('WEBGL_debug_renderer_info');
    r.unmaskedRenderer = dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : 'no-ext';
    r.unmaskedVendor   = dbg ? gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL)   : 'no-ext';
    r.maxTextureSize = gl.getParameter(gl.MAX_TEXTURE_SIZE);
    r.maxViewportDims = Array.from(gl.getParameter(gl.MAX_VIEWPORT_DIMS));
    r.maxVertUnif = gl.getParameter(gl.MAX_VERTEX_UNIFORM_VECTORS);
    r.nExtensions = gl.getSupportedExtensions().length;
    r.hasDebugShaders = gl.getSupportedExtensions().includes('WEBGL_debug_shaders');
    r.hasTimerQuery = gl.getSupportedExtensions().includes('EXT_disjoint_timer_query_webgl2')
                   || gl.getSupportedExtensions().includes('EXT_disjoint_timer_query');
    const fp = gl.getShaderPrecisionFormat(gl.FRAGMENT_SHADER, gl.HIGH_FLOAT);
    r.fragHighFloat = [fp.rangeMin, fp.rangeMax, fp.precision];
    const c2 = document.createElement('canvas');
    r.failIfCaveat = !!c2.getContext('webgl', {failIfMajorPerformanceCaveat: true});
  } else { r.webgl = 'null-context'; }
  document.getElementById('out').textContent = JSON.stringify(r);
})();
</script></body>
```

The probe reads a **WebGL2** context where available, which is why the
extension counts (30 / 36) are WebGL2 counts.
