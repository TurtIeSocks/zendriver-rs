//! Real-network end-to-end test for `browser_install_chrome`.
//!
//! Gated behind BOTH `integration-tests` AND `fetcher` cargo features AND
//! marked `#[ignore]` so a default `cargo test` run never touches the
//! network. To exercise it explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp \
//!     --features "integration-tests fetcher" \
//!     --test integration_fetcher -- --ignored
//! ```
//!
//! Smoke shape: `browser_install_chrome` against the live CFT manifest
//! into a `tempfile::tempdir` cache → assert the returned path exists and
//! is executable on Unix → tempdir cleans up on drop.
//!
//! Does NOT touch the browser — the fetcher is independent of the
//! `SessionState`'s `Browser` slot.
//!
//! ## Why this test is slow + flaky-by-design
//!
//! `ensure_chrome` downloads ~150 MB and extracts it. On a cold cache
//! that's 30–120s wall-clock depending on link speed, and CFT CDN
//! availability is the dominant failure mode. The test is intended as a
//! manual smoke when iterating on fetcher integration, not as a CI gate.

#![cfg(all(feature = "integration-tests", feature = "fetcher"))]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
#[ignore = "downloads ~150MB Chrome via real network; run with --features \"integration-tests fetcher\" -- --ignored"]
async fn end_to_end_install_chrome_to_tempdir() {
    let client = ()
        .serve(
            TokioChildProcess::new(Command::new(BIN_PATH).configure(|cmd| {
                cmd.arg("--log").arg("error");
            }))
            .expect("spawn child"),
        )
        .await
        .expect("rmcp client init");

    // Cache into a tempdir so we don't pollute the user's real cache and
    // we get automatic cleanup on test exit.
    let cache_root = tempfile::tempdir().expect("create tempdir");
    let cache_path = cache_root
        .path()
        .to_str()
        .expect("tempdir path is valid utf-8")
        .to_string();

    let install_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_install_chrome").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("cache_dir".into(), serde_json::json!(cache_path));
                m
            }),
        )
        .await
        .expect("browser_install_chrome ok");
    let body = structured(&install_resp);
    let path = body["path"].as_str().expect("path populated").to_string();
    assert!(!path.is_empty(), "path empty; body: {body}");

    // Returned path should exist on disk and be a regular file.
    let meta = tokio::fs::metadata(&path)
        .await
        .expect("returned path exists on disk");
    assert!(meta.is_file(), "returned path is not a file: {path}");

    // Unix: executable bit set (the fetcher sets it before atomic
    // promote — see `Fetcher::ensure_chrome` step 4).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert!(
            meta.permissions().mode() & 0o111 != 0,
            "binary missing executable bit: {path}"
        );
    }

    // Neither version nor channel was set, so both echoes should be
    // absent (serialize-skip on `None`).
    assert!(
        body.get("version_requested")
            .is_none_or(serde_json::Value::is_null),
        "version_requested should not be set; body: {body}"
    );
    assert!(
        body.get("channel_requested")
            .is_none_or(serde_json::Value::is_null),
        "channel_requested should not be set; body: {body}"
    );

    client.cancel().await.expect("clean shutdown");
}

/// Pull the structured payload out of a tool call result, falling back to
/// parsing the text content slot when rmcp didn't emit a structured one.
fn structured(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    if let Some(v) = result.structured_content.clone() {
        return v;
    }
    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text())
        .map(|t| t.text.clone())
        .expect("text or structured content present");
    serde_json::from_str(&text).expect("text payload is JSON")
}
