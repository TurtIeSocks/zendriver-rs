//! Port of `zendriver/examples/set_user_agent.py`.
//!
//! Launch Chrome with a custom User-Agent, locale, and platform configured
//! via [`StealthProfile`], then read them back via `navigator.*` to verify
//! the override took effect.
//!
//! Python `tab.set_user_agent("...", accept_language="de", platform="Win32")`
//! is a single-call helper that internally drives
//! `Emulation.setUserAgentOverride`. zendriver-rs lifts that into the
//! `StealthProfile` builder because the launcher already wires UA overrides
//! through `StealthObserver`, so per-tab mutation has no equivalent yet.
//! Setting the UA at launch matches the spec's "no JS-visible drift between
//! launch and first frame" stealth property.
//!
//! `navigator.platform` reads as `Win32` once `Platform::Win32` is set.

use zendriver::Browser;
use zendriver::stealth::{Platform, StealthProfile};

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let profile = StealthProfile::native()
        .user_agent("My user agent")
        .locale("de")
        .platform(Platform::Win32);

    let browser = Browser::builder()
        .headless(true)
        .stealth(profile)
        .launch()
        .await?;
    let tab = browser.main_tab();
    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    let ua: String = tab.evaluate("navigator.userAgent").await?;
    let lang: String = tab.evaluate("navigator.language").await?;
    let platform: String = tab.evaluate("navigator.platform").await?;

    println!("{ua}"); // My user agent
    println!("{lang}"); // de
    println!("{platform}"); // Win32

    browser.close().await?;
    Ok(())
}
