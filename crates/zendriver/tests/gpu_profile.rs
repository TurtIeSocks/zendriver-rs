//! Real-Chrome verification that the spoofed WebGL surface is coherent.
//!
//! Every other check on the GPU tier tables is a unit test, a generated table,
//! or a model of Blink. This file is the only place the tables meet an actual
//! browser, so it reads the pairs a fingerprinter cross-checks and asserts the
//! relations real hardware always satisfies. It also covers the other half of
//! the split the tables encode: everything the table does *not* serve has to
//! keep coming from the real context, which only a browser can show.
//!
//! Run with:
//! ```sh
//! cargo test -p zendriver --test gpu_profile \
//!     --features integration-tests -- --ignored
//! ```
#![cfg(feature = "integration-tests")]
// Test-only: [`spoofed_surface`] panics when the spoof never installs, so a
// broken patch fails loudly instead of being read as a coherent native
// surface. Matches the convention in `fingerprint_integration.rs`.
#![allow(clippy::panic)]

use std::time::Duration;

use serde_json::Value;
use zendriver::stealth::StealthProfile;
use zendriver::{Browser, BrowserError, GpuBackend, Tab, ZendriverError};

/// Reads the pairs a fingerprinter cross-checks, plus the two reads that prove
/// per-context state still reaches the real backend.
///
/// The device capabilities and the delegated values are read in the **same**
/// invocation on purpose: a run where `maxTexture` matches the tier table is a
/// run where the spoof is demonstrably live, so the delegated reads below
/// cannot be passing merely because the patch never installed.
///
/// `stencilGl` is a second context because context attributes are fixed at
/// creation: `getContext` on a canvas that already has one ignores the
/// attributes and hands back the existing context.
const CHECK_JS: &str = r#"
(() => {
  const gl = document.createElement('canvas').getContext('webgl2');
  if (!gl) return JSON.stringify({error: 'no webgl2'});
  const dbg = gl.getExtension('WEBGL_debug_renderer_info');
  const listed = gl.getSupportedExtensions();
  const claimedButMissing = listed.filter(n => gl.getExtension(n) === null);
  const aliased = gl.getParameter(gl.ALIASED_POINT_SIZE_RANGE);
  // Mutable state: a table that answered BLEND would answer false forever.
  const blendBefore = gl.getParameter(gl.BLEND);
  gl.enable(gl.BLEND);
  const blendAfter = gl.getParameter(gl.BLEND);
  // Context-attribute-dependent state: STENCIL_BITS is a property of the
  // context that was actually created, not of the device.
  const stencilGl = document.createElement('canvas').getContext('webgl2', {stencil: true});
  // MAX_DRAW_BUFFERS is table-served but the DRAW_BUFFERn enums are not, and
  // whether one answers at all is a property of the real backend. Read every
  // index the served cap claims, plus the first index past it. The correct
  // answer depends on what is bound, so the sweep runs twice.
  const maxDraw = gl.getParameter(gl.MAX_DRAW_BUFFERS);
  const sweep = () => {
    const out = [];
    for (let i = 0; i < maxDraw; i++) out.push(gl.getParameter(gl['DRAW_BUFFER' + i]));
    return out;
  };
  const drawBuffers = sweep();
  const drawBufferPastCap = gl.getParameter(gl['DRAW_BUFFER' + maxDraw]);
  // Again with a framebuffer object bound, where BACK is an illegal value and
  // ES 3.0 specifies COLOR_ATTACHMENT0 at index 0 and NONE at every other.
  gl.bindFramebuffer(gl.FRAMEBUFFER, gl.createFramebuffer());
  const drawBuffersFbo = sweep();
  const drawBufferPastCapFbo = gl.getParameter(gl['DRAW_BUFFER' + maxDraw]);
  gl.bindFramebuffer(gl.FRAMEBUFFER, null);
  const drawBuffersRebound = sweep();
  const GL_BACK = gl.BACK, GL_NONE = gl.NONE, GL_COLOR_ATTACHMENT0 = gl.COLOR_ATTACHMENT0;
  return JSON.stringify({
    renderer: dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
    maxTexture: gl.getParameter(gl.MAX_TEXTURE_SIZE),
    maxViewport: Array.from(gl.getParameter(gl.MAX_VIEWPORT_DIMS)),
    combined: gl.getParameter(gl.MAX_COMBINED_TEXTURE_IMAGE_UNITS),
    frag: gl.getParameter(gl.MAX_TEXTURE_IMAGE_UNITS),
    vert: gl.getParameter(gl.MAX_VERTEX_TEXTURE_IMAGE_UNITS),
    claimedButMissing,
    aliasedIsFloat32: aliased instanceof Float32Array,
    blendBefore,
    blendAfter,
    stencilRequested: stencilGl ? stencilGl.getContextAttributes().stencil : null,
    stencilBits: stencilGl ? stencilGl.getParameter(stencilGl.STENCIL_BITS) : null,
    stencilBitsDefault: gl.getParameter(gl.STENCIL_BITS),
    maxDraw,
    drawBuffers,
    drawBufferPastCap,
    drawBuffersFbo,
    drawBufferPastCapFbo,
    drawBuffersRebound,
    GL_BACK,
    GL_NONE,
    GL_COLOR_ATTACHMENT0,
  });
})()
"#;

