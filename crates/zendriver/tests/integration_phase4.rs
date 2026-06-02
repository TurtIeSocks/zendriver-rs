//! Phase 4 end-to-end tests against real Chrome + wiremock.
//!
//! Each test serves a tiny HTML fixture via `wiremock`, launches a headless
//! Chrome via `Browser::builder()`, drives the new P4 surface (multi-tab,
//! cookies, storage, frames, nav history, network idle, traversal refresh),
//! and asserts on observable behavior via CDP or DOM state.
//!
//! Gated behind the `integration-tests` feature so CI can skip on
//! Chrome-less runners; CI exercises these on the integration job.

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::{Browser, Cookie, SameSite};

/// Spin up a mock HTTP server that returns `html` at `/`. Shared between
/// every test below — keeps fixtures inline + isolated.
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

#[tokio::test]
#[serial]
async fn new_tab_opens_second_tab() {
    // Launch starts with exactly one tab in the registry (the main tab).
    // `Browser::new_tab` issues `Target.createTarget` and waits for the
    // `TabRegistrar` observer to register the new session, so by the time
    // it resolves the registry must have grown to 2.
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    assert_eq!(browser.tab_count().await, 1);

    let _tab2 = browser.new_tab().await.unwrap();
    assert_eq!(
        browser.tabs().await.len(),
        2,
        "new_tab should add a second entry to the tab registry"
    );

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn tab_close_removes_from_registry() {
    // After opening a second tab + closing it, the registry should drop
    // back to the single main tab. The drop happens via the
    // `Target.detachedFromTarget` event the `TabRegistrar` observer
    // listens for, so there is a small async window between `Tab::close`
    // returning and the registry actually shrinking — poll briefly.
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab2 = browser.new_tab().await.unwrap();
    assert_eq!(browser.tab_count().await, 2);

    tab2.close().await.unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if browser.tab_count().await == 1 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("tab.close did not remove tab from registry within 5s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn cookies_set_and_all_roundtrip() {
    // Insert two cookies via the browser-scoped jar, then read them back
    // via `all()`. Order from Chrome is unspecified; assert membership by
    // name rather than position.
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let jar = browser.cookies();

    jar.set(Cookie {
        name: "alpha".into(),
        value: "1".into(),
        domain: "example.test".into(),
        path: "/".into(),
        expires: None,
        http_only: false,
        secure: false,
        same_site: Some(SameSite::Lax),
        url: None,
        ..Default::default()
    })
    .await
    .unwrap();
    jar.set(Cookie {
        name: "beta".into(),
        value: "2".into(),
        domain: "example.test".into(),
        path: "/".into(),
        expires: None,
        http_only: true,
        secure: false,
        same_site: None,
        url: None,
        ..Default::default()
    })
    .await
    .unwrap();

    let all = jar.all().await.unwrap();
    let names: std::collections::HashSet<_> = all.iter().map(|c| c.name.clone()).collect();
    assert!(names.contains("alpha"), "alpha cookie missing from all()");
    assert!(names.contains("beta"), "beta cookie missing from all()");

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn cookies_save_and_load_roundtrip() {
    // Save the browser cookie store to a tempfile, then clear + reload it
    // and assert the same cookies come back. The two-cookie set covers
    // both the SameSite::Lax and `same_site = None` paths through the
    // CDP camelCase boundary.
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let jar = browser.cookies();

    jar.set(Cookie {
        name: "saved_a".into(),
        value: "v1".into(),
        domain: "example.test".into(),
        path: "/".into(),
        expires: None,
        http_only: false,
        secure: false,
        same_site: Some(SameSite::Lax),
        url: None,
        ..Default::default()
    })
    .await
    .unwrap();
    jar.set(Cookie {
        name: "saved_b".into(),
        value: "v2".into(),
        domain: "example.test".into(),
        path: "/api".into(),
        expires: None,
        http_only: true,
        secure: false,
        same_site: None,
        url: None,
        ..Default::default()
    })
    .await
    .unwrap();

    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    jar.save_to_file(&path).await.unwrap();

    // Clear the jar and confirm the wipe took.
    jar.clear().await.unwrap();
    let after_clear = jar.all().await.unwrap();
    assert!(
        !after_clear.iter().any(|c| c.name == "saved_a"),
        "clear() should have removed saved_a"
    );

    // Hydrate from the tempfile and confirm both cookies are back.
    jar.load_from_file(&path).await.unwrap();
    let reloaded = jar.all().await.unwrap();
    let names: std::collections::HashSet<_> = reloaded.iter().map(|c| c.name.clone()).collect();
    assert!(
        names.contains("saved_a"),
        "saved_a missing after load_from_file"
    );
    assert!(
        names.contains("saved_b"),
        "saved_b missing after load_from_file"
    );

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn local_storage_set_get_clear() {
    // Set two keys, read them back, then clear + assert empty. Storage
    // is per-origin so a real navigation is required before any op
    // (DOMStorage rejects requests on `about:blank` because it has no
    // tangible origin).
    let mock =
        fixture_with_html(r#"<!doctype html><html><body><div id="d">x</div></body></html>"#).await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let storage = tab.local_storage();
    storage.set("theme", "dark").await.unwrap();
    storage.set("lang", "en").await.unwrap();

    let theme = storage.get("theme").await.unwrap();
    assert_eq!(theme.as_deref(), Some("dark"));
    let lang = storage.get("lang").await.unwrap();
    assert_eq!(lang.as_deref(), Some("en"));

    storage.clear().await.unwrap();
    let all = storage.get_all().await.unwrap();
    assert!(all.is_empty(), "storage should be empty after clear()");

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn back_forward_navigation_history() {
    // Goto A, goto B, back should land on A, forward should land on B.
    // Two distinct fixtures via two MockServer instances so the URLs
    // differ unambiguously.
    let mock_a =
        fixture_with_html(r#"<!doctype html><html><body><div id="a">A</div></body></html>"#).await;
    let mock_b =
        fixture_with_html(r#"<!doctype html><html><body><div id="b">B</div></body></html>"#).await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock_a.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();
    tab.goto(&mock_b.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    tab.back().await.unwrap();
    tab.wait_for_load().await.unwrap();
    let url_after_back = tab.url().await.unwrap().to_string();
    assert!(
        url_after_back.starts_with(&mock_a.uri()),
        "back should land on A, got: {url_after_back}"
    );

    tab.forward().await.unwrap();
    tab.wait_for_load().await.unwrap();
    let url_after_fwd = tab.url().await.unwrap().to_string();
    assert!(
        url_after_fwd.starts_with(&mock_b.uri()),
        "forward should land on B, got: {url_after_fwd}"
    );

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn reload_dispatches_page_reload() {
    // Reload should re-execute the page; assert by writing a sentinel
    // into the main world, reloading, and confirming the sentinel is gone
    // because the JS environment was thrown away with the document.
    let mock =
        fixture_with_html(r#"<!doctype html><html><body><div id="d">x</div></body></html>"#).await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    tab.evaluate_main::<serde_json::Value>("window.sentinel = 'pre-reload'; null")
        .await
        .unwrap();
    let before: String = tab.evaluate_main("window.sentinel").await.unwrap();
    assert_eq!(before, "pre-reload");

    tab.reload().await.unwrap();
    tab.wait_for_load().await.unwrap();

    // After a real reload the JS realm is fresh; window.sentinel must
    // be `undefined`, which our null-coalescing eval surfaces as None.
    let after: Option<String> = tab
        .evaluate_main("typeof window.sentinel === 'undefined' ? null : window.sentinel")
        .await
        .unwrap();
    assert_eq!(after, None, "reload should have cleared window.sentinel");

    // Sanity-check the DOM is still present (so we didn't just navigate
    // away to a blank page).
    let id: Option<String> = tab
        .find()
        .css("#d")
        .one()
        .await
        .unwrap()
        .attr("id")
        .await
        .unwrap();
    assert_eq!(id.as_deref(), Some("d"));

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn wait_for_idle_on_spa_with_delayed_xhr() {
    // Fixture fires an XHR ~300ms after the inline script runs.
    // `wait_for_idle` should observe the request, wait for it to complete,
    // then require its quiet window before resolving. The request is timed to
    // fire well inside the (widened) quiet window so the catch is not a race;
    // see the `wait_for_idle_with` call below for the timing rationale.
    //
    // Use a single mock server for both the page and the data endpoint so
    // the fetch is same-origin — a cross-origin fetch from a separate
    // wiremock instance would be CORS-blocked and `window.x` would never
    // be set, failing the read-back assertion below for an unrelated
    // reason.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/data"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                r#"<!doctype html><html><body>
          <script>
            setTimeout(() => {
              fetch("/data").then(r => r.text()).then(t => { window.x = t; });
            }, 300);
          </script>
        </body></html>"#
                    .as_bytes()
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;
    let mock_page = mock;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock_page.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let start = std::time::Instant::now();
    // Use an explicit 800ms quiet window (vs the 500ms default). The fixture
    // delays its XHR ~300ms, so the request fires roughly in the *middle* of
    // the window — leaving ~500ms of slack before the window would otherwise
    // close. With the old 500ms-delay / 500ms-window pairing the request
    // landed within ~30ms of the window closing, so any CDP event lag or
    // timer jitter (common under CI load) let `wait_for_idle` close the
    // window before the request registered and it returned ~500ms early.
    tab.wait_for_idle_with(Duration::from_secs(30), Duration::from_millis(800))
        .await
        .unwrap();
    let elapsed = start.elapsed();

    let x: String = tab.evaluate_main("window.x || ''").await.unwrap();
    // Primary, *causal* assertion: the XHR result is only set after the
    // delayed request completes. Reading it after `wait_for_idle` returns
    // proves idle detection observed the request and waited for it to finish
    // — had it returned at the first quiet window (before the XHR fired),
    // `window.x` would still be unset. This check does not depend on timing,
    // so it does not flake.
    assert_eq!(x, "ok", "XHR result should be present once idle resolves");

    // Sanity floor: confirm a quiet window was actually enforced *after* the
    // request (i.e. idle didn't resolve the instant the XHR finished). Kept
    // well below the ~1.13s expected elapsed (~300ms delay + 800ms window) so
    // scheduler jitter never trips it; the assertion above is what proves the
    // delayed request was awaited.
    assert!(
        elapsed >= Duration::from_millis(600),
        "wait_for_idle returned too early ({elapsed:?}); expected delayed XHR + quiet window"
    );

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn frame_find_inside_iframe() {
    // Fixture page hosts an iframe with `srcdoc` content. After load the
    // tab's frame registry should contain the main frame + the iframe;
    // querying inside the child frame must locate its scoped element.
    //
    // `srcdoc` keeps the iframe same-origin so it routes through the
    // standard same-session frame path (not the OOPIF observer chain).
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <iframe id="f" srcdoc="<button id='b'>x</button>"></iframe>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // The iframe registers via a `Page.frameAttached` event the lifecycle
    // subscriber consumes asynchronously — poll briefly until the
    // registry shows the second frame.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let child = loop {
        let frames = tab.frames().await.unwrap();
        if let Some(child) = frames.into_iter().find(|f| !f.is_main()) {
            break child;
        }
        if std::time::Instant::now() >= deadline {
            panic!("iframe never registered as a child frame within 5s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // The child frame's document holds the `<button id="b">`; querying
    // through the frame must locate it. The main tab's document does NOT
    // contain that button — confirms the query is frame-scoped.
    let btn = child.find().css("#b").one().await.unwrap();
    let id: Option<String> = btn.attr("id").await.unwrap();
    assert_eq!(id.as_deref(), Some("b"));

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn traversal_refresh_survives_reload() {
    // Find an element + its parent traversal, then `location.reload()`
    // and confirm the parent traversal still resolves — the chain
    // refresh re-runs the original selector against the fresh document.
    let mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <div id="root"><span id="child">x</span></div>
        </body></html>"#,
    )
    .await;
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let child = tab.find().css("#child").one().await.unwrap();

    // Trigger a real reload, then wait for the new document so the
    // original element handles are stale.
    tab.evaluate_main::<serde_json::Value>("location.reload(); null")
        .await
        .ok(); // reload tears down the eval context — error is fine.
    tab.wait_for_load().await.unwrap();

    // `parent()` carries a `Traversal { Parent }` origin whose ancestor
    // is the `Query/TabMain { Css("#child"), nth=0 }` origin. After the
    // reload the cached backend/remote ids are stale; auto-refresh
    // re-runs `document.querySelectorAll("#child")` against the fresh
    // document and re-traverses `this.parentElement`.
    let parent = child.parent().await.unwrap().expect("#child has a parent");
    let parent_id: Option<String> = parent.attr("id").await.unwrap();
    assert_eq!(
        parent_id.as_deref(),
        Some("root"),
        "traversal parent should still resolve to #root after reload"
    );

    browser.close().await.unwrap();
}
