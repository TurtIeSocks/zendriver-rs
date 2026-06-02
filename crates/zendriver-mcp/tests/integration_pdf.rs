//! Real-Chrome end-to-end test for `browser_pdf` / `browser_save_mhtml`.
//!
//! Gated behind `integration-tests` AND marked `#[ignore]`. Run with:
//!
//! ```bash
//! cargo test -p zendriver-mcp --features integration-tests \
//!     --test integration_pdf -- --ignored
//! ```

#![cfg(feature = "integration-tests")]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
#[ignore = "requires real Chrome; run with --features integration-tests -- --ignored"]
async fn end_to_end_pdf_export() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pdf_path = dir.path().join("page.pdf");

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
            m.insert("url".into(), serde_json::json!("https://example.com"));
            m
        }))
        .await
        .expect("browser_goto ok");

    // Export to a temp path.
    let pdf_resp = client
        .call_tool(CallToolRequestParams::new("browser_pdf").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert(
                "save_path".into(),
                serde_json::json!(pdf_path.to_string_lossy()),
            );
            m
        }))
        .await
        .expect("browser_pdf ok");
    let body = structured(&pdf_resp);
    assert!(
        body["byte_len"].as_u64().unwrap_or(0) > 0,
        "byte_len should be > 0; body: {body}"
    );
    assert!(body["saved_path"].is_string(), "saved_path; body: {body}");
    let bytes = std::fs::read(&pdf_path).expect("read pdf");
    assert!(bytes.starts_with(b"%PDF"), "file should be a PDF");

    // MHTML inline (no save_path) returns base64.
    let mhtml_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_save_mhtml").with_arguments(Default::default()),
        )
        .await
        .expect("browser_save_mhtml ok");
    let mhtml_body = structured(&mhtml_resp);
    assert!(
        mhtml_body["base64"].is_string(),
        "mhtml base64; body: {mhtml_body}"
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