/// Bounded polling for the bootstrap install-race — the same one
/// `fingerprint_integration.rs` documents for canvas. The stealth bootstrap is
/// injected via `Page.addScriptToEvaluateOnNewDocument`, and
/// `Tab::wait_for_load()` has no happens-before edge to "the main-world
/// bootstrap finished patching", so an early read can still observe native
/// WebGL. Measured here, not assumed: on a spoofed browser the first
/// `evaluate_main` read after `wait_for_load()` returned the unpatched
/// SwiftShader surface and every later read returned the spoof.
const MAX_POLL_TRIES: u32 = 20;
const POLL_DELAY: Duration = Duration::from_millis(50);

/// The capture the persona's GPU renderer resolves to.
///
/// Every assertion below this point also holds against an unpatched
/// SwiftShader surface (16384/16384 vs 8192/8192 both satisfy `viewport >=
/// texture`), so a green run only proves internal coherence, not that the
/// shipped tier tables reached the browser. This file compares the observed
/// values against the actual resolved tier instead.
///
/// There is no public route to that resolved [`GpuProfile`](zendriver_stealth::GpuProfile)
/// from this crate: `zendriver_stealth::gpu` is a `pub mod`, but the
/// functions that would resolve one — `profile_for_tier`,
/// `device_for_renderer` — and the `Tier` type itself are all `pub(crate)`
/// inside `zendriver-stealth`, and `Persona::gpu` is only the caller-pinned
/// *override*, not the resolved profile `push_webgl` builds internally. So
/// this reads the same measured capture the tier's generated table was built
/// from — table-derived, not a hand-copied literal — rather than the table
/// itself.
///
/// This is the Metal tier because every test below pins a `MacIntel` persona
/// (see [`mac_persona`]). The default renderer is platform-derived — a
/// `MacIntel` persona resolves the Apple Metal row, a `Win32` one the D3D11
/// row, a `LinuxX86_64` one the SwiftShader row — so pinning the platform is
/// what makes the expected tier the same on every host this test runs on.
///
/// Pinning also keeps the test meaningful rather than merely deterministic.
/// `spoofed_surface` below proves the patch installed by observing that the
/// main world reports a *different* renderer than the unpatched isolated
/// world, and `StealthProfile::spoofed()` launches Chrome on SwiftShader. A
/// `LinuxX86_64` persona resolves a SwiftShader row, so it would spoof
/// SwiftShader's values over a SwiftShader backend, leaving nothing to tell
/// apart. `Win32` resolves the D3D11 row and would discriminate as well as
/// `MacIntel` does — both its renderer string and its numbers differ from the
/// backend's — so the pin decides *which* capture this file includes, not
/// whether the test can work at all.
const RESOLVED_TIER_CAPTURE: &str =
    include_str!("../../zendriver-stealth/data/gpu-tiers/metal-macos.json");

/// The persona every test here launches with: a Mac, so the platform-derived
/// default renderer resolves to the Apple Metal row on any host.
fn mac_persona() -> zendriver::stealth::Persona {
    zendriver::stealth::Persona {
        platform: Some(zendriver::stealth::Platform::MacIntel),
        ..Default::default()
    }
}

