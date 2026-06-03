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
