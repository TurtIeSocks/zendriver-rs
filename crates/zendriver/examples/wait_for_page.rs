//! Port of `zendriver/examples/wait_for_page.py`, scoped to the P3 surface.
//!
//! The Python version uses `tab.expect_request(...)` /
//! `tab.expect_response(...)` to capture a specific network exchange. CDP
//! network interception lands in zendriver-rs P4
//! (`zendriver-interception`), so this port keeps the structural shape —
//! navigate, wait for the load event, read post-load state — and asserts
//! on the document instead of on a request id.
//!
//! Demonstrates [`Tab::wait_for_load`] (subscribes to
//! `Page.frameStoppedLoading` before navigation completes) + post-load
//! reads via [`Tab::url`] / [`Tab::title`] / [`Tab::find`].

use zendriver::Browser;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();
    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    let url = tab.url().await?;
    let title = tab.title().await?;
    println!("loaded {url} (title={title:?})");

    let h1 = tab.find().css("h1").one().await?;
    println!("h1 text: {}", h1.inner_text().await?);

    browser.close().await?;
    Ok(())
}
