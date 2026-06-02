//! Demonstrates the browser-context HTTP API (`tab.request()`).
//!
//! `tab.request()` runs `fetch` inside the page, so the request inherits the
//! page's cookies and same-origin CORS rules. Use `.bypass_cors()` to route
//! through Chrome's privileged `Network.loadNetworkResource` path instead
//! (GET only; bypasses CORS).
//!
//! Sequence:
//!   1. Navigate to example.com so that cookies / origin context is
//!      established.
//!   2. Issue a GET to `https://httpbin.org/get` — prints status + truncated
//!      body.
//!   3. Issue a POST with a JSON body to `https://httpbin.org/post` — prints
//!      status + truncated body.
//!   4. Close the browser.
//!
//! No special cargo features required:
//! `cargo run --example browser_request`.

use serde_json::json;
use zendriver::Browser;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();

    // Navigate first so the page context (cookies / origin) is established.
    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    // ── GET ──────────────────────────────────────────────────────────────────
    let resp = tab.request().get("https://httpbin.org/get").send().await?;

    let body = resp.text()?;
    println!(
        "GET  status={} body_len={} snippet={:.80}",
        resp.status(),
        body.len(),
        body
    );

    // ── POST with JSON body ───────────────────────────────────────────────────
    let resp = tab
        .request()
        .post("https://httpbin.org/post")
        .json(&json!({"hello": "zendriver"}))?
        .send()
        .await?;

    let body = resp.text()?;
    println!(
        "POST status={} body_len={} snippet={:.80}",
        resp.status(),
        body.len(),
        body
    );

    browser.close().await?;
    Ok(())
}
