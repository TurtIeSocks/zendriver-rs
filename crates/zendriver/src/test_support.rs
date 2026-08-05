//! Shared scaffolding for the mock-CDP unit tests.
//!
//! Two concerns live here, both about tests that drive a scripted CDP
//! sequence: a test one frame out of step with the code should *fail* rather
//! than hang, and a sequence several tests replay should be written down
//! once — the actionability gate's run of predicate calls above all.

#![allow(clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use serde_json::{Value, json};
use zendriver_transport::testing::MockConnection;

/// Await `method`, bounded.
///
/// [`MockConnection::expect_cmd`] silently discards non-matching frames and
/// has no timeout of its own, so a test that is one frame short of the real
/// sequence hangs forever instead of failing. Every wait goes through here.
pub(crate) async fn expect(mock: &mut MockConnection, method: &str) -> u64 {
    match tokio::time::timeout(Duration::from_secs(5), mock.expect_cmd(method)).await {
        Ok(id) => id,
        Err(_) => panic!("timed out waiting for {method}"),
    }
}

/// Serve one `Runtime.callFunctionOn`, answering it with `result`, and hand
/// back the `params` that went out so the caller can assert on the JS source
/// or the argument array.
pub(crate) async fn serve_call(mock: &mut MockConnection, result: Value) -> Value {
    let id = expect(mock, "Runtime.callFunctionOn").await;
    let params = mock.last_sent()["params"].clone();
    mock.reply(id, json!({ "result": result })).await;
    params
}

/// The JS source of one `Runtime.callFunctionOn`, answered with `result`.
///
/// Thin wrapper over [`serve_call`] for the common case of asserting what a
/// probe actually executed.
pub(crate) async fn serve_call_js(mock: &mut MockConnection, result: Value) -> String {
    serve_call(mock, result).await["functionDeclaration"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Serve the `scroll_into_view` an action runs before its actionability
/// gate, asserting the call really is that scroll — a fixture that silently
/// absorbed the first `Runtime.callFunctionOn` would keep passing if the
/// scroll ever went missing, which is the regression this guards.
pub(crate) async fn serve_scroll_into_view(mock: &mut MockConnection) {
    let js = serve_call_js(mock, json!({ "type": "undefined" })).await;
    assert!(
        js.contains("scrollIntoView"),
        "expected scroll_into_view ahead of the actionability gate, got: {js}"
    );
}

/// Serve `n` actionability predicates, answering each one `true`.
///
/// Pass the count the action's
/// [`crate::query::actionability::ActionabilityCheck`] implies: 4 for `FULL`,
/// 2 for `TEXT_INPUT`, 1 for `VISIBLE_ONLY`.
pub(crate) async fn serve_gate_probes(mock: &mut MockConnection, n: usize) {
    for _ in 0..n {
        serve_call(mock, json!({ "value": true, "type": "boolean" })).await;
    }
}
