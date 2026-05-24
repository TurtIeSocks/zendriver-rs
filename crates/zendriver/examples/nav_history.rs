//! Walk a tab through `goto A`, `goto B`, `back`, `forward`, `reload` — the
//! full P4 navigation-history surface.
//!
//! Demonstrates:
//!   - [`Tab::goto`] (push a new entry; main-frame-scoped).
//!   - [`Tab::back`] / [`Tab::forward`] (traverse the history stack via
//!     `Page.navigateToHistoryEntry`).
//!   - [`Tab::reload`] (same URL, fresh JS realm).
//!   - [`Tab::url`] (post-navigation read to confirm we landed where
//!     expected).
//!
//! Uses two `data:` URLs so the example is fully self-contained and the
//! starts-with assertions are unambiguous.

use zendriver::Browser;

const PAGE_A: &str = "data:text/html,<!doctype html><title>A</title><h1>page A</h1>";
const PAGE_B: &str = "data:text/html,<!doctype html><title>B</title><h1>page B</h1>";

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();

    tab.goto(PAGE_A).await?;
    tab.wait_for_load().await?;
    println!("after goto A:        {}", tab.url().await?);

    tab.goto(PAGE_B).await?;
    tab.wait_for_load().await?;
    println!("after goto B:        {}", tab.url().await?);

    tab.back().await?;
    tab.wait_for_load().await?;
    let url_back = tab.url().await?.to_string();
    println!("after back:          {url_back}");
    assert!(
        url_back.starts_with("data:text/html,") && url_back.contains("page%20A"),
        "back should land on A, got: {url_back}"
    );

    tab.forward().await?;
    tab.wait_for_load().await?;
    let url_fwd = tab.url().await?.to_string();
    println!("after forward:       {url_fwd}");
    assert!(
        url_fwd.contains("page%20B"),
        "forward should land on B, got: {url_fwd}"
    );

    // Reload should keep the URL but rebuild the JS realm.
    tab.evaluate_main::<serde_json::Value>("window.sentinel = 'pre-reload'; null")
        .await?;
    tab.reload().await?;
    tab.wait_for_load().await?;
    println!("after reload:        {}", tab.url().await?);

    let post_reload: Option<String> = tab
        .evaluate_main("typeof window.sentinel === 'undefined' ? null : window.sentinel")
        .await?;
    assert_eq!(
        post_reload, None,
        "reload should have wiped window.sentinel, got {post_reload:?}"
    );
    println!("window.sentinel after reload = {post_reload:?} (None = expected)");

    browser.close().await?;
    Ok(())
}
