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

use serial_test::serial;
use zendriver::Browser;
use zendriver::{Persona, Seed, Strategy, Surface};

/// Canvas JS that creates a 50×20 canvas, draws text, and returns `toDataURL`.
const CANVAS_READ_JS: &str = r#"(() => {
    const c = document.createElement('canvas');
    c.width = 50; c.height = 20;
    const x = c.getContext('2d');
    x.fillText('zd', 2, 12);
    return c.toDataURL();
})()"#;

/// Seeded canvas: two `toDataURL` reads in the same page must be identical —
/// the farble is deterministic per-seed, so the same pixel data produces the
/// same noise each time.
#[tokio::test]
#[serial]
#[ignore] // run with: cargo test ... -- --ignored
async fn seeded_canvas_is_stable_across_reads() {
    let browser = Browser::builder()
        .persona(Persona::builder().seed(Seed::from_u64(42)).build())
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("about:blank").await.unwrap();
    tab.wait_for_load().await.unwrap();

    let a: String = tab.evaluate::<String>(CANVAS_READ_JS).await.unwrap();
    let b: String = tab.evaluate::<String>(CANVAS_READ_JS).await.unwrap();
    assert_eq!(a, b, "Seeded canvas must be STABLE across repeat reads");

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

    let a: String = tab.evaluate::<String>(CANVAS_READ_JS).await.unwrap();
    let b: String = tab.evaluate::<String>(CANVAS_READ_JS).await.unwrap();
    // Under Native the shim is absent; the browser's own canvas output is
    // deterministic for the same drawing commands, so both reads are equal.
    assert_eq!(a, b, "Native canvas must also be stable (no farble noise)");

    browser.close().await.unwrap();
}

/// Random strategy: two reads within the same page must DIFFER because the
/// shim re-seeds from `Math.random()` on every `getImageData` call.
#[tokio::test]
#[serial]
#[ignore] // run with: cargo test ... -- --ignored
async fn random_canvas_differs_across_reads() {
    let browser = Browser::builder()
        .persona(Persona::builder().seed(Seed::from_u64(99)).build())
        .surface(Surface::Canvas, Strategy::Random)
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("about:blank").await.unwrap();
    tab.wait_for_load().await.unwrap();

    let a: String = tab.evaluate::<String>(CANVAS_READ_JS).await.unwrap();
    let b: String = tab.evaluate::<String>(CANVAS_READ_JS).await.unwrap();
    // With `Random` each call gets a fresh PRNG seed; the overwhelming
    // majority of the time the two data URLs will differ. (Probability of
    // accidental equality is astronomically small for a 50×20 RGBA canvas.)
    assert_ne!(
        a, b,
        "Random canvas strategy must produce different outputs per read"
    );

    browser.close().await.unwrap();
}
