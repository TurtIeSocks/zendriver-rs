//! Tab — handle to a single CDP target session.

use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::time::timeout;
use tracing::trace;
use zendriver_transport::SessionHandle;

use crate::error::{Result, ZendriverError};
use crate::input::InputController;

const DEFAULT_LOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Tab {
    pub(crate) inner: Arc<TabInner>,
}

pub(crate) struct TabInner {
    pub(crate) session: SessionHandle,
    pub(crate) isolated_world: tokio::sync::Mutex<IsolatedWorldCache>,
    /// Weak ref to the owning `BrowserInner`. Used by future P4 tasks to
    /// reach Browser-wide resources (CookieJar, tabs registry). `Weak`
    /// breaks the Browser→Tab→Browser cycle.
    #[allow(dead_code)]
    pub(crate) browser: std::sync::Weak<crate::browser::BrowserInner>,
    /// Per-Tab input controller. Each tab owns its own cursor + held-modifier
    /// state — distinct tabs in the same Browser have independent pointers.
    /// `Element` actions clone this `Arc` to drive `mouse::*` / `keyboard::*`
    /// dispatch helpers; the shared mutex inside `InputController` serializes
    /// per-tab writes without crossing tab boundaries.
    pub(crate) input: Arc<InputController>,
    /// CDP `targetId` for the page target this tab wraps. Cached at Tab
    /// construction time (from `Target.attachedToTarget`'s `target_info`)
    /// so multi-tab orchestration (`Browser::new_tab` correlation,
    /// `Tab::activate`, `Tab::close`'s `Target.closeTarget` upgrade) can
    /// dispatch by `targetId` without re-querying `Target.getTargetInfo`
    /// per call.
    pub(crate) target_id: String,
}

#[derive(Default)]
pub(crate) struct IsolatedWorldCache {
    pub(crate) main_frame_id: Option<String>,
    pub(crate) context_id: Option<i64>,
}

impl Tab {
    pub(crate) fn new(
        session: SessionHandle,
        browser: std::sync::Weak<crate::browser::BrowserInner>,
        input: Arc<InputController>,
        target_id: String,
    ) -> Self {
        Self {
            inner: Arc::new(TabInner {
                session,
                isolated_world: tokio::sync::Mutex::new(IsolatedWorldCache::default()),
                browser,
                input,
                target_id,
            }),
        }
    }

    /// Test-only constructor: builds a `Tab` with a deterministic seeded
    /// [`InputController`] (native input profile, seed `42`) and an empty
    /// `Weak` browser ref. Replaces the P3 `Tab::new(sess, Weak::new())`
    /// pattern that paired with `Tab::input() -> Option<_>`; now that
    /// `Tab::input()` returns `&Arc<InputController>` unconditionally, tests
    /// must seed a controller at construction time.
    ///
    /// The synthetic `target_id` is derived from the session_id — tests that
    /// need a specific `targetId` should use [`Tab::new_for_test_with_target`].
    #[cfg(test)]
    pub(crate) fn new_for_test(session: SessionHandle) -> Self {
        let target_id = format!("test-target-{}", session.session_id());
        Self::new(
            session,
            std::sync::Weak::new(),
            crate::input::InputController::new_with_seed(
                zendriver_stealth::InputProfile::native(),
                42,
            ),
            target_id,
        )
    }

    /// The CDP `targetId` for the page target this tab wraps. Stable for
    /// the lifetime of the underlying target — used by `Browser::new_tab`
    /// to correlate a `Target.createTarget` response with the [`Tab`] that
    /// the [`crate::browser::TabRegistrar`] subsequently registers.
    #[must_use]
    pub fn target_id(&self) -> &str {
        &self.inner.target_id
    }

    /// The per-Tab [`InputController`]. Each tab carries its own cursor +
    /// modifier state; `Element` actions (`click`, `hover`, `type_text`,
    /// `press`) call this to drive `mouse::*` / `keyboard::*` dispatch.
    /// Always returns a valid handle — distinct from the P3 shape that
    /// returned `Option` to handle the `Weak::new()` test case.
    #[must_use]
    pub fn input(&self) -> &Arc<InputController> {
        &self.inner.input
    }

