//! Phase 1 end-to-end test: real Chrome against a wiremock HTTP fixture.
//!
//! Gated behind the `integration-tests` feature so CI can skip it on
//! Chrome-less runners.

#![cfg(feature = "integration-tests")]

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::Browser;

#[tokio::test]
#[serial]
async fn click_dispatches_event_to_dom_listener() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<!doctype html>
            <html><body>
              <button id="b" onclick="window.clicked = true">x</button>
            </body></html>"#,
        ))
        .mount(&mock)
        .await;

    let browser = Browser::builder()
        .headless(true)
        // GitHub Actions runs as root in the runner container; Chromium's
        // user-namespace sandbox refuses to start without `--no-sandbox`,
        // and the small `/dev/shm` (~64 MB) in the runner makes the
        // renderer crash unless `/tmp` is used instead.
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .launch()
        .await
        .expect("launch failed");
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.expect("goto");
    tab.wait_for_load().await.expect("wait_for_load");

    let btn = tab.find().css("#b").one().await.expect("find #b");
    btn.click().await.expect("click");

    // Use evaluate_main: the onclick handler sets `window.clicked` on the page's
    // main world, but the default `evaluate` runs in an isolated world where
    // page globals are not visible. We must read it from the main world.
    let clicked: bool = tab.evaluate_main("window.clicked").await.expect("eval");
    assert!(clicked, "button click should have set window.clicked");

    browser.close().await.expect("close");
}
