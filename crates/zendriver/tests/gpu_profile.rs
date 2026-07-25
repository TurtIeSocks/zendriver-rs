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
use zendriver::{Browser, Tab};

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
    include_str!("../../zendriver-stealth/data/gpu-tiers/metal-apple-family3.json");

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