/// `(MAX_TEXTURE_SIZE, MAX_VIEWPORT_DIMS)` from the capture the default
/// persona's GPU tier resolves to — see [`RESOLVED_TIER_CAPTURE`].
fn resolved_tier_texture_and_viewport() -> (i64, [i64; 2]) {
    let capture: Value = serde_json::from_str(RESOLVED_TIER_CAPTURE).expect("capture json");
    let params = &capture["capture"]["webgl2"]["params"];
    let tex = params["MAX_TEXTURE_SIZE"]
        .as_i64()
        .expect("capture MAX_TEXTURE_SIZE");
    let dims = params["MAX_VIEWPORT_DIMS"]
        .as_array()
        .expect("capture MAX_VIEWPORT_DIMS array");
    let vp = [
        dims[0].as_i64().expect("capture maxViewport[0]"),
        dims[1].as_i64().expect("capture maxViewport[1]"),
    ];
    (tex, vp)
}

/// Writes `name` into the temp dir and returns its `file://` URL.
///
/// The probe must not run on `about:blank`: that is an opaque origin, and this
/// branch's GPU work is verified from a secure-context page everywhere else
/// (`gpu_backend.rs`, `examples/probe_gpu.rs`), because `navigator.gpu` is
/// `[SecureContext]`-gated. WebGL itself is not gated, but probing from the
/// same kind of page keeps this test comparable with the captures the tier
/// tables were derived from.
fn file_url(name: &str) -> String {
    let page = std::env::temp_dir().join(name);
    std::fs::write(&page, "<!doctype html><title>probe</title>").expect("write probe page");
    let p = page.display().to_string().replace('\\', "/");
    if p.starts_with('/') {
        format!("file://{p}") // Unix: /path -> file:///path
    } else {
        format!("file:///{p}") // Windows: C:/path -> file:///C:/path
    }
}

/// Returns the `CHECK_JS` read from the main world, once the spoof is provably
/// live there.
///
/// Two things this guards, both of which would otherwise let these tests pass
/// while verifying nothing — a native WebGL surface satisfies every assertion
/// below, so an unpatched read is a silent false pass:
///
/// 1. **World.** The bootstrap mutates `WebGLRenderingContext.prototype` in the
///    main world, and every isolated world holds its own copy of that
///    prototype. So [`Tab::evaluate`] (isolated) reads the *native* surface —
///    measured, not inferred: it reported SwiftShader's 8192/8192 while the
///    main world reported the spoofed 16384/16384 in the same tab.
/// 2. **Timing.** See [`MAX_POLL_TRIES`] — an early main-world read can still
///    land before the bootstrap patched anything.
///
/// The isolated-world read turns (1) into the baseline that settles (2): it is
/// a free, same-tab sample of the unpatched surface, so a main-world read that
/// reports a different renderer is necessarily the spoof rather than a
/// not-yet-patched native read.
async fn spoofed_surface(tab: &Tab) -> Value {
    let raw: String = tab.evaluate(CHECK_JS).await.expect("isolated-world probe");
    let native: Value = serde_json::from_str(&raw).expect("probe json");
    if let Some(err) = native["error"].as_str() {
        panic!("isolated-world WebGL probe failed: {err}");
    }
    for attempt in 0..MAX_POLL_TRIES {
        let raw: String = tab.evaluate_main(CHECK_JS).await.expect("main-world probe");
        let got: Value = serde_json::from_str(&raw).expect("probe json");
        // `CHECK_JS` creates two fresh WebGL2 contexts per invocation and
        // never releases them, so a long poll accumulates contexts against
        // Chrome's live-context cap. Once that cap is hit this returns
        // `{error: 'no webgl2'}`, which — because it carries neither
        // `renderer` field — would otherwise read as "different from
        // `native`" and get returned below as though it were the spoof,
        // panicking downstream on a missing key instead of naming the real
        // cause.
        if let Some(err) = got["error"].as_str() {
            panic!("main-world WebGL probe failed: {err}");
        }
        if got["renderer"] != native["renderer"] {
            return got;
        }
        if attempt + 1 < MAX_POLL_TRIES {
            tokio::time::sleep(POLL_DELAY).await;
        }
    }
    panic!(
        "the WebGL spoof never installed after {MAX_POLL_TRIES} main-world reads — every read \
         still matches the unpatched isolated-world surface {native:#}. Either the bootstrap's \
         WebGL patch is broken, or this browser has no WebGL2 at all (both reads would then \
         carry an `error` field)."
    );
}

