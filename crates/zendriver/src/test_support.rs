//! Shared scaffolding for the mock-CDP unit tests.
//!
//! Everything here exists to keep one frame sequence in a single place: the
//! **isolated-world call**. Element reads (`element::reads`) and the
//! actionability gate (`query::actionability`) both run their JS in the tab's
//! isolated world, so a test driving either has to answer a
//! resolve → call → release handshake rather than a bare
//! `Runtime.callFunctionOn`. Inlining that in every test would bury what each
//! one is actually asserting.

#![allow(clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use serde_json::{Value, json};
use zendriver_transport::testing::MockConnection;

/// The `executionContextId` [`serve_isolated_world`] hands out.
pub(crate) const ISOLATED_CONTEXT_ID: i64 = 42;

/// The isolated-world `objectId` [`serve_isolated_call`] resolves the element
/// to. Deliberately unlike the page-world ids the tests construct elements
/// with (`R1`, `R17`, …) so an assertion can tell the two worlds apart.
pub(crate) const ISOLATED_OBJECT_ID: &str = "R_ISO";

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

/// Serve the two-frame isolated-world handshake, yielding
/// [`ISOLATED_CONTEXT_ID`].
///
/// `Tab::ensure_isolated_world` caches the context, so this is paid once per
/// tab — before the *first* isolated call in a test, not before each one.
pub(crate) async fn serve_isolated_world(mock: &mut MockConnection) {
    let id = expect(mock, "Page.getFrameTree").await;
    mock.reply(id, json!({ "frameTree": { "frame": { "id": "F1" } } }))
        .await;
    let id = expect(mock, "Page.createIsolatedWorld").await;
    mock.reply(id, json!({ "executionContextId": ISOLATED_CONTEXT_ID }))
        .await;
}

/// Serve one isolated-world invocation: the `DOM.resolveNode` that lifts the
/// element into the world, the `Runtime.callFunctionOn` (answered with
/// `result`), and the `Runtime.releaseObject` that frees the handle again.
///
/// Asserts the call targeted the *isolated* handle along the way, and hands
/// back the JS source that was sent so callers can make claims about it.
pub(crate) async fn serve_isolated_call(mock: &mut MockConnection, result: Value) -> String {
    let id = expect(mock, "DOM.resolveNode").await;
    assert_eq!(
        mock.last_sent()["params"]["executionContextId"],
        ISOLATED_CONTEXT_ID,
        "the node must be re-resolved into the isolated world",
    );
    mock.reply(id, json!({ "object": { "objectId": ISOLATED_OBJECT_ID } }))
        .await;

    let id = expect(mock, "Runtime.callFunctionOn").await;
    let sent = mock.last_sent();
    assert_eq!(
        sent["params"]["objectId"], ISOLATED_OBJECT_ID,
        "the call must run against the isolated handle, not the page-world one",
    );
    let js = sent["params"]["functionDeclaration"]
        .as_str()
        .unwrap()
        .to_string();
    mock.reply(id, json!({ "result": result })).await;

    let id = expect(mock, "Runtime.releaseObject").await;
    mock.reply(id, json!({})).await;

    js
}

/// Serve `n` actionability predicates, answering each one `true`.
///
/// The gate runs isolated so a page cannot shadow the
/// `getBoundingClientRect` / `getComputedStyle` / `elementFromPoint` the
/// predicates read — the two extra frames per probe are what that costs.
/// Pass the count the action's [`crate::query::actionability::ActionabilityCheck`]
/// implies: 4 for `FULL`, 2 for `TEXT_INPUT`, 1 for `VISIBLE_ONLY`.
pub(crate) async fn serve_gate_probes(mock: &mut MockConnection, n: usize) {
    for _ in 0..n {
        serve_isolated_call(mock, json!({ "value": true, "type": "boolean" })).await;
    }
}
