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
//!
//! The bottom three tests (increment 2) cover `visible_only` combined with
//! `include_frames()` — the cross-frame fan-out paths (`one_across_frames`,
//! `many_across_frames`, `consider_scope_best`) ignored `visible_only`
//! entirely until this change.

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::{Browser, Tab};

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

/// Poll `tab.frames()` until at least one non-main frame has registered
/// (mirrors `find_predicate_iframe.rs`'s wait loop) so `include_frames()`
/// queries have a same-origin iframe document to descend into.
async fn wait_for_child_frame(tab: &Tab) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frames = tab.frames().await.unwrap();
        if frames.iter().any(|f| !f.is_main()) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("expected at least 1 child frame within 10s; saw {frames:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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

// ---------------------------------------------------------------------------
// Cross-frame fan-out: `include_frames()` + `visible_only` (increment 2)
// ---------------------------------------------------------------------------

/// `include_frames().visible_only(true).one()` must skip a hidden match in
/// the main document and return the first VISIBLE match, even when that
/// means crossing into a same-origin iframe. Before this fix, the
/// cross-frame "first hit wins" branch ignores `visible_only` entirely and
/// returns the hidden main-document div.
///
/// Fixture: main page has `#main-hidden.cross` (`display:none`); its lone
/// iframe (`/frame`) has `#frame-visible.cross` (plainly visible).
#[tokio::test]
#[serial]
#[ignore] // headful; run on the integration job or locally with Chrome
async fn include_frames_visible_only_one_skips_hidden_main_for_visible_frame_match() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/frame"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                br#"<!doctype html><html><body>
              <div class="cross" id="frame-visible">frame visible</div>
            </body></html>"#
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                br#"<!doctype html><html><body>
              <div class="cross" id="main-hidden" style="display:none">main hidden</div>
              <iframe src="/frame"></iframe>
            </body></html>"#
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
    wait_for_child_frame(&tab).await;

    let el = tab
        .find()
        .css(".cross")
        .include_frames()
        .visible_only(true)
        .one()
        .await
        .expect("visible_only should cross into the frame for the visible match");
    let id = el.attr("id").await.unwrap().unwrap_or_default();
    assert_eq!(
        id, "frame-visible",
        "include_frames().visible_only(true).one() must skip the hidden main-document div \
         and return the visible frame div, not stop at the first (hidden) DOM hit"
    );

    browser.close().await.unwrap();
}

/// `include_frames().visible_only(true).many()` must return exactly the
/// VISIBLE set across main + frame, concatenated main-first — mirroring
/// `many_across_frames`'s existing (unfiltered) ordering guarantee.
///
/// Fixture: main page has one hidden + one visible `.cross`; its iframe
/// (`/frame`) also has one hidden + one visible `.cross`.
#[tokio::test]
#[serial]
#[ignore] // headful; run on the integration job or locally with Chrome
async fn include_frames_visible_only_many_returns_visible_set_main_first() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/frame"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                br#"<!doctype html><html><body>
              <div class="cross" id="frame-hidden" style="display:none">frame hidden</div>
              <div class="cross" id="frame-visible">frame visible</div>
            </body></html>"#
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                br#"<!doctype html><html><body>
              <div class="cross" id="main-hidden" style="display:none">main hidden</div>
              <div class="cross" id="main-visible">main visible</div>
              <iframe src="/frame"></iframe>
            </body></html>"#
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
    wait_for_child_frame(&tab).await;

    let all = tab
        .find_all()
        .css(".cross")
        .include_frames()
        .visible_only(true)
        .many()
        .await
        .expect("visible_only should find the visible set across main + frame");
    assert_eq!(
        all.len(),
        2,
        "expected exactly the 2 visible .cross elements (1 main + 1 frame)"
    );
    let mut ids = Vec::new();
    for el in &all {
        ids.push(el.attr("id").await.unwrap().unwrap_or_default());
    }
    assert_eq!(
        ids,
        vec!["main-visible", "frame-visible"],
        "main-first ordering must be preserved after visibility filtering"
    );

    browser.close().await.unwrap();
}

/// `best_match()` + `visible_only(true)` + `include_frames()`: visibility
/// filtering must happen BEFORE the closest-text-length ranking. Fixture:
/// the main document has a HIDDEN `.cross` whose text length is closest to
/// the needle, and a VISIBLE `.cross` whose text is noticeably longer
/// (farther from the needle length); the harmless same-origin iframe
/// (`/frame`) has no matching text at all, so it only exercises the
/// cross-frame fan-out's frame-loop code path without contributing a
/// candidate.
///
/// (Both length-competing candidates are kept in the SAME — main — scope
/// deliberately: `consider_scope_best`/`resolve_many_inner` is exercised
/// per-scope regardless, and this sidesteps an unrelated, pre-existing gap
/// where `resolve_text_many` never threads `contextId` into its
/// `Runtime.evaluate` calls the way `resolve_css_many` /
/// `resolve_predicate_many` do — so a *text* selector scoped to a child
/// frame does not reliably evaluate against that frame's own document.
/// That gap is orthogonal to `visible_only` and out of scope for this fix;
/// flagged separately rather than papered over here.)
///
/// Before this fix, `consider_scope_best` ranks by length only and the
/// hidden, closer match wins; after the fix the hidden candidate is
/// filtered out before the `nth` pick and the visible, farther candidate
/// is returned.
#[tokio::test]
#[serial]
#[ignore] // headful; run on the integration job or locally with Chrome
async fn include_frames_best_match_visible_only_prefers_visible_over_closer_length() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/frame"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            br#"<!doctype html><html><body><p>no match here</p></body></html>"#.to_vec(),
            "text/html",
        ))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                br#"<!doctype html><html><body>
              <div class="cross" id="hidden-close" style="display:none">target!</div>
              <div class="cross" id="visible-far">target with a lot of extra padding text here</div>
              <iframe src="/frame"></iframe>
            </body></html>"#
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
    wait_for_child_frame(&tab).await;

    let el = tab
        .find()
        .text("target")
        .best_match()
        .include_frames()
        .visible_only(true)
        .one()
        .await
        .expect("visible_only should filter out the closer-length hidden match");
    let id = el.attr("id").await.unwrap().unwrap_or_default();
    assert_eq!(
        id, "visible-far",
        "best_match + visible_only must filter to VISIBLE candidates before length-ranking: \
         the hidden #hidden-close is closer in length to \"target\" but must be excluded, \
         leaving the farther but VISIBLE #visible-far as the winner"
    );

    browser.close().await.unwrap();
}
