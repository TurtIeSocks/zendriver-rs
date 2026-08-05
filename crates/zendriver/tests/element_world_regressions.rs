//! Real-Chrome guards for the two element flows that a world change breaks.
//!
//! Both cases below shipped broken twice, and the mock unit suite was green
//! through both of them — by construction, not by oversight.
//! `MockConnection` replays a scripted frame sequence, so a test written
//! against the wrong sequence asserts the wrong sequence just as happily as
//! the right one. Neither defect is visible without a browser:
//!
//! 1. **Reads after a cross-document navigation.** Element reads were moved
//!    into the tab's isolated world, whose `executionContextId` is cached per
//!    tab. A navigation destroys that context; the cache was never
//!    invalidated, and the recovery path matched only `"Cannot find context"`
//!    — the wording `Runtime.evaluate` produces. `DOM.resolveNode` instead
//!    answers `-32000 "Node with given id does not belong to the document"`,
//!    so nothing recovered and every element API on the tab stayed broken for
//!    the rest of its life, from the second page onward.
//!
//! 2. **Actionability inside an iframe.** `Tab::ensure_isolated_world`
//!    creates exactly one world, for the tab's *main* frame.
//!    `getBoundingClientRect()` on a node inside an iframe is
//!    iframe-relative, while `document.elementFromPoint` evaluated in the
//!    main-frame world hit-tests the top document — so the two never agreed
//!    and the gate reported `NotActionable("occluded by overlay")` for every
//!    `click` / `hover` / `tap` on any element inside any iframe.
//!
//! Neither test asserts *which* world the JS runs in; that is an
//! implementation choice. They assert the observable behavior any world
//! choice has to preserve, so they stay meaningful when the isolated-world
//! move is re-landed with per-frame world resolution.
//!
//! Deliberately NOT `#[ignore]`d: the `test-integration` job runs
//! `-E 'kind(test)'` (non-ignored) on every PR with Chrome provisioned, while
//! `#[ignore]` would route these to `nightly-ignored-tests`, which carries
//! `continue-on-error: true` on a daily cron. These guard a regression that
//! already shipped twice, so they have to block a merge rather than yellow-dot
//! one. Both launch headless Chrome against a loopback fixture and finish in
//! ~3s combined.
//!
//! Running these locally: give `cargo` a target directory that no *other*
//! checkout of this same package shares. Cargo keys an integration-test
//! binary on package + target name, not on source path, so pointing two trees
//! at one `CARGO_TARGET_DIR` lets a run silently execute the binary the other
//! tree built. That reads as a clean pass of code you never compiled — it
//! produced exactly one false "both pass" while these tests were being
//! written against the pre-fix revision.
//!
//! Gated behind the `integration-tests` feature so CI can skip on
//! Chrome-less runners; CI exercises these on the integration job.

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::Browser;

/// Mount `html` at `at` on `mock`. These fixtures need several routes each
/// (a second page to navigate to, an iframe document to embed), so routes are
/// added one at a time rather than through a single-page helper.
async fn mount(mock: &MockServer, at: &str, html: &str) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(html.as_bytes().to_vec(), "text/html"),
        )
        .mount(mock)
        .await;
}

