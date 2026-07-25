//! Real-Chrome GPU backend tests. All `#[ignore]` — they launch a browser.
//!
//! Run with: `cargo test -p zendriver --test gpu_backend -- --ignored`

use std::path::Path;

use zendriver::{Browser, BrowserError, GpuBackend, ZendriverError};

/// Builds a `file://` URL for `page`.
///
/// Both tests below need to probe from a secure-context page:
/// `navigator.gpu` is `[SecureContext]`-gated and `about:blank` is an opaque
/// origin where `isSecureContext` is false, so WebGPU would be invisible no
/// matter which backend Chrome is running. For the canary this also matters
/// for WebGL: the recorded extension count was measured on a `file://` page,
/// and matching that condition removes a class of false-alarm investigation
/// when the canary fires. Mirrors the construction in
/// `examples/probe_gpu.rs`.
fn file_url(page: &Path) -> String {
    let path_str = page.display().to_string().replace('\\', "/");
    if path_str.starts_with('/') {
        format!("file://{path_str}") // Unix: /path -> file:///path
    } else {
        format!("file:///{path_str}") // Windows: C:/path -> file:///C:/path
    }
}

/// Keep in sync with `examples/probe_gpu.rs`. Only the WebGL2 subset the
/// canary compares is needed here.
const CAPS_JS: &str = r#"
(() => {
  const gl = document.createElement('canvas').getContext('webgl2');
  if (!gl) return JSON.stringify({ error: 'no webgl2 context' });
  const dbg = gl.getExtension('WEBGL_debug_renderer_info');
  return JSON.stringify({
    unmaskedRenderer: dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
    maxTextureSize: gl.getParameter(gl.MAX_TEXTURE_SIZE),
    maxViewportDims: Array.from(gl.getParameter(gl.MAX_VIEWPORT_DIMS)),
    maxVertexUniformVectors: gl.getParameter(gl.MAX_VERTEX_UNIFORM_VECTORS),
    extensionCount: gl.getSupportedExtensions().length,
  });
})()
"#;

/// Pins ANGLE's SwiftShader constants so a future Chrome update that moves
/// them gets caught here instead of silently invalidating the tier tables
/// derived from these numbers. Runs on any host, including GPU-less CI,
/// because SwiftShader is a software rasterizer.
#[tokio::test]
#[ignore = "launches real Chrome"]
async fn swiftshader_tier_matches_recorded_baseline() {
    let browser = Browser::builder()
        .gpu_backend(GpuBackend::SwiftShader)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    // Probe from the same kind of page the recorded constants were measured
    // on — see `file_url`'s doc comment for why this must not be
    // `about:blank`.
    let page = std::env::temp_dir().join("zendriver-swiftshader-canary.html");
    std::fs::write(&page, "<!doctype html><title>probe</title>").expect("write probe page");
    tab.goto(&file_url(&page)).await.expect("goto");
    tab.wait_for_load().await.expect("load");
    let raw: String = tab.evaluate(CAPS_JS).await.expect("evaluate");
    browser.close().await.ok();

    let got: serde_json::Value = serde_json::from_str(&raw).expect("probe json");

    assert!(
        got["unmaskedRenderer"]
            .as_str()
            .unwrap_or_default()
            .contains("SwiftShader"),
        "expected the SwiftShader backend, got: {got:#}"
    );

    // These are ANGLE's SwiftShader constants as measured on 2026-07-24. A
    // failure here does NOT mean this test is wrong — it means Chrome's
    // ANGLE constants moved, and every tier table derived from them must be
    // re-derived. Update these values together with the tables, and note the
    // Chrome version the new numbers were measured against.
    assert_eq!(got["maxTextureSize"], 8192, "ANGLE drift: {got:#}");
    assert_eq!(
        got["maxViewportDims"],
        serde_json::json!([8192, 8192]),
        "ANGLE drift: {got:#}"
    );
    assert_eq!(got["maxVertexUniformVectors"], 4096, "ANGLE drift: {got:#}");
    assert_eq!(got["extensionCount"], 30, "ANGLE drift: {got:#}");
}

const ADAPTER_JS: &str = r#"
(async () => {
  if (!('gpu' in navigator)) return JSON.stringify({ adapter: null, reason: 'no navigator.gpu' });
  const a = await navigator.gpu.requestAdapter();
  if (!a) return JSON.stringify({ adapter: null, reason: 'requestAdapter resolved null' });
  let deviceOk = false;
  try { deviceOk = !!(await a.requestDevice()); } catch (e) { deviceOk = false; }
  return JSON.stringify({
    adapter: { vendor: a.info ? a.info.vendor : null, architecture: a.info ? a.info.architecture : null },
    deviceOk,
  });
})()
"#;

/// Proves the headline claim of `GpuBackend::Native`: a real adapter and a
/// **working** `requestDevice()`. Skips cleanly when no GPU is available —
/// but the skip path is narrowed to the specific failure modes that mean "no
/// GPU here" (`BrowserError::GpuBackendUnavailable` on launch, or a null
/// adapter after launch) so a skip can never quietly paper over a
/// misconfigured test.
#[tokio::test]
#[ignore = "launches real Chrome and requires a usable GPU"]
async fn native_backend_yields_a_real_adapter_and_device() {
    let browser = match Browser::builder()
        .gpu_backend(GpuBackend::Native)
        .launch()
        .await
    {
        Ok(b) => b,
        Err(e) => {
            // A GPU-less host is a legitimate skip, not a failure. `Native`
            // deliberately has no fallback, so a launch failure IS the
            // expected outcome here — but only if it's specifically the
            // GPU-unavailable variant. Any other launch error (bad flags, a
            // missing binary, a handshake timeout unrelated to the GPU) is a
            // real failure and must not be swallowed as a "no GPU" skip.
            assert!(
                matches!(
                    e,
                    ZendriverError::Browser(BrowserError::GpuBackendUnavailable)
                ),
                "launch failed for a reason other than GpuBackendUnavailable, \
                 this is a real failure, not a no-GPU skip: {e:?}"
            );
            eprintln!("skipping: Native backend unavailable on this host: {e}");
            return;
        }
    };
    let tab = browser.main_tab();
    // Secure context required — see `file_url`'s doc comment. On
    // `about:blank` this test would take the "no adapter on this host" skip
    // branch on a machine that has a real GPU, quietly reporting a pass
    // while verifying nothing. Distinct filename from the canary's page so
    // the two tests can't interfere if ever run concurrently in-process.
    let page = std::env::temp_dir().join("zendriver-native-adapter.html");
    std::fs::write(&page, "<!doctype html><title>probe</title>").expect("write probe page");
    tab.goto(&file_url(&page)).await.expect("goto");
    tab.wait_for_load().await.expect("load");
    let raw: String = tab.evaluate(ADAPTER_JS).await.expect("evaluate");
    browser.close().await.ok();

    let got: serde_json::Value = serde_json::from_str(&raw).expect("probe json");
    if got["adapter"].is_null() {
        eprintln!("skipping: no GPU adapter on this host ({})", got["reason"]);
        return;
    }

    assert!(
        got["adapter"]["vendor"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "a real adapter must report a vendor, got: {got:#}"
    );
    assert_eq!(
        got["deviceOk"], true,
        "the headline claim for GpuBackend::Native is a WORKING device, got: {got:#}"
    );
}
