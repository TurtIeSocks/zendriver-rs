//! Real-Chrome end-to-end test for the interception tool group.
//!
//! Gated behind BOTH `integration-tests` AND `interception` cargo features
//! AND marked `#[ignore]` so a default `cargo test` run never spawns
//! Chrome. To exercise it explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp \
//!     --features "integration-tests interception" \
//!     --test integration_interception -- --ignored
//! ```
//!
//! Smoke shape: open → goto example.com → `browser_intercept_add_rule`
//! (Block pattern `*.json`) → assert `_list_rules` returns one entry →
//! `_remove_rule(rule_id)` → assert `_list_rules` returns zero → close.
//!
//! Asserting that a request was actually blocked end-to-end requires more
//! plumbing (a controlled HTTP origin we can issue an XHR to from page JS
//! and observe the failure) — out of scope for v0; the per-rule actor
//! itself is covered by `crates/zendriver-interception` tests.

#![cfg(all(feature = "integration-tests", feature = "interception"))]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
#[ignore = "requires real Chrome; run with --features \"integration-tests interception\" -- --ignored"]
async fn end_to_end_add_list_remove_rule() {
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

    // 2. Navigate somewhere real so the tab has a session to attach the
    //    interception actor to. example.com is the standard cheap target.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!("https://example.com"));
            m
        }))
        .await
        .expect("browser_goto ok");

    // 3. Add a Block rule on `*.json`.
    let add_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_intercept_add_rule").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("pattern".into(), serde_json::json!("*.json"));
                m.insert("action".into(), serde_json::json!({ "kind": "block" }));
                m
            }),
        )
        .await
        .expect("browser_intercept_add_rule ok");
    let add_body = structured(&add_resp);
    let rule_id = add_body["rule_id"]
        .as_str()
        .expect("rule_id populated")
        .to_string();
    assert!(!rule_id.is_empty(), "rule_id empty; body: {add_body}");

    // 4. List should return exactly one entry with our rule.
    let list_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_intercept_list_rules")
                .with_arguments(Default::default()),
        )
        .await
        .expect("browser_intercept_list_rules ok");
    let list_body = structured(&list_resp);
    let rules = list_body["rules"].as_array().expect("rules array present");
    assert_eq!(rules.len(), 1, "rules: {rules:?}");
    assert_eq!(rules[0]["rule_id"].as_str(), Some(rule_id.as_str()));
    assert_eq!(rules[0]["pattern"].as_str(), Some("*.json"));
    assert_eq!(rules[0]["action_kind"].as_str(), Some("block"));

    // 5. Remove the rule by id.
    let remove_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_intercept_remove_rule").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("rule_id".into(), serde_json::json!(rule_id));
                m
            }),
        )
        .await
        .expect("browser_intercept_remove_rule ok");
    assert_eq!(structured(&remove_resp)["removed"], serde_json::json!(true));

    // 6. List should now be empty.
    let list_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_intercept_list_rules")
                .with_arguments(Default::default()),
        )
        .await
        .expect("browser_intercept_list_rules ok (post-remove)");
    let list_body = structured(&list_resp);
    let rules = list_body["rules"].as_array().expect("rules array present");
    assert!(rules.is_empty(), "expected empty rules; got: {rules:?}");

    // 7. clear_rules on an empty registry is a no-op.
    let clear_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_intercept_clear_rules")
                .with_arguments(Default::default()),
        )
        .await
        .expect("browser_intercept_clear_rules ok");
    assert_eq!(structured(&clear_resp)["cleared"], serde_json::json!(0));

    // 8. Close.
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