/// Every element API must keep working after a cross-document navigation.
///
/// `goto A → read → goto B → read` is about as ordinary as a scraper gets,
/// and it is the flow a per-tab context cache breaks: the first page looks
/// perfect, and everything after it fails. So the assertions here are on the
/// *second* and third pages, and they cover a read (`inner_text`), a
/// serialization (`outer_html`), an attribute lookup, a predicate
/// (`is_visible`) and a full gated action (`click`) — all four routes died
/// the same way, through the same funnel.
#[tokio::test]
#[serial]
async fn element_apis_survive_a_cross_document_navigation() {
    let mock = MockServer::start().await;
    mount(
        &mock,
        "/a",
        r#"<!doctype html><html><body>
             <div id="t" data-page="a">page A</div>
             <button id="b" onclick="this.textContent='clicked A'">press A</button>
           </body></html>"#,
    )
    .await;
    mount(
        &mock,
        "/b",
        r#"<!doctype html><html><body>
             <div id="t" data-page="b">page B</div>
             <button id="b" onclick="this.textContent='clicked B'">press B</button>
           </body></html>"#,
    )
    .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();

    // Page A: establishes whatever per-tab state the element path caches.
    tab.goto(&format!("{}/a", mock.uri())).await.unwrap();
    let el = tab.find().css("#t").one().await.unwrap();
    assert_eq!(el.inner_text().await.unwrap().trim(), "page A");

    // Page B is where a stale cache bites. Every one of these used to fail
    // with `-32000 "Node with given id does not belong to the document"`.
    tab.goto(&format!("{}/b", mock.uri())).await.unwrap();
    let el = tab.find().css("#t").one().await.unwrap();
    assert_eq!(
        el.inner_text().await.unwrap().trim(),
        "page B",
        "inner_text must still work after the first navigation"
    );
    assert!(
        el.outer_html().await.unwrap().contains("page B"),
        "outer_html must still work after the first navigation"
    );
    assert_eq!(el.attr("data-page").await.unwrap().as_deref(), Some("b"));
    assert!(
        el.is_visible().await.unwrap(),
        "is_visible must still work after the first navigation"
    );

    // The actionability gate reads through the same funnel, so a gated
    // action is the strongest single check that the whole path recovered.
    let button = tab.find().css("#b").one().await.unwrap();
    button.click().await.unwrap();
    assert_eq!(
        button.inner_text().await.unwrap().trim(),
        "clicked B",
        "a gated click must still land after the first navigation"
    );

    // A third hop, so a fix that merely survives one navigation cannot pass.
    tab.goto(&format!("{}/a", mock.uri())).await.unwrap();
    let el = tab.find().css("#t").one().await.unwrap();
    assert_eq!(el.inner_text().await.unwrap().trim(), "page A");
    assert_eq!(el.attr("data-page").await.unwrap().as_deref(), Some("a"));

    browser.close().await.unwrap();
}

/// Pointer actions must reach an element inside a same-origin iframe.
///
/// The iframe is deliberately offset from the top-left. That is the whole
/// point of the fixture: the button's own rect is iframe-relative, so a
/// hit-test performed against the TOP document at those same numbers lands on
/// the outer page's spacer instead of the button, and the gate rejects a
/// perfectly clickable element as occluded. An iframe pinned at (0, 0) would
/// hide the bug, since both coordinate spaces would agree.
#[tokio::test]
#[serial]
async fn hover_and_click_reach_an_element_inside_an_iframe() {
    let mock = MockServer::start().await;
    mount(
        &mock,
        "/outer",
        r#"<!doctype html><html><body style="margin:0">
             <div style="height:200px">spacer</div>
             <iframe src="/inner" style="position:absolute;top:200px;left:100px;
                     width:400px;height:300px;border:0"></iframe>
           </body></html>"#,
    )
    .await;
    mount(
        &mock,
        "/inner",
        r#"<!doctype html><html><body style="margin:0">
             <button id="go" style="width:200px;height:60px"
                     onmouseover="this.dataset.hovered='yes'"
                     onclick="this.textContent='clicked'">click me</button>
           </body></html>"#,
    )
    .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&format!("{}/outer", mock.uri())).await.unwrap();

    let button = tab
        .find()
        .css("#go")
        .include_frames()
        .one()
        .await
        .expect("the button inside the iframe must be findable");
    assert_eq!(button.inner_text().await.unwrap().trim(), "click me");

    // Both of these used to fail with NotActionable("occluded by overlay").
    // Asserting the DOM side effect, not just the absence of an error: a gate
    // that passes while the synthesized event lands somewhere else entirely
    // is still broken, just more quietly.
    button
        .hover()
        .await
        .expect("hovering an element inside an iframe must pass the actionability gate");
    assert_eq!(
        button.attr("data-hovered").await.unwrap().as_deref(),
        Some("yes"),
        "the hover must actually reach the button"
    );

    button
        .click()
        .await
        .expect("clicking an element inside an iframe must pass the actionability gate");
    assert_eq!(
        button.inner_text().await.unwrap().trim(),
        "clicked",
        "the click must actually land on the button"
    );

    browser.close().await.unwrap();
}
