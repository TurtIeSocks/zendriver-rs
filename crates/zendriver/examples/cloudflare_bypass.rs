//! Demonstrates the P5 Cloudflare Turnstile bypass driver.
//!
//! Sequence:
//!   1. Launch a headless browser and navigate to a Cloudflare-protected
//!      URL. Set this to whatever endpoint you actually want to clear.
//!   2. Call [`Tab::cloudflare`] to construct a [`CloudflareBypass`] bound
//!      to the tab's session.
//!   3. Call [`CloudflareBypass::wait_for_clearance`] with a 30s budget.
//!      The driver runs a single CDP poll loop that, per tick, looks for
//!      the `cf-turnstile-response` token, the Turnstile challenge
//!      iframe's bounding box (shadow-DOM aware), and any challenge marker
//!      on the page. When the interactive iframe is present and we have
//!      not yet clicked it, the driver dispatches a single raw left-click
//!      at the canonical (15% x, 50% y) offset (Turnstile checkbox).
//!      Resolves the first tick a token is observed or the iframe is gone
//!      after a click.
//!   4. Print the [`ClearanceOutcome`] enum variant + the page title.
//!
//! Outcomes:
//!   - `TokenAcquired(_)` — Turnstile yielded a `cf-turnstile-response`
//!     token, either after clicking the checkbox or directly (invisible
//!     Turnstile, where the iframe never mounts).
//!   - `ChallengeGone` — the interactive iframe was clicked and the
//!     challenge container then disappeared without a token (e.g.
//!     clearance-cookie shortcut).
//!   - `Err(NoChallenge)` — the full timeout window elapsed without any
//!     challenge markers ever being observed (no container, no hidden
//!     input, no iframe). The page likely has no Cloudflare gate; not a
//!     failure.
//!   - `Err(ClearanceTimeout)` — 30s elapsed with markers present but
//!     neither success state observed.
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
