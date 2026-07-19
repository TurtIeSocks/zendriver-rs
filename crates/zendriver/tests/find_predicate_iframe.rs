//! Nested-iframe traversal + predicate finder integration tests.
//!
//! Gated behind the `integration-tests` feature; CI exercises these on the
//! integration job where a real Chrome binary is available.
//!
//! The fixture/server pattern mirrors `integration_phase4.rs` exactly:
//! wiremock serves HTML at `/`, and additional paths are mounted inline when
//! a test needs multiple routes.

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::Browser;

/// Spin up a mock HTTP server that returns `html` at `/`.
async fn fixture_with_html(html: &str) -> MockServer {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(html.as_bytes().to_vec(), "text/html"),
        )
        .mount(&mock)
        .await;
    mock
}

// ---------------------------------------------------------------------------
// T8: Nested-iframe traversal
// ---------------------------------------------------------------------------

/// Verify that `find().css(sel).include_frames().one()` crosses two levels of
/// same-origin iframes to locate an element nested inside an iframe inside
/// another iframe.
///
/// Fixture layout:
///   `/` (outer) → embeds `/mid` via `<iframe src="/mid">`
///   `/mid`      → embeds `/inner` via `<iframe src="/inner">`
///   `/inner`    → contains `<div id="deep">found</div>`
///
/// All three pages are served from the same wiremock origin so Chrome keeps
/// them same-origin (single CDP session, no OOPIF).
#[tokio::test]
#[serial]
#[ignore] // headful; run on the integration job or locally with Chrome
async fn include_frames_finds_element_two_iframes_deep() {
    let mock = MockServer::start().await;

    // Innermost page — holds the target element.
    Mock::given(method("GET"))
        .and(path("/inner"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            br#"<!doctype html><html><body><div id="deep">found</div></body></html>"#.to_vec(),
            "text/html",
        ))
        .mount(&mock)
        .await;

    // Middle page — embeds inner.
    Mock::given(method("GET"))
        .and(path("/mid"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            br#"<!doctype html><html><body><iframe src="/inner"></iframe></body></html>"#.to_vec(),
            "text/html",
        ))
        .mount(&mock)
        .await;

    // Outer page — embeds mid.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            br#"<!doctype html><html><body><iframe id="mid" src="/mid"></iframe></body></html>"#
                .to_vec(),
            "text/html",
        ))
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Wait until at least two child frames register (mid + inner).
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frames = tab.frames().await.unwrap();
        let child_count = frames.iter().filter(|f| !f.is_main()).count();
        if child_count >= 2 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "expected at least 2 child frames within 10s; saw {} (total {})",
                child_count,
                frames.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let el = tab
        .find()
        .css("#deep")
        .include_frames()
        .one()
        .await
        .expect("element nested two iframes deep must be found with include_frames()");
    assert_eq!(
        el.inner_text().await.unwrap(),
        "found",
        "#deep should contain 'found'"
    );

    let all = tab
        .find_all()
        .css("#deep")
        .include_frames()
        .many()
        .await
        .unwrap();
    assert_eq!(all.len(), 1, "exactly one #deep element across all frames");

    browser.close().await.unwrap();
}

// ---------------------------------------------------------------------------
// T9: Predicate finds + select_all + mixing guard
// ---------------------------------------------------------------------------

/// Find an element using a combination of tag, attr, attr_regex, and text
/// predicates. Fixture: `<button class="primary active" data-id="4821">Buy now</button>`.
#[tokio::test]
#[serial]
#[ignore]
async fn predicate_finds_by_tag_attr_text() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <button class="primary active" data-id="4821">Buy now</button>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let el = tab
        .find()
        .tag("button")
        .attr_contains("class", "active")
        .attr_regex("data-id", r"^\d{4}$")
        .containing_text("Buy")
        .one()
        .await
        .unwrap();
    assert!(
        el.inner_text().await.unwrap().contains("Buy"),
        "button text should contain 'Buy'"
    );

    browser.close().await.unwrap();
}

