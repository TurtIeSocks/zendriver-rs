//! Real-Chrome end-to-end test for the snapshot + eval tool groups.
//!
//! Gated behind the `integration-tests` cargo feature AND marked
//! `#[ignore]` so a default `cargo test` run never spawns Chrome. To
//! exercise it explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp --features integration-tests --test integration_snapshot -- --ignored
//! ```
//!
//! Round-trips:
//! `browser_open` → `browser_goto` → `browser_html` (full) →
//! `browser_html` (selector h1) → `browser_screenshot` (default PNG) →
//! `browser_evaluate` (1+2) → `browser_evaluate_main` (document.title) →
//! `browser_close`.

#![cfg(feature = "integration-tests")]

use base64::Engine as _;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
#[ignore = "requires real Chrome; run with --features integration-tests -- --ignored"]
async fn end_to_end_snapshot_on_example_com() {
    let client = ()
        .serve(
            TokioChildProcess::new(Command::new(BIN_PATH).configure(|cmd| {
                cmd.arg("--log").arg("error");
            }))
            .expect("spawn child"),
        )
        .await
        .expect("rmcp client init");

    // 1. Launch headless Chrome.
    client
        .call_tool(CallToolRequestParams::new("browser_open").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("headless".into(), serde_json::json!(true));
            m
        }))
        .await
        .expect("browser_open ok");

    // 2. Navigate to example.com.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!("https://example.com"));
            m
        }))
        .await
        .expect("browser_goto ok");

    // 3. browser_html (no selector, trimmed) → non-empty, contains <h1>.
    let html_resp = client
        .call_tool(CallToolRequestParams::new("browser_html").with_arguments(Default::default()))
        .await
        .expect("browser_html ok");
    let html_text = text(&html_resp);
    assert!(
        !html_text.is_empty(),
        "browser_html (full) should return non-empty text"
    );
    assert!(
        html_text.contains("<h1"),
        "browser_html (full) should contain `<h1`, got: {html_text:.200?}"
    );

    // 4. browser_html with selector h1 → returns the h1's innerHTML, which
    //    is the text "Example Domain" (no nested tags on example.com).
    let h1_resp = client
        .call_tool(CallToolRequestParams::new("browser_html").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("selector".into(), serde_json::json!({ "css": "h1" }));
            m
        }))
        .await
        .expect("browser_html h1 ok");
    let h1_text = text(&h1_resp);
    assert!(
        h1_text.contains("Example Domain"),
        "browser_html selector=h1 innerHTML should contain 'Example Domain', got: {h1_text:?}"
    );
    // Sanity: the h1 innerHTML is a substring of the full body (the full
    // page contains the h1, after all).
    assert!(
        html_text.contains(&h1_text),
        "h1 innerHTML should be a substring of full page HTML"
    );

    // 5. browser_screenshot (default PNG) → returns non-empty image bytes.
    let shot_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_screenshot").with_arguments(Default::default()),
        )
        .await
        .expect("browser_screenshot ok");
    let image = shot_resp
        .content
        .iter()
        .find_map(|c| c.as_image())
        .expect("screenshot must include an image content block");
    assert_eq!(image.mime_type, "image/png", "default format is PNG");
    // base64-decode the inline data and confirm we got real PNG bytes (PNG
    // magic header is `\x89PNG\r\n\x1a\n`).
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.data)
        .expect("inline image data is valid base64");
    assert!(
        bytes.len() > 100,
        "PNG screenshot should be substantially larger than 100 bytes, got {}",
        bytes.len()
    );
    assert_eq!(
        &bytes[..8],
        b"\x89PNG\r\n\x1a\n",
        "decoded screenshot must start with the PNG magic header"
    );
    // structured_content carries the metadata mirror.
    let meta = shot_resp
        .structured_content
        .as_ref()
        .expect("structured_content populated");
    assert_eq!(meta["format"], serde_json::json!("png"));
    assert_eq!(meta["byte_len"], serde_json::json!(bytes.len()));

    // 6. browser_evaluate `1 + 2` → 3.
    let eval_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_evaluate").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("expression".into(), serde_json::json!("1 + 2"));
                m
            }),
        )
        .await
        .expect("browser_evaluate ok");
    let eval_body = structured(&eval_resp);
    assert_eq!(
        eval_body["value"],
        serde_json::json!(3),
        "browser_evaluate body: {eval_body}"
    );

    // 7. browser_evaluate_main `document.title` → "Example Domain".
    let title_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_evaluate_main").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("expression".into(), serde_json::json!("document.title"));
                m
            }),
        )
        .await
        .expect("browser_evaluate_main ok");
    let title_body = structured(&title_resp);
    assert_eq!(
        title_body["value"],
        serde_json::json!("Example Domain"),
        "browser_evaluate_main body: {title_body}"
    );

    // 8. Close.
    client
        .call_tool(CallToolRequestParams::new("browser_close").with_arguments(Default::default()))
        .await
        .expect("browser_close ok");

    client.cancel().await.expect("clean shutdown");
}

/// Pull the first text content block out of a tool call result.
fn text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .find_map(|c| c.as_text())
        .map(|t| t.text.clone())
        .expect("text content present")
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
