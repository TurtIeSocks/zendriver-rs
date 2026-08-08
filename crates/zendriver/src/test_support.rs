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

use crate::query::actionability::ActionabilityCheck;

/// How long [`expect`] waits before declaring the sequence broken. Generous
/// enough to survive a loaded CI runner, since exceeding it fails the test.
const EXPECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long [`try_expect`] waits before concluding nothing more is coming.
/// Short, because every call *spends* this much wall-clock on the expected
/// path — the absence of a frame is the answer being asked for.
const SILENCE_TIMEOUT: Duration = Duration::from_millis(500);

/// Await `method`, bounded.
///
/// [`MockConnection::expect_cmd`] silently discards non-matching frames and
/// has no timeout of its own, so a test that is one frame short of the real
/// sequence hangs forever instead of failing. Prefer this for any wait on a
/// specific method; use [`try_expect`] when the *absence* of a frame is what
/// the test is asking about.
pub(crate) async fn expect(mock: &mut MockConnection, method: &str) -> u64 {
    match tokio::time::timeout(EXPECT_TIMEOUT, mock.expect_cmd(method)).await {
        Ok(id) => id,
        Err(_) => panic!("timed out waiting for {method}"),
    }
}

/// Await `method`, returning `None` if nothing arrives within
/// [`SILENCE_TIMEOUT`].
///
/// The counterpart to [`expect`]: silence is a legitimate answer here, used
/// to drain a run of frames of unknown length ("keep taking `mouseMoved`s
/// until they stop"). Named rather than spelled inline so the short bound
/// reads as the end-of-stream signal it is, instead of looking like [`expect`]
/// with an unexplained ten-times-tighter deadline.
pub(crate) async fn try_expect(mock: &mut MockConnection, method: &str) -> Option<u64> {
    tokio::time::timeout(SILENCE_TIMEOUT, mock.expect_cmd(method))
        .await
        .ok()
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

/// Parameter names of a `function(a, b, ...){ ... }` declaration, in order.
///
/// `call_on_main` prepends the element handle to the argument list, so the
/// parameter at index `i` binds to `arguments[i]`. Asserting the parameter
/// list against the emitted arguments is what catches an off-by-one — a body
/// declared `function(v)` reads the *element* out of `arguments[0]` while the
/// caller's value sits unread at `arguments[1]`, and every assertion about
/// the arguments array alone still passes.
///
/// Stricter than a `starts_with("function(el")` check for the same reason:
/// that accepts `function(elephant, dy, dx)` — right prefix, transposed
/// coordinates.
///
/// The first `(`/`)` pair is the parameter list: parameter names can't
/// contain parentheses, so a body that does (`setTimeout(...)`) is safe.
pub(crate) fn js_params(decl: &str) -> Vec<&str> {
    let open = decl.find('(').expect("declaration opens a param list");
    let close = decl.find(')').expect("declaration closes its param list");
    decl[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

/// Reply to every `Input.dispatchMouseEvent` the mock has queued until it
/// goes quiet, returning each frame's `params`.
///
/// A gesture's frame count is not known up front — a realistic move emits one
/// per Bezier segment — so silence is the terminator, which is what
/// [`try_expect`] is for. Callers project whatever fields they assert on.
pub(crate) async fn drain_mouse_dispatches(mock: &mut MockConnection) -> Vec<Value> {
    let mut frames = Vec::new();
    while let Some(id) = try_expect(mock, "Input.dispatchMouseEvent").await {
        frames.push(mock.last_sent()["params"].clone());
        mock.reply(id, json!({})).await;
    }
    frames
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

/// Serve one pass of the actionability gate for `require`, answering every
/// predicate `true`.
///
/// Takes the check set rather than a count so the number of probes is derived
/// from the same value the code under test gates on. A hand-written count
/// that drifts from the set does not fail cleanly: it consumes the *next*,
/// unrelated `Runtime.callFunctionOn`, and the test dies later at an
/// `expect` for a frame that was already eaten.
pub(crate) async fn serve_gate_probes(mock: &mut MockConnection, require: ActionabilityCheck) {
    for _ in 0..require.probe_count() {
        serve_call(mock, json!({ "value": true, "type": "boolean" })).await;
    }
}
