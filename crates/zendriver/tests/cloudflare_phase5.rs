//! Nightly Cloudflare bypass tests against real-internet demos.
//!
//! Gated behind `cloudflare-tests` feature (which also requires
//! `cloudflare` + `integration-tests`). Run in CI on cron `0 7 * * *`.
//! Failures are not blocking (`continue-on-error: true`) — external
//! demo sites flake and Cloudflare's challenge surface changes.

#![cfg(feature = "cloudflare-tests")]

use serial_test::serial;
use std::time::Duration;
use zendriver::stealth::StealthProfile;
use zendriver::{Browser, ClearanceOutcome};

#[tokio::test]
#[serial]
async fn cloudflare_bypass_clears_nopecha_demo() {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto("https://nopecha.com/demo/cloudflare")
        .await
        .expect("goto");
    tab.wait_for_load().await.expect("load");
    // Let the challenge iframe mount before polling.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let outcome = tab
        .cloudflare()
        .wait_for_clearance(Duration::from_secs(30))
        .await
        .expect("clearance");
    assert!(
        matches!(
            outcome,
            ClearanceOutcome::TokenAcquired(_) | ClearanceOutcome::ChallengeGone
        ),
        "unexpected clearance outcome: {outcome:?}"
    );
    browser.close().await.expect("close");
}
