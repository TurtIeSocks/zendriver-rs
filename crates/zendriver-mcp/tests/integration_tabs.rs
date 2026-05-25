//! Real-Chrome end-to-end test for the tab + frame tool group.
//!
//! Gated behind the `integration-tests` cargo feature AND marked
//! `#[ignore]` so a default `cargo test` run never spawns Chrome. To
//! exercise it explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp --features integration-tests --test integration_tabs -- --ignored
//! ```
//!
//! The test drives the binary over stdio (same shape as
//! `integration_lifecycle.rs`) and rounds-trip
//! `browser_open` → `browser_goto` → `browser_tab_list` →
//! `browser_tab_new` → `browser_tab_list` (count grew) →
//! `browser_tab_close` → `browser_tab_list` (count shrank) →
//! `browser_frame_list` → `browser_close`.

#![cfg(feature = "integration-tests")]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
#[ignore = "requires real Chrome; run with --features integration-tests -- --ignored"]
async fn end_to_end_tabs_lifecycle() {
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
    client
        .call_tool(CallToolRequestParams::new("browser_open").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("headless".into(), serde_json::json!(true));
            m
        }))
        .await
        .expect("browser_open ok");

    // 2. Navigate to example.com on the initial tab.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!("https://example.com"));
            m
        }))
        .await
        .expect("browser_goto ok");

    // 3. tab_list reports 1 tab.
    let list = client
        .call_tool(
            CallToolRequestParams::new("browser_tab_list").with_arguments(Default::default()),
        )
        .await
        .expect("browser_tab_list ok");
    let body = structured(&list);
    assert_eq!(
        body["tabs"].as_array().map(Vec::len),
        Some(1),
        "tab_list body: {body}"
    );

    // 4. tab_new opens another tab (about:blank).
    let new_tab = client
        .call_tool(CallToolRequestParams::new("browser_tab_new").with_arguments(Default::default()))
        .await
        .expect("browser_tab_new ok");
    let new_tab_body = structured(&new_tab);
    let new_id = new_tab_body["id"]
        .as_str()
        .expect("tab_new returns an id")
        .to_string();
    assert!(
        new_tab_body["is_current"].as_bool().unwrap_or(false),
        "tab_new with default activate=true should make the new tab current: {new_tab_body}"
    );

    // 5. tab_list now reports 2 tabs, one of which is current.
    let list2 = client
        .call_tool(
            CallToolRequestParams::new("browser_tab_list").with_arguments(Default::default()),
        )
        .await
        .expect("browser_tab_list ok");
    let body2 = structured(&list2);
    assert_eq!(
        body2["tabs"].as_array().map(Vec::len),
        Some(2),
        "tab_list body: {body2}"
    );
    let current_id_in_list = body2["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["is_current"].as_bool() == Some(true))
        .and_then(|t| t["id"].as_str())
        .map(str::to_string);
    assert_eq!(
        current_id_in_list.as_deref(),
        Some(new_id.as_str()),
        "the freshly-opened tab should be reported as current; body: {body2}"
    );

    // 6. tab_close(None) closes the current tab — count back to 1.
    let close_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_tab_close").with_arguments(Default::default()),
        )
        .await
        .expect("browser_tab_close ok");
    let close_body = structured(&close_resp);
    assert_eq!(
        close_body["closed_id"].as_str(),
        Some(new_id.as_str()),
        "tab_close(None) should close the current tab: {close_body}"
    );

    let list3 = client
        .call_tool(
            CallToolRequestParams::new("browser_tab_list").with_arguments(Default::default()),
        )
        .await
        .expect("browser_tab_list ok");
    let body3 = structured(&list3);
    assert_eq!(
        body3["tabs"].as_array().map(Vec::len),
        Some(1),
        "tab_list body: {body3}"
    );

    // 7. frame_list returns at least 1 frame (the main frame).
    let frames = client
        .call_tool(
            CallToolRequestParams::new("browser_frame_list").with_arguments(Default::default()),
        )
        .await
        .expect("browser_frame_list ok");
    let frames_body = structured(&frames);
    let frame_arr = frames_body["frames"]
        .as_array()
        .expect("frames field is array");
    assert!(
        !frame_arr.is_empty(),
        "frame_list should report at least the main frame: {frames_body}"
    );
    assert!(
        frame_arr
            .iter()
            .any(|f| f["is_main"].as_bool() == Some(true)),
        "exactly one frame should be the main frame: {frames_body}"
    );

    // 8. Close browser.
    client
        .call_tool(CallToolRequestParams::new("browser_close").with_arguments(Default::default()))
        .await
        .expect("browser_close ok");

    client.cancel().await.expect("clean shutdown");
}

#[tokio::test]
#[ignore = "stealth profile change is a SessionState-only mutation; integration-mode just sanity-checks it round-trips over stdio"]
async fn set_stealth_profile_round_trip() {
    let client = ()
        .serve(
            TokioChildProcess::new(Command::new(BIN_PATH).configure(|cmd| {
                cmd.arg("--log").arg("error");
            }))
            .expect("spawn child"),
        )
        .await
        .expect("rmcp client init");

    // No browser open — set profile, then `browser_status` should report
    // the new profile + closed browser.
    let resp = client
        .call_tool(
            CallToolRequestParams::new("browser_set_stealth_profile").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("profile".into(), serde_json::json!("spoof_linux"));
                m
            }),
        )
        .await
        .expect("set_stealth_profile ok");
    let body = structured(&resp);
    assert_eq!(body["active_profile"], serde_json::json!("spoof_linux"));
    assert_eq!(body["takes_effect_on_next_open"], serde_json::json!(false));

    let status = client
        .call_tool(CallToolRequestParams::new("browser_status").with_arguments(Default::default()))
        .await
        .expect("browser_status ok");
    let status_body = structured(&status);
    assert_eq!(status_body["profile"], serde_json::json!("spoof_linux"));

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
