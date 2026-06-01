//! Smoke for [`BrowserContext`] per-context proxy isolation.
//!
//! Two [`BrowserContext`]s share the same upstream rotating proxy. Each is
//! wired to that proxy via [`Browser::create_browser_context_with`], which
//! threads `proxyServer` / `proxyBypassList` into
//! `Target.createBrowserContext` — so every tab opened in the context is
//! transparently routed through the upstream without the
//! single-process `--proxy-server` flag the older `proxy_auth` example uses.
//!
//! With a rotating upstream, two GETs to `https://ipv4.webshare.io/` from
//! distinct contexts should produce two distinct exit IPs most runs. The
//! same IP twice can happen by luck (the upstream picked the same exit
//! twice in a row); the warning at the bottom flags that case but does not
//! treat it as a hard failure.
//!
//! Per-tab `handle_auth` (rather than [`BrowserBuilder::proxy_auth`]) is
//! used because the proxy is bound to the *context*, not the launch — each
//! new context's tab needs its own `Fetch.authRequired` handler installed.
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

    let (proxy_host_port, user, pass) = split_proxy(&proxy);

    // --- context 1 -------------------------------------------------------
    let ctx1 = browser
        .create_browser_context_with(Some(&proxy_host_port), Some("<-loopback>"))
        .await?;
    let tab1 = ctx1.new_tab().await?;
    // `start()` is sync and returns an `InterceptHandle`; bind it so the
    // actor stays alive for the lifetime of the tab. Dropping the handle
    // tears interception down silently.
    let _auth1 = if let (Some(u), Some(p)) = (user.as_deref(), pass.as_deref()) {
        Some(tab1.intercept().handle_auth(u, p).start())
    } else {
        None
    };
    tab1.goto("https://ipv4.webshare.io/").await?;
    tab1.wait_for_load().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let ip1: String = tab1.evaluate("document.body.textContent.trim()").await?;
    println!("context 1 exit IP = {ip1}");

    // --- context 2 -------------------------------------------------------
    let ctx2 = browser
        .create_browser_context_with(Some(&proxy_host_port), Some("<-loopback>"))
        .await?;
    let tab2 = ctx2.new_tab().await?;
    let _auth2 = if let (Some(u), Some(p)) = (user.as_deref(), pass.as_deref()) {
        Some(tab2.intercept().handle_auth(u, p).start())
    } else {
        None
    };
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

/// Split a proxy URL `scheme://[user[:pass]@]host:port[/...]` into
/// `(scheme://host:port, Some(user), Some(pass))`.
///
/// Chrome's `--proxy-server` flag (and CDP's `proxyServer` field) want the
/// host/port without credentials; the credentials are answered separately
/// via `Fetch.authRequired` from the interception actor.
fn split_proxy(url: &str) -> (String, Option<String>, Option<String>) {
    let u = url::Url::parse(url).expect("bad proxy URL");
    let host = u.host_str().expect("proxy URL missing host");
    let port = u.port_or_known_default().expect("proxy URL missing port");
    let user = (!u.username().is_empty()).then(|| u.username().to_string());
    let pass = u.password().map(String::from);
    (format!("{}://{}:{}", u.scheme(), host, port), user, pass)
}
