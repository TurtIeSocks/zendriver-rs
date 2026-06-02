//! Demonstrates the zendriver-imperva bypass driver.
//!
//! Sequence:
//!   1. Launch a stealth-enabled headless browser and navigate to an
//!      Imperva-protected URL. The example uses a placeholder URL — set
//!      `IMPERVA_DEMO_URL` env var to override, since there is no
//!      universally-stable public Imperva demo page.
//!   2. Call [`Tab::imperva`] to construct an [`ImpervaBypass`] bound to
//!      the tab's session.
//!   3. Call [`ImpervaBypass::wait_for_clearance`] with a 60s budget.
//!      The driver detects the active Imperva surface (modern reese84,
//!      legacy Incapsula, or CAPTCHA escalation) and polls until both
//!      hybrid signals (reese84 cookie set + body markers cleared) hit.
//!   4. Print the [`ImpervaClearanceOutcome`] variant + page title.
//!
//! Outcomes:
//!   - `TokenAcquired { reese84, sessions }` — full hybrid clearance.
//!   - `ChallengeGone` — body markers cleared without a reese84 token
//!     (legacy flow).
//!   - `AlreadyClear` — no Imperva surface present at navigation time.
//!   - `Err(CaptchaRequired { kind })` — escalation to CAPTCHA without a
//!     solver callback. Pass `.on_captcha(...)` to register one.
//!   - `Err(Timeout { last_surface, .. })` — 60s elapsed without
//!     clearance.
//!
//! Requires the `imperva` cargo feature:
//! `cargo run --example imperva_bypass --features imperva`.

use std::time::Duration;

use zendriver::stealth::StealthProfile;
use zendriver::{Browser, ImpervaClearanceOutcome, ImpervaError};

#[tokio::main]
#[allow(clippy::result_large_err)]
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let url = std::env::var("IMPERVA_DEMO_URL")
        .unwrap_or_else(|_| "https://example.com/imperva-protected".into());

    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await?;
    let tab = browser.main_tab();

    tab.goto(&url).await?;
    tab.wait_for_load().await?;

    match tab
        .imperva()
        .timeout(Duration::from_secs(60))
        .wait_for_clearance()
        .await
    {
        Ok(ImpervaClearanceOutcome::TimedOut { last_surface }) => {
            println!("clearance timed out; last_surface = {last_surface:?}");
        }
        Ok(outcome) => println!("cleared: {outcome:?}"),
        Err(ImpervaError::CaptchaRequired { kind }) => {
            println!("captcha required ({kind:?}); register .on_captcha(...) to solve");
        }
        Err(e) => return Err(e.into()),
    }

    let title = tab.title().await?;
    println!("title = {title:?}");

    browser.close().await?;
    Ok(())
}