    /// Escape hatch: raw `SessionHandle` for advanced users who need to send
    /// CDP commands the high-level API doesn't expose.
    pub fn session(&self) -> &SessionHandle {
        &self.inner.session
    }

    /// Helper: call a CDP method on this tab's session, parsing transport
    /// errors into `ZendriverError`.
    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value> {
        trace!(%method, "tab.call");
        let res = self.inner.session.call(method, params).await?;
        Ok(res)
    }

    /// Navigate the tab to the given URL. Does NOT wait for the load to
    /// complete — call `wait_for_load` after.
    pub async fn goto(&self, url: impl AsRef<str>) -> Result<()> {
        // Enable Page domain so we get FrameStoppedLoading events.
        self.call("Page.enable", json!({})).await?;
        let url_s = url.as_ref().to_string();
        let res = self.call("Page.navigate", json!({ "url": url_s })).await?;
        if let Some(err) = res.get("errorText").and_then(|v| v.as_str()) {
            if !err.is_empty() {
                return Err(ZendriverError::Navigation(err.to_string()));
            }
        }
        Ok(())
    }

    /// Wait until the main frame's load event fires.
    pub async fn wait_for_load(&self) -> Result<()> {
        // Subscribe before any `goto` to avoid missing the event; in P1 we
        // accept that callers may have a small race. P3+ revisits.
        let mut stream = self
            .inner
            .session
            .subscribe::<Value>("Page.frameStoppedLoading");
        timeout(DEFAULT_LOAD_TIMEOUT, stream.next())
            .await
            .map_err(|_| ZendriverError::Timeout(DEFAULT_LOAD_TIMEOUT))?
            .ok_or_else(|| ZendriverError::Navigation("page event stream closed".into()))?;
        Ok(())
    }

