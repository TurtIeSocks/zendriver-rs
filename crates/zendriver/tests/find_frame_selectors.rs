//! Frame-scoped selector regression tests: verify `frame.find().<kind>()`
//! resolves against the FRAME's own document, not the main tab's.
//!
//! Gated behind the `integration-tests` feature; CI exercises these on the
//! integration job where a real Chrome binary is available. Fixture/server
//! pattern mirrors `find_predicate_iframe.rs`.
//!
//! Regression context: `resolve_xpath_many`, `resolve_text_many`, and
//! `resolve_text_regex_many` in `crates/zendriver/src/query/selectors.rs`
//! used to omit CDP `contextId` on their `Tab`/`Frame` `Runtime.evaluate`
//! arm, so a frame-scoped `.xpath()`/`.text()`/`.text_regex()` query
//! silently ran against the MAIN document instead of the frame's. Each test
//! below puts a distinct element in the iframe and a similar-but-different
//! one in the main document, then asserts the frame-scoped query finds only
//! the iframe's element.

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::{AriaRole, Browser};

/// Outer page embeds `/frame` in an iframe and also has its own
/// distinctly-labeled button, so a query that accidentally hits the main
/// document (instead of the frame's) finds the WRONG element instead of
/// finding nothing.
///
/// Deliberate whitespace/indentation around the button: the `text_regex`
/// resolver (`resolve_text_regex_many`/`_one`) has no "narrowest match"
/// step (unlike the plain-substring `.text()` path), so an unanchored
/// pattern matching the button's text also matches `<body>`/`<html>`'s
/// *aggregate* `textContent` and — being first in document order — wins
/// `.one()`'s `[0]`. That's a separate, pre-existing gap (not this fix's
/// scope); anchoring the regex to the button's exact (unpadded) text and
/// keeping surrounding whitespace on the ancestors sidesteps it here so
/// this test isolates just the frame-`contextId` behavior under test.
///
/// `main-btn`'s text deliberately starts with the same `unique-` prefix as
/// the frame's button (but a different suffix) so a buggy frame-scoped
/// query that accidentally falls back to the main document's `contextId`
/// finds a WRONG match here (`main-btn`) instead of erroring out — a much
/// stronger regression signal than "not found".
const OUTER_HTML: &str = r#"<!doctype html><html><body>
    <button id="main-btn" role="button" aria-label="Main Button">unique-main-decoy-text</button>
    <iframe id="child" src="/frame"></iframe>
</body></html>"#;

const FRAME_HTML: &str = r#"<!doctype html><html><body>
    <button id="frame-btn" role="button" aria-label="Frame Button">unique-frame-text</button>
</body></html>"#;

async fn fixture() -> MockServer {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(OUTER_HTML.as_bytes().to_vec(), "text/html"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/frame"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(FRAME_HTML.as_bytes().to_vec(), "text/html"),
        )
        .mount(&mock)
        .await;
    mock
}

/// Navigate, wait for the child iframe to register, and return its
/// `Frame` handle (as opposed to the main frame).
async fn child_frame(tab: &zendriver::Tab) -> zendriver::Frame {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let frames = tab.frames().await.unwrap();
        if let Some(f) = frames.iter().find(|f| !f.is_main()) {
            return f.clone();
        }
        if std::time::Instant::now() >= deadline {
            panic!("expected a child frame to register within 10s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
#[serial]
#[ignore] // headful; run on the integration job or locally with Chrome
async fn frame_text_finds_frame_element_not_main_doc() {
    let mock = fixture().await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let frame = child_frame(&tab).await;

    let el = frame
        .find()
        .text("unique-frame-text")
        .one()
        .await
        .expect("frame.find().text() must find the frame's own button");
    assert_eq!(
        el.attr("id").await.unwrap().as_deref(),
        Some("frame-btn"),
        "text() scoped to the frame must resolve to the frame's button, not the main doc's"
    );

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
#[ignore]
async fn frame_xpath_finds_frame_element_not_main_doc() {
    let mock = fixture().await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let frame = child_frame(&tab).await;

    let el = frame
        .find()
        .xpath("//button")
        .one()
        .await
        .expect("frame.find().xpath() must find the frame's own button");
    assert_eq!(
        el.attr("id").await.unwrap().as_deref(),
        Some("frame-btn"),
        "xpath() scoped to the frame must resolve to the frame's button, not the main doc's"
    );

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
#[ignore]
async fn frame_text_regex_finds_frame_element_not_main_doc() {
    let mock = fixture().await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let frame = child_frame(&tab).await;

    // NOTE: the regex resolver (`resolve_text_regex_many`/`_one`) has no
    // "narrowest match" step (unlike the plain-substring `.text()` path),
    // so `[0]` in document order can land on an ancestor (`<body>`/`<html>`)
    // whose *rendered* `innerText` equals the leaf's — a separate,
    // pre-existing gap unrelated to frame `contextId` scoping. So this
    // test asserts on rendered *content* (which document the match came
    // from) rather than requiring the match to be the `<button>` node
    // itself.
    let el = frame
        .find()
        .text_regex(regex::Regex::new("unique-frame-text").unwrap())
        .one()
        .await
        .expect("frame.find().text_regex() must find a match in the frame's own document");
    let text = el.inner_text().await.unwrap();
    assert!(
        text.contains("unique-frame-text"),
        "text_regex() scoped to the frame must resolve within the frame's document; got: {text:?}"
    );
    assert!(
        !text.contains("unique-main-decoy-text"),
        "text_regex() scoped to the frame must NOT pick up the main document's content; got: {text:?}"
    );

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
#[ignore]
async fn frame_role_finds_frame_element_not_main_doc() {
    let mock = fixture().await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let frame = child_frame(&tab).await;

    let el = frame
        .find()
        .role(AriaRole::Button)
        .one()
        .await
        .expect("frame.find().role() must find the frame's own button");
    assert_eq!(
        el.attr("id").await.unwrap().as_deref(),
        Some("frame-btn"),
        "role() scoped to the frame must resolve to the frame's button, not the main doc's"
    );

    browser.close().await.unwrap();
}
