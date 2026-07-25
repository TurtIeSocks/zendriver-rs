//! Dump this host's real GPU surface as JSON.
//!
//! Run against the host GPU (the useful case — captures real values):
//!
//! ```text
//! cargo run -p zendriver --example probe_gpu -- native
//! ```
//!
//! Or against the software rasterizer, which is what the ANGLE-drift canary
//! test compares against:
//!
//! ```text
//! cargo run -p zendriver --example probe_gpu -- swiftshader
//! ```
//!
//! The output is the input format for the tier tables: capture it on a real
//! device, then hand it to the profile dataset.

use zendriver::{Browser, GpuBackend};

/// Reads every value the tier tables need. Kept as one expression so it can be
/// evaluated in a single CDP round-trip.
const PROBE_JS: &str = r#"
(async () => {
  const out = {};
  out.gpuInNavigator = ('gpu' in navigator);
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
    tab.goto("about:blank").await?;
    tab.wait_for_load().await?;
    // `Tab::evaluate` already sends `awaitPromise: true` (tab.rs:1096), so the
    // async IIFE above resolves before the value comes back.
    let json: String = tab.evaluate(PROBE_JS).await?;
    println!("{json}");
    browser.close().await?;
    Ok(())
}
