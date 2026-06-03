//! Headful integration: a listed host is blocked (net::ERR_BLOCKED_BY_CLIENT)
//! while an unlisted host loads. Needs a local Chrome + outbound network.
//!
//! Run with:
//! ```sh
//! cargo test -p zendriver --test tracker_blocklist_integration \
//!     --features integration-tests -- --ignored
//! ```
#![cfg(feature = "integration-tests")]

use serial_test::serial;
use zendriver::Browser;

// Fetches `url` from the page and reports "ok" or "blocked" — a blocked
// request rejects the fetch promise with a TypeError.
const FETCH_PROBE: &str = r#"(async (u) => {
    try {
        await fetch(u, { mode: 'no-cors', cache: 'no-store' });
        return 'ok';
    } catch (e) {
        return 'blocked:' + e;
    }
})"#;

#[tokio::test]
#[serial]
#[ignore] // run with: cargo test ... -- --ignored
async fn listed_host_is_blocked_unlisted_loads() {
    // Inline custom blocklist — deterministic, no dependence on bundled-list
    // contents. example.com is the unlisted control; example.org is "blocked".
    let browser = Browser::builder()
        .tracker_blocklist_add(["example.org".to_string()])
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("https://example.com/").await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Listed host -> blocked.
    let blocked: String = tab
        .evaluate::<String>(&format!("({FETCH_PROBE})('https://example.org/')"))
        .await
        .unwrap();
    assert!(
        blocked.starts_with("blocked:"),
        "listed host should be blocked, got: {blocked}"
    );

    // Unlisted host -> loads (a same-origin/CORS-opaque fetch resolves).
    let ok: String = tab
        .evaluate::<String>(&format!("({FETCH_PROBE})('https://example.com/')"))
        .await
        .unwrap();
    assert_eq!(ok, "ok", "unlisted host should load, got: {ok}");

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
#[ignore]
async fn subdomain_of_listed_host_is_blocked() {
    let browser = Browser::builder()
        .tracker_blocklist_add(["example.org".to_string()])
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("https://example.com/").await.unwrap();
    tab.wait_for_load().await.unwrap();

    let blocked: String = tab
        .evaluate::<String>(&format!("({FETCH_PROBE})('https://www.example.org/')"))
        .await
        .unwrap();
    assert!(
        blocked.starts_with("blocked:"),
        "subdomain of a listed host should be blocked, got: {blocked}"
    );

    browser.close().await.unwrap();
}
