//! End-to-end HTTP smoke test.
//!
//! Spawns the compiled `zendriver-mcp` binary with `--http <addr>`,
//! connects to it via the rmcp streamable-HTTP client, and asserts that:
//!
//! 1. The `browser_status` tool is advertised in the server's tool list.
//! 2. A call to `browser_status` returns a structured JSON body whose
//!    `open` field is `false` (no browser launched).
//!
//! Run with `cargo test -p zendriver-mcp --test http_smoke`.
//!
//! # Assumed test port
//!
//! Binds 127.0.0.1:18765 — a high port unlikely to collide with running
//! services on a developer's machine. `axum::serve` does not surface the
//! bound address back over the CLI, so we cannot use `:0` and have the
//! kernel pick a free port (we would need a separate channel to learn the
//! chosen port). If 18765 is occupied the test will fail at the bind
//! step inside the spawned binary, surfacing as a connect-refused on the
//! client side after the polling window expires.
//!
//! # Child cleanup
//!
//! [`ChildGuard`] kills the spawned binary on drop so a panic mid-test
//! does not leak the process. `client.cancel().await` is best-effort —
//! the kill is the load-bearing cleanup.

use std::process::Stdio;
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use tokio::process::{Child, Command};

/// Cargo sets `CARGO_BIN_EXE_<bin name>` for integration tests in the
/// owning crate, pointing at the freshly-built binary on disk.
const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

/// Port to bind for the test. Documented in the module comment.
const TEST_ADDR: &str = "127.0.0.1:18765";

/// RAII guard that kills the spawned child on drop. Without it, a panic
/// during assertion would leak the test binary (and the bound port).
struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            // `start_kill` is the sync-friendly variant — usable from a
            // Drop impl. Best-effort: we cannot await the wait() here.
            let _ = child.start_kill();
        }
    }
}

/// Poll the TCP port until the server is ready to accept connections.
///
/// Without this, the rmcp client would race the server's `bind()` call
/// and the initial connect would fail intermittently. Returns once a
/// connect succeeds (the server is listening) or after `attempts` tries.
async fn wait_for_server_ready(addr: &str, attempts: usize) -> std::io::Result<()> {
    let mut last_err = None;
    for _ in 0..attempts {
        match tokio::net::TcpStream::connect(addr).await {
            Ok(_) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "no attempts")))
}

#[tokio::test]
async fn browser_status_round_trip_over_http() {
    let child = Command::new(BIN_PATH)
        .arg("--http")
        .arg(TEST_ADDR)
        .arg("--log")
        .arg("error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn binary with --http");
    let _guard = ChildGuard(Some(child));

    wait_for_server_ready(TEST_ADDR, 50)
        .await
        .expect("zendriver-mcp HTTP server never came up");

    let uri = format!("http://{TEST_ADDR}/mcp");
    let transport = StreamableHttpClientTransport::from_uri(uri.as_str());

    let client = ().serve(transport).await.expect("rmcp HTTP client init");

    let tools = client.list_all_tools().await.expect("list tools");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&"browser_status"),
        "browser_status should be advertised; tools: {names:?}"
    );

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

    client.cancel().await.ok();
    // _guard drops here → child is killed.
}
