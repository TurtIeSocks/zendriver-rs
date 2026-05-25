//! Real-Chrome end-to-end test for the cookie + storage tool groups.
//!
//! Gated behind the `integration-tests` cargo feature AND marked
//! `#[ignore]` so a default `cargo test` run never spawns Chrome. To
//! exercise it explicitly:
//!
//! ```bash
//! cargo test -p zendriver-mcp --features integration-tests --test integration_cookies_storage -- --ignored
//! ```
//!
//! Cookie flow: open → goto httpbin /cookies/set/foo/bar → cookies_get
//! confirms `foo=bar` is present → cookies_persist(save, tempfile) →
//! cookies_clear → cookies_get returns no `foo` → cookies_persist(load,
//! tempfile) → cookies_get returns `foo` again.
//!
//! Storage flow: goto example.com → storage_set(local key1 val1) →
//! storage_get(local key1) returns val1 → storage_clear(local) →
//! storage_get(local) returns empty.
//!
//! Both flows run in the same Chrome instance to amortize the launch cost
//! (Chrome cold-boot dwarfs the per-tool latency we're verifying).

#![cfg(feature = "integration-tests")]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;

const BIN_PATH: &str = env!("CARGO_BIN_EXE_zendriver-mcp");

#[tokio::test]
#[ignore = "requires real Chrome; run with --features integration-tests -- --ignored"]
async fn end_to_end_cookies_storage_lifecycle() {
    let client = ()
        .serve(
            TokioChildProcess::new(Command::new(BIN_PATH).configure(|cmd| {
                cmd.arg("--log").arg("error");
            }))
            .expect("spawn child"),
        )
        .await
        .expect("rmcp client init");

    // Distinct tempfile per process so concurrent test invocations don't
    // clobber each other.
    let tempfile = std::env::temp_dir().join(format!(
        "zendriver-mcp-it-cookies-{}.json",
        std::process::id()
    ));
    let _ = tokio::fs::remove_file(&tempfile).await;

    // 1. Launch Chrome.
    client
        .call_tool(CallToolRequestParams::new("browser_open").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("headless".into(), serde_json::json!(true));
            m
        }))
        .await
        .expect("browser_open ok");

    // 2. Visit httpbin's cookie-setter; the server responds with
    // `Set-Cookie: foo=bar; Path=/`, which Chrome commits before
    // navigation completes.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert(
                "url".into(),
                serde_json::json!("https://httpbin.org/cookies/set/foo/bar"),
            );
            m.insert("wait_for".into(), serde_json::json!("idle"));
            m
        }))
        .await
        .expect("browser_goto ok");

    // 3. cookies_get returns at least one cookie named `foo` with value `bar`.
    let get_initial = client
        .call_tool(
            CallToolRequestParams::new("browser_cookies_get").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("name".into(), serde_json::json!("foo"));
                m
            }),
        )
        .await
        .expect("browser_cookies_get ok");
    let initial_body = structured(&get_initial);
    let initial_arr = initial_body["cookies"]
        .as_array()
        .expect("cookies field is array");
    assert_eq!(
        initial_arr.len(),
        1,
        "expected exactly one foo cookie: {initial_body}"
    );
    assert_eq!(
        initial_arr[0]["name"].as_str(),
        Some("foo"),
        "cookie name: {initial_body}"
    );
    assert_eq!(
        initial_arr[0]["value"].as_str(),
        Some("bar"),
        "cookie value: {initial_body}"
    );

    // 4. Persist (save).
    let save = client
        .call_tool(
            CallToolRequestParams::new("browser_cookies_persist").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("direction".into(), serde_json::json!("save"));
                m.insert(
                    "path".into(),
                    serde_json::json!(tempfile.to_string_lossy().into_owned()),
                );
                m
            }),
        )
        .await
        .expect("browser_cookies_persist save ok");
    let save_body = structured(&save);
    assert_eq!(save_body["direction"], serde_json::json!("save"));
    assert!(
        save_body["count"].as_u64().unwrap_or(0) >= 1,
        "save count: {save_body}"
    );

    // 5. Clear everything.
    let clear = client
        .call_tool(
            CallToolRequestParams::new("browser_cookies_clear").with_arguments(Default::default()),
        )
        .await
        .expect("browser_cookies_clear ok");
    assert_eq!(structured(&clear)["ok"], serde_json::json!(true));

    // 6. cookies_get(name=foo) now returns empty.
    let get_after_clear = client
        .call_tool(
            CallToolRequestParams::new("browser_cookies_get").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("name".into(), serde_json::json!("foo"));
                m
            }),
        )
        .await
        .expect("browser_cookies_get ok");
    let cleared = structured(&get_after_clear);
    assert_eq!(
        cleared["cookies"].as_array().map(Vec::len),
        Some(0),
        "after clear: {cleared}"
    );

    // 7. Persist (load) — restore from tempfile.
    let load = client
        .call_tool(
            CallToolRequestParams::new("browser_cookies_persist").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("direction".into(), serde_json::json!("load"));
                m.insert(
                    "path".into(),
                    serde_json::json!(tempfile.to_string_lossy().into_owned()),
                );
                m
            }),
        )
        .await
        .expect("browser_cookies_persist load ok");
    let load_body = structured(&load);
    assert_eq!(load_body["direction"], serde_json::json!("load"));

    // 8. cookies_get(name=foo) returns the restored cookie.
    let get_after_load = client
        .call_tool(
            CallToolRequestParams::new("browser_cookies_get").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("name".into(), serde_json::json!("foo"));
                m
            }),
        )
        .await
        .expect("browser_cookies_get ok");
    let restored = structured(&get_after_load);
    let restored_arr = restored["cookies"]
        .as_array()
        .expect("cookies field is array");
    assert_eq!(restored_arr.len(), 1, "after load: {restored}");
    assert_eq!(restored_arr[0]["value"].as_str(), Some("bar"));

    // 9. Storage flow — switch to example.com so we have a stable origin.
    client
        .call_tool(CallToolRequestParams::new("browser_goto").with_arguments({
            let mut m = serde_json::Map::new();
            m.insert("url".into(), serde_json::json!("https://example.com"));
            m
        }))
        .await
        .expect("browser_goto example.com ok");

    // 10. storage_set local key1=val1.
    let set = client
        .call_tool(
            CallToolRequestParams::new("browser_storage_set").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("kind".into(), serde_json::json!("local"));
                m.insert("key".into(), serde_json::json!("key1"));
                m.insert("value".into(), serde_json::json!("val1"));
                m
            }),
        )
        .await
        .expect("browser_storage_set ok");
    assert_eq!(structured(&set)["ok"], serde_json::json!(true));

    // 11. storage_get local key1 — returns { values: { key1: "val1" } }.
    let get = client
        .call_tool(
            CallToolRequestParams::new("browser_storage_get").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("kind".into(), serde_json::json!("local"));
                m.insert("key".into(), serde_json::json!("key1"));
                m
            }),
        )
        .await
        .expect("browser_storage_get ok");
    let get_body = structured(&get);
    assert_eq!(
        get_body["values"]["key1"].as_str(),
        Some("val1"),
        "storage get: {get_body}"
    );

    // 12. storage_clear local → storage_get returns empty.
    let clear_storage = client
        .call_tool(
            CallToolRequestParams::new("browser_storage_clear").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("kind".into(), serde_json::json!("local"));
                m
            }),
        )
        .await
        .expect("browser_storage_clear ok");
    assert_eq!(structured(&clear_storage)["ok"], serde_json::json!(true));

    let get_after_storage_clear = client
        .call_tool(
            CallToolRequestParams::new("browser_storage_get").with_arguments({
                let mut m = serde_json::Map::new();
                m.insert("kind".into(), serde_json::json!("local"));
                m
            }),
        )
        .await
        .expect("browser_storage_get ok");
    let cleared_body = structured(&get_after_storage_clear);
    assert_eq!(
        cleared_body["values"].as_object().map(serde_json::Map::len),
        Some(0),
        "storage after clear: {cleared_body}"
    );

    // 13. Close browser.
    client
        .call_tool(CallToolRequestParams::new("browser_close").with_arguments(Default::default()))
        .await
        .expect("browser_close ok");

    let _ = tokio::fs::remove_file(&tempfile).await;
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
