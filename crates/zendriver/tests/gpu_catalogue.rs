//! Real-Chrome verification that a catalogued GPU identity reaches the page.
//!
//! Everything else guarding the catalogue compares generated artifacts against
//! each other: the composed string round-trips through `device_for_renderer`,
//! the entry derives its own vendor, the tier tables regenerate identically.
//! All of that stays green if the identity never reaches a browser at all.
//!
//! Run with:
//! ```sh
//! cargo test -p zendriver --test gpu_catalogue \
//!     --features integration-tests -- --ignored
//! ```
#![cfg(feature = "integration-tests")]
#![allow(clippy::panic)]

use std::time::Duration;

use serde_json::Value;
use zendriver::stealth::{GpuDevice, Persona, Platform, StealthProfile};
use zendriver::{Browser, GpuBackend};

/// Same bootstrap install race `gpu_profile.rs` documents: the stealth script
/// is injected via `Page.addScriptToEvaluateOnNewDocument`, and
/// `wait_for_load()` has no happens-before edge to "the main world finished
/// patching", so an early read can still see native WebGL.
const MAX_POLL_TRIES: u32 = 20;
const POLL_DELAY: Duration = Duration::from_millis(50);

/// Reads the three surfaces that have to agree for an identity to be coherent:
/// the name, the WebGPU adapter derived from it, and a capability value that
/// distinguishes one tier from the other.
const IDENTITY_JS: &str = r#"(async () => {
  const gl = document.createElement('canvas').getContext('webgl2');
  if (!gl) return {error: 'no webgl2'};
  const dbg = gl.getExtension('WEBGL_debug_renderer_info');
  let adapter = null;
  try {
    const a = navigator.gpu ? await navigator.gpu.requestAdapter() : null;
    if (a && a.info) adapter = {vendor: a.info.vendor, architecture: a.info.architecture};
  } catch (e) { /* no adapter on this host */ }
  return {
    renderer: dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
    vendor: dbg ? gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL) : null,
    vectors: gl.getParameter(gl.MAX_VERTEX_UNIFORM_VECTORS),
    adapter,
  };
})()"#;

fn file_url(name: &str) -> String {
    let page = std::env::temp_dir().join(name);
    std::fs::write(&page, "<!doctype html><title>probe</title>").expect("write probe page");
    let p = page.display().to_string().replace('\\', "/");
    if p.starts_with('/') {
        format!("file://{p}")
    } else {
        format!("file:///{p}")
    }
}

/// A catalogued device must reach the page as one coherent identity.
///
/// The NVIDIA D3D11 tier is chosen because its
/// `MAX_VERTEX_UNIFORM_VECTORS` is 4095 where every other shipped tier reports
/// 4096 or less — one number that proves the *tier* resolved from the
/// *identity*, not merely that a renderer string was substituted.
#[tokio::test]
#[ignore = "launches real Chrome"]
async fn a_catalogued_device_reaches_the_page_coherently() {
    let device = GpuDevice::by_name("NVIDIA GeForce RTX 4090").expect("catalogued");
    let persona = Persona {
        platform: Some(Platform::Win32),
        ..Persona::builder().gpu_device(device).build()
    };
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .persona(persona)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto(&file_url("zendriver-gpu-catalogue.html"))
        .await
        .expect("navigate");
    tab.wait_for_load().await.expect("load");

    // MUST be evaluate_main: `evaluate` runs in an isolated world the patch
    // does not reach, so it reads the unpatched surface and proves nothing.
    let mut got = Value::Null;
    for attempt in 0..MAX_POLL_TRIES {
        got = tab
            .evaluate_main(IDENTITY_JS)
            .await
            .expect("main-world probe");
        if got["renderer"]
            .as_str()
            .is_some_and(|r| r.contains("RTX 4090"))
        {
            break;
        }
        if attempt + 1 < MAX_POLL_TRIES {
            tokio::time::sleep(POLL_DELAY).await;
        }
    }
    browser.close().await.ok();

    let renderer = got["renderer"].as_str().unwrap_or_default();
    assert_eq!(
        renderer,
        &device.renderer(),
        "the page must read exactly the composed string: {got:#}"
    );
    assert!(
        renderer.contains("(0x00002684)"),
        "the device id must survive composition: {got:#}"
    );
    assert_eq!(
        got["vendor"].as_str(),
        Some("Google Inc. (NVIDIA)"),
        "vendor must be derived from the renderer, not from a device row: {got:#}"
    );
    assert_eq!(
        got["vectors"].as_i64(),
        Some(4095),
        "the identity must resolve the NVIDIA D3D11 tier, whose vectors are \
         4095 against 4096 elsewhere: {got:#}"
    );
    if let Some(adapter) = got["adapter"].as_object() {
        assert_eq!(
            adapter.get("vendor").and_then(Value::as_str),
            Some("nvidia"),
            "the WebGPU adapter must name the same GPU as WebGL: {got:#}"
        );
    }
}

