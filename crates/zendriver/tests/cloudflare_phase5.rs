//! Nightly Cloudflare bypass tests against real-internet demos.
//!
//! Gated behind `cloudflare-tests` feature (which also requires
//! `cloudflare` + `integration-tests`). Run in CI on cron `0 7 * * *`.
//! Failures are not blocking (`continue-on-error: true`) — external
//! demo sites flake and Cloudflare's challenge surface changes.
//!
//! ## Why a wiremock-served local page
//!
//! Cloudflare's *interactive* test sitekey `3x00000000000000000000FF` only
//! mounts its checkbox iframe when its pre-mount fingerprint check passes.
//! As of mid-2026 that check rejects every headless Chrome we've tried —
//! including this crate's spoofed stealth profile — so the click flow
//! cannot be exercised end-to-end against any public demo.
//!
//! What *is* reproducible is the **invisible** test sitekey
//! `1x00000000000000000000AA`: Cloudflare's loader script populates the
//! `cf-turnstile-response` input with a dummy token without ever mounting
//! an iframe. This still exercises the bypass driver's full poll loop +
//! token-decode path against real `challenges.cloudflare.com` script
//! traffic; only the click-at-bbox path is unit-tested in isolation (see
//! `zendriver_cloudflare::bypass` tests).

#![cfg(feature = "cloudflare-tests")]

use serial_test::serial;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::stealth::StealthProfile;
use zendriver::{Browser, ClearanceOutcome};

/// HTML page embedding Cloudflare Turnstile with the invisible test
/// sitekey. The Cloudflare loader populates `cf-turnstile-response` with
/// the dummy token `XXXX.DUMMY.TOKEN.XXXX` after `api.js` finishes
/// initializing.
const INVISIBLE_TURNSTILE_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>invisible-turnstile-demo</title></head>
<body>
<script src="https://challenges.cloudflare.com/turnstile/v0/api.js" async defer></script>
<div class="cf-turnstile" data-sitekey="1x00000000000000000000AA"></div>
</body></html>"#;

#[tokio::test]
#[serial]
async fn cloudflare_bypass_acquires_token_from_invisible_turnstile() {
    // Serve the demo page locally so the test does not depend on any
    // 3rd-party demo URL remaining alive.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(INVISIBLE_TURNSTILE_HTML, "text/html"),
        )
        .mount(&server)
        .await;

    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto(&server.uri()).await.expect("goto");
    tab.wait_for_load().await.expect("load");

    let outcome = tab
        .cloudflare()
        .wait_for_clearance(Duration::from_secs(30))
        .await
        .expect("clearance");
    match outcome {
        ClearanceOutcome::TokenAcquired(token) => {
            assert!(
                !token.is_empty(),
                "invisible Turnstile should populate token"
            );
        }
        other => panic!("expected TokenAcquired from invisible sitekey, got {other:?}"),
    }
    browser.close().await.expect("close");
}