/// The pairs a fingerprinter cross-checks must agree with each other.
///
/// The shipped bug this work fixes reported a viewport beside a texture max
/// from a different backend. Note the relation is `viewport >= texture`, not
/// equality: a viewport larger than the texture max is legitimate and real
/// (D3D11 reports 32767 viewport with a 16384 texture max).
#[tokio::test]
#[ignore = "launches real Chrome"]
async fn spoofed_profile_is_internally_coherent() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .persona(mac_persona())
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto(&file_url("zendriver-gpu-profile.html"))
        .await
        .expect("goto");
    tab.wait_for_load().await.expect("load");
    let got = spoofed_surface(&tab).await;
    browser.close().await.ok();

    let tex = got["maxTexture"].as_i64().expect("maxTexture");
    let vp_dims = got["maxViewport"].as_array().expect("maxViewport array");
    assert_eq!(
        vp_dims.len(),
        2,
        "MAX_VIEWPORT_DIMS must have 2 elements: {got:#}"
    );
    let vp = [
        vp_dims[0].as_i64().expect("maxViewport[0]"),
        vp_dims[1].as_i64().expect("maxViewport[1]"),
    ];
    // Both elements, not just [0]: the shipped bug this work fixes paired
    // values across backends, and a divergent second element would slip
    // through a check of [0] alone.
    for (i, &v) in vp.iter().enumerate() {
        assert!(
            v >= tex,
            "viewport[{i}] {v} below texture max {tex} — the exact pair this work fixes: {got:#}"
        );
    }

    // Every assertion above also holds against an unpatched SwiftShader
    // surface (8192/8192 satisfies `viewport >= texture` just as well as
    // 16384/16384 does), so it proves self-coherence, not that the shipped
    // tier tables reached the browser. Compare against the tier the persona
    // actually resolved to — see `RESOLVED_TIER_CAPTURE` — so serving the
    // wrong tier, or falling back to the native surface, fails loudly here
    // instead of passing silently.
    // The renderer the platform-derived default picked. A MacIntel persona
    // must land on the Apple Metal row; before that default was derived from
    // the platform at all, this string was Apple's under every persona,
    // including Win32 ones Chrome could never pair it with.
    let renderer = got["renderer"].as_str().expect("renderer");
    assert!(
        renderer.contains("Apple") && renderer.contains("Metal"),
        "a MacIntel persona must resolve the Apple Metal row, got {renderer}"
    );

    let (expect_tex, expect_vp) = resolved_tier_texture_and_viewport();
    assert_eq!(
        tex, expect_tex,
        "MAX_TEXTURE_SIZE {tex} does not match the resolved tier's capture {expect_tex} — \
         wrong tier or an unpatched native surface: {got:#}"
    );
    assert_eq!(
        vp, expect_vp,
        "MAX_VIEWPORT_DIMS {vp:?} does not match the resolved tier's capture {expect_vp:?} — \
         wrong tier or an unpatched native surface: {got:#}"
    );

    let (c, f, v) = (
        got["combined"].as_i64().expect("combined"),
        got["frag"].as_i64().expect("frag"),
        got["vert"].as_i64().expect("vert"),
    );
    assert!(c >= f + v, "combined {c} < frag {f} + vert {v}: {got:#}");
    // The captures render this range as `[1, 1023]` because `JSON.stringify`
    // collapses `1.0`; the generator applies the spec-declared float type
    // instead. If that plumbing broke anywhere along the chain the page gets
    // an Int32Array, which is a one-`instanceof` tell.
    assert_eq!(
        got["aliasedIsFloat32"], true,
        "ALIASED_POINT_SIZE_RANGE must be a Float32Array, not Int32Array: {got:#}"
    );

    // MAX_DRAW_BUFFERS is served from the table; whether a DRAW_BUFFERn enum
    // answers at all comes from the real backend. This test launches on
    // SwiftShader (6 draw buffers) with a persona serving 8, which is the
    // default pairing rather than a contrived one — so without the patch's
    // gap fill, DRAW_BUFFER6/7 read null beside a cap claiming they exist. No
    // driver reports 8 and then refuses DRAW_BUFFER6: ES 3.0 derives the valid
    // range from MAX_DRAW_BUFFERS itself.
    let max_draw = got["maxDraw"].as_i64().expect("maxDraw");
    let drawn = got["drawBuffers"].as_array().expect("drawBuffers array");
    assert_eq!(
        drawn.len() as i64,
        max_draw,
        "the probe must read every index below the served cap: {got:#}"
    );
    let null_indices: Vec<usize> = drawn
        .iter()
        .enumerate()
        .filter(|(_, v)| v.is_null())
        .map(|(i, _)| i)
        .collect();
    assert!(
        null_indices.is_empty(),
        "MAX_DRAW_BUFFERS is {max_draw} but DRAW_BUFFER{null_indices:?} answered null; a page \
         reads that pair in two lines: {got:#}"
    );
    // The other direction: an index at or above the served cap is out of range
    // for the claimed device too, so it must keep delegating rather than
    // gaining a fabricated value.
    assert!(
        got["drawBufferPastCap"].is_null(),
        "DRAW_BUFFER{max_draw} is at the served cap and must stay delegated (null): {got:#}"
    );
    assert!(
        got["drawBufferPastCapFbo"].is_null(),
        "DRAW_BUFFER{max_draw} must stay delegated with an FBO bound too: {got:#}"
    );

    // Not answering null is only half of it. The value has to be the one a real
    // device of the claimed size reports, and that differs by what is bound:
    // BACK at every in-range index on the default framebuffer (which is what
    // all three committed captures record), and NONE on a bound framebuffer
    // object, where BACK is an outright illegal value.
    let enum_of = |k: &str| got[k].as_i64().unwrap_or_else(|| panic!("{k}: {got:#}"));
    let (back, none, attach0) = (
        enum_of("GL_BACK"),
        enum_of("GL_NONE"),
        enum_of("GL_COLOR_ATTACHMENT0"),
    );
    let sweep = |k: &str| -> Vec<Option<i64>> {
        got[k]
            .as_array()
            .unwrap_or_else(|| panic!("{k} array: {got:#}"))
            .iter()
            .map(|v| v.as_i64())
            .collect()
    };
    assert_eq!(
        sweep("drawBuffers"),
        vec![Some(back); max_draw as usize],
        "with the default framebuffer bound a real device answers BACK at every index below \
         MAX_DRAW_BUFFERS, so a filled index reading anything else is still distinguishable \
         from the device being claimed: {got:#}"
    );
    // A bound FBO starts at COLOR_ATTACHMENT0 for index 0 and NONE for the
    // rest. The indices the backend has deliver that by delegation; the filled
    // ones have to agree rather than contradict them.
    let mut want_fbo = vec![Some(none); max_draw as usize];
    want_fbo[0] = Some(attach0);
    assert_eq!(
        sweep("drawBuffersFbo"),
        want_fbo,
        "with a framebuffer object bound only NONE and COLOR_ATTACHMENTi are legal, so no \
         index may answer BACK: {got:#}"
    );
    // And the choice tracks the binding rather than latching on first read.
    assert_eq!(
        sweep("drawBuffersRebound"),
        vec![Some(back); max_draw as usize],
        "unbinding the FBO must put every in-range index back to BACK: {got:#}"
    );
}

