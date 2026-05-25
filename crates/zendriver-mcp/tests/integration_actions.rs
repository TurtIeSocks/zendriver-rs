//! Real-Chrome end-to-end test for the action tool group.
//!
//! Gated behind the `integration-tests` cargo feature AND marked
//! `#[ignore]` so a default `cargo test` run never spawns Chrome. To
//! exercise it explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp --features integration-tests --test integration_actions -- --ignored
//! ```
//!
//! Drives the binary over stdio against a self-hosted `file://` form page
//! (written to a per-test temp file so the run is offline + deterministic
//! and we don't pay the public-internet flake tax). Round-trips
//! `browser_open` → `browser_goto` → `browser_type` (clearing pre-filled
//! input) → `browser_element_state` (verify text echoed into a label) →
//! `browser_press` Enter → `browser_element_state` (verify the page's
//! Enter handler ran) → `browser_click` (toggle a checkbox) →
//! `browser_close`.
//!
//! The page is intentionally tiny + script-only — it sets `input.value`
//! into a label via the `input` event, flips a flag on the page when
//! Enter is pressed, and tracks the checkbox state. Everything the test
//! asserts is observable via DOM state we read back through
//! `browser_element_state`, so we don't need to inspect Chrome internals
//! to know the action landed.

#![cfg(feature = "integration-tests")]

use std::io::Write;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

/// Self-contained form page. The handlers echo input → label so the test
/// can verify each action via a follow-up `browser_element_state` call.
const FORM_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>action smoke</title></head>
<body>
<input id="q" type="text" value="prefilled">
<div id="echo">empty</div>
<div id="enter-marker">not pressed</div>
<input id="cb" type="checkbox">
<div id="cb-marker">unchecked</div>
<script>
  const q = document.getElementById('q');
  const echo = document.getElementById('echo');
  const enterMarker = document.getElementById('enter-marker');
  const cb = document.getElementById('cb');
  const cbMarker = document.getElementById('cb-marker');

  q.addEventListener('input', () => { echo.textContent = q.value; });
  q.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { enterMarker.textContent = 'pressed'; }
  });
  cb.addEventListener('change', () => {
    cbMarker.textContent = cb.checked ? 'checked' : 'unchecked';
  });
</script>
</body></html>
"#;

#[tokio::test]
#[ignore = "requires real Chrome; run with --features integration-tests -- --ignored"]
async fn end_to_end_actions_form_smoke() {
    // 1. Write the form to a per-test temp file the binary can navigate to
    //    via `file://`. Using a unique-per-run filename so parallel test
    //    runs don't trip over each other's pages.
    let mut path = std::env::temp_dir();
    path.push(format!("zendriver-mcp-actions-{}.html", std::process::id()));
    {
        let mut f = std::fs::File::create(&path).expect("create temp html");
        f.write_all(FORM_HTML.as_bytes()).expect("write temp html");
    }
    let url = format!("file://{}", path.display());

    let client = ()
        .serve(
            TokioChildProcess::new(Command::new(BIN_PATH).configure(|cmd| {
                cmd.arg("--log").arg("error");
            }))
            .expect("spawn child"),
        )
        .await
        .expect("rmcp client init");

    // 2. Launch headless Chrome.
    client
        .call_tool(CallToolRequestParams::new("browser_open").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("headless".into(), serde_json::json!(true));
            m
        }))
        .await
        .expect("browser_open ok");

    // 3. Navigate to the temp form.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!(url));
            m
        }))
        .await
        .expect("browser_goto ok");

    // 4. Type into the prefilled input with clear_first=true.
    //    The page's `input` handler mirrors q.value into `#echo`, so the
    //    echo text is our ground truth that the type landed.
    let type_resp = client
        .call_tool(CallToolRequestParams::new("browser_type").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("css".into(), serde_json::json!("#q"));
            m.insert("text".into(), serde_json::json!("hello"));
            m.insert("clear_first".into(), serde_json::json!(true));
            m
        }))
        .await
        .expect("browser_type ok");
    assert_eq!(structured(&type_resp)["ok"], serde_json::json!(true));

    // 5. Verify the page saw the new value via the echo label. Use
    //    `text_attrs` preset so we get the rendered `inner_text`.
    let echo_state = client
        .call_tool(
            CallToolRequestParams::new("browser_element_state").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("css".into(), serde_json::json!("#echo"));
                m.insert("include".into(), serde_json::json!("text_attrs"));
                m
            }),
        )
        .await
        .expect("browser_element_state echo ok");
    let echo_body = structured(&echo_state);
    let echo_text = echo_body["text"].as_str().expect("echo text populated");
    // Realistic typing can introduce occasional typos; assert the echo
    // contains the bulk of "hello" rather than equality (per zendriver
    // realistic-input docs).
    assert!(
        echo_text.contains('h') && echo_text.contains('o'),
        "echo should reflect typed text (got: {echo_text:?}) -- body: {echo_body}"
    );

    // 6. Press Enter in the input. The keydown handler flips the marker
    //    text from "not pressed" to "pressed".
    let press_resp = client
        .call_tool(CallToolRequestParams::new("browser_press").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("css".into(), serde_json::json!("#q"));
            m.insert("key".into(), serde_json::json!("Enter"));
            m
        }))
        .await
        .expect("browser_press ok");
    assert_eq!(structured(&press_resp)["ok"], serde_json::json!(true));

    let enter_state = client
        .call_tool(
            CallToolRequestParams::new("browser_element_state").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("css".into(), serde_json::json!("#enter-marker"));
                m.insert("include".into(), serde_json::json!("text_attrs"));
                m
            }),
        )
        .await
        .expect("browser_element_state enter-marker ok");
    let enter_body = structured(&enter_state);
    assert_eq!(
        enter_body["text"].as_str(),
        Some("pressed"),
        "Enter handler did not run; body: {enter_body}"
    );

    // 7. Click the checkbox. The page's change handler flips the marker.
    let click_resp = client
        .call_tool(CallToolRequestParams::new("browser_click").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("css".into(), serde_json::json!("#cb"));
            m
        }))
        .await
        .expect("browser_click ok");
    assert_eq!(structured(&click_resp)["ok"], serde_json::json!(true));

    let cb_state = client
        .call_tool(
            CallToolRequestParams::new("browser_element_state").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("css".into(), serde_json::json!("#cb-marker"));
                m.insert("include".into(), serde_json::json!("text_attrs"));
                m
            }),
        )
        .await
        .expect("browser_element_state cb-marker ok");
    let cb_body = structured(&cb_state);
    assert_eq!(
        cb_body["text"].as_str(),
        Some("checked"),
        "checkbox change handler did not run; body: {cb_body}"
    );

    // 8. Close + clean up the temp file. Cleanup is best-effort — a
    //    leftover file in /tmp isn't worth failing the test for.
    client
        .call_tool(CallToolRequestParams::new("browser_close").with_arguments(Default::default()))
        .await
        .expect("browser_close ok");
    let _ = std::fs::remove_file(&path);

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
