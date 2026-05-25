//! End-to-end stdio smoke test.
//!
//! Spawns the compiled `zendriver-mcp` binary as a child process, talks
//! to it as an rmcp client over stdio, and asserts that:
//!
//! 1. The `browser_status` tool is advertised in the server's tool list.
//! 2. A call to `browser_status` returns a structured JSON body whose
//!    `open` field is `false` (no browser launched).
//!
//! Run with `cargo test -p zendriver-mcp --test stdio_smoke`.

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

/// Cargo sets `CARGO_BIN_EXE_<bin name>` for integration tests in the
/// owning crate, pointing at the freshly-built binary on disk.
const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
async fn browser_status_round_trip_over_stdio() {
    let client = ()
        .serve(
            TokioChildProcess::new(Command::new(BIN_PATH).configure(|cmd| {
                cmd.arg("--log").arg("error");
            }))
            .expect("spawn child"),
        )
        .await
        .expect("rmcp client init");

    let tools = client.list_all_tools().await.expect("list tools");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&"browser_status"),
        "browser_status should be advertised; tools: {names:?}"
    );
    // The find / element_state surface lands in this dispatch — make sure
    // the wiring landed in `server.rs` and not just in the per-module
    // source. A missing delegator would silently drop the tool from the
    // advertised list.
    for expected in ["browser_find", "browser_find_all", "browser_element_state"] {
        assert!(
            names.contains(&expected),
            "{expected} should be advertised; tools: {names:?}"
        );
    }

    let result = client
        .call_tool(
            CallToolRequestParams::new("browser_status").with_arguments(serde_json::Map::new()),
        )
        .await
        .expect("call browser_status");

    // Prefer the structured payload (rmcp emits it when the tool returns
    // `Json<T>`). Fall back to parsing the unstructured text content if
    // the structured slot is absent.
    let body: serde_json::Value = match result.structured_content.clone() {
        Some(v) => v,
        None => {
            let text = result
                .content
                .iter()
                .find_map(|c| c.as_text())
                .map(|t| t.text.clone())
                .expect("text content present");
            serde_json::from_str(&text).expect("text payload is JSON")
        }
    };

    assert_eq!(body["open"], serde_json::json!(false), "body: {body}");
    assert_eq!(body["tab_count"], serde_json::json!(0), "body: {body}");
    assert!(body["current_tab"].is_null(), "body: {body}");
    assert_eq!(body["profile"], serde_json::json!("auto"), "body: {body}");

    client.cancel().await.expect("clean shutdown");
}