/// Everything the table does **not** serve must still come from the real
/// context.
///
/// This is the branch's headline fix in a real browser. Freezing per-context
/// mutable state into the served table made `gl.enable(gl.BLEND);
/// gl.getParameter(gl.BLEND)` answer `false` forever — a contradiction a page
/// reaches in two adjacent expressions, and one that breaks every
/// state-caching renderer that saves and restores through `getParameter`. Two
/// classes are checked, because they fail for different reasons:
///
/// 1. **Mutable state** (`BLEND`) — set by the page, so a table answer is
///    stale the moment the page writes it.
/// 2. **Context-attribute-dependent state** (`STENCIL_BITS`) — fixed by the
///    attributes the page passed to `getContext`, so a table answer describes
///    a context that was never created.
///
/// The served capabilities are read in the same invocation (see [`CHECK_JS`])
/// and asserted here against the resolved tier, so this cannot pass by the
/// spoof simply being absent.
#[tokio::test]
#[ignore = "launches real Chrome"]
async fn delegated_state_comes_from_the_real_context_not_the_table() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .persona(mac_persona())
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    // Distinct filename per test — same reasoning as `gpu_backend.rs`.
    tab.goto(&file_url("zendriver-gpu-delegation.html"))
        .await
        .expect("goto");
    tab.wait_for_load().await.expect("load");
    let got = spoofed_surface(&tab).await;
    browser.close().await.ok();

    // The spoof is live in this very read: a served capability still comes
    // from the tier table, so the delegated reads below are not passing
    // merely because the patch never installed.
    let (expect_tex, _) = resolved_tier_texture_and_viewport();
    assert_eq!(
        got["maxTexture"].as_i64(),
        Some(expect_tex),
        "MAX_TEXTURE_SIZE must still be table-served in the same run, else this test proves \
         nothing about delegation: {got:#}"
    );

    assert_eq!(
        got["blendBefore"], false,
        "BLEND starts disabled on a fresh context: {got:#}"
    );
    assert_eq!(
        got["blendAfter"], true,
        "gl.enable(gl.BLEND) must be visible to gl.getParameter(gl.BLEND) — a frozen table \
         answers false forever: {got:#}"
    );

    assert_eq!(
        got["stencilRequested"], true,
        "the second context must have been created with {{stencil: true}}: {got:#}"
    );
    let stencil_bits = got["stencilBits"].as_i64().expect("stencilBits");
    let stencil_default = got["stencilBitsDefault"]
        .as_i64()
        .expect("stencilBitsDefault");
    assert!(
        stencil_bits > 0,
        "STENCIL_BITS must reflect the context that was actually created ({{stencil: true}}), \
         got {stencil_bits}: {got:#}"
    );
    // Non-zero alone would also hold for a table that happened to serve a
    // stencil-enabled capture. What proves the value tracks the *context* is
    // that the two contexts in this same document disagree — one asked for a
    // stencil buffer and one did not. A served value cannot do that: both
    // reads go through the same prototype and would answer identically.
    assert_ne!(
        stencil_bits, stencil_default,
        "STENCIL_BITS must differ between a {{stencil: true}} context and a default one; both \
         reporting {stencil_bits} means the value is served, not delegated: {got:#}"
    );
}

