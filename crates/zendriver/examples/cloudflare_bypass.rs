//! Demonstrates the P5 Cloudflare Turnstile bypass driver.
//!
//! Sequence:
//!   1. Launch a headless browser and navigate to a Cloudflare-protected
//!      demo URL. Set this to whatever endpoint you actually want to clear;
//!      `nopecha.com/demo/cloudflare` is the conventional public demo.
//!   2. Call [`Tab::cloudflare`] to construct a [`CloudflareBypass`] bound
//!      to the tab's session.
//!   3. Call [`CloudflareBypass::wait_for_clearance`] with a 30s budget.
//!      The driver detects the Turnstile iframe + bounding box, dispatches
//!      a raw `mousedown`/`mouseup` at the canonical (15% x, 50% y) offset
//!      inside the iframe — the checkbox spot — and polls the page until
//!      the `cf-turnstile-response` input picks up a non-empty token
//!      (success) or the challenge container disappears (cookie shortcut).
//!   4. Print the [`ClearanceOutcome`] enum variant + the page title.
//!
//! Outcomes:
//!   - `TokenAcquired(_)` — Turnstile yielded a `cf-turnstile-response`
//!     token; the page is cleared.
//!   - `ChallengeGone` — the challenge container disappeared without
//!     yielding a token (cookie shortcut).
//!   - `Err(NoChallenge)` — there was nothing to clear at navigation time.
//!     The demo site may have already passed you through; not a failure.
//!   - `Err(ClearanceTimeout)` — 30s elapsed without resolution.
//!
//! Requires the `cloudflare` cargo feature:
//! `cargo run --example cloudflare_bypass --features cloudflare`.

use std::time::Duration;

use zendriver::{Browser, CloudflareError};

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();

    tab.goto("https://nopecha.com/demo/cloudflare").await?;
    tab.wait_for_load().await?;

    match tab
        .cloudflare()
        .wait_for_clearance(Duration::from_secs(30))
        .await
    {
        Ok(outcome) => println!("cleared: {outcome:?}"),
        Err(CloudflareError::NoChallenge) => {
            println!("no challenge detected (page already cleared or no CF gate present)");
        }
        Err(CloudflareError::ClearanceTimeout) => {
            println!("clearance timed out within 30s");
        }
        Err(e) => return Err(e.into()),
    }

    let title = tab.title().await?;
    println!("title = {title:?}");

    browser.close().await?;
    Ok(())
}
