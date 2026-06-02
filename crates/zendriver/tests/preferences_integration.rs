//! Preferences writer integration tests — gated headful.
//!
//! Verifies: (a) Chrome launches cleanly when the port writes default
//! suppression prefs to an owned temp profile; (b) a user-supplied profile
//! whose `Default/Preferences` contains existing keys has those keys preserved
//! after launch (Chrome may add its own keys, but explicit pre-seeded ones
//! must survive).
//!
//! Gated behind `integration-tests` feature and marked `#[ignore]` so CI
//! Chrome-less runners skip them; run with `-- --ignored` locally.

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use serial_test::serial;
use zendriver::Browser;

/// Owned temp profile: Chrome must start cleanly with the default suppression
/// prefs already written into `Default/Preferences`. The temp dir is internal
/// (owned by the port), so we assert indirectly — a successful launch + close
/// is the observable invariant. Unit-level coverage of the file contents lives
/// in `preferences::io_tests`.
#[tokio::test]
#[serial]
#[ignore]
async fn owned_profile_gets_suppression_prefs() {
    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let _tab = browser.new_tab().await.unwrap();
    browser.close().await.unwrap();
}

/// Supplied profile: after launch + close, a key we seeded into
/// `Default/Preferences` before launch must still be present.
/// Chrome writes its own metadata keys during startup, but must not wipe
/// pre-existing entries that it doesn't own.
#[tokio::test]
#[serial]
#[ignore]
async fn supplied_profile_preserved() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Default")).unwrap();
    std::fs::write(dir.path().join("Default/Preferences"), r#"{"foo":1}"#).unwrap();

    let browser = Browser::builder()
        .headless(true)
        .user_data_dir(dir.path())
        .launch()
        .await
        .unwrap();
    browser.close().await.unwrap();

    let contents = std::fs::read_to_string(dir.path().join("Default/Preferences")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&contents).unwrap();
    assert_eq!(
        v["foo"],
        serde_json::json!(1),
        "pre-seeded key must survive Chrome startup"
    );
}
