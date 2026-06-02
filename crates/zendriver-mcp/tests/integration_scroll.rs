//! Real-Chrome end-to-end test for `browser_scroll` (and a `browser_mouse`
//! move smoke).
//!
//! Gated behind `integration-tests` AND marked `#[ignore]`. Run with:
//!
//! ```bash
//! cargo test -p zendriver-mcp --features integration-tests \
//!     --test integration_scroll -- --ignored
//! ```

#![cfg(feature = "integration-tests")]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

/// A document tall enough that scrolling moves `window.scrollY`.
const TALL_PAGE: &str = "data:text/html,<body style='height:5000px;margin:0'>tall</body>";

#[tokio::test]
#[ignore = "requires real Chrome; run with --features integration-tests -- --ignored"]
async fn end_to_end_scroll_and_mouse() {
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
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!(TALL_PAGE));
            m
        }))
        .await
        .expect("browser_goto ok");

    // Scroll down 1000px; positive dy = down.
    let scroll_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_scroll").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("dy".into(), serde_json::json!(1000.0));
                m
            }),
        )
        .await
        .expect("browser_scroll ok");
    let body = structured(&scroll_resp);
    assert!(
        body["scroll_y"].as_f64().unwrap_or(0.0) > 0.0,
        "scroll_y should be > 0 after scrolling down; body: {body}"
    );

    // Mouse move smoke — should not error.
    let mouse_resp = client
        .call_tool(CallToolRequestParams::new("browser_mouse").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("action".into(), serde_json::json!("move"));
            m.insert("x".into(), serde_json::json!(50.0));
            m.insert("y".into(), serde_json::json!(50.0));
            m
        }))
        .await
        .expect("browser_mouse ok");
    let mouse_body = structured(&mouse_resp);
    assert_eq!(
        mouse_body["ok"],
        serde_json::json!(true),
        "body: {mouse_body}"
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
