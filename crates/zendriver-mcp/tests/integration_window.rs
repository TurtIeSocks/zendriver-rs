//! Real-Chrome end-to-end test for `browser_get_window` / `browser_set_window`.
//!
//! Gated behind `integration-tests` AND marked `#[ignore]`. Run with:
//!
//! ```bash
//! cargo test -p zendriver-mcp --features integration-tests \
//!     --test integration_window -- --ignored
//! ```
//!
//! Best-effort: headless Chrome emulates a window and generally honors
//! `Browser.setWindowBounds`, but exact geometry can be clamped by the
//! platform. The test asserts the set call round-trips a plausible size.

#![cfg(feature = "integration-tests")]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
#[ignore = "requires real Chrome; run with --features integration-tests -- --ignored"]
async fn end_to_end_window_resize() {
    let client = ()
        .serve(
            TokioChildProcess::new(Command::new(BIN_PATH).configure(|cmd| {
                cmd.arg("--log").arg("error");
            }))
            .expect("spawn child"),
        )
        .await
        .expect("rmcp client init");

    client
        .call_tool(CallToolRequestParams::new("browser_open").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("headless".into(), serde_json::json!(true));
            m
        }))
        .await
        .expect("browser_open ok");

    // Resize to 800x600.
    let set_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_set_window").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("mode".into(), serde_json::json!("size"));
                m.insert("width".into(), serde_json::json!(800));
                m.insert("height".into(), serde_json::json!(600));
                m
            }),
        )
        .await
        .expect("browser_set_window ok");
    let set_body = structured(&set_resp);
    assert_eq!(
        set_body["width"],
        serde_json::json!(800),
        "body: {set_body}"
    );
    assert_eq!(
        set_body["height"],
        serde_json::json!(600),
        "body: {set_body}"
    );

    // Read it back.
    let get_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_get_window").with_arguments(Default::default()),
        )
        .await
        .expect("browser_get_window ok");
    let get_body = structured(&get_resp);
    assert_eq!(
        get_body["width"],
        serde_json::json!(800),
        "body: {get_body}"
    );

    client
        .call_tool(CallToolRequestParams::new("browser_close").with_arguments(Default::default()))
        .await
        .expect("browser_close ok");
    client.cancel().await.expect("clean shutdown");
}

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
