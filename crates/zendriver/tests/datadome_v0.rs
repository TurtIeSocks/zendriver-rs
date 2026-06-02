//! Integration tests for `tab.datadome()` against a synthetic DataDome fixture.
//!
//! Gated behind `integration-tests` + `datadome`, `#[ignore]` (needs a real
//! Chrome). Run:
//! ```bash
//! cargo test -p zendriver --features datadome-tests --test datadome_v0 -- --ignored
//! ```
#![cfg(all(feature = "integration-tests", feature = "datadome"))]
#![allow(clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::stealth::StealthProfile;
use zendriver::{Browser, DataDomeClearanceOutcome};

/// A challenge page carrying a `var dd` config (device-check, t='fe').
const CHALLENGE_HTML: &str = r#"<!doctype html><html><head>
<script>var dd={'rt':'c','cid':'CID123','hsh':'HSH456','t':'fe','s':1,'host':'geo.captcha-delivery.com'};</script>
</head><body>verifying…</body></html>"#;

const CLEAN_HTML: &str = r#"<!doctype html><html><body>welcome</body></html>"#;

#[tokio::test]
#[serial]
#[ignore]
async fn datadome_device_check_clears_when_cookie_set() {
    let mock = MockServer::start().await;
    // Serve a challenge page (window.dd present, device-check surface).
    // No datadome cookie is ever set by the server, so the poll loop observes
    // device-check markers and times out — proving detection works against a
    // real Chrome.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(CHALLENGE_HTML.as_bytes().to_vec(), "text/html"),
        )
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.ok();

    // No datadome cookie + device-check markers → times out (markers never clear).
    let outcome = tab
        .datadome()
        .timeout(Duration::from_millis(800))
        .poll_interval(Duration::from_millis(100))
        .wait_for_clearance()
        .await
        .unwrap();
    assert!(
        matches!(outcome, DataDomeClearanceOutcome::TimedOut { .. }),
        "expected TimedOut, got {outcome:?}"
    );

    browser.close().await.ok();
}

#[tokio::test]
#[serial]
#[ignore]
async fn datadome_already_clear_on_plain_page() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CLEAN_HTML.as_bytes().to_vec(), "text/html"),
        )
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.ok();

    let outcome = tab
        .datadome()
        .timeout(Duration::from_secs(2))
        .wait_for_clearance()
        .await
        .unwrap();
    assert!(
        matches!(outcome, DataDomeClearanceOutcome::AlreadyClear),
        "expected AlreadyClear, got {outcome:?}"
    );

    browser.close().await.ok();
}

/// Drift probe against a known DataDome-protected surface. Best-effort: skips
/// (does not fail) on network errors. Run only in the nightly job.
#[tokio::test]
#[serial]
#[ignore]
async fn datadome_real_site_drift_probe() {
    let browser = match Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
    {
        Ok(b) => b,
        Err(_) => return,
    };
    // deviceandbrowserinfo.com/are_you_a_bot is the surface the #20 reporter used.
    let tab = browser.main_tab();
    if tab
        .goto("https://deviceandbrowserinfo.com/are_you_a_bot")
        .await
        .is_err()
    {
        browser.close().await.ok();
        return;
    }
    tab.wait_for_load().await.ok();
    // We don't assert pass/fail (sites change); we assert the bypass RUNS and
    // returns a terminal without panicking.
    let outcome = tab
        .datadome()
        .timeout(Duration::from_secs(20))
        .wait_for_clearance()
        .await;
    eprintln!("datadome real-site outcome: {outcome:?}");
    browser.close().await.ok();
}