/// The capture behind the tier a `Win32` persona resolves — see
/// [`webgpu_limits_come_from_the_tier_not_the_host`] for why that persona and
/// not the `MacIntel` one the rest of this file uses.
const D3D11_TIER_CAPTURE: &str =
    include_str!("../../zendriver-stealth/data/gpu-tiers/d3d11-fl11.json");

/// Reads the WebGPU adapter the way a fingerprinter does — twice over.
///
/// The `_shape` block is the cheaper half of that: before reading a single
/// limit, a script can ask what kind of object it is holding. Serving the
/// tier's values by handing back a plain object and a `Set` answered `"Object"`
/// / `"Set"`, failed both `instanceof` checks, and put 36 own properties on
/// something a real Chrome gives none — four one-line tells traded for the
/// coherence gain, which is no trade at all. `webgpu.js` overrides the
/// `GPUSupportedLimits` / `GPUSupportedFeatures` prototypes instead, so these
/// fields must come back **identical to the host's** while the values beside
/// them do not.
const ADAPTER_JS: &str = r#"(async () => {
  if (!('gpu' in navigator) || !navigator.gpu) return {error: 'no navigator.gpu'};
  const a = await navigator.gpu.requestAdapter();
  if (!a) return {error: 'requestAdapter resolved null'};
  return {
    vendor: a.info ? a.info.vendor : null,
    maxBufferSize: a.limits ? a.limits.maxBufferSize : null,
    maxStorageBuffersPerShaderStage: a.limits ? a.limits.maxStorageBuffersPerShaderStage : null,
    hasAstc: a.features ? a.features.has('texture-compression-astc') : null,
    featureCount: a.features ? a.features.size : null,
    shape: {
      limitsCtor: a.limits ? a.limits.constructor.name : null,
      limitsOwnProps: a.limits ? Object.getOwnPropertyNames(a.limits).length : null,
      limitsIsInstance: (typeof GPUSupportedLimits !== 'undefined' && a.limits)
        ? (a.limits instanceof GPUSupportedLimits) : null,
      featuresCtor: a.features ? a.features.constructor.name : null,
      featuresIsInstance: (typeof GPUSupportedFeatures !== 'undefined' && a.features)
        ? (a.features instanceof GPUSupportedFeatures) : null,
      featuresTag: a.features ? Object.prototype.toString.call(a.features) : null,
      // A setlike interface's default iterator IS its `values` method.
      featuresIterIsValues: a.features ? (a.features[Symbol.iterator] === a.features.values) : null,
    },
    // The setlike members must all agree with `size`, or one read contradicts
    // another on the same object.
    spreadCount: a.features ? [...a.features].length : null,
    forEachCount: a.features ? (() => { let n = 0; a.features.forEach(() => n++); return n; })() : null,
    firstEntry: a.features ? [...a.features.entries()][0] : null,
  };
})()"#;

