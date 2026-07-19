//! Phase 5 end-to-end tests against real Chrome + wiremock.
//!
//! Each test serves a tiny HTML fixture via `wiremock`, launches a headless
//! Chrome via `Browser::builder()`, exercises one P5 sub-area
//! (interception / expect_* / cloudflare detection), and asserts on
//! observable behavior via CDP, wiremock counters, or page-side JS state.
//!
//! Gated behind the `integration-tests` feature so CI can skip on
//! Chrome-less runners. The `fetcher` happy-path is *not* covered here —
//! downloading a full CfT zip is too heavy for PR CI; that lives behind
//! the separate `fetcher-network-tests` feature.

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use std::io::Write;
use std::time::Duration;

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::Browser;

/// Short pause after `intercept().start()` so the actor's fire-and-forget
/// `Fetch.enable` has reached Chrome before we issue a navigation that may
/// otherwise race past the interception attachment.
const INTERCEPT_SETTLE: Duration = Duration::from_millis(150);

#[tokio::test]
#[serial]
async fn interception_block_rule_prevents_request() {
    // Wiremock serves a real `/blocked/x.json` endpoint with `.expect(0)`
    // — wiremock's `Drop` will panic if any request lands on it. The
    // interception block rule should intercept the in-page `fetch()` before
    // the network bytes leave Chrome, so the count stays zero.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                r#"<!doctype html><html><body>
              <script>
                window.fetchErr = null;
                fetch('/blocked/x.json')
                  .then(r => r.text())
                  .then(t => { window.fetchResult = t; })
                  .catch(e => { window.fetchErr = String(e); });
              </script>
            </body></html>"#
                    .as_bytes()
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/blocked/x.json"))
        .respond_with(ResponseTemplate::new(200).set_body_string("should-not-arrive"))
        .expect(0)
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();

    // Start interception BEFORE the navigation; the actor's `Fetch.enable`
    // is fire-and-forget so give it a brief moment to land before goto.
    let _intercept = tab.intercept().block("*/blocked/*").unwrap().start();
    tokio::time::sleep(INTERCEPT_SETTLE).await;

    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // The page's fetch fires synchronously during the body parse; wait
    // long enough for Chrome to either complete it or hit the block path.
    // 500ms is comfortably above the localhost RTT + intercept dispatch.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The fetch should have rejected with a network error since Chrome
    // refused the request via `Fetch.failRequest { BlockedByClient }`.
    let err: Option<String> = tab.evaluate_main("window.fetchErr").await.unwrap_or(None);
    assert!(
        err.is_some(),
        "fetch to /blocked/x.json should have errored; got window.fetchErr = {err:?}"
    );

    browser.close().await.unwrap();

    // wiremock's `Drop` runs `verify()` on the MockServer; the
    // `.expect(0)` on `/blocked/x.json` panics if the request landed.
    drop(mock);
}

