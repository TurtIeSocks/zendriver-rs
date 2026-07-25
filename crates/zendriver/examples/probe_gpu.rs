//! Dump this host's real GPU surface as JSON.
//!
//! Three modes (pass as argument; default is `disabled`):
//!
//! - `native` — Use the host GPU (Metal, Vulkan, etc.; the useful case for
//!   capturing real device values).
//! - `swiftshader` — Use the ANGLE/SwiftShader software rasterizer (note:
//!   SwiftShader has no WebGPU adapter, so `adapter` is legitimately `null`).
//! - `disabled` — WebGPU is disabled entirely (run with no argument for this).
//!
//! ```text
//! cargo run -p zendriver --example probe_gpu -- native
//! cargo run -p zendriver --example probe_gpu -- swiftshader
//! cargo run -p zendriver --example probe_gpu -- disabled
//! ```
//!
//! ## Output schema
//!
//! The JSON contains one object with these top-level keys (always present):
//!
//! - `gpuInNavigator` — boolean, whether `navigator.gpu` exists.
//! - `isSecureContext` — boolean, whether the page is a secure context (required
//!   for WebGPU access).
//! - `adapter` — WebGPU adapter info object, or `null` if no adapter is available
//!   (normal under `swiftshader`). When present, contains:
//!   - `vendor`, `architecture`, `device`, `description` (strings or `null`)
//!   - `limits` — object of numeric device limits (30+ keys typical on real GPUs)
//!   - `features` — array of capability names (e.g., `"texture-adapter-specific-format-features"`)
//! - `deviceOk` — result of `adapter.requestDevice()`: boolean `true` if it
//!   succeeded, string `"reject: <ErrorName>"` if it threw, or `null` if no
//!   adapter was available to request from.
//! - `webgl1` — WebGL 1 context info (see below), or `null` if unavailable.
//! - `webgl2` — WebGL 2 context info (see below), or `null` if unavailable.
//! - `adapterErr` — (only present if `requestAdapter()` threw) diagnostic string.
//!
//! WebGL context objects (`webgl1` / `webgl2`) contain:
//!
//! - `unmaskedVendor` / `unmaskedRenderer` — real GPU vendor/model (from the
//!   `WEBGL_debug_renderer_info` extension), or absent if not supported.
//! - `extensions` — array of extension names.
//! - `params` — object of numeric GL parameters (e.g., `MAX_TEXTURE_SIZE`).
//! - `precision` — object of shader precision formats, keyed by
//!   `VERTEX_SHADER/<level>` / `FRAGMENT_SHADER/<level>` (e.g.,
//!   `"VERTEX_SHADER/HIGH_FLOAT": [127, 127, 24]` for `rangeMin, rangeMax, precision`).
//! - `enums` — object mapping each `params` key to its GL enum number (e.g.,
//!   `"MAX_TEXTURE_SIZE": 3379`). The runtime patch receives the number from
//!   `getParameter(3379)` and needs this to map it back to a name.
//!
//! This output is the input format for GPU tier tables: capture it on a real
//! device, then hand it to the profile dataset.
//!
//! **Note:** This example writes a temporary HTML file to the system temp
//! directory (`zendriver-probe-gpu.html`) and leaves it there.

use zendriver::{Browser, GpuBackend};

