//! Real-Chrome end-to-end test for the expect *drive* extensions — dialog
//! accept. Gated behind `integration-tests` AND `expect`, marked `#[ignore]`.
//!
//! ```bash
//! cargo test -p zendriver-mcp --features "integration-tests expect" \
//!     --test integration_expect_drive -- --ignored
//! ```

#![cfg(all(feature = "integration-tests", feature = "expect"))]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
#[ignore = "requires real Chrome; run with --features \"integration-tests expect\" -- --ignored"]
async fn end_to_end_dialog_accept() {
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
            m.insert(
                "url".into(),
                serde_json::json!("data:text/html,<body>dialog</body>"),
            );
            m
        }))
        .await
        .expect("browser_goto ok");

    // Register a dialog expectation that ACCEPTS the dialog. The subscriber is
    // active by the time register returns, so the later confirm() is caught.
    let reg = client
        .call_tool(
            CallToolRequestParams::new("browser_expect_register").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("kind".into(), serde_json::json!("dialog"));
                m.insert("dialog_action".into(), serde_json::json!("accept"));
                m
            }),
        )
        .await
        .expect("browser_expect_register ok");
    let reg_body = structured(&reg);
    let id = reg_body["expectation_id"]
        .as_str()
        .expect("expectation_id")
        .to_string();

    // Trigger a confirm(); the spawned task accepts it, so this resolves true.
    let eval = client
        .call_tool(
            CallToolRequestParams::new("browser_evaluate_main").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("expression".into(), serde_json::json!("confirm('ok?')"));
                m
            }),
        )
        .await
        .expect("browser_evaluate_main ok");
    let eval_body = structured(&eval);
    assert_eq!(
        eval_body["value"],
        serde_json::json!(true),
        "accepted confirm() returns true; body: {eval_body}"
    );

    // Await the matched dialog event; it should report it was driven `accept`.
    let awaited = client
        .call_tool(
            CallToolRequestParams::new("browser_expect_await").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("expectation_id".into(), serde_json::json!(id));
                m.insert("timeout_ms".into(), serde_json::json!(10_000_u64));
                m
            }),
        )
        .await
        .expect("browser_expect_await ok");
    let ev = structured(&awaited);
    assert_eq!(ev["event"]["kind"], serde_json::json!("dialog"), "ev: {ev}");
    assert_eq!(
        ev["event"]["dialog_type"],
        serde_json::json!("confirm"),
        "ev: {ev}"
    );
    assert_eq!(
        ev["event"]["driven"],
        serde_json::json!("accept"),
        "ev: {ev}"
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