/// Verify that `select_all` returns all matching elements.
/// Fixture: a `<ul>` with exactly 3 `<li>` items.
#[tokio::test]
#[serial]
#[ignore]
async fn select_all_returns_all_matches() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <ul id="list">
            <li>one</li>
            <li>two</li>
            <li>three</li>
          </ul>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let items = tab.select_all("ul li").await.unwrap();
    assert_eq!(items.len(), 3, "expected 3 <li> elements");

    browser.close().await.unwrap();
}

/// Regression test: `.text_regex()` must resolve to the innermost
/// matching element, not an ancestor. Chrome normalizes `innerText` such
/// that `<html>`/`<body>` — whose only text-bearing descendant is the
/// `<span>` — end up with an `innerText` identical to the leaf's, so an
/// un-narrowed regex filter matches all three and `.one()`'s `[0]`
/// (document order) picks the ancestor instead of the `<span>`. Assert
/// on the resolved element's tag name (a distinguishing property, not
/// just presence) to confirm it's the leaf.
#[tokio::test]
#[serial]
#[ignore] // headful; run on the integration job or locally with Chrome
async fn text_regex_resolves_to_innermost_leaf_not_ancestor() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body><div><span>match-me</span></div></body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let el = tab
        .find()
        .text_regex(regex::Regex::new("match-me").unwrap())
        .one()
        .await
        .expect("text_regex(\"match-me\") must find a match");

    let tag = el
        .evaluate::<String>("el.tagName.toLowerCase()")
        .await
        .unwrap();
    assert_eq!(
        tag, "span",
        "text_regex() must resolve to the innermost matching <span>, not an ancestor \
         (<div>/<body>/<html>) whose innerText happens to equal the leaf's"
    );

    browser.close().await.unwrap();
}

/// Case-insensitive predicate matchers (Phase 3, item 1): `.attr_i()` matches
/// an attribute value regardless of case (CSS `[name="value" i]`), and
/// `.containing_text_i()` matches element text regardless of case (JS
/// post-filter lowercasing both sides). Fixture: `<div class="Primary">` +
/// `<span>HELLO</span>`.
#[tokio::test]
#[serial]
#[ignore]
async fn predicate_finds_with_case_insensitive_attr_and_text() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <div class="Primary">box</div>
          <span>HELLO</span>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let div = tab
        .find()
        .tag("div")
        .attr_i("class", "primary")
        .one()
        .await
        .expect("attr_i(\"class\", \"primary\") must match class=\"Primary\" case-insensitively");
    assert_eq!(div.inner_text().await.unwrap(), "box");

    // `.tag("span")` narrows the CSS candidate set so the JS text
    // post-filter doesn't also match ancestors (`<html>`/`<body>`) whose
    // `innerText` includes the span's text — the same document-order
    // ambiguity `text_regex_resolves_to_innermost_leaf_not_ancestor` above
    // guards against.
    let span = tab
        .find()
        .tag("span")
        .containing_text_i("hello")
        .one()
        .await
        .expect("containing_text_i(\"hello\") must match text \"HELLO\" case-insensitively");
    let tag = span
        .evaluate::<String>("el.tagName.toLowerCase()")
        .await
        .unwrap();
    assert_eq!(tag, "span");

    browser.close().await.unwrap();
}

/// Mixing `.css()` and a predicate method on the same query must return
/// `Err(ZendriverError::ConflictingSelectors)`.
#[tokio::test]
#[serial]
#[ignore]
async fn mixing_predicate_and_css_errors() {
    let mock = fixture_with_html(r#"<!doctype html><html><body><div>x</div></body></html>"#).await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let err = tab.find().css("div").tag("span").one().await.unwrap_err();
    assert!(
        matches!(err, zendriver::ZendriverError::ConflictingSelectors),
        "expected ConflictingSelectors, got {err:?}"
    );

    browser.close().await.unwrap();
}