#[tokio::test]
#[serial]
async fn interception_respond_serves_fake_body() {
    // The page calls `fetch('/api/health')` which would normally 404
    // (we never mount a real route for it). The respond rule synthesizes
    // a 200 with the fake body, so the page sees `"hello-from-respond"`.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                r#"<!doctype html><html><body>
              <script>
                window.r = null;
                fetch('/api/health').then(r => r.text()).then(t => { window.r = t; });
              </script>
            </body></html>"#
                    .as_bytes()
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();

    let body = b"hello-from-respond".to_vec();
    let _intercept = tab
        .intercept()
        .respond(
            "*/api/health",
            200,
            vec![("content-type".into(), "text/plain".into())],
            body,
        )
        .unwrap()
        .start();
    tokio::time::sleep(INTERCEPT_SETTLE).await;

    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Poll for the fetch to resolve; `window.r` flips from `null` to the
    // body string. 2s budget covers Chrome scheduling on slow CI.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let r: String = loop {
        let v: Option<String> = tab.evaluate_main("window.r").await.unwrap_or(None);
        if let Some(s) = v {
            break s;
        }
        if std::time::Instant::now() >= deadline {
            panic!("respond rule never delivered a body to window.r within 2s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert_eq!(r, "hello-from-respond");

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn expect_response_returns_matched() {
    // The page fires an XHR ~200ms after load against `/api/data`. The
    // expectation is registered BEFORE goto so the subscriber is live
    // when the response lands.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/data"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                r#"<!doctype html><html><body>
              <script>
                setTimeout(() => { fetch('/api/data'); }, 200);
              </script>
            </body></html>"#
                    .as_bytes()
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();

    // Register expectation first so the `Network.responseReceived`
    // subscriber is in place before the navigation triggers the fetch.
    let expectation = tab
        .expect_response("/api/data")
        .timeout(Duration::from_secs(5));

    tab.goto(&mock.uri()).await.unwrap();

    let matched = expectation.await.expect("expect_response should resolve");
    assert!(
        matched.url.contains("/api/data"),
        "matched URL should contain /api/data; got {}",
        matched.url
    );
    assert_eq!(matched.status, 200);

    browser.close().await.unwrap();
}

// Disabled: under `--headless=new` (Chromium / Chrome Canary observed
// locally and on CI) the renderer suppresses `alert()` / `confirm()` /
// `prompt()` without firing `Page.javascriptDialogOpened`, so the
// expectation correctly times out. The underlying `expect_dialog`
// plumbing is exercised by the unit test in
// `expect/dialog.rs::expect_dialog_resolves_on_javascript_dialog_opened`,
// which drives the same code path against a mocked CDP transport.
// TODO(P5): re-enable once we have a per-tab dialog-routing override
// (Page.setOverrideUserAgent style) that forces headless to surface the
// event, or move the trigger to a path-driven `window.confirm` from a
// user-gesture context which headless still routes through CDP.
#[tokio::test]
#[serial]
#[ignore = "headless=new suppresses Page.javascriptDialogOpened for script-driven alert()"]
async fn expect_dialog_resolves_on_alert() {
    // The page schedules `alert('hi')` 100ms after load. Register
    // `expect_dialog` BEFORE navigation so the `Page.javascriptDialogOpened`
    // subscriber is live before the dialog opens — Chrome blocks JS at
    // the alert until handled.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                r#"<!doctype html><html><body>
              <script>setTimeout(() => alert('hi'), 100);</script>
            </body></html>"#
                    .as_bytes()
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();

    let expectation = tab.expect_dialog().timeout(Duration::from_secs(5));

    // Don't `await` goto's wait_for_load — the load event may stall
    // until the dialog is dismissed. Just dispatch navigate and let the
    // expectation resolve when the alert opens.
    tab.goto(&mock.uri()).await.unwrap();

    let matched = expectation.await.expect("expect_dialog should resolve");
    assert_eq!(matched.message, "hi");
    matched.accept(None).await.unwrap();

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn cloudflare_is_challenge_present_returns_false_on_normal_page() {
    // A vanilla page has no Cloudflare Turnstile iframe in any shadow
    // root, so the detector script returns null → `is_challenge_present`
    // returns `false`. Confirms the detector doesn't false-positive on
    // ordinary pages.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                r#"<!doctype html><html><body><h1>plain page</h1></body></html>"#
                    .as_bytes()
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let present = tab
        .cloudflare()
        .is_challenge_present()
        .await
        .expect("is_challenge_present should not error on a normal page");
    assert!(
        !present,
        "vanilla page must not be reported as carrying a Cloudflare challenge"
    );

    browser.close().await.unwrap();
}

// Bucketed into the nightly-only `#[ignore]` lane (per the recently-added
// `nightly-ignored-tests` job) rather than the per-PR gate: this exercises a
// brand-new CDP flow (`Page.setInterceptFileChooserDialog`) rather than a
// known Chrome limitation like `expect_dialog_resolves_on_alert` above.
// Verified locally against real headless Chrome before landing.
#[tokio::test]
#[serial]
#[ignore = "new Page.setInterceptFileChooserDialog flow; nightly-only per project convention"]
async fn expect_file_chooser_intercepts_button_triggered_hidden_input() {
    // The button's onclick indirects through a hidden <input type=file> —
    // the case `Element::upload_files` can't reach (it only knows how to
    // target a direct file input's backend node) but `expect_file_chooser`
    // can, since it intercepts `Page.fileChooserOpened` regardless of what
    // triggered it.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                r#"<!doctype html><html><body>
              <input type="file" id="hidden-input" style="display:none" />
              <button id="picker-btn"
                onclick="document.getElementById('hidden-input').click()">
                Pick file
              </button>
            </body></html>"#
                    .as_bytes()
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "hello from a button-triggered picker").unwrap();
    let path = tmp.path().to_path_buf();

    // Register BEFORE the click — the intercept must be enabled on Chrome's
    // side before the click can race it.
    let fc = tab
        .expect_file_chooser(&[&path])
        .await
        .unwrap()
        .timeout(Duration::from_secs(5));

    let button = tab.find().css("#picker-btn").one().await.unwrap();
    button.click().await.unwrap();

    let matched = fc.await.expect("expect_file_chooser should resolve");
    assert_eq!(matched.mode, zendriver::FileChooserMode::SelectSingle);

    let file_count: i64 = tab
        .evaluate_main("document.getElementById('hidden-input').files.length")
        .await
        .unwrap();
    assert_eq!(file_count, 1);

    let file_name: String = tab
        .evaluate_main("document.getElementById('hidden-input').files[0].name")
        .await
        .unwrap();
    let expected_name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(file_name, expected_name);

    browser.close().await.unwrap();
}

// Same nightly-only rationale as the button test above.
#[tokio::test]
#[serial]
#[ignore = "new Page.setInterceptFileChooserDialog flow; nightly-only per project convention"]
async fn expect_file_chooser_also_answers_a_directly_clicked_input() {
    // Confirms the same intercept path works when the file input is clicked
    // directly (not just via a button/label indirection) — the same CDP
    // event fires either way.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                r#"<!doctype html><html><body>
              <input type="file" id="f" />
            </body></html>"#
                    .as_bytes()
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "hello from a directly-clicked input").unwrap();
    let path = tmp.path().to_path_buf();

    let fc = tab
        .expect_file_chooser(&[&path])
        .await
        .unwrap()
        .timeout(Duration::from_secs(5));

    let input = tab.find().css("#f").one().await.unwrap();
    input.click().await.unwrap();

    fc.await.expect("expect_file_chooser should resolve");

    let file_name: String = tab
        .evaluate_main("document.getElementById('f').files[0].name")
        .await
        .unwrap();
    let expected_name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(file_name, expected_name);

    browser.close().await.unwrap();
}
