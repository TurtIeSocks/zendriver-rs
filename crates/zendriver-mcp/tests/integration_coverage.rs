//! Real-Chrome end-to-end tests for the new T1-T8 tools:
//! `browser_request`, `browser_monitor_start/read/stop`, and
//! `browser_fingerprint_generate` → `browser_open { persona }`.
//!
//! Gated behind `integration-tests`, `monitor`, AND `fingerprints` cargo
//! features, and marked `#[ignore]` so a default `cargo test` run never
//! spawns Chrome.  To exercise them explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp \
//!     --features "integration-tests monitor fingerprints" \
//!     --test integration_coverage -- --ignored
//! ```
//!
//! All three tests share the same over-the-wire harness established by
//! `integration_interception.rs` and `integration_expect.rs`:
//!   - spawn the `zendriver-mcp` binary as a child process (stdio MCP),
//!   - drive it via an rmcp client,
//!   - call tools by name, inspect the JSON payload, then close.

#![cfg(all(
    feature = "integration-tests",
    feature = "monitor",
    feature = "fingerprints"
))]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

// ---------------------------------------------------------------------------
// Test 1: browser_request GET + POST JSON
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires real Chrome; run with --features \"integration-tests monitor fingerprints\" -- --ignored"]
async fn browser_request_get_and_post_json() {
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

    // 2. Navigate to httpbin so requests have a stable same-origin session.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!("https://httpbin.org/get"));
            m
        }))
        .await
        .expect("browser_goto ok");

    // 3. GET https://httpbin.org/get → expect status 200 and a non-empty body.
    let get_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_request").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("method".into(), serde_json::json!("GET"));
                m.insert("url".into(), serde_json::json!("https://httpbin.org/get"));
                m
            }),
        )
        .await
        .expect("browser_request GET ok");
    let get_body = structured(&get_resp);
    assert_eq!(
        get_body["status"].as_u64(),
        Some(200),
        "GET /get should return 200; body: {get_body}",
    );
    let body_str = get_body["body"].as_str().expect("body field present");
    assert!(
        !body_str.is_empty(),
        "response body should not be empty; body: {get_body}",
    );
    // httpbin echoes the request URL back in JSON — spot-check
    assert!(
        body_str.contains("httpbin.org"),
        "GET body should mention httpbin.org; got: {body_str}",
    );

    // 4. POST https://httpbin.org/post with a JSON payload.
    let post_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_request").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("method".into(), serde_json::json!("POST"));
                m.insert("url".into(), serde_json::json!("https://httpbin.org/post"));
                m.insert("json".into(), serde_json::json!({"hello": "world"}));
                m
            }),
        )
        .await
        .expect("browser_request POST ok");
    let post_body = structured(&post_resp);
    assert_eq!(
        post_body["status"].as_u64(),
        Some(200),
        "POST /post should return 200; body: {post_body}",
    );
    // httpbin echoes the JSON payload under the "json" key
    let echo_body = post_body["body"].as_str().expect("body field present");
    assert!(
        echo_body.contains("hello"),
        "POST body echo should contain 'hello'; got: {echo_body}",
    );

    // 5. Close.
    client
        .call_tool(CallToolRequestParams::new("browser_close").with_arguments(Default::default()))
        .await
        .expect("browser_close ok");
    client.cancel().await.expect("clean shutdown");
}

