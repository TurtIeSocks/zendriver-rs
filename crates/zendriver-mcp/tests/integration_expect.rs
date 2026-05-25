//! Real-Chrome end-to-end test for the expect tool group.
//!
//! Gated behind BOTH `integration-tests` AND `expect` cargo features AND
//! marked `#[ignore]` so a default `cargo test` run never spawns Chrome.
//! To exercise it explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp \
//!     --features "integration-tests expect" \
//!     --test integration_expect -- --ignored
//! ```
//!
//! Smoke shape: open → register(Response, url_substr: "example.com") →
//! returns id → goto example.com → await(id, 5000) → assert the event
//! carries `url` containing "example.com" → close.
//!
//! A separate `register_then_cancel` test exercises the cancel path
//! without needing the network event to fire — it just confirms the
//! task is torn down without ever resolving.

#![cfg(all(feature = "integration-tests", feature = "expect"))]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
#[ignore = "requires real Chrome; run with --features \"integration-tests expect\" -- --ignored"]
async fn register_response_then_goto_then_await_resolves_with_event() {
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

    // 2. Register a Response expectation BEFORE the goto so the
    //    subscriber is live when example.com's response arrives. Default
    //    pre-await timeout (60s) is fine — we just need it longer than
    //    the navigation.
    let reg_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_expect_register").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("kind".into(), serde_json::json!("response"));
                m.insert(
                    "matcher".into(),
                    serde_json::json!({ "url_substr": "example.com" }),
                );
                m
            }),
        )
        .await
        .expect("browser_expect_register ok");
    let reg_body = structured(&reg_resp);
    let expectation_id = reg_body["expectation_id"]
        .as_str()
        .expect("expectation_id populated")
        .to_string();
    assert!(
        !expectation_id.is_empty(),
        "expectation_id empty; body: {reg_body}",
    );

    // 3. Trigger the action that produces the awaited event.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!("https://example.com"));
            m
        }))
        .await
        .expect("browser_goto ok");

    // 4. Await the expectation. 5s outer timeout is generous given the
    //    page already loaded above — the response has long since fired.
    let await_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_expect_await").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("expectation_id".into(), serde_json::json!(expectation_id));
                m.insert("timeout_ms".into(), serde_json::json!(5_000));
                m
            }),
        )
        .await
        .expect("browser_expect_await ok");
    let await_body = structured(&await_resp);
    assert_eq!(
        await_body["expectation_id"].as_str(),
        Some(expectation_id.as_str()),
    );
    let event = &await_body["event"];
    assert_eq!(event["kind"].as_str(), Some("response"), "event: {event}");
    let url = event["url"].as_str().expect("event.url present");
    assert!(
        url.contains("example.com"),
        "event url should contain example.com; got {url}",
    );

    // 5. Close.
    client
        .call_tool(CallToolRequestParams::new("browser_close").with_arguments(Default::default()))
        .await
        .expect("browser_close ok");
    client.cancel().await.expect("clean shutdown");
}

#[tokio::test]
#[ignore = "requires real Chrome; run with --features \"integration-tests expect\" -- --ignored"]
async fn register_then_cancel_drops_the_expectation() {
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
    // current_tab needs a navigated tab; about:blank suffices.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!("about:blank"));
            m
        }))
        .await
        .expect("browser_goto ok");

    // Register a Dialog expectation — nothing ever opens a dialog here,
    // so the only way for this test to terminate is via the cancel path.
    let reg_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_expect_register").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("kind".into(), serde_json::json!("dialog"));
                m.insert("pre_await_timeout_ms".into(), serde_json::json!(60_000));
                m
            }),
        )
        .await
        .expect("browser_expect_register ok");
    let expectation_id = structured(&reg_resp)["expectation_id"]
        .as_str()
        .expect("expectation_id populated")
        .to_string();

    let cancel_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_expect_cancel").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert(
                    "expectation_id".into(),
                    serde_json::json!(expectation_id.clone()),
                );
                m
            }),
        )
        .await
        .expect("browser_expect_cancel ok");
    assert_eq!(
        structured(&cancel_resp)["cancelled"],
        serde_json::json!(true),
    );

    // Awaiting the same id now must surface ExpectationNotFound — the
    // cancel path removed it from the registry.
    let await_res = client
        .call_tool(
            CallToolRequestParams::new("browser_expect_await").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("expectation_id".into(), serde_json::json!(expectation_id));
                m.insert("timeout_ms".into(), serde_json::json!(500));
                m
            }),
        )
        .await;
    let err = await_res.expect_err("await on cancelled id should error");
    assert!(
        format!("{err:?}").contains("not found"),
        "expected 'not found' in error; got {err:?}",
    );

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
