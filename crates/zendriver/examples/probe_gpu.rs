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
//! ## Capturing a tier
//!
//! `--emit-tier <name>` validates the capture, stamps it with provenance, and
//! writes it straight into `crates/zendriver-stealth/data/gpu-tiers/`, ready to
//! commit:
//!
//! ```text
//! cargo run -p zendriver --example probe_gpu -- native --emit-tier d3d11-fl11
//! cargo run -p zendriver --example probe_gpu -- native --emit-tier vulkan-intel-uhd620-mesa24 \
//!     --driver "Intel UHD Graphics 620, Mesa 24.0.9"
//! ```
//!
//! This exists because the alternative — piping stdout through a shell
//! one-liner — is not portable. The POSIX `VAR=x cmd` prefix has no PowerShell
//! equivalent, `python3` is `python` on Windows, and worst of all
//! `>` in Windows PowerShell 5.1 writes UTF-16 with a BOM, which produces a
//! capture file that looks fine in an editor and fails to parse. Doing the
//! work in Rust sidesteps all three and writes the same bytes everywhere.
//!
//! `--driver` is for Vulkan captures only, whose values are read off the
//! physical device and so must record which device and driver produced them.
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

/// Where `--emit-tier` writes. Resolved from the manifest directory at compile
/// time, so the command works from any working directory rather than only from
/// the workspace root.
const TIER_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../zendriver-stealth/data/gpu-tiers"
);

/// Reads every value the tier tables need. Kept as one expression so it can be
/// evaluated in a single CDP round-trip.
const PROBE_JS: &str = r#"
(async () => {
  const out = {};
  out.isSecureContext = window.isSecureContext;
  // Provenance: which browser produced these numbers. ANGLE's constants can
  // move between Chrome versions, so a capture without a version is a capture
  // nobody can date.
  out.userAgent = navigator.userAgent;
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

/// The `Chrome/<version>` token out of a user-agent string.
///
/// ANGLE's capability constants move between Chrome versions, so a capture
/// without a version is a capture nobody can date.
fn chrome_version(user_agent: &str) -> Option<&str> {
    let rest = user_agent.split("Chrome/").nth(1)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    Some(&rest[..end]).filter(|v| !v.is_empty())
}

/// Reject a tier name that is not the shape every existing tier already uses.
///
/// The name becomes a path segment, so this also keeps it from escaping the
/// data directory. Checked before the browser launches — being told the name
/// is wrong after waiting through a full probe is a bad trade.
fn validate_tier_name(tier: &str) -> Result<(), Box<dyn std::error::Error>> {
    if tier.is_empty()
        || !tier
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "tier name {tier:?} must be lowercase letters, digits and dashes (e.g. d3d11-fl11)"
        )
        .into());
    }
    Ok(())
}

/// Validate a capture, stamp it with provenance, and write it into the tier
/// data directory.
///
/// The two checks are the point: a capture taken outside a secure context
/// silently lacks all WebGPU data, and one that fell back to SwiftShader
/// describes a different backend than its name claims. Both produce a file
/// that looks perfectly fine and is wrong, so they are refused here rather
/// than left for a reviewer to notice.
fn emit_tier(
    json: &str,
    tier: &str,
    backend: GpuBackend,
    driver: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let capture: serde_json::Value = serde_json::from_str(json)?;

    if capture["isSecureContext"] != serde_json::Value::Bool(true) {
        return Err("not a secure context; the capture's WebGPU data would be missing".into());
    }

    let renderer = capture["webgl2"]["unmaskedRenderer"]
        .as_str()
        .ok_or("capture has no webgl2.unmaskedRenderer")?;
    // Only meaningful for a hardware capture: `swiftshader` is a legitimate
    // tier to capture deliberately, and asserting unconditionally would make
    // it impossible to record.
    if backend == GpuBackend::Native && renderer.contains("SwiftShader") {
        return Err(format!("GPU did not engage; renderer is {renderer:?}").into());
    }

    let chrome = capture["userAgent"]
        .as_str()
        .and_then(chrome_version)
        .unwrap_or("unknown");
    let os = format!(
        "{} {}",
        sysinfo::System::name().unwrap_or_default(),
        sysinfo::System::kernel_version().unwrap_or_default()
    );
    let mut provenance = format!("probed: Chrome {chrome} on {}", os.trim());
    if let Some(driver) = driver.map(str::trim).filter(|d| !d.is_empty()) {
        provenance.push_str(&format!(", {driver}"));
    }

    // `serde_json::Map` is a `BTreeMap` here (no `preserve_order` feature), so
    // this is written with sorted keys and stays diffable against the captures
    // already committed.
    let out = serde_json::json!({
        "tier": tier,
        "provenance": provenance,
        "capture": capture,
    });

    let path = std::path::Path::new(TIER_DIR).join(format!("{tier}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&out)? + "\n")?;

    let params = capture["webgl2"]["params"]
        .as_object()
        .map_or(0, serde_json::Map::len);
    println!("wrote {}", path.display());
    println!("  provenance: {provenance}");
    println!("  renderer:   {renderer}");
    println!("  webgl2:     {params} parameters");
    println!("\nNext: register the tier in the six places listed in");
    println!(".claude/skills/capture-gpu-tier/SKILL.md, then `cargo run -p gpu-tier-gen`.");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut backend_arg = None;
    let mut tier = None;
    let mut driver = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--emit-tier" => tier = Some(args.next().ok_or("--emit-tier needs a name")?),
            "--driver" => driver = Some(args.next().ok_or("--driver needs a value")?),
            other => backend_arg = Some(other.to_string()),
        }
    }

    if let Some(tier) = &tier {
        validate_tier_name(tier)?;
    }

    let backend = match backend_arg.as_deref() {
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
    browser.close().await?;

    match tier {
        Some(tier) => emit_tier(&json, &tier, backend, driver.as_deref())?,
        None => println!("{json}"),
    }
    Ok(())
}