// ---------------------------------------------------------------------------
// Test 2: browser_monitor_start → trigger fetch → browser_monitor_read sees Http event → stop
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires real Chrome; run with --features \"integration-tests monitor fingerprints\" -- --ignored"]
async fn browser_monitor_start_read_stop() {
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

    // 2. Navigate to example.com to establish a stable session.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!("https://example.com"));
            m
        }))
        .await
        .expect("browser_goto ok");

    // 3. Start the network monitor, filtering for "httpbin" so we don't
    //    drown in example.com noise.
    let start_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_monitor_start").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("url_pattern".into(), serde_json::json!("httpbin.org"));
                m
            }),
        )
        .await
        .expect("browser_monitor_start ok");
    let start_body = structured(&start_resp);
    let handle = start_body["handle"]
        .as_str()
        .expect("handle present")
        .to_string();
    assert!(
        !handle.is_empty(),
        "handle must be non-empty; body: {start_body}"
    );

    // 4. Trigger a fetch to httpbin from within the page via browser_evaluate.
    client
        .call_tool(
            CallToolRequestParams::new("browser_evaluate").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert(
                    "expression".into(),
                    serde_json::json!("fetch('https://httpbin.org/get').then(r => r.text())"),
                );
                m
            }),
        )
        .await
        .expect("browser_evaluate ok");

    // 5. Give the drain task a moment to observe the exchange, then read.
    //    The fetch is async; a brief pause lets the browser complete the
    //    request before we poll.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let read_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_monitor_read").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("handle".into(), serde_json::json!(handle));
                m
            }),
        )
        .await
        .expect("browser_monitor_read ok");
    let read_body = structured(&read_resp);
    let events = read_body["events"]
        .as_array()
        .expect("events array present");
    assert!(
        !events.is_empty(),
        "monitor should have captured at least one Http event from the fetch; body: {read_body}",
    );
    // Verify at least one event is an Http event for httpbin.org
    let http_ev = events.iter().find(|ev| {
        ev["kind"].as_str() == Some("http")
            && ev["url"]
                .as_str()
                .map(|u| u.contains("httpbin.org"))
                .unwrap_or(false)
    });
    assert!(
        http_ev.is_some(),
        "expected an Http event for httpbin.org; got events: {events:?}",
    );

    // 6. Stop the monitor.
    let stop_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_monitor_stop").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("handle".into(), serde_json::json!(handle));
                m
            }),
        )
        .await
        .expect("browser_monitor_stop ok");
    assert_eq!(structured(&stop_resp)["stopped"], serde_json::json!(true));

    // 7. Close.
    client
        .call_tool(CallToolRequestParams::new("browser_close").with_arguments(Default::default()))
        .await
        .expect("browser_close ok");
    client.cancel().await.expect("clean shutdown");
}

// ---------------------------------------------------------------------------
// Test 3: browser_fingerprint_generate { source: "generative", seed: 1 }
//         → persona JSON → browser_open { persona } (launch succeeds)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires real Chrome; run with --features \"integration-tests monitor fingerprints\" -- --ignored"]
async fn fingerprint_generate_then_browser_open_with_persona() {
    let client = ()
        .serve(
            TokioChildProcess::new(Command::new(BIN_PATH).configure(|cmd| {
                cmd.arg("--log").arg("error");
            }))
            .expect("spawn child"),
        )
        .await
        .expect("rmcp client init");

    // 1. Generate a persona (generative source, seed=1 for reproducibility).
    let gen_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_fingerprint_generate").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("source".into(), serde_json::json!("generative"));
                m.insert("seed".into(), serde_json::json!(1));
                m
            }),
        )
        .await
        .expect("browser_fingerprint_generate ok");
    let gen_body = structured(&gen_resp);
    let persona = gen_body["persona"].clone();
    assert!(
        persona.is_object(),
        "persona must be a JSON object; body: {gen_body}",
    );

    // 2. Open the browser with the generated persona.
    //    A successful launch (no error returned) proves the persona is accepted.
    let open_resp = client
        .call_tool(CallToolRequestParams::new("browser_open").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("headless".into(), serde_json::json!(true));
            m.insert("persona".into(), persona);
            m
        }))
        .await
        .expect("browser_open with persona ok");
    let open_body = structured(&open_resp);
    // headless=true should be reflected in the output
    assert_eq!(
        open_body["headless"],
        serde_json::json!(true),
        "headless flag reflected; body: {open_body}",
    );

    // 3. Close.
    client
        .call_tool(CallToolRequestParams::new("browser_close").with_arguments(Default::default()))
        .await
        .expect("browser_close ok");
    client.cancel().await.expect("clean shutdown");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Pull the structured payload out of a tool call result, falling back to
/// parsing the text content slot when rmcp did not emit a structured one.
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
