//! Demonstrates the P5 interception API by blocking any subresource whose
//! URL matches `*/ads/*`, then navigating to a real page.
//!
//! Sequence:
//!   1. Build a [`Browser`] in headless mode.
//!   2. Register a `block` rule on the main tab via the [`InterceptBuilder`]
//!      fluent API; `start()` spawns the per-tab actor that drives
//!      `Fetch.enable` + `Fetch.continueRequest` / `Fetch.failRequest` in
//!      the background. Bind the returned [`InterceptHandle`] — its `Drop`
//!      tears the actor down, so letting it go out of scope mid-flow would
//!      silently disable interception.
//!   3. `goto` + `wait_for_load` example.com. The page itself doesn't load
//!      anything under `/ads/`, so no rule fires; the example is here to
//!      show the *shape* of the API, not a hit count. Adapt the URL and
//!      pattern to whatever you actually want to block.
//!   4. Print the page title to prove the navigation succeeded with the
//!      actor in the loop.
//!
//! Requires the `interception` cargo feature:
//! `cargo run --example intercept_block_ads --features interception`.

use zendriver::Browser;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();

    // `start()` returns an `InterceptHandle`; binding it keeps the actor
    // alive. Letting it drop would tear interception down.
    let _intercept = tab.intercept().block("*/ads/*")?.start();

    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    let title = tab.title().await?;
    println!("title = {title:?}");

    browser.close().await?;
    Ok(())
}
