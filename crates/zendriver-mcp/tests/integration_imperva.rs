//! Real-Chrome end-to-end test for `browser_solve_imperva`.
//!
//! Gated behind BOTH `integration-tests` AND `imperva` cargo features AND
//! marked `#[ignore]` so a default `cargo test` run never spawns Chrome. To
//! exercise it explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp \
//!     --features "integration-tests imperva" \
//!     --test integration_imperva -- --ignored
//! ```
//!
//! Smoke shape: open → goto an Imperva-protected page →
//! `browser_solve_imperva` (long timeout) → assert the outcome is one of the
//! four expected variants → close.
//!
//! ## Why this test is "best effort"
//!
//! The target is third-party infrastructure that can change shape at any
//! time — Imperva can rotate surfaces, the site can move behind a different
//! WAF, or drop Imperva entirely. The test accepts any of the four terminal
//! outcomes (`token_acquired`, `challenge_gone`, `already_clear`, `timeout`)
//! as a pass and only fails on a structural error (CAPTCHA-without-solver /
//! CDP / JS exception, which surface here as a `call_tool` failure). Use it
//! as a manual smoke when iterating on imperva integration, not as a CI gate.

#![cfg(all(feature = "integration-tests", feature = "imperva"))]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

/// Imperva's own marketing site commonly sits behind their bot-management
/// surface. Swap if it stops serving an Imperva challenge.
const IMPERVA_URL: &str = "https://www.imperva.com/";

#[tokio::test]
#[ignore = "requires real Chrome + reachable Imperva-protected page; run with --features \"integration-tests imperva\" -- --ignored"]
async fn end_to_end_solve_imperva() {
    let client = ()
        .serve(
            TokioChildProcess::new(Command::new(BIN_PATH).configure(|cmd| {
                cmd.arg("--log").arg("error");
            }))
            .expect("spawn child"),
        )
        .await
        .expect("rmcp client init");

    // 1. Launch headless Chrome (stealth is on by default — required for
    //    Imperva).
    client
        .call_tool(CallToolRequestParams::new("browser_open").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("headless".into(), serde_json::json!(true));
            m
        }))
        .await
        .expect("browser_open ok");

    // 2. Navigate to the protected page.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!(IMPERVA_URL));
            m
        }))
        .await
        .expect("browser_goto ok");

    // 3. Drive the bypass with a long timeout.
    let solve_resp = client
        .call_tool(
            CallToolRequestParams::new("browser_solve_imperva").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("timeout_ms".into(), serde_json::json!(45_000_u64));
                m
            }),
        )
        .await
        .expect("browser_solve_imperva ok");
    let body = structured(&solve_resp);
    let outcome = body["outcome"]
        .as_str()
        .expect("outcome populated")
        .to_string();
    assert!(
        matches!(
            outcome.as_str(),
            "token_acquired" | "challenge_gone" | "already_clear" | "timeout"
        ),
        "unexpected outcome `{outcome}`; body: {body}"
    );
    // `reese84` is populated only on `token_acquired`.
    if outcome == "token_acquired" {
        let token = body["reese84"]
            .as_str()
            .expect("reese84 populated on token_acquired outcome");
        assert!(!token.is_empty(), "reese84 empty; body: {body}");
    } else {
        assert!(
            body.get("reese84").is_none_or(serde_json::Value::is_null),
            "reese84 should not be set for outcome `{outcome}`; body: {body}"
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
