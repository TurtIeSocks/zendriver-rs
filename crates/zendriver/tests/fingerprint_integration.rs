//! Headful fingerprint-stability integration tests.
//!
//! Gated by `#[cfg(feature = "integration-tests")]` — same gate as
//! `integration_phase2.rs`. Additionally marked `#[ignore]` so they only run
//! explicitly (`cargo test -- --ignored`) on machines with a local Chrome.
//!
//! These tests are intentionally **not** run in the normal test matrix; they
//! require a real Chrome binary and exercise the actual JS farble shims.
//!
//! To run locally:
//! ```sh
//! cargo test -p zendriver --test fingerprint_integration \
//!     --features integration-tests -- --ignored
//! ```

#![cfg(feature = "integration-tests")]

use std::time::Duration;

use serial_test::serial;
use zendriver::stealth::StealthProfile;
use zendriver::{Browser, Persona, Seed, Strategy, Surface, Tab};

/// Canvas JS that creates a 50×20 canvas, draws text, and returns `toDataURL`.
const CANVAS_READ_JS: &str = r#"(() => {
    const c = document.createElement('canvas');
    c.width = 50; c.height = 20;
    const x = c.getContext('2d');
    x.fillText('zd', 2, 12);
    return c.toDataURL();
})()"#;

/// Bounded polling for the install-race (D2 in the canvas-farble
/// investigation): the farble bootstrap is injected via
/// `Page.addScriptToEvaluateOnNewDocument`, but `Tab::wait_for_load()` keys
/// on `Page.frameStoppedLoading` with no happens-before edge to "the
/// main-world bootstrap finished its `__zdReplace` calls" — so the first
/// couple of `evaluate_main` reads right after `wait_for_load()` on a
/// freshly-navigated page can observe the unpatched native canvas API.
/// Rather than re-plumbing `wait_for_load` itself (a readiness global would
/// leak an `Object.keys(window)` tell — the bootstrap is deliberately
/// closure-local), the tests poll here until a read diverges from a known
/// native baseline, proving the patch is live, before taking the real
/// comparison reads.
const MAX_POLL_TRIES: u32 = 20;
const POLL_DELAY: Duration = Duration::from_millis(50);

/// Reads `CANVAS_READ_JS` from a fresh, unspoofed browser (the builder
/// default, `StealthProfile::native()`). Under `ProfileKind::Native` the
/// canvas farble patch is never injected at all (the observer only sends the
/// bootstrap for `ProfileKind::Spoofed`), so this is a genuine native-Chrome
/// baseline — used by [`poll_until_farbled`] to recognize "farble hasn't
/// installed yet" instead of mistaking a not-yet-patched read for a
/// coincidentally-equal farbled one.
async fn native_canvas_baseline() -> String {
    let browser = Browser::builder().launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto("about:blank").await.unwrap();
    tab.wait_for_load().await.unwrap();
    let baseline: String = tab.evaluate_main(CANVAS_READ_JS).await.unwrap();
    browser.close().await.unwrap();
    baseline
}

/// Poll `tab` until an `evaluate_main(CANVAS_READ_JS)` read diverges from
/// `native_baseline`, proving the main-world farble patch is active, and
/// return that (now-confirmed-farbled) read. Bounded at [`MAX_POLL_TRIES`] so
/// a genuine farble regression fails loudly instead of hanging forever.
async fn poll_until_farbled(tab: &Tab, native_baseline: &str) -> String {
    for attempt in 0..MAX_POLL_TRIES {
        let read: String = tab.evaluate_main(CANVAS_READ_JS).await.unwrap();
        if read != native_baseline {
            return read;
        }
        if attempt + 1 < MAX_POLL_TRIES {
            tokio::time::sleep(POLL_DELAY).await;
        }
    }
    panic!(
        "canvas farble never activated after {MAX_POLL_TRIES} reads (every read still equals \
         the native baseline) — either a genuine install-race regression or the farble patch \
         itself is broken"
    );
}

