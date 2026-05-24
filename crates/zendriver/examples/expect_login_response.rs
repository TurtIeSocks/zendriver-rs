//! Demonstrates the P5 [`Tab::expect_response`] expectation API.
//!
//! Sequence:
//!   1. Render a tiny login-style form via `data:` URL. The form's submit
//!      handler `fetch()`s `https://example.com/login` (no real backend —
//!      example.com just 404s for that path, but the response still fires
//!      `Network.responseReceived`, which is what the expectation
//!      subscribes to).
//!   2. Register `tab.expect_response("*/login")` BEFORE clicking submit.
//!      The subscriber task is spawned synchronously inside the call, so it
//!      is live before the click — the response cannot slip past us.
//!   3. Click submit; the page fires the `fetch()`.
//!   4. Await the [`ResponseExpectation`]; assert the URL matched. Print the
//!      status code (404 from example.com, demonstrating that *any* response
//!      arrival satisfies the expectation regardless of HTTP status).
//!
//! Requires the `expect` cargo feature:
//! `cargo run --example expect_login_response --features expect`.

use std::time::Duration;

use zendriver::Browser;

const FORM_HTML: &str = "data:text/html,\
<!doctype html><html><body>\
<form id='f' onsubmit=\"event.preventDefault();fetch('https://example.com/login',{method:'POST',mode:'no-cors'});\">\
<input id='user' name='user' value='rin' />\
<input id='pass' name='pass' type='password' value='hunter2' />\
<button id='go' type='submit'>Log in</button>\
</form></body></html>";

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();
    tab.goto(FORM_HTML).await?;
    tab.wait_for_load().await?;

    // Register expectation BEFORE the trigger action — the subscriber is
    // live by the time `expect_response` returns, so the response cannot
    // race past us.
    let expectation = tab
        .expect_response("*/login")
        .timeout(Duration::from_secs(10));

    let go = tab.find().css("#go").one().await?;
    go.click().await?;

    let matched = expectation.await?;
    println!(
        "matched: url={} status={} status_text={:?}",
        matched.url, matched.status, matched.status_text
    );
    assert!(
        matched.url.contains("/login"),
        "matched url should contain /login; got {}",
        matched.url
    );

    browser.close().await?;
    Ok(())
}
