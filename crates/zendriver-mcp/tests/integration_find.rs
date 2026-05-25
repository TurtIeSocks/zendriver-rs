//! Real-Chrome end-to-end test for the find / find_all / element_state
//! tool group.
//!
//! Gated behind the `integration-tests` cargo feature AND marked
//! `#[ignore]` so a default `cargo test` run never spawns Chrome. To
//! exercise it explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp --features integration-tests --test integration_find -- --ignored
//! ```
//!
//! The test drives the binary over stdio and round-trips
//! `browser_open` → `browser_goto` → `browser_find` (h1) →
//! `browser_find_all` (a) → `browser_element_state` (h1, text_attrs) →
//! `browser_close`.

#![cfg(feature = "integration-tests")]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
#[ignore = "requires real Chrome; run with --features integration-tests -- --ignored"]
async fn end_to_end_find_on_example_com() {
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

    // 3. browser_find with css = "h1" → found=true, text contains "Example Domain".
    let find_resp = client
        .call_tool(CallToolRequestParams::new("browser_find").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("css".into(), serde_json::json!("h1"));
            m
        }))
        .await
        .expect("browser_find h1 ok");
    let find_body = structured(&find_resp);
    assert_eq!(
        find_body["found"],
        serde_json::json!(true),
        "browser_find h1 body: {find_body}"
    );
    let snippet = find_body["element"]["text_snippet"]
        .as_str()
        .expect("text_snippet is string");
    assert!(
        snippet.contains("Example Domain"),
        "h1 text_snippet should contain 'Example Domain', got: {snippet:?}"
    );
    // Tag probe should land "h1" — page is static, no stale issues.
    assert_eq!(
        find_body["element"]["tag"],
        serde_json::json!("h1"),
        "h1 descriptor tag: {find_body}"
    );

    // 4. browser_find_all with css = "a" → at least 1 link.
    let find_all_resp = client
        .call_tool(CallToolRequestParams::new("browser_find_all").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("css".into(), serde_json::json!("a"));
            m
        }))
        .await
        .expect("browser_find_all a ok");
    let find_all_body = structured(&find_all_resp);
    let elements = find_all_body["elements"]
        .as_array()
        .expect("elements is array");
    assert!(
        !elements.is_empty(),
        "example.com should have at least one anchor: {find_all_body}"
    );

    // 5. browser_element_state with include = "text_attrs" → text + attrs populated.
    let state_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_element_state").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("css".into(), serde_json::json!("h1"));
                m.insert("include".into(), serde_json::json!("text_attrs"));
                m
            }),
        )
        .await
        .expect("browser_element_state h1 ok");
    let state_body = structured(&state_resp);
    assert_eq!(
        state_body["exists"],
        serde_json::json!(true),
        "element_state h1 body: {state_body}"
    );
    let text = state_body["text"].as_str().expect("text populated");
    assert!(
        text.contains("Example Domain"),
        "element_state text_attrs should populate text with 'Example Domain', got: {text:?}"
    );
    // text_attrs preset SHOULD populate text/attrs/inner_html, and SHOULD
    // NOT populate visible / enabled / bounding_box / in_viewport.
    assert!(state_body["attrs"].is_object(), "attrs populated");
    assert!(state_body["inner_html"].is_string(), "inner_html populated");
    assert!(
        state_body["visible"].is_null() || !state_body.as_object().unwrap().contains_key("visible"),
        "text_attrs preset should NOT populate visible"
    );
    assert!(
        state_body["bounding_box"].is_null()
            || !state_body.as_object().unwrap().contains_key("bounding_box"),
        "text_attrs preset should NOT populate bounding_box"
    );

    // 6. Missing element → found:false (no error).
    let miss_resp = client
        .call_tool(CallToolRequestParams::new("browser_find").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("css".into(), serde_json::json!("#definitely-not-here"));
            // Tight timeout so we don't burn 5s on every run.
            m.insert("timeout_ms".into(), serde_json::json!(250));
            m
        }))
        .await
        .expect("browser_find miss ok (should not error)");
    let miss_body = structured(&miss_resp);
    assert_eq!(
        miss_body["found"],
        serde_json::json!(false),
        "missing element should report found=false: {miss_body}"
    );

    // 7. Close browser.
    client
        .call_tool(CallToolRequestParams::new("browser_close").with_arguments(Default::default()))
        .await
        .expect("browser_close ok");

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
