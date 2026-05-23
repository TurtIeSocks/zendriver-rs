//! Phase 3 end-to-end tests against real Chrome + wiremock.
//!
//! Each test serves a tiny HTML fixture via `wiremock`, launches a headless
//! Chrome via `Browser::builder()`, drives the new P3 selector + action +
//! actionability + isolated-eval surface, and asserts on observable DOM
//! state via `evaluate_main` / `evaluate`.
//!
//! Gated behind the `integration-tests` feature so CI can skip on
//! Chrome-less runners; CI exercises these on the integration job.

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use std::io::Write;
use std::time::Duration;

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::error::ZendriverError;
use zendriver::{AriaRole, Browser};

/// Spin up a mock HTTP server that returns `html` at `/`. Shared between
/// every test below — keeps fixtures inline + isolated.
async fn fixture_with_html(html: &str) -> MockServer {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .mount(&mock)
        .await;
    mock
}

#[tokio::test]
#[serial]
async fn click_triggers_dom_event() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <button id="b" onclick="window.clicked = true">x</button>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let btn = tab.find().css("#b").one().await.unwrap();
    btn.click().await.unwrap();

    // `onclick` writes to the page's main world; default `evaluate` is
    // isolated, so read via `evaluate_main`.
    let clicked: bool = tab.evaluate_main("window.clicked").await.unwrap();
    assert!(clicked, "button click should have set window.clicked");

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn type_text_fills_input() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <input id="x" />
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let input = tab.find().css("#x").one().await.unwrap();
    input.type_text("hello").await.unwrap();

    let value: String = tab
        .evaluate_main("document.getElementById('x').value")
        .await
        .unwrap();
    assert_eq!(value, "hello");

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn hover_triggers_mouseover_event() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <div id="h" style="width:200px;height:200px;background:#eee"
               onmouseover="window.hovered = true">hover me</div>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let el = tab.find().css("#h").one().await.unwrap();
    el.hover().await.unwrap();

    let hovered: bool = tab.evaluate_main("!!window.hovered").await.unwrap();
    assert!(hovered, "hover should have fired the mouseover handler");

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn scroll_into_view_scrolls_deep_child() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <div style="height:5000px">
            <span id="bottom" style="position:absolute;top:4900px">x</span>
          </div>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let el = tab.find().css("#bottom").one().await.unwrap();
    el.scroll_into_view().await.unwrap();

    // After scroll_into_view (block:'center'), the element's top should be
    // within the viewport (0 .. innerHeight).
    let in_viewport: bool = tab
        .evaluate_main(
            "(() => { \
               const r = document.getElementById('bottom').getBoundingClientRect(); \
               return r.top >= 0 && r.top <= window.innerHeight; \
             })()",
        )
        .await
        .unwrap();
    assert!(
        in_viewport,
        "scroll_into_view should bring #bottom into the viewport"
    );

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn upload_files_sets_input_files() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <input type="file" id="f" />
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Write a tiny temp file — `DOM.setFileInputFiles` wants an absolute
    // path that exists on disk; tempfile gives us that with cleanup on drop.
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "hello").unwrap();
    let path = tmp.path().to_path_buf();

    let el = tab.find().css("#f").one().await.unwrap();
    el.upload_files(&[&path]).await.unwrap();

    let count: f64 = tab
        .evaluate_main("document.getElementById('f').files.length")
        .await
        .unwrap();
    assert_eq!(count as i64, 1, "files.length should be 1 after upload");

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn not_actionable_on_display_none_returns_error() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <button id="b" style="display:none" onclick="window.clicked=true">x</button>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Shorten the actionability poll deadline indirectly: the find() poll
    // succeeds (element exists in the DOM); the click()'s 5s actionability
    // gate is the wait we expect to hit. Wrap in tokio::time::timeout to
    // bound the test if the gate ever regresses to wait-forever.
    let btn = tab.find().css("#b").one().await.unwrap();
    let res = tokio::time::timeout(Duration::from_secs(8), btn.click()).await;

    match res {
        Ok(Err(ZendriverError::NotActionable(_, reason))) => {
            assert!(
                reason.contains("visible") || reason.contains("display"),
                "expected reason to mention visibility; got: {reason}"
            );
        }
        Ok(Err(other)) => panic!("expected NotActionable, got: {other:?}"),
        Ok(Ok(())) => panic!("expected NotActionable, click succeeded"),
        Err(_) => panic!("test timed out — actionability gate did not bound itself"),
    }

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn isolated_eval_does_not_see_page_globals() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <div id="d">x</div>
          <script>window.evil = "should not be visible";</script>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let el = tab.find().css("#d").one().await.unwrap();
    // Element::evaluate runs in the tab's isolated world. window.evil
    // should not be reachable from there — assert we see `null`.
    let v: Option<String> = el
        .evaluate("typeof window.evil === 'undefined' ? null : window.evil")
        .await
        .unwrap();
    assert_eq!(v, None, "isolated world should NOT see window.evil");

    // Sanity: main world DOES see it. Confirms the fixture script ran.
    let v: String = tab.evaluate_main("window.evil").await.unwrap();
    assert_eq!(v, "should not be visible");

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn xpath_finds_nested_element() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <div id="foo">
            <span>first</span>
            <span id="target">second</span>
          </div>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let el = tab
        .find()
        .xpath(r#"//div[@id="foo"]/span[2]"#)
        .one()
        .await
        .unwrap();
    let id: Option<String> = el.attr("id").await.unwrap();
    assert_eq!(id.as_deref(), Some("target"));

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn text_selector_case_insensitive() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <button id="s">Submit</button>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Lower-case needle should match capitalized button text.
    let el = tab.find().text("submit").one().await.unwrap();
    let id: Option<String> = el.attr("id").await.unwrap();
    assert_eq!(id.as_deref(), Some("s"));

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn aria_role_finds_button() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <div role="button" id="r" tabindex="0">go</div>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // role() compiles to `[role="button"]` — should locate our <div>.
    let el = tab.find().role(AriaRole::Button).one().await.unwrap();
    let id: Option<String> = el.attr("id").await.unwrap();
    assert_eq!(id.as_deref(), Some("r"));

    browser.close().await.unwrap();
}
