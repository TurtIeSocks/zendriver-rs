//! Real-Chrome verification that the spoofed WebGL surface is coherent.
//!
//! Every other check on the GPU tier tables is a unit test, a generated table,
//! or a model of Blink. This file is the only place the tables meet an actual
//! browser, so it reads the pairs a fingerprinter cross-checks and asserts the
//! relations real hardware always satisfies.
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
    for attempt in 0..MAX_POLL_TRIES {
        let raw: String = tab.evaluate_main(CHECK_JS).await.expect("main-world probe");
        let got: Value = serde_json::from_str(&raw).expect("probe json");
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
    // The captures render this range as `[1, 1023]` because `JSON.stringify`
    // collapses `1.0`; the generator applies the spec-declared float type
    // instead. If that plumbing broke anywhere along the chain the page gets
    // an Int32Array, which is a one-`instanceof` tell.
    assert_eq!(
        got["aliasedIsFloat32"], true,
        "ALIASED_POINT_SIZE_RANGE must be a Float32Array, not Int32Array: {got:#}"
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
