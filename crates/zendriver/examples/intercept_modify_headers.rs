//! Demonstrates the P5 [`InterceptBuilder::modify_request`] rule, which lets
//! you mutate the outbound headers/method/body for every request whose URL
//! matches a pattern.
//!
//! The closure returns a [`RequestOverrides`] for each matched
//! [`RequestInfo`]; per CDP semantics, `headers` is *replacement*, not
//! merge — so we explicitly copy the original header map and stamp our
//! `X-Custom` header on top before handing it back.
//!
//! The example navigates to `https://httpbin.org/headers`, which echoes
//! every request header back as JSON in the response body. Print the body
//! to verify `X-Custom: zendriver-demo` made the round-trip.
//!
//! Requires the `interception` cargo feature:
//! `cargo run --example intercept_modify_headers --features interception`.

use zendriver::Browser;
use zendriver::RequestOverrides;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();

    let _intercept = tab
        .intercept()
        .modify_request("*httpbin.org/headers*", |req| {
            // CDP replaces — not merges — the header set. Copy the originals
            // forward and stamp our custom header on top. Headers are an
            // ordered Vec to preserve duplicates / Set-Cookie semantics.
            let mut headers = req.headers.clone();
            headers.push(("X-Custom".into(), "zendriver-demo".into()));
            RequestOverrides {
                headers: Some(headers),
                ..Default::default()
            }
        })?
        .start();

    tab.goto("https://httpbin.org/headers").await?;
    tab.wait_for_load().await?;

    // httpbin renders the response as `<pre>` JSON; grab its text content.
    let body: String = tab.evaluate_main("document.body.innerText").await?;
    println!("{body}");

    browser.close().await?;
    Ok(())
}
