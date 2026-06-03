# Native-function `toString` masking — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every function/getter the spoofed stealth bootstrap installs report as native (`[native code]`, correct `.name`/`.length`, `function get NAME()` form for accessors) in every frame, defeating `Function.prototype.toString` / `.name` fingerprinting.

**Architecture:** Add one JS prelude (`patches/_native.js`) that overrides `Function.prototype.toString` against a closure-private `WeakMap` and exposes three closure-local helpers (`__zdReplace`, `__zdGetter`, `__zdMark`). Wrap the whole bootstrap in a single outer IIFE so the helpers are in scope for every patch but never leak to `globalThis`. Route all 16 patch files' mutation sites through the helpers. Cross-realm coverage is free because the bootstrap is injected into every frame via `Page.addScriptToEvaluateOnNewDocument`.

**Tech Stack:** Rust (`zendriver-stealth` bootstrap assembly + unit tests), JavaScript (injected patches), `zendriver` integration test against real Chrome (CDP).

**Spec:** `docs/superpowers/specs/2026-06-03-stealth-tostring-masking-tracker-blocklist-design.md` §3.

**Scope:** spoofed profile only. No public API or MCP surface added.

---

## File Structure

| File | Responsibility | Action |
|------|----------------|--------|
| `crates/zendriver-stealth/src/patches/_native.js` | The masking prelude: toString override + `__zdReplace`/`__zdGetter`/`__zdMark` | **Create** |
| `crates/zendriver-stealth/src/patches.rs` | Bootstrap assembly: add `NATIVE` const, wrap in outer IIFE, unit tests | Modify |
| `crates/zendriver-stealth/src/patches/{webdriver,navigator_props,plugins,permissions,codecs,user_agent_data,broken_image}.js` | Identity patches | Modify (route through helpers) |
| `crates/zendriver-stealth/src/patches/{webgl,canvas,audio,client_rects,fonts,hardware,webrtc,webgpu}.js` | Surface patches | Modify (route through helpers) |
| `crates/zendriver/tests/stealth_native_masking.rs` | Real-Chrome assertions (`#[ignore]`) | **Create** |

`chrome.js` and `_prng.js` have no function/getter mutation sites — leave them unchanged.

## The mechanical transform (applies in Tasks 2–3)

Three site kinds, three transforms. **Bodies are unchanged** — only the call wrapping each site changes:

1. **METHOD** — `Proto.prototype.foo = function(args){ BODY }`
   becomes
   `__zdReplace(Proto.prototype, 'foo', (orig) => function(args){ BODY });`
   (if the body calls the original, it already captured it as a local `const orig = ...`; reuse the `orig` parameter the factory provides and delete the now-redundant local capture).