/// Seeded canvas: two `toDataURL` reads in the same page must be identical —
/// the farble PRNG is reset per call and keyed by (seed, content), so the
/// same pixel data produces the same noise every time it's read.
#[tokio::test]
#[serial]
#[ignore] // run with: cargo test ... -- --ignored
async fn seeded_canvas_is_stable_across_reads() {
    let native_baseline = native_canvas_baseline().await;

    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .persona(Persona::builder().seed(Seed::from_u64(42)).build())
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("about:blank").await.unwrap();
    tab.wait_for_load().await.unwrap();

    let a = poll_until_farbled(&tab, &native_baseline).await;
    let b: String = tab.evaluate_main(CANVAS_READ_JS).await.unwrap();
    assert_eq!(
        a, b,
        "Seeded canvas must be STABLE across repeat reads (against real farble, not native)"
    );

    browser.close().await.unwrap();
}

/// Native strategy: two reads must also be equal (no farble applied, pure
/// browser output). The meaningful native-vs-seeded distinction (the data URL
/// itself differs between seeded and native runs) is verified manually — it
/// requires two separate browser instances comparing absolute pixel values,
/// which is impractical as an automated offline assertion.
#[tokio::test]
#[serial]
#[ignore] // run with: cargo test ... -- --ignored
async fn native_canvas_is_stable_across_reads() {
    let browser = Browser::builder()
        .persona(Persona::builder().seed(Seed::from_u64(42)).build())
        .surface(Surface::Canvas, Strategy::Native)
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("about:blank").await.unwrap();
    tab.wait_for_load().await.unwrap();

    let a: String = tab.evaluate_main(CANVAS_READ_JS).await.unwrap();
    let b: String = tab.evaluate_main(CANVAS_READ_JS).await.unwrap();
    // The builder default is `StealthProfile::native()` (no `.stealth()` call
    // here), which never injects the farble bootstrap regardless of the
    // per-surface strategy — this exercises the genuine no-farble path. The
    // browser's own canvas output is deterministic for the same drawing
    // commands, so both reads are equal.
    assert_eq!(a, b, "Native canvas must also be stable (no farble noise)");

    browser.close().await.unwrap();
}

/// Random strategy: two reads within the same page must be STABLE. The seed
/// fed into the farble PRNG is drawn once via `Math.random()` per page load
/// (see `patches::seed_token`), but the PRNG itself is reset per call and
/// keyed by (seed, content) — same fix as `Seeded` — so repeat reads of
/// identical content within that one page load reproduce the same noise.
/// `Random` differs from `Seeded` only in *where the seed comes from*: a
/// fresh `Math.random()` draw per page load instead of the persona's fixed
/// seed, so two separate page loads get independent noise, but reads within
/// one page load never do. (The per-page-load re-randomization itself is
/// already covered at the bootstrap-templating level by
/// `zendriver_stealth::patches::tests::random_canvas_uses_math_random_seed`,
/// which asserts the generated script contains a fresh
/// `Math.random()*4294967296` draw; re-proving that here would cost a second
/// real-Chrome browser launch per run for no additional signal, so it's
/// deliberately skipped.)
#[tokio::test]
#[serial]
#[ignore] // run with: cargo test ... -- --ignored
async fn random_canvas_stable_within_page() {
    let native_baseline = native_canvas_baseline().await;

    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .persona(Persona::builder().seed(Seed::from_u64(99)).build())
        .surface(Surface::Canvas, Strategy::Random)
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("about:blank").await.unwrap();
    tab.wait_for_load().await.unwrap();

    let a = poll_until_farbled(&tab, &native_baseline).await;
    let b: String = tab.evaluate_main(CANVAS_READ_JS).await.unwrap();
    assert_eq!(
        a, b,
        "Random canvas strategy must be stable WITHIN a page (same per-page-random seed + \
         content-keyed PRNG); it only differs across separate page loads"
    );

    browser.close().await.unwrap();
}