/// Reads every value the tier tables need. Kept as one expression so it can be
/// evaluated in a single CDP round-trip.
const PROBE_JS: &str = r#"
(async () => {
  const out = {};
  out.isSecureContext = window.isSecureContext;
  out.gpuInNavigator = ('gpu' in navigator);
  out.adapter = null;
  out.deviceOk = null;
  try {
    const a = navigator.gpu ? await navigator.gpu.requestAdapter() : null;
    out.adapter = a ? {
      vendor: a.info ? a.info.vendor : null,
      architecture: a.info ? a.info.architecture : null,
      device: a.info ? a.info.device : null,
      description: a.info ? a.info.description : null,
      limits: a.limits ? Object.fromEntries(
        Object.keys(Object.getPrototypeOf(a.limits))
          .map(k => [k, a.limits[k]])
          .filter(([, v]) => typeof v === 'number')) : null,
      features: a.features ? Array.from(a.features) : null,
    } : null;
    if (a) {
      try { out.deviceOk = !!(await a.requestDevice()); }
      catch (e) { out.deviceOk = 'reject: ' + e.name; }
    }
  } catch (e) { out.adapterErr = String(e); }

  function readContext(kind) {
    const gl = document.createElement('canvas').getContext(kind);
    if (!gl) return null;
    const r = { extensions: gl.getSupportedExtensions(), params: {}, precision: {} };
    const dbg = gl.getExtension('WEBGL_debug_renderer_info');
    if (dbg) {
      r.unmaskedVendor = gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL);
      r.unmaskedRenderer = gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL);
    }
    // Every numeric GL enum the context recognises. Unknown enums throw or
    // return null, which is how non-parameters are filtered out.
    for (const name of Object.keys(Object.getPrototypeOf(gl))) {
      const val = gl[name];
      if (typeof val !== 'number') continue;
      try {
        const got = gl.getParameter(val);
        if (got === null || typeof got === 'object' && !ArrayBuffer.isView(got)) continue;
        r.params[name] = ArrayBuffer.isView(got) ? Array.from(got) : got;
      } catch (e) { /* not a gettable parameter */ }
    }
    for (const st of ['VERTEX_SHADER', 'FRAGMENT_SHADER']) {
      for (const pt of ['LOW_FLOAT','MEDIUM_FLOAT','HIGH_FLOAT','LOW_INT','MEDIUM_INT','HIGH_INT']) {
        const f = gl.getShaderPrecisionFormat(gl[st], gl[pt]);
        if (f) r.precision[st + '/' + pt] = [f.rangeMin, f.rangeMax, f.precision];
      }
    }
    r.enums = {};
    for (const name of Object.keys(Object.getPrototypeOf(gl))) {
      const val = gl[name];
      if (typeof val === 'number' && Object.prototype.hasOwnProperty.call(r.params, name)) {
        r.enums[name] = val;
      }
    }
    return r;
  }

  out.webgl1 = readContext('webgl');
  out.webgl2 = readContext('webgl2');
  return JSON.stringify(out);
})()
"#;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary
async fn main() -> zendriver::Result<()> {
    let backend = match std::env::args().nth(1).as_deref() {
        Some("native") => GpuBackend::Native,
        Some("swiftshader") => GpuBackend::SwiftShader,
        Some("disabled") | None => GpuBackend::Disabled,
        Some(other) => {
            eprintln!("unknown backend {other:?}; expected native | swiftshader | disabled");
            return Ok(());
        }
    };

    let browser = Browser::builder().gpu_backend(backend).launch().await?;
    let tab = browser.main_tab();
    // MUST be a secure context. `navigator.gpu` is `[SecureContext]`-gated,
    // and `about:blank` is an opaque origin where `isSecureContext` is false —
    // WebGPU is then invisible no matter which backend Chrome is running, so
    // the probe would silently report `adapter: null` on a machine with a
    // perfectly good GPU. WebGL is not gated this way, which is why it reports
    // correctly either way and masks the problem.
    let page = std::env::temp_dir().join("zendriver-probe-gpu.html");
    std::fs::write(&page, "<!doctype html><title>probe</title>")?;
    let path_str = page.display().to_string().replace('\\', "/");
    let file_url = if path_str.starts_with('/') {
        format!("file://{}", path_str) // Unix: /path → file:///path
    } else {
        format!("file:///{}", path_str) // Windows: C:/path → file:///C:/path
    };
    tab.goto(&file_url).await?;
    tab.wait_for_load().await?;
    // `Tab::evaluate` already sends `awaitPromise: true` (tab.rs:1096), so the
    // async IIFE above resolves before the value comes back.
    let json: String = tab.evaluate(PROBE_JS).await?;
    println!("{json}");
    browser.close().await?;
    Ok(())
}