2. **GETTER** — `Object.defineProperty(Obj, 'p', { get: GETFN, enumerable: E, configurable: true })`
   becomes
   `__zdGetter(Obj, 'p', GETFN, { enumerable: E });`
   (preserve each site's original `enumerable` value).

3. **VALUE_FN / CTOR** — a function value or constructor reachable from the page (object-literal members, dynamically-created methods, a wrapper constructor)
   becomes
   `__zdMark(function NAME(args){ BODY }, 'NAME', LENGTH)` in place of the bare function expression
   (`LENGTH` = the declared parameter count Chrome reports for that native; use the obvious arity).

Helper reference (defined in `_native.js`):
- `__zdReplace(obj, prop, make)` → installs `make(orig)`, copies `orig.name`/`orig.length`, marks native.
- `__zdGetter(obj, prop, getFn, { enumerable, setFn })` → accessor whose getter/setter read `function get/set NAME() { [native code] }`.
- `__zdMark(fn, name, length)` → marks an arbitrary fn native with the given name/length; returns `fn`.

---

## Task 1: Masking prelude + outer-IIFE assembly

**Files:**
- Create: `crates/zendriver-stealth/src/patches/_native.js`
- Modify: `crates/zendriver-stealth/src/patches.rs` (add `NATIVE` const at the top with the other `include_str!`s; rewrite `bootstrap_script` to wrap in an outer IIFE; update the IIFE-shape test; add prelude tests)
- Test: `crates/zendriver-stealth/src/patches.rs` (inline `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `patches.rs`:

```rust
#[test]
fn bootstrap_installs_native_masking_prelude() {
    let s = bootstrap_script(&Persona::default(), &mock_identity());
    assert!(s.contains("__zdReplace"), "replace helper missing");
    assert!(s.contains("__zdGetter"), "getter helper missing");
    assert!(s.contains("__zdMark"), "mark helper missing");
    assert!(
        s.contains("Function.prototype, \"toString\""),
        "toString override missing"
    );
}

#[test]
fn bootstrap_wraps_everything_in_outer_masking_iife() {
    let s = bootstrap_script(&Persona::default(), &mock_identity());
    assert!(s.starts_with("(function(){"), "outer masking IIFE must be first");
    // identity IIFE is now nested inside the outer one.
    assert!(s.contains("(function(fp){"), "identity IIFE still present (nested)");
    assert!(s.trim_end().ends_with("})();"), "outer IIFE is invoked");
}
```

Also **replace** the existing `bootstrap_is_an_iife_taking_fp` test body (it currently asserts `s.starts_with("(function(fp){")`, which the wrap breaks):

```rust
#[test]
fn bootstrap_is_an_iife_taking_fp() {
    let s = bootstrap_script(&Persona::default(), &mock_identity());
    // The identity patches still run inside `(function(fp){…})({…})`, now
    // nested within the outer masking IIFE.
    assert!(s.contains("(function(fp){"));
    assert!(s.contains("})({"), "fp arg JSON should follow");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zendriver-stealth patches:: --lib`
Expected: FAIL — `bootstrap_installs_native_masking_prelude` / `bootstrap_wraps_everything_in_outer_masking_iife` fail (no prelude / wrong prefix).

- [ ] **Step 3: Create the prelude `patches/_native.js`**

```js
// Native-function masking prelude.
//
// Runs FIRST inside the bootstrap's single outer IIFE (see patches.rs
// `bootstrap_script`). Declares __zdReplace / __zdGetter / __zdMark as
// closure-locals — they are NOT placed on globalThis, so nothing leaks to the
// page (Object.keys / getOwnPropertyNames(window) stay clean). The only global
// side effect is overriding Function.prototype.toString, which MUST persist
// beyond the IIFE so page scripts that inspect patched functions see native
// code. Because the whole bootstrap is injected via
// Page.addScriptToEvaluateOnNewDocument (every frame), the override is
// re-installed per realm, so cross-realm probes
// (iframe.contentWindow.Function.prototype.toString.call(fn)) also see native.
const __zdFnToString = Function.prototype.toString;
const __zdMarks = new WeakMap(); // fn -> native display string
const __zdNativeStr = (name) => "function " + name + "() { [native code] }";

const __zdFakeToString = function toString() {
  const s = __zdMarks.get(this);
  return s !== undefined ? s : __zdFnToString.call(this);
};
__zdMarks.set(__zdFakeToString, __zdNativeStr("toString"));
__zdMarks.set(__zdFnToString, __zdNativeStr("toString"));
Object.defineProperty(Function.prototype, "toString", {
  value: __zdFakeToString,
  writable: true,
  enumerable: false,
  configurable: true,
});

// Match a native function's own name/length shape
// (writable:false, enumerable:false, configurable:true).
const __zdNameLen = (fn, name, length) => {
  Object.defineProperty(fn, "name", { value: name, configurable: true });
  Object.defineProperty(fn, "length", { value: length, configurable: true });
};

// Mark an arbitrary function native (value-function members, constructors).
const __zdMark = (fn, name, length) => {
  __zdNameLen(fn, name, length);
  __zdMarks.set(fn, __zdNativeStr(name));
  return fn;
};

// Replace obj[prop] (a method) with make(orig), copying the original's
// name/length and marking the result native.
const __zdReplace = (obj, prop, make) => {
  const orig = obj[prop];
  const fn = make(orig);
  const name = orig && orig.name ? orig.name : prop;
  const length =
    orig && typeof orig.length === "number" ? orig.length : fn.length;
  __zdMark(fn, name, length);
  obj[prop] = fn;
  return fn;
};

// Define an accessor whose getter (and optional setter) report the native
// `function get NAME() { [native code] }` form.
const __zdGetter = (obj, prop, getFn, opts) => {
  opts = opts || {};
  __zdNameLen(getFn, "get " + prop, 0);
  __zdMarks.set(getFn, "function get " + prop + "() { [native code] }");
  const desc = { get: getFn, enumerable: !!opts.enumerable, configurable: true };
  if (opts.setFn) {
    __zdNameLen(opts.setFn, "set " + prop, 1);
    __zdMarks.set(opts.setFn, "function set " + prop + "() { [native code] }");
    desc.set = opts.setFn;
  }
  Object.defineProperty(obj, prop, desc);
};
```

- [ ] **Step 4: Wire the prelude + outer IIFE in `patches.rs`**

Add the const alongside the other identity `include_str!`s (after line 25):

```rust
// --- Native-function masking prelude (runs first, wraps everything) ------
const NATIVE: &str = include_str!("patches/_native.js");
```

Rewrite `bootstrap_script` so the prelude is first and the whole script is one outer IIFE. Build into a `body` string, then wrap:

```rust
#[must_use]
pub fn bootstrap_script(persona: &Persona, identity: &Fingerprint) -> String {
    // Prelude first: installs the toString override + closure-local helpers
    // that every patch below routes through.
    let mut body = String::from(NATIVE);
    body.push('\n');
    body.push_str(&identity_iife(persona, identity));

    let seed = persona.seed.unwrap_or_else(Seed::random).value();

    body.push('\n');
    body.push_str(PRNG);

    push_noise(&mut body, Surface::Canvas, persona.canvas.as_ref(), CANVAS, seed);
    push_noise(&mut body, Surface::Audio, persona.audio.as_ref(), AUDIO, seed);
    push_noise(&mut body, Surface::ClientRects, persona.client_rects.as_ref(), CLIENT_RECTS, seed);

    push_webgl(&mut body, persona.webgl.as_ref());
    push_webgpu(&mut body, persona.webgpu.as_ref(), persona.webgl.as_ref());
    push_fonts(&mut body, persona.fonts.as_ref(), seed);
    push_hardware(&mut body, persona.hardware.as_ref());
    push_webrtc(&mut body, persona.webrtc.as_ref());

    // Single outer IIFE: helpers stay closure-local (no globalThis leak); the
    // Function.prototype.toString override inside still persists globally.
    format!("(function(){{\n{body}\n}})();")
}
```

(The `push_*` helpers are unchanged — they already append to the `&mut String` they're given.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p zendriver-stealth patches:: --lib`
Expected: PASS — all prelude/IIFE tests green, and the existing identity/surface/token tests still pass (wrapping is behavior-preserving; tokens untouched).

- [ ] **Step 6: Commit**

```bash
git add crates/zendriver-stealth/src/patches/_native.js crates/zendriver-stealth/src/patches.rs
git commit -m "feat(stealth): native-function toString masking prelude + outer IIFE"
```

---

## Task 2: Route identity patches through the helpers

Apply the mechanical transform (top of plan) to each identity patch. **chrome.js has no sites — skip it.**

**Files (modify):** `webdriver.js`, `navigator_props.js`, `plugins.js`, `permissions.js`, `codecs.js`, `user_agent_data.js`, `broken_image.js`
**Test:** `crates/zendriver-stealth/src/patches.rs` (inline tests)

Per-file sites:

| File:line | Kind | Target | Helper |
|-----------|------|--------|--------|
| `webdriver.js:4` | GETTER | `Navigator.prototype.webdriver` (enumerable: true) | `__zdGetter` |
| `navigator_props.js:3` | GETTER | `Navigator.prototype.platform` | `__zdGetter` |
| `navigator_props.js:7` | GETTER | `Navigator.prototype.hardwareConcurrency` | `__zdGetter` |
| `navigator_props.js:11` | GETTER | `Navigator.prototype.deviceMemory` | `__zdGetter` |
| `navigator_props.js:15` | GETTER | `Navigator.prototype.languages` | `__zdGetter` |
| `plugins.js:3` | GETTER | `Navigator.prototype.plugins` | `__zdGetter` |
| `plugins.js:4,20` | VALUE_FN | Plugin members + `PluginArray` `item`/`namedItem`/`refresh` | `__zdMark` |
| `permissions.js:6` | METHOD | `navigator.permissions.query` | `__zdReplace` |
| `codecs.js:4` | METHOD | `HTMLMediaElement.prototype.canPlayType` | `__zdReplace` |
| `user_agent_data.js:3` | GETTER | `Navigator.prototype.userAgentData` | `__zdGetter` |
| `user_agent_data.js:8,21` | VALUE_FN | `getHighEntropyValues`, `toJSON` | `__zdMark` |
| `broken_image.js:7,15` | GETTER | `HTMLImageElement.prototype.naturalWidth`/`naturalHeight` | `__zdGetter` |

- [ ] **Step 1: Write the failing routing assertions**

Add to the `patches.rs` tests module:

```rust
#[test]
fn identity_patches_route_through_masking_helpers() {
    let s = bootstrap_script(&Persona::default(), &mock_identity());
    assert!(s.contains("__zdGetter(Navigator.prototype, 'webdriver'"), "webdriver");
    assert!(s.contains("__zdGetter(Navigator.prototype, 'plugins'"), "plugins getter");
    assert!(s.contains("__zdReplace"), "permissions/codecs methods routed");
    assert!(s.contains("__zdMark"), "value-fn members marked");
    // No raw defineProperty getter on Navigator.prototype.webdriver remains.
    assert!(
        !s.contains("Object.defineProperty(Navigator.prototype, 'webdriver'"),
        "webdriver should go through __zdGetter, not raw defineProperty"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zendriver-stealth patches::tests::identity_patches_route_through_masking_helpers --lib`
Expected: FAIL (patches still use raw `defineProperty`/assignment).

- [ ] **Step 3: Edit each identity patch file**

Worked example — **`webdriver.js`** in full:

```js
// Defeats: bot.sannysoft.com `WebDriver (New)` + `WebDriver Advanced` rows.
// Patches Navigator.prototype (not navigator directly) so
// Object.getOwnPropertyNames(navigator) doesn't reveal the hack. Routed through
// __zdGetter so the getter reports `function get webdriver() { [native code] }`.
__zdGetter(Navigator.prototype, 'webdriver', () => false, { enumerable: true });
```

Worked example — a **VALUE_FN** member (pattern for `user_agent_data.js` `getHighEntropyValues`, `plugins.js` array methods): wrap the function expression in `__zdMark(fn, 'name', length)`, e.g.

```js
// before:  getHighEntropyValues: function (hints) { /* BODY */ },
// after:
getHighEntropyValues: __zdMark(function getHighEntropyValues(hints) { /* BODY */ }, 'getHighEntropyValues', 1),
```

For the remaining files, read each and apply the transform from the table — **the function/getter bodies stay identical**; only the wrapping call changes (`Object.defineProperty(O,'p',{get,enumerable:E})` → `__zdGetter(O,'p',get,{enumerable:E})`; `O.p = function`→`__zdReplace`; bare value functions → `__zdMark`). Preserve each getter's original `enumerable` flag.

- [ ] **Step 4: Run the routing + existing identity tests**

Run: `cargo test -p zendriver-stealth patches:: --lib`
Expected: PASS — routing assertion green; existing identity tests (`bootstrap_includes_all_nine_patches`, `persona_overrides_simple_identity_fields`, `navigator_languages_derives_base_lang_not_en`, etc.) still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/src/patches/
git commit -m "feat(stealth): route identity patches through native-masking helpers"
```

---

## Task 3: Route surface patches through the helpers

Same transform for the persona surfaces. **Preserve all `SEED` / `WEBGL_*` / `FONT_ALLOW` / `HW_*` / `WEBGPU_*` token placeholders verbatim** — `patches.rs` substitutes them.

**Files (modify):** `webgl.js`, `canvas.js`, `audio.js`, `client_rects.js`, `fonts.js`, `hardware.js`, `webrtc.js`, `webgpu.js`

| File:line | Kind | Target | Helper | Tokens |
|-----------|------|--------|--------|--------|
| `webgl.js:8,20` | METHOD | `WebGL{,2}RenderingContext.prototype.getParameter` (both blocks) | `__zdReplace` | WEBGL_VENDOR, WEBGL_RENDERER |
| `canvas.js:13,19` | METHOD | `CanvasRenderingContext2D.prototype.getImageData`, `HTMLCanvasElement.prototype.toDataURL` | `__zdReplace` | SEED |
| `audio.js:5,11` | METHOD | `AnalyserNode.prototype.getFloatFrequencyData`/`getByteTimeDomainData` | `__zdReplace` | SEED |
| `client_rects.js:5,10` | METHOD | `Element.prototype.getBoundingClientRect`/`getClientRects` | `__zdReplace` | SEED |
| `fonts.js:4,14` | METHOD | `CanvasRenderingContext2D.prototype.measureText`, `document.fonts.check` | `__zdReplace` | FONT_ALLOW, SEED |
| `hardware.js:3,13,22` | METHOD | `navigator.getBattery`, `navigator.mediaDevices.enumerateDevices`, `speechSynthesis.getVoices` | `__zdReplace` | HW_BATTERY, HW_MEDIA_DEVICES, HW_VOICES |
| `webrtc.js:6,9` | CTOR + METHOD | `window.RTCPeerConnection` (ctor), `pc.addEventListener` (instance) | `__zdMark` | WEBRTC_POLICY, WEBRTC_FAKE_IP |
| `webgpu.js:19,28` | GETTER | `navigator.gpu` (block mode), `GPUAdapter.prototype.info` | `__zdGetter` | WEBGPU_VENDOR, WEBGPU_ARCHITECTURE, WEBGPU_MODE |

- [ ] **Step 1: Write the failing routing assertion**

```rust
#[test]
fn surface_patches_route_through_masking_helpers() {
    let p = Persona {
        webgl: Some(WebglSpec {
            strategy: Some(Strategy::Value),
            unmasked_vendor: Some("Google Inc. (NVIDIA)".into()),
            unmasked_renderer: Some("ANGLE (NVIDIA GeForce RTX 4090)".into()),
        }),
        canvas: Some(SurfaceCfg { strategy: Some(Strategy::Seeded) }),
        webgpu: Some(SurfaceCfg { strategy: Some(Strategy::Value) }),
        seed: Some(Seed::from_u64(1)),
        ..Persona::default()
    };
    let s = bootstrap_script(&p, &mock_identity());
    assert!(s.contains("__zdReplace(WebGLRenderingContext.prototype, 'getParameter'") ||
            s.contains("__zdReplace(proto, 'getParameter'"), "webgl routed");
    assert!(s.contains("__zdReplace"), "canvas/getImageData routed");
    assert!(s.contains("__zdGetter(GPUAdapter.prototype, 'info'"), "webgpu info getter routed");
    // tokens still substituted, not left raw:
    assert!(!s.contains("SEED") && !s.contains("WEBGL_VENDOR"), "tokens substituted");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zendriver-stealth patches::tests::surface_patches_route_through_masking_helpers --lib`
Expected: FAIL.

- [ ] **Step 3: Edit each surface patch file**

Worked example — **`webgl.js`** in full (both blocks, names/tokens preserved):

```js
// Defeats: bot.sannysoft.com `WebGL Vendor` + `WebGL Renderer` rows.
const VENDOR = 'Google Inc. (Intel)';
const RENDERER = 'ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)';
[WebGLRenderingContext.prototype, WebGL2RenderingContext.prototype].forEach(proto => {
    __zdReplace(proto, 'getParameter', (orig) => function(param) {
        if (param === 37445) return VENDOR;    // UNMASKED_VENDOR_WEBGL
        if (param === 37446) return RENDERER;  // UNMASKED_RENDERER_WEBGL
        return orig.call(this, param);
    });
});

// Persona-driven value substitution (WEBGL_VENDOR / WEBGL_RENDERER tokens).
(function (vendor, renderer) {
  const VENDOR = 0x9245, RENDERER = 0x9246; // UNMASKED_VENDOR_WEBGL / RENDERER
  function patch(proto) {
    __zdReplace(proto, 'getParameter', (orig) => function (p) {
      if (vendor && p === VENDOR) return vendor;
      if (renderer && p === RENDERER) return renderer;
      return orig.call(this, p);
    });
  }
  if (window.WebGLRenderingContext) patch(WebGLRenderingContext.prototype);
  if (window.WebGL2RenderingContext) patch(WebGL2RenderingContext.prototype);
})(WEBGL_VENDOR, WEBGL_RENDERER);
```

Worked example — **CTOR** (`webrtc.js`): wrap the wrapper constructor with `__zdMark`, e.g.

```js
// before:  window.RTCPeerConnection = function RTCPeerConnection(cfg) { /* BODY */ };
// after:
window.RTCPeerConnection = __zdMark(function RTCPeerConnection(cfg) { /* BODY */ }, 'RTCPeerConnection', 0);
```

For the remaining files, read each and apply the transform from the table — **bodies and tokens unchanged**, only the wrapping call changes. For files that currently capture `const orig = proto.x;` then reassign, fold that capture into the `(orig) => …` factory parameter.

- [ ] **Step 4: Run the full stealth unit suite**

Run: `cargo test -p zendriver-stealth --lib`
Expected: PASS — routing assertion green; all existing surface/token tests (`webgl_value_substitutes_persona_renderer`, `no_unsubstituted_tokens_remain`, `webgpu_value_substitutes_coherent_adapter_from_renderer`, etc.) still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/src/patches/
git commit -m "feat(stealth): route surface patches through native-masking helpers"
```

---

## Task 4: Real-Chrome integration test

**Files:**
- Create: `crates/zendriver/tests/stealth_native_masking.rs`

This is `#[ignore]` + `#[cfg(feature = "integration-tests")]` (same gate as `fingerprint_integration.rs`); it needs a local Chrome and is not in the normal matrix.

- [ ] **Step 1: Write the test**

```rust
//! Headful integration: spoofed-profile patches must report as native code.
//!
//! Run with:
//! ```sh
//! cargo test -p zendriver --test stealth_native_masking \
//!     --features integration-tests -- --ignored
//! ```
#![cfg(feature = "integration-tests")]

use serial_test::serial;
use zendriver::Browser;
use zendriver::{Persona, Seed};

// Probes the most-checked patched method + a getter + the toString override
// itself, returning a JSON blob the Rust side asserts on.
const PROBE_JS: &str = r#"(() => {
    const gp = WebGLRenderingContext.prototype.getParameter;
    const wd = Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver').get;
    return JSON.stringify({
        gpStr: gp.toString(),
        gpName: gp.name,
        gpLen: gp.length,
        wdStr: wd.toString(),
        ftsStr: Function.prototype.toString.toString(),
    });
})()"#;

// Cross-realm: a child frame's Function.prototype.toString must also mask the
// parent's patched method (validates per-frame bootstrap injection).
const CROSS_FRAME_JS: &str = r#"(() => {
    const f = document.createElement('iframe');
    document.body.appendChild(f);
    const cwToString = f.contentWindow.Function.prototype.toString;
    const out = cwToString.call(WebGLRenderingContext.prototype.getParameter);
    f.remove();
    return out;
})()"#;

