//! Smoke for [`BrowserContext`] per-context proxy isolation.
//!
//! Two [`BrowserContext`]s share the same upstream rotating proxy. Each is
//! built via [`Browser::browser_context`]'s
//! [`BrowserContextBuilder`](zendriver::BrowserContextBuilder), which threads
//! a userinfo-free `proxyServer` / `proxyBypassList` into
//! `Target.createBrowserContext` and registers the embedded `user:pass` as
//! that context's proxy credentials — so every tab opened in the context is
//! transparently authenticated, with no per-tab `InterceptHandle` to hold.
//!
//! With a rotating upstream, two GETs to `https://ipv4.webshare.io/` from
//! distinct contexts should produce two distinct exit IPs most runs. The
//! same IP twice can happen by luck (the upstream picked the same exit
//! twice in a row); the warning at the bottom flags that case but does not
//! treat it as a hard failure.
//!
//! Requires Chrome installed. Run with:
//! ```sh
//! ZD_PROXY="http://user:pass@p.webshare.io:80" \
//!   cargo run --example browser_context_isolation --features interception
//! ```

use std::time::Duration;
use zendriver::Browser;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let proxy = std::env::var("ZD_PROXY")
        .expect("Set ZD_PROXY=http://user:pass@host:port (rotating proxy recommended)");

    let browser = Browser::builder().headless(true).launch().await?;

    // --- context 1 -------------------------------------------------------
    // `.proxy()` auto-splits embedded `user:pass` into per-context proxy
    // credentials and strips them from the `proxyServer` sent to Chrome;
    // `build()` registers those credentials so the auth actor is installed
    // automatically on every tab this context opens.
    let ctx1 = browser
        .browser_context()
        .proxy(&proxy)
        .proxy_bypass("<-loopback>")
        .build()
        .await?;
    let tab1 = ctx1.new_tab().await?;
    tab1.goto("https://ipv4.webshare.io/").await?;
    tab1.wait_for_load().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let ip1: String = tab1.evaluate("document.body.textContent.trim()").await?;
    println!("context 1 exit IP = {ip1}");

    // --- context 2 -------------------------------------------------------
    let ctx2 = browser
        .browser_context()
        .proxy(&proxy)
        .proxy_bypass("<-loopback>")
        .build()
        .await?;
    let tab2 = ctx2.new_tab().await?;
    tab2.goto("https://ipv4.webshare.io/").await?;
    tab2.wait_for_load().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let ip2: String = tab2.evaluate("document.body.textContent.trim()").await?;
    println!("context 2 exit IP = {ip2}");

    if ip1 == ip2 {
        eprintln!(
            "WARNING: both contexts returned the same IP. Rotation may not be working, OR \
             the upstream gave the same exit twice in a row. Re-run a few times to confirm."
        );
    } else {
        println!("OK: contexts have isolated proxies (rotation observed).");
    }

    // Drop the contexts before tearing down the browser so the background
    // `Target.disposeBrowserContext` calls scheduled by `BrowserContext`'s
    // `Drop` have a live connection to ride out on.
    drop(ctx1);
    drop(ctx2);
    tokio::time::sleep(Duration::from_millis(500)).await;
    browser.close().await?;
    Ok(())
}
