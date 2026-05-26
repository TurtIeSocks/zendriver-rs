//! Nightly Imperva bypass tests against real-internet sites.
//!
//! Gated behind `imperva-tests` feature. Run in CI on cron `0 8 * * *`.
//! Failures are not blocking (`continue-on-error: true`) — Imperva
//! configurations on public sites change unpredictably.
//!
//! **Site list TODO.** No universally stable public Imperva demo exists.
//! Candidates to evaluate (each #[ignore]'d until validated):
//!   - User-controlled Imperva trial deployment
//!   - Public sites known to use Imperva ABP (rotate periodically)
//!   - A `IMPERVA_TEST_URL` env-var-driven test for ad-hoc validation
//!
//! The single concrete test below uses the `IMPERVA_TEST_URL` env var so
//! a maintainer can run it locally against any target site without
//! recompiling.

#![cfg(feature = "imperva-tests")]

use serial_test::serial;
use std::time::Duration;
use zendriver::stealth::StealthProfile;
use zendriver::{Browser, ImpervaClearanceOutcome};

/// Env-var-driven smoke test. Set `IMPERVA_TEST_URL` to a known-protected
/// site. Skipped if unset.
#[tokio::test]
#[serial]
async fn imperva_bypass_env_driven_smoke() {
    let Ok(url) = std::env::var("IMPERVA_TEST_URL") else {
        eprintln!("IMPERVA_TEST_URL unset; skipping");
        return;
    };

    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto(&url).await.expect("goto");
    tab.wait_for_load().await.expect("load");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let outcome = tab
        .imperva()
        .timeout(Duration::from_secs(60))
        .wait_for_clearance()
        .await
        .expect("clearance");

    assert!(
        matches!(
            outcome,
            ImpervaClearanceOutcome::TokenAcquired { .. }
                | ImpervaClearanceOutcome::ChallengeGone
                | ImpervaClearanceOutcome::AlreadyClear
        ),
        "unexpected outcome: {outcome:?}"
    );
    browser.close().await.expect("close");
}
