//! Nightly stealth tests against real-internet sites.
//!
//! Gated behind `stealth-tests` feature (which also requires `integration-tests`).
//! Run in CI on cron `0 6 * * *`. Failures are not blocking (`continue-on-error: true`).

#![cfg(feature = "stealth-tests")]

use serial_test::serial;
use std::time::Duration;
use zendriver::Browser;
use zendriver::stealth::StealthProfile;

#[tokio::test]
#[serial]
async fn spoofed_passes_sannysoft_intoli_block() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto("https://bot.sannysoft.com").await.expect("goto");
    tab.wait_for_load().await.expect("load");
    // Some sannysoft tests are async; give them a moment.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let results: Vec<(String, bool)> = tab
        .evaluate_main(
            r#"
        // sannysoft historically colored passing rows pure-green and failing
        // rows red. Around early 2026 it switched its pass shade to an
        // olive `rgb(200, 216, 109)`. Accept the new shade plus the legacy
        // greens so the test survives if the site reverts. Failure cells
        // remain red-dominant (R >> G && R >> B), so we infer pass = "any
        // non-failure, non-default background that the page picked".
        function isPassBg(bg) {
            const m = bg.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
            if (!m) return false;
            const r = +m[1], g = +m[2], b = +m[3];
            // Legacy greens.
            if (r === 0 && g >= 128 && b <= 128) return true;
            if (r <= 128 && g === 255 && b === 0) return true;
            // 2026 olive: r ≈ 200, g ≈ 216, b ≈ 109. Treat any cell where
            // green dominates red AND red dominates blue as a pass — that
            // covers the olive and any further yellow-green tweak without
            // false-positiving on the red failures.
            return g > r && r > b && g >= 150;
        }
        Array.from(document.querySelectorAll('table tr')).map(tr => {
            const cells = tr.querySelectorAll('td');
            if (cells.length < 2) return null;
            const name = cells[0].textContent.trim();
            const bg = window.getComputedStyle(cells[1]).backgroundColor;
            return [name, isPassBg(bg)];
        }).filter(x => x !== null)
    "#,
        )
        .await
        .expect("scrape");

    let intoli_test_names = [
        "User Agent",
        "WebDriver",
        "Chrome",
        "Permissions",
        "Plugins Length",
        "Languages",
        "WebGL Vendor",
        "WebGL Renderer",
        "Broken Image Dimensions",
    ];
    let intoli_failures: Vec<_> = results
        .iter()
        .filter(|(name, ok)| !ok && intoli_test_names.iter().any(|t| name.contains(t)))
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        intoli_failures.is_empty(),
        "spoofed profile failed Intoli rows: {intoli_failures:?}"
    );
    browser.close().await.expect("close");
}

#[tokio::test]
#[serial]
async fn spoofed_passes_areyouheadless() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto("https://arh.antoinevastel.com/bots/areyouheadless")
        .await
        .expect("goto");
    tab.wait_for_load().await.expect("load");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let result: String = tab
        .evaluate_main("document.querySelector('#res').textContent")
        .await
        .expect("scrape");
    assert!(
        result.contains("not Chrome headless"),
        "areyouheadless flagged us: {result}"
    );
    browser.close().await.expect("close");
}

#[tokio::test]
#[serial]
async fn spoofed_passes_intoli_basic_test() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto(
        "https://intoli.com/blog/not-possible-to-block-chrome-headless/chrome-headless-test.html",
    )
    .await
    .expect("goto");
    tab.wait_for_load().await.expect("load");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let results: Vec<(String, String)> = tab
        .evaluate_main(
            r#"
        Array.from(document.querySelectorAll('#results tr')).map(tr => {
            const cells = tr.querySelectorAll('td');
            if (cells.length < 2) return null;
            return [cells[0].textContent.trim(), cells[1].textContent.trim()];
        }).filter(x => x !== null)
    "#,
        )
        .await
        .expect("scrape");

    let fails: Vec<_> = results
        .iter()
        .filter(|(_, status)| status.to_lowercase().contains("fail"))
        .collect();
    assert!(fails.is_empty(), "intoli basic test fails: {fails:?}");
    browser.close().await.expect("close");
}

#[tokio::test]
#[serial]
async fn native_hides_webdriver_and_scrubs_user_agent() {
    // The native profile keeps its "no JS prototype patches" contract while
    // still (a) scrubbing the headless UA and (b) hiding navigator.webdriver
    // via the --disable-blink-features=AutomationControlled launch flag (a
    // command-line flag, not a JS shim — so the contract holds).
    let browser = Browser::builder()
        .stealth(StealthProfile::native())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto("https://bot.sannysoft.com").await.expect("goto");
    tab.wait_for_load().await.expect("load");
    tokio::time::sleep(Duration::from_secs(5)).await;

    // UA row should pass (HeadlessChrome scrubbed).
    let ua: String = tab.evaluate_main("navigator.userAgent").await.expect("ua");
    assert!(
        !ua.contains("HeadlessChrome"),
        "native profile must scrub UA: got {ua}"
    );

    // WebDriver row should pass — the AutomationControlled flag hides it.
    let wd: bool = tab.evaluate_main("navigator.webdriver").await.expect("wd");
    assert!(
        !wd,
        "native profile must hide webdriver via the AutomationControlled flag"
    );

    browser.close().await.expect("close");
}

/// Assert that the WebGPU adapter vendor reported by `navigator.gpu` coheres
/// with the spoofed WebGL renderer. If the platform has no GPU, the test is a
/// no-op pass (both values null). This validates the `Surface::Webgpu`
/// coherence patch ships correctly with `stealth()`.
#[tokio::test]
#[serial]
async fn webgpu_adapter_coheres_with_webgl_renderer() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto("about:blank").await.expect("goto");
    tab.wait_for_load().await.ok();

    let v: serde_json::Value = tab
        .evaluate_main(
            r#"(async () => {
            const a = navigator.gpu && await navigator.gpu.requestAdapter();
            const c = document.createElement('canvas').getContext('webgl');
            const dbg = c && c.getExtension('WEBGL_debug_renderer_info');
            return {
              gpuVendor: a && a.info ? a.info.vendor : null,
              webglRenderer: dbg ? c.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
            };
        })()"#,
        )
        .await
        .expect("eval");

    // If WebGPU is available, its vendor must be substring-consistent with
    // the WebGL renderer (both nvidia / both intel / etc.). If gpuVendor is
    // null (no GPU in this env), the test is a no-op pass.
    if let Some(vendor) = v.get("gpuVendor").and_then(|x| x.as_str()) {
        let renderer = v
            .get("webglRenderer")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_lowercase();
        assert!(
            renderer.contains(vendor) || (vendor == "intel" && renderer.contains("intel")),
            "webgpu vendor {vendor} must cohere with webgl renderer {renderer}"
        );
    }

    browser.close().await.expect("close");
}