#[tokio::test]
#[serial]
#[ignore] // run with: cargo test ... -- --ignored
async fn patched_functions_report_native_code() {
    let browser = Browser::builder()
        .persona(Persona::builder().seed(Seed::from_u64(42)).build())
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("about:blank").await.unwrap();
    tab.wait_for_load().await.unwrap();

    let raw: String = tab.evaluate::<String>(PROBE_JS).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();

    assert!(
        v["gpStr"].as_str().unwrap().contains("[native code]"),
        "getParameter.toString() must read native: {raw}"
    );
    assert_eq!(v["gpName"], "getParameter", "name must match native");
    assert_eq!(v["gpLen"], 1, "length must match native");
    assert_eq!(
        v["wdStr"], "function get webdriver() { [native code] }",
        "webdriver getter must read native getter form"
    );
    assert!(
        v["ftsStr"].as_str().unwrap().contains("[native code]"),
        "Function.prototype.toString must mask itself"
    );

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
#[ignore]
async fn masking_holds_cross_realm() {
    let browser = Browser::builder()
        .persona(Persona::builder().seed(Seed::from_u64(7)).build())
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("about:blank").await.unwrap();
    tab.wait_for_load().await.unwrap();

    let cross: String = tab.evaluate::<String>(CROSS_FRAME_JS).await.unwrap();
    assert!(
        cross.contains("[native code]"),
        "cross-realm toString must mask the parent's patched fn: {cross}"
    );

    browser.close().await.unwrap();
}
```

- [ ] **Step 2: Run the test (needs local Chrome)**

Run: `cargo test -p zendriver --test stealth_native_masking --features integration-tests -- --ignored`
Expected: PASS — both tests green. (If `masking_holds_cross_realm` fails because this Chrome doesn't inject into a synchronously-created `about:blank` child frame, that is the documented residual limit in spec §5.4 — note it and keep the primary test as the gate.)

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/tests/stealth_native_masking.rs
git commit -m "test(stealth): real-Chrome native-masking + cross-realm assertions"
```

---

## Task 5: Pre-push gates

- [ ] **Step 1: Format + lint + unit tests (parallel-safe; run all three)**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p zendriver-stealth --lib
```
Expected: fmt clean, clippy no warnings, stealth unit tests PASS. Fix anything flagged, re-stage.

- [ ] **Step 2: Feature-gated clippy (CLAUDE.md — stealth is feature-gated)**

```bash
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings
```
Expected: no warnings.

- [ ] **Step 3: Confirm no public-API/MCP change**

This feature adds no public items, so the MCP coverage ledger and schema snapshots are untouched. Sanity check that `git diff` shows only `zendriver-stealth/src/patches/*` and the new integration test.

- [ ] **Step 4: Final commit (only if fmt/clippy changed anything)**

```bash
git add -A
git commit -m "chore(stealth): fmt + clippy for native-masking"
```

---

## Self-Review

- **Spec coverage (§3):** prelude + helpers (Task 1) ✓; cross-realm via all-frames injection (Task 1 outer-IIFE + Task 4 cross-realm test) ✓; route all 16 files (Tasks 2–3, `chrome.js`/`_prng.js` correctly excluded — no sites) ✓; spoofed-only / no public API (Task 5 step 3) ✓; tests incl. `.name`/`.length`/getter-form/self-mask (Task 4) ✓; residual limit acknowledged (Task 4 step 2) ✓.
- **Placeholders:** none — prelude shown in full; transform rule + per-file site tables are exact; integration test complete. Tasks 2–3 intentionally show the rule + worked examples rather than re-pasting 14 unread file bodies; the transform is uniform and each site is enumerated with file:line.
- **Type/name consistency:** `__zdReplace` / `__zdGetter` / `__zdMark` / `__zdMarks` / `__zdNameLen` / `__zdFakeToString` / `__zdFnToString` used identically across the prelude, the routing assertions, and the transform examples; `NATIVE` const matches the `include_str!` path; `bootstrap_script` signature unchanged.