    /// Evaluate a JavaScript expression in an isolated world (sandbox; no
    /// page globals visible). Default for stealth-safe execution. The result
    /// is deserialized into `T`. Throws `JsException` if the expression
    /// raises.
    ///
    /// If the cached isolated-world execution context was destroyed (e.g. by
    /// a page navigation), the cache is invalidated and the evaluation is
    /// retried once.
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let js = js.as_ref();
        for attempt in 0..2 {
            let ctx_id = self.ensure_isolated_world().await?;
            let res = self
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": js,
                        "contextId": ctx_id,
                        "returnByValue": true,
                        "awaitPromise": true,
                    }),
                )
                .await;
            match res {
                Ok(v) => {
                    if let Some(details) = v.get("exceptionDetails") {
                        let msg = details
                            .get("exception")
                            .and_then(|e| e.get("description"))
                            .and_then(|d| d.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        return Err(ZendriverError::JsException(msg));
                    }
                    let value = v
                        .get("result")
                        .and_then(|r| r.get("value"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    return serde_json::from_value(value).map_err(ZendriverError::Serde);
                }
                // Chrome returns -32000 "Cannot find context with specified
                // id" when the execution context we cached was destroyed
                // (typically by a navigation). `From<CallError>` maps that
                // to `Navigation` (see `error.rs`), so we match on that
                // variant here — not on `Cdp` as the original P2 plan
                // suggested.
                Err(ZendriverError::Navigation(ref m))
                    if attempt == 0 && m.contains("Cannot find context") =>
                {
                    self.inner.isolated_world.lock().await.context_id = None;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    /// Evaluate a JavaScript expression in the page main world (page globals
    /// accessible). Escape hatch for cases where isolated-world semantics
    /// don't fit. The result is deserialized into `T`. Throws `JsException`
    /// if the expression raises.
    pub async fn evaluate_main<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let res = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": js.as_ref(),
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        if let Some(details) = res.get("exceptionDetails") {
            let msg = details
                .get("exception")
                .and_then(|e| e.get("description"))
                .and_then(|d| d.as_str())
                .unwrap_or("unknown")
                .to_string();
            return Err(ZendriverError::JsException(msg));
        }
        let value = res
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(ZendriverError::Serde)
    }

    /// Ensure an isolated-world execution context exists for this tab's main
    /// frame, returning its `executionContextId`. Cached after first call.
    pub(crate) async fn ensure_isolated_world(&self) -> Result<i64> {
        let mut cache = self.inner.isolated_world.lock().await;
        if let Some(ctx) = cache.context_id {
            return Ok(ctx);
        }
        // Discover the main frame id.
        let tree = self.call("Page.getFrameTree", json!({})).await?;
        let frame_id = tree["frameTree"]["frame"]["id"]
            .as_str()
            .ok_or_else(|| ZendriverError::Navigation("no main frame in Page.getFrameTree".into()))?
            .to_string();
        let res = self
            .call(
                "Page.createIsolatedWorld",
                json!({
                    "frameId": frame_id,
                    "worldName": "zendriver-eval",
                    "grantUniversalAccess": false,
                }),
            )
            .await?;
        let ctx_id = res["executionContextId"].as_i64().ok_or_else(|| {
            ZendriverError::Navigation(
                "Page.createIsolatedWorld did not return executionContextId".into(),
            )
        })?;
        cache.main_frame_id = Some(frame_id);
        cache.context_id = Some(ctx_id);
        Ok(ctx_id)
    }

    /// Get the tab's current URL.
    pub async fn url(&self) -> Result<url::Url> {
        let res = self.call("Target.getTargetInfo", json!({})).await?;
        let s = res["targetInfo"]["url"]
            .as_str()
            .ok_or_else(|| ZendriverError::Navigation("target has no url".into()))?;
        url::Url::parse(s).map_err(|e| ZendriverError::Navigation(e.to_string()))
    }

    /// Get the tab's `<title>`.
    pub async fn title(&self) -> Result<String> {
        let res = self.call("Target.getTargetInfo", json!({})).await?;
        Ok(res["targetInfo"]["title"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    /// Capture a full-viewport PNG screenshot of this tab. Sends
    /// `Page.captureScreenshot { format: "png" }` and base64-decodes the
    /// returned `data` field into the raw PNG bytes.
    ///
    /// For element-scoped screenshots, see [`Element::screenshot`].
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        let res = self
            .call("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        let data = res.get("data").and_then(|v| v.as_str()).ok_or_else(|| {
            ZendriverError::Navigation("Page.captureScreenshot returned no data".into())
        })?;
        BASE64
            .decode(data)
            .map_err(|e| ZendriverError::Navigation(format!("invalid base64 in screenshot: {e}")))
    }

    /// Close this tab in Chrome.
    ///
    /// Sends `Target.closeTarget { targetId }` at browser scope (no
    /// `session_id`) using the cached [`TabInner::target_id`]. Chrome
    /// destroys the page target, which in turn produces a
    /// `Target.detachedFromTarget` event whose
    /// [`crate::browser::TabRegistrar::on_target_detached`] handler removes
    /// this tab from the [`crate::browser::BrowserInner::tabs`] registry.
    ///
    /// P1 shipped this as `Target.detachFromTarget` only, which severed the
    /// CDP session but left the underlying page target alive in Chrome. P4
    /// upgrades to a real close so multi-tab workflows reclaim memory and
    /// the registry stays in sync with browser state.
    pub async fn close(self) -> Result<()> {
        let target_id = self.target_id().to_string();
        self.inner
            .session
            .connection()
            .call_raw("Target.closeTarget", json!({ "targetId": target_id }), None)
            .await?;
        Ok(())
    }

    /// Bring this tab to the foreground in Chrome.
    ///
    /// Sends `Target.activateTarget { targetId }` at browser scope (no
    /// `session_id`) using the cached [`TabInner::target_id`]. Chrome
    /// focuses the page target so it becomes the visible/active tab.
    ///
    /// Unlike `close`, this consumes `&self` — the tab remains usable
    /// after activation. Useful in multi-tab workflows where you want to
    /// surface a specific tab without tearing it down.
    pub async fn activate(&self) -> Result<()> {
        let target_id = self.target_id().to_string();
        self.inner
            .session
            .connection()
            .call_raw(
                "Target.activateTarget",
                json!({ "targetId": target_id }),
                None,
            )
            .await?;
        Ok(())
    }
}

impl Tab {
    /// Begin a chainable element query against this tab. Pick a selector
    /// kind (`.css`, `.xpath`, `.text`, `.text_exact`, `.text_regex`,
    /// `.text_regex_with_flags`, `.role`, `.role_named`), optionally
    /// apply modifiers (`.nth`, `.visible_only`, `.in_frame`,
    /// `.timeout`), then terminate with `.one()` / `.one_or_none()`.
    pub fn find(&self) -> crate::query::FindBuilder<'_> {
        crate::query::FindBuilder::new_for_tab(self)
    }

    /// Begin a chainable element query against this tab that returns
    /// ALL matches. Mirrors `find()` selectors + modifiers (no `nth`),
    /// terminated with `.many()` (errors on empty) or `.many_or_empty()`
    /// (returns empty `Vec` instead).
    pub fn find_all(&self) -> crate::query::FindAllBuilder<'_> {
        crate::query::FindAllBuilder::new_for_tab(self)
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn goto_sends_page_enable_then_page_navigate_with_url() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.goto("https://example.com").await }
        });

        let id_enable = mock.expect_cmd("Page.enable").await;
        mock.reply(id_enable, json!({})).await;

        let id_nav = mock.expect_cmd("Page.navigate").await;
        assert_eq!(mock.last_sent()["params"]["url"], "https://example.com");
        mock.reply(id_nav, json!({ "frameId": "F1" })).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn goto_returns_navigation_error_when_chrome_reports_errortext() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.goto("https://bad.test").await }
        });

        let id_enable = mock.expect_cmd("Page.enable").await;
        mock.reply(id_enable, json!({})).await;

        let id_nav = mock.expect_cmd("Page.navigate").await;
        mock.reply(id_nav, json!({ "errorText": "net::ERR_NAME_NOT_RESOLVED" }))
            .await;

        let res = fut.await.unwrap();
        match res {
            Err(ZendriverError::Navigation(m)) => assert!(m.contains("ERR_NAME_NOT_RESOLVED")),
            other => panic!("unexpected: {other:?}"),
        }
        conn.shutdown();
    }

    // --- main-world evaluate (escape hatch) ----------------------------

    #[tokio::test]
    async fn evaluate_main_returns_typed_value() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate_main::<i32>("1+1").await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["expression"], "1+1");
        // Main-world evaluate must NOT pass a contextId.
        assert!(mock.last_sent()["params"].get("contextId").is_none());
        mock.reply(id, json!({ "result": { "value": 2, "type": "number" } }))
            .await;
        let n = fut.await.unwrap().unwrap();
        assert_eq!(n, 2);
        conn.shutdown();
    }

    #[tokio::test]
    async fn evaluate_main_returns_js_exception_when_chrome_reports_one() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate_main::<i32>("throw new Error('boom')").await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            json!({
                "result": { "type": "object", "subtype": "error" },
                "exceptionDetails": {
                    "exception": { "description": "Error: boom\n    at <anonymous>:1:7" }
                }
            }),
        )
        .await;
        let res = fut.await.unwrap();
        match res {
            Err(ZendriverError::JsException(m)) => assert!(m.contains("Error: boom")),
            other => panic!("unexpected: {other:?}"),
        }
        conn.shutdown();
    }

    // --- isolated-world evaluate ---------------------------------------

    #[tokio::test]
    async fn evaluate_isolated_creates_world_then_evaluates() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("1+1").await }
        });

        // 1. Page.getFrameTree → main frame id.
        let id_tree = mock.expect_cmd("Page.getFrameTree").await;
        mock.reply(
            id_tree,
            json!({ "frameTree": { "frame": { "id": "MAIN_FRAME" } } }),
        )
        .await;

        // 2. Page.createIsolatedWorld → executionContextId.
        let id_world = mock.expect_cmd("Page.createIsolatedWorld").await;
        assert_eq!(mock.last_sent()["params"]["frameId"], "MAIN_FRAME");
        assert_eq!(mock.last_sent()["params"]["worldName"], "zendriver-eval");
        mock.reply(id_world, json!({ "executionContextId": 42 }))
            .await;

        // 3. Runtime.evaluate with that contextId.
        let id_eval = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["expression"], "1+1");
        assert_eq!(mock.last_sent()["params"]["contextId"], 42);
        mock.reply(
            id_eval,
            json!({ "result": { "value": 2, "type": "number" } }),
        )
        .await;

        let n = fut.await.unwrap().unwrap();
        assert_eq!(n, 2);
        conn.shutdown();
    }

    #[tokio::test]
    async fn evaluate_caches_context_id_across_calls() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        // First call: full handshake + eval.
        let fut1 = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("1").await }
        });
        let id_tree = mock.expect_cmd("Page.getFrameTree").await;
        mock.reply(
            id_tree,
            json!({ "frameTree": { "frame": { "id": "MAIN_FRAME" } } }),
        )
        .await;
        let id_world = mock.expect_cmd("Page.createIsolatedWorld").await;
        mock.reply(id_world, json!({ "executionContextId": 7 }))
            .await;
        let id_eval1 = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["contextId"], 7);
        mock.reply(
            id_eval1,
            json!({ "result": { "value": 1, "type": "number" } }),
        )
        .await;
        assert_eq!(fut1.await.unwrap().unwrap(), 1);

        // Second call: must reuse the cached contextId → next outbound
        // frame should be Runtime.evaluate, with NO Page.getFrameTree or
        // Page.createIsolatedWorld in between.
        let fut2 = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("2").await }
        });
        let id_eval2 = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["contextId"], 7);
        assert_eq!(mock.last_sent()["params"]["expression"], "2");
        mock.reply(
            id_eval2,
            json!({ "result": { "value": 2, "type": "number" } }),
        )
        .await;
        assert_eq!(fut2.await.unwrap().unwrap(), 2);

        conn.shutdown();
    }

    #[tokio::test]
    async fn evaluate_recreates_world_after_context_destroyed_error() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        // --- Call 1: establishes cache, succeeds. ---
        let fut1 = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("1").await }
        });
        let id_tree = mock.expect_cmd("Page.getFrameTree").await;
        mock.reply(
            id_tree,
            json!({ "frameTree": { "frame": { "id": "MAIN_FRAME" } } }),
        )
        .await;
        let id_world = mock.expect_cmd("Page.createIsolatedWorld").await;
        mock.reply(id_world, json!({ "executionContextId": 7 }))
            .await;
        let id_eval1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_eval1,
            json!({ "result": { "value": 1, "type": "number" } }),
        )
        .await;
        assert_eq!(fut1.await.unwrap().unwrap(), 1);

        // --- Call 2: cached contextId is now stale. Runtime.evaluate
        //     returns -32000 "Cannot find context with specified id";
        //     evaluate must invalidate the cache, re-run the discovery
        //     handshake with a NEW contextId, then re-issue Runtime.evaluate.
        let fut2 = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("2").await }
        });
        // First Runtime.evaluate uses cached id 7 → CDP returns error.
        let id_eval_fail = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["contextId"], 7);
        mock.reply_err(
            id_eval_fail,
            -32000,
            "Cannot find context with specified id",
        )
        .await;

        // Cache invalidated → discovery handshake re-runs.
        let id_tree2 = mock.expect_cmd("Page.getFrameTree").await;
        mock.reply(
            id_tree2,
            json!({ "frameTree": { "frame": { "id": "MAIN_FRAME_2" } } }),
        )
        .await;
        let id_world2 = mock.expect_cmd("Page.createIsolatedWorld").await;
        assert_eq!(mock.last_sent()["params"]["frameId"], "MAIN_FRAME_2");
        mock.reply(id_world2, json!({ "executionContextId": 99 }))
            .await;

        // Retried Runtime.evaluate uses the fresh contextId.
        let id_eval_retry = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["contextId"], 99);
        mock.reply(
            id_eval_retry,
            json!({ "result": { "value": 2, "type": "number" } }),
        )
        .await;
        assert_eq!(fut2.await.unwrap().unwrap(), 2);

        // --- Call 3: cache is fresh again → straight to Runtime.evaluate.
        let fut3 = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("3").await }
        });
        let id_eval3 = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["contextId"], 99);
        mock.reply(
            id_eval3,
            json!({ "result": { "value": 3, "type": "number" } }),
        )
        .await;
        assert_eq!(fut3.await.unwrap().unwrap(), 3);

        conn.shutdown();
    }

    #[tokio::test]
    async fn url_returns_parsed_url_from_target_info() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.url().await }
        });

        let id = mock.expect_cmd("Target.getTargetInfo").await;
        mock.reply(
            id,
            json!({ "targetInfo": { "url": "https://example.com/x", "title": "ok" } }),
        )
        .await;
        let u = fut.await.unwrap().unwrap();
        assert_eq!(u.as_str(), "https://example.com/x");
        conn.shutdown();
    }

    #[tokio::test]
    async fn close_sends_target_close_target_with_target_id() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S42");
        // `Tab::new_for_test` derives a deterministic target_id from the
        // session_id: `test-target-S42` here.
        let tab = Tab::new_for_test(sess);
        assert_eq!(tab.target_id(), "test-target-S42");

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.close().await }
        });

        let id = mock.expect_cmd("Target.closeTarget").await;
        assert_eq!(mock.last_sent()["params"]["targetId"], "test-target-S42");
        // Browser-scope command — no session_id.
        assert!(mock.last_sent().get("sessionId").is_none());
        mock.reply(id, json!({ "success": true })).await;
        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn activate_sends_target_activate_target_with_target_id() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S99");
        // `Tab::new_for_test` derives a deterministic target_id from the
        // session_id: `test-target-S99` here.
        let tab = Tab::new_for_test(sess);
        assert_eq!(tab.target_id(), "test-target-S99");

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.activate().await }
        });

        let id = mock.expect_cmd("Target.activateTarget").await;
        assert_eq!(mock.last_sent()["params"]["targetId"], "test-target-S99");
        // Browser-scope command — no session_id.
        assert!(mock.last_sent().get("sessionId").is_none());
        mock.reply(id, json!({})).await;
        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn screenshot_sends_page_capturescreenshot_without_clip_and_decodes_base64() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.screenshot().await }
        });

        let id = mock.expect_cmd("Page.captureScreenshot").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["format"], "png");
        // Tab::screenshot must NOT pass a clip — that's Element::screenshot.
        assert!(sent["params"].get("clip").is_none());
        // "PNG!" → b"PNG!" once base64-decoded.
        mock.reply(id, json!({ "data": "UE5HIQ==" })).await;

        let bytes = fut.await.unwrap().unwrap();
        assert_eq!(bytes, b"PNG!");
        conn.shutdown();
    }

    #[tokio::test]
    async fn title_returns_string_from_target_info() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.title().await }
        });

        let id = mock.expect_cmd("Target.getTargetInfo").await;
        mock.reply(
            id,
            json!({ "targetInfo": { "url": "https://x", "title": "Hello" } }),
        )
        .await;
        let s = fut.await.unwrap().unwrap();
        assert_eq!(s, "Hello");
        conn.shutdown();
    }
}