/// `navigator.gpu` must describe the device the persona claims, not the one
/// running the page.
///
/// The adapter's `info` has named the claimed GPU since well before this
/// branch, but `.limits` and `.features` came from the host — so an adapter
/// could report `vendor: "nvidia"` above a 4 GiB Metal buffer limit. That is
/// the same shape of gap the tier tables closed for WebGL, and this is the
/// browser-side proof it is closed for WebGPU too.
///
/// **Why `Win32` here when every other test in this file pins `MacIntel`.**
/// This test needs a real adapter to decorate, so it launches
/// `GpuBackend::Native` — and the host that can supply one is, for the machine
/// these captures were taken on, a Mac with a Metal adapter. A `MacIntel`
/// persona would then serve the Metal tier's limits over a Metal host and
/// prove nothing: the claimed and observed values would agree whether or not
/// the patch ran. `Win32` resolves the D3D11 tier, whose `maxBufferSize` is
/// exactly 2 GiB against Metal's 4 GiB - 4, so the two disagree by
/// construction. The test asserts that disagreement rather than assuming it,
/// and skips if it ever fails to hold (a Windows host running this would have
/// nothing to tell apart either).
#[tokio::test]
#[ignore = "launches real Chrome and requires a usable GPU"]
async fn webgpu_limits_come_from_the_tier_not_the_host() {
    let browser = match Browser::builder()
        .stealth(StealthProfile::spoofed())
        // A GPU-less host has no adapter to decorate, so there is nothing for
        // this test to observe. `Native` has no fallback and fails the launch
        // there, which is the skip signal — same handling as
        // `gpu_backend.rs::native_backend_yields_a_real_adapter_and_device`.
        .gpu_backend(GpuBackend::Native)
        .persona(zendriver::stealth::Persona {
            platform: Some(zendriver::stealth::Platform::Win32),
            ..Default::default()
        })
        .launch()
        .await
    {
        Ok(b) => b,
        Err(e) => {
            assert!(
                matches!(
                    e,
                    ZendriverError::Browser(BrowserError::GpuBackendUnavailable)
                ),
                "a host without a usable GPU must fail with GpuBackendUnavailable; any other \
                 launch error is a real failure, not a no-GPU skip: {e:?}"
            );
            eprintln!("skipping: Native backend unavailable on this host: {e}");
            return;
        }
    };
    let tab = browser.main_tab();
    // Secure context required: `navigator.gpu` is `[SecureContext]`-gated, so
    // on `about:blank` this would take the no-adapter skip on a machine that
    // has one. Distinct filename per test, like the others here.
    tab.goto(&file_url("zendriver-gpu-webgpu.html"))
        .await
        .expect("goto");
    tab.wait_for_load().await.expect("load");

    // The isolated world holds its own copy of GPUAdapter.prototype, so it
    // reads the host's real adapter — the same free in-tab baseline
    // `spoofed_surface` uses for WebGL.
    let native: Value = tab
        .evaluate(ADAPTER_JS)
        .await
        .expect("isolated-world probe");
    if let Some(err) = native["error"].as_str() {
        browser.close().await.ok();
        eprintln!("skipping: no WebGPU adapter on this host ({err})");
        return;
    }
    let capture: Value = serde_json::from_str(D3D11_TIER_CAPTURE).expect("capture json");
    let expected = &capture["capture"]["adapter"]["limits"];
    let expect_buffer = expected["maxBufferSize"].as_u64().expect("capture limit");
    let host_buffer = native["maxBufferSize"].as_u64().expect("host limit");
    if host_buffer == expect_buffer {
        browser.close().await.ok();
        eprintln!(
            "skipping: this host's own maxBufferSize is {host_buffer}, the same value the \
             claimed tier serves, so the read cannot tell them apart"
        );
        return;
    }

    // Same install race as the WebGL probe — see `MAX_POLL_TRIES`.
    let mut got = Value::Null;
    for attempt in 0..MAX_POLL_TRIES {
        got = tab
            .evaluate_main(ADAPTER_JS)
            .await
            .expect("main-world probe");
        if got["maxBufferSize"].as_u64() == Some(expect_buffer) {
            break;
        }
        if attempt + 1 < MAX_POLL_TRIES {
            tokio::time::sleep(POLL_DELAY).await;
        }
    }
    browser.close().await.ok();

    assert_eq!(
        got["maxBufferSize"].as_u64(),
        Some(expect_buffer),
        "the adapter must report the resolved tier's maxBufferSize ({expect_buffer}), not this \
         host's ({host_buffer}): {got:#}"
    );
    assert_eq!(
        got["maxStorageBuffersPerShaderStage"].as_u64(),
        expected["maxStorageBuffersPerShaderStage"].as_u64(),
        "a second limit, so the first cannot pass on a coincidence: {got:#}"
    );
    // Features travel with the limits. The D3D11 tier measured no ASTC, and the
    // Metal host this most often runs on measured it — so a `true` here is the
    // host's list surviving.
    assert_eq!(
        got["hasAstc"], false,
        "the D3D11 tier claims no ASTC support; a true here is the host's feature set: {got:#}"
    );
    assert_ne!(
        native["hasAstc"], got["hasAstc"],
        "if the host and the claimed tier agree about ASTC this assertion proves nothing — \
         host {native:#} vs main world {got:#}"
    );
    assert_eq!(
        got["featureCount"].as_u64(),
        capture["capture"]["adapter"]["features"]
            .as_array()
            .map(|f| f.len() as u64),
        "`features.size` must count the tier's list, not the host's: {got:#}"
    );

    // The other half: the values are the claimed device's, the OBJECTS are the
    // host's own. Handing back a plain object and a `Set` would answer every
    // value correctly and still be caught by `constructor.name` alone.
    assert_eq!(
        got["shape"], native["shape"],
        "the adapter's limits/features objects must be indistinguishable from the host's real \
         ones — brand, constructor name, own-property count and setlike identity all included. \
         host {native:#} vs main world {got:#}"
    );
    assert_eq!(
        got["shape"]["limitsOwnProps"].as_u64(),
        Some(0),
        "a real GPUSupportedLimits carries every limit on its prototype and nothing of its own; \
         own properties here mean the object was replaced rather than decorated: {got:#}"
    );
    // Every setlike read must agree with `size`, or the object contradicts
    // itself in two adjacent expressions.
    let count = got["featureCount"].as_u64();
    assert_eq!(
        got["spreadCount"].as_u64(),
        count,
        "spread disagrees: {got:#}"
    );
    assert_eq!(
        got["forEachCount"].as_u64(),
        count,
        "forEach disagrees: {got:#}"
    );
    // `entries()` on a setlike yields [value, value] pairs.
    let first = got["firstEntry"].as_array().expect("firstEntry pair");
    assert_eq!(first.len(), 2, "entries() must yield pairs: {got:#}");
    assert_eq!(first[0], first[1], "a setlike pairs a value with itself");
}

/// `getSupportedExtensions` and `getExtension` must agree: an extension the
/// list claims but `getExtension` returns `null` for is a contradiction no
/// real context produces.
#[tokio::test]
#[ignore = "launches real Chrome"]
async fn extension_lists_agree_with_get_extension() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .persona(mac_persona())
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    // Distinct filename from the test above so the two cannot interfere if
    // ever run concurrently in-process — same reasoning as `gpu_backend.rs`.
    tab.goto(&file_url("zendriver-gpu-ext.html"))
        .await
        .expect("goto");
    tab.wait_for_load().await.expect("load");
    let got = spoofed_surface(&tab).await;
    browser.close().await.ok();

    let missing = got["claimedButMissing"].as_array().expect("array");
    assert!(
        missing.is_empty(),
        "every claimed extension must resolve; these did not: {missing:?}"
    );
}
