//! Seeded farble persona with a per-surface override.
//!
//! Demonstrates:
//! - Building a `Persona` with an explicit reproducible seed.
//! - Overriding a single surface's render strategy (`Webrtc → Native`).
//! - Launching the browser and navigating to `about:blank`.
//!
//! The seed pins the canvas / audio / clientRects farble so the same
//! pixel-level noise is produced on every run — useful for regression
//! testing and identity consistency across sessions.
//!
//! Run with:
//! ```sh
//! cargo run --example persona_basic
//! ```

use zendriver::{Browser, Persona, Seed, Strategy, Surface};

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    // Build a seeded persona so canvas / audio / clientRects farble is
    // deterministic, then override WebRTC to Native so real IP candidates
    // are allowed through (e.g. for WebRTC-based apps).
    let persona = Persona::builder()
        .seed(Seed::from_u64(42))
        .device_memory_gb(8)
        .timezone("America/New_York")
        .build();

    let browser = Browser::builder()
        .persona(persona)
        // One-off surface override: allow WebRTC to reveal the real IP.
        .surface(Surface::Webrtc, Strategy::Native)
        .launch()
        .await?;

    let tab = browser.main_tab();
    tab.goto("about:blank").await?;
    tab.wait_for_load().await?;

    // Read back a couple of navigator fields to show the persona is applied.
    let mem: serde_json::Value = tab.evaluate("navigator.deviceMemory").await?;
    let tz: String = tab
        .evaluate("Intl.DateTimeFormat().resolvedOptions().timeZone")
        .await?;

    println!("navigator.deviceMemory = {mem}");
    println!("timezone               = {tz}");

    browser.close().await?;
    Ok(())
}
