//! Real-Chrome end-to-end test for the lifecycle + navigation tool group.
//!
//! Gated behind the `integration-tests` cargo feature AND marked
//! `#[ignore]` so a default `cargo test` run never spawns Chrome. To
//! exercise it explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp --features integration-tests --test integration_lifecycle -- --ignored
//! ```
//!
//! The test drives the binary over stdio (same shape as `stdio_smoke.rs`)
//! and rounds-trip `browser_open` → `browser_goto` → `browser_status` →
//! `browser_close`. The full lifecycle exercise is the primary value —
//! once Chrome is launched, the cost of running a few extra tool calls
//! is negligible.

#![cfg(feature = "integration-tests")]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
#[ignore = "requires real Chrome; run with --features integration-tests -- --ignored"]
async fn end_to_end_open_goto_close() {
    let client = ()
        .serve(
            TokioChildProcess::new(Command::new(BIN_PATH).configure(|cmd| {
                cmd.arg("--log").arg("error");
            }))
            .expect("spawn child"),
        )
        .await
        .expect("rmcp client init");

    // 1. Launch Chrome.
    let open = client
        .call_tool(CallToolRequestParams::new("browser_open").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("headless".into(), serde_json::json!(true));
            m
        }))
        .await
        .expect("browser_open ok");
    let open_body = structured(&open);
    assert_eq!(open_body["headless"], serde_json::json!(true));

    // 2. Navigate to example.com.
    let goto = client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!("https://example.com"));
            m
        }))
        .await
        .expect("browser_goto ok");
    let goto_body = structured(&goto);
    assert!(
        goto_body["url"]
            .as_str()
            .is_some_and(|u| u.contains("example.com")),
        "url body: {goto_body}"
    );

    // 3. Status should now report open + a current tab.
    let status = client
        .call_tool(CallToolRequestParams::new("browser_status").with_arguments(Default::default()))
        .await
        .expect("browser_status ok");
    let status_body = structured(&status);
    assert_eq!(status_body["open"], serde_json::json!(true));
    assert!(status_body["tab_count"].as_u64().unwrap_or(0) >= 1);

    // 4. Close.
    let close = client
        .call_tool(CallToolRequestParams::new("browser_close").with_arguments(Default::default()))
        .await
        .expect("browser_close ok");
    assert_eq!(structured(&close)["ok"], serde_json::json!(true));

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
