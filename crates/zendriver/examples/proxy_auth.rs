//! Drive Chrome through an authenticated upstream proxy.
//!
//! Equivalent to cdpdriver/zendriver#208 — but without the Chrome-extension
//! workaround the Python project requires. We pass `--proxy-server` directly,
//! then `BrowserBuilder::proxy_auth(user, pass)` (feature `interception`)
//! spawns an internal interception actor that answers every
//! `Fetch.authRequired` challenge with the stored credentials via
//! `Fetch.continueWithAuth`.
//!
//! Scope: applies to the main tab only. For multi-tab apps, install per-tab
//! via `tab.intercept().handle_auth(user, pass).start()`.

#[tokio::main]
#[allow(clippy::result_large_err)]
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    // Substitute with your authenticated proxy. The credentials supplied to
    // `proxy_auth` are sent to the proxy, not to the upstream origin server.
    let proxy_server = std::env::var("ZD_PROXY").unwrap_or_else(|_| "http://127.0.0.1:3128".into());
    let proxy_user = std::env::var("ZD_PROXY_USER").unwrap_or_else(|_| "user".into());
    let proxy_pass = std::env::var("ZD_PROXY_PASS").unwrap_or_else(|_| "pass".into());

    let browser = zendriver::Browser::builder()
        .headless(true)
        .arg(format!("--proxy-server={proxy_server}"))
        .proxy_auth(proxy_user, proxy_pass)
        .launch()
        .await?;

    let tab = browser.main_tab();
    tab.goto("https://www.myexternalip.com/raw").await?;
    tab.wait_for_load().await?;
    let ip: String = tab.evaluate("document.body.textContent.trim()").await?;
    println!("external IP via proxy: {ip}");

    browser.close().await?;
    Ok(())
}