/// Two different catalogued devices must produce two different identities.
///
/// A wiring bug that ignored the pinned device and served the platform default
/// would pass every assertion above, because the Win32 default *is* an NVIDIA
/// D3D11 part. This is what separates "the catalogue works" from "the default
/// happened to match".
#[tokio::test]
#[ignore = "launches real Chrome"]
async fn two_catalogued_devices_produce_two_identities() {
    let intel = GpuDevice::by_name("Intel(R) UHD Graphics 630").expect("catalogued");
    let persona = Persona {
        platform: Some(Platform::Win32),
        ..Persona::builder().gpu_device(intel).build()
    };
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .gpu_backend(GpuBackend::SwiftShader)
        .persona(persona)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto(&file_url("zendriver-gpu-catalogue-2.html"))
        .await
        .expect("navigate");
    tab.wait_for_load().await.expect("load");

    let mut got = Value::Null;
    for attempt in 0..MAX_POLL_TRIES {
        got = tab
            .evaluate_main(IDENTITY_JS)
            .await
            .expect("main-world probe");
        if got["renderer"]
            .as_str()
            .is_some_and(|r| r.contains("Intel"))
        {
            break;
        }
        if attempt + 1 < MAX_POLL_TRIES {
            tokio::time::sleep(POLL_DELAY).await;
        }
    }
    browser.close().await.ok();

    assert_eq!(
        got["renderer"].as_str(),
        Some(intel.renderer().as_str()),
        "an Intel identity must not be replaced by the Win32 default: {got:#}"
    );
    assert_eq!(
        got["vendor"].as_str(),
        Some("Google Inc. (Intel)"),
        "vendor must follow the pinned renderer: {got:#}"
    );
    // Intel is not NVIDIA, so it takes the tier without ANGLE's
    // skipVSConstantRegisterZero workaround: 4096, not 4095.
    assert_eq!(
        got["vectors"].as_i64(),
        Some(4096),
        "an Intel device must resolve the non-NVIDIA D3D11 tier: {got:#}"
    );
}

/// `nearest_gpu_device` must answer with the host's own backend or not at all.
///
/// This machine's answer depends on its hardware, so the assertion is the
/// invariant rather than a specific device: whatever comes back must be a
/// device the host's backend could actually report, and `None` is a correct
/// answer on a host whose backend has no catalogue.
#[tokio::test]
#[ignore = "launches real Chrome"]
async fn nearest_gpu_device_matches_the_hosts_backend() {
    let found = match zendriver::nearest_gpu_device().await {
        Ok(found) => found,
        Err(e) => {
            // No usable GPU here, so there is no renderer to match against.
            eprintln!("skipping: {e}");
            return;
        }
    };
    let Some(device) = found else {
        eprintln!("skipping: this host's backend has no catalogue (Linux or software)");
        return;
    };
    // The composed identity must be one the catalogue itself would produce,
    // and must round-trip to a tier — never a bare string assembled ad hoc.
    let renderer = device.renderer();
    assert!(
        renderer.starts_with("ANGLE ("),
        "not an ANGLE identity: {renderer}"
    );
    assert!(
        GpuDevice::by_name(device.model()).is_ok()
            || !GpuDevice::search(Some(device.model()), None).is_empty(),
        "nearest returned {} which the catalogue cannot find",
        device.model()
    );
}
