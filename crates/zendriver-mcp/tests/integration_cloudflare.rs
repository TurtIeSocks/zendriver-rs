//! Real-Chrome end-to-end test for `browser_solve_turnstile`.
//!
//! Gated behind BOTH `integration-tests` AND `cloudflare` cargo features
//! AND marked `#[ignore]` so a default `cargo test` run never spawns
//! Chrome. To exercise it explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp \
//!     --features "integration-tests cloudflare" \
//!     --test integration_cloudflare -- --ignored
//! ```
//!
//! Smoke shape: open → goto a known Turnstile-protected page →
//! `browser_solve_turnstile` (long timeout) → assert the outcome is one
//! of the three expected variants → close.
//!
//! ## Why this test is "best effort"
//!
//! The target page is third-party infrastructure that can change shape at
//! any time — Cloudflare can rotate Turnstile placements, the
//! demonstration site can move, or the site can stop using Turnstile
//! entirely. The test therefore accepts any of the three terminal
//! outcomes (`solved`, `challenge_gone`, `timeout`) as a pass and only
//! fails on a structural error (`no challenge detected` is mapped to a
//! hard error, which surfaces here as a `call_tool` failure). Use it as
//! a manual smoke when iterating on cloudflare integration, not as a CI
//! gate.

#![cfg(all(feature = "integration-tests", feature = "cloudflare"))]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

/// Public Turnstile demonstration page maintained by Cloudflare. As of
/// 2026 still serves a managed Turnstile challenge on first visit.
const TURNSTILE_DEMO_URL: &str = "https://nopecha.com/demo/cloudflare";

#[tokio::test]
#[ignore = "requires real Chrome + reachable Turnstile demo page; run with --features \"integration-tests cloudflare\" -- --ignored"]
async fn end_to_end_solve_turnstile() {
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

    // 2. Navigate to a known Turnstile-protected page. We don't wait for
    //    idle — the challenge mounts asynchronously and the bypass driver
    //    has its own poll loop.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!(TURNSTILE_DEMO_URL));
            m
        }))
        .await
        .expect("browser_goto ok");

    // 3. Drive the bypass with a long timeout so a slow challenge still
    //    has room to resolve.
    let solve_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_solve_turnstile").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("timeout_ms".into(), serde_json::json!(45_000_u64));
                m
            }),
        )
        .await
        .expect("browser_solve_turnstile ok");
    let body = structured(&solve_resp);
    let outcome = body["outcome"]
        .as_str()
        .expect("outcome populated")
        .to_string();
    assert!(
        matches!(outcome.as_str(), "solved" | "challenge_gone" | "timeout"),
        "unexpected outcome `{outcome}`; body: {body}"
    );
    // When `solved`, `token` should be a non-empty string; otherwise it
    // should be absent (serialize-skip on `None`).
    if outcome == "solved" {
        let token = body["token"]
            .as_str()
            .expect("token populated on solved outcome");
        assert!(!token.is_empty(), "token empty; body: {body}");
    } else {
        assert!(
            body.get("token").is_none_or(serde_json::Value::is_null),
            "token should not be set for outcome `{outcome}`; body: {body}"
        );
    }

    // 4. Close.
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
