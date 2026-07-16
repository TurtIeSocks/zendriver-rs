//! Integration coverage for the `visible_only` find modifier (T16 wiring).
//!
//! `FindBuilder`/`FindAllBuilder` expose `.visible_only()`, but until this
//! change it was a silent NO-OP — the poll loop never consulted
//! `actionability::check_visible`. These tests drive a real headless
//! Chrome (matches `find_predicate_iframe.rs`'s pattern) so the visibility
//! probe's JS round-trip actually runs.
//!
//! Gated behind the `integration-tests` feature; CI exercises these on the
//! integration job where a real Chrome binary is available.

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

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

/// `.visible_only().one()` skips a hidden sibling matching the same
/// selector and returns the visible one; `.visible_only().many()` returns
/// exactly the visible set.
///
/// Fixture: two `.item` divs sharing a selector — the first is
/// `display:none`, the second is plainly visible.
#[tokio::test]
#[serial]
#[ignore] // headful; run on the integration job or locally with Chrome
async fn visible_only_filters_out_display_none_candidates() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <div class="item" id="hidden" style="display:none">hidden</div>
          <div class="item" id="shown">shown</div>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let el = tab
        .find()
        .css(".item")
        .visible_only(true)
        .one()
        .await
        .expect("visible_only should still find the visible sibling");
    let id = el.attr("id").await.unwrap().unwrap_or_default();
    assert_eq!(id, "shown", "visible_only().one() must skip the hidden div");

    let all = tab
        .find_all()
        .css(".item")
        .visible_only(true)
        .many()
        .await
        .expect("visible_only should find at least the visible one");
    assert_eq!(
        all.len(),
        1,
        "visible_only().many() must exclude the hidden div"
    );
    let all_id = all[0].attr("id").await.unwrap().unwrap_or_default();
    assert_eq!(all_id, "shown");

    browser.close().await.unwrap();
}

/// nth-of-visible: with 3 `.item` matches where the middle one is
/// `display:none`, `.visible_only().nth(1)` must return the THIRD DOM
/// element (the second *visible* one) — proving `nth` counts only visible
/// candidates, not raw DOM position.
#[tokio::test]
#[serial]
#[ignore] // headful; run on the integration job or locally with Chrome
async fn visible_only_nth_counts_only_visible_candidates() {
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <div class="item" id="first">first</div>
          <div class="item" id="second" style="display:none">second</div>
          <div class="item" id="third">third</div>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let el = tab
        .find()
        .css(".item")
        .visible_only(true)
        .nth(1)
        .one()
        .await
        .expect("nth(1) among visible candidates should resolve");
    let id = el.attr("id").await.unwrap().unwrap_or_default();
    assert_eq!(
        id, "third",
        "visible_only().nth(1) must count visible-only rank, landing on #third (the DOM's 3rd element, 2nd visible one), not #second (raw DOM nth(1))"
    );

    browser.close().await.unwrap();
}
