//! Tab — handle to a single CDP target session.

use std::sync::Arc;
use std::time::Duration;

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
    /// Weak ref to the owning `BrowserInner`. Used by Element actions to
    /// reach the shared `InputController`. `Weak` breaks the
    /// Browser→Tab→Browser cycle. Read by `Tab::input()`, which is
    /// consumed by later P3 tasks (Element actions).
    #[allow(dead_code)]
    pub(crate) browser: std::sync::Weak<crate::browser::BrowserInner>,
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
    ) -> Self {
        Self {
            inner: Arc::new(TabInner {
                session,
                isolated_world: tokio::sync::Mutex::new(IsolatedWorldCache::default()),
                browser,
            }),
        }
    }

    /// Returns a strong handle to the owning Browser's `InputController`,
    /// or `None` if the Browser has been dropped (typically only in
    /// test-only Tabs constructed with `Weak::new()`). Consumed by later
    /// P3 tasks (Element actions).
    #[allow(dead_code)]
    pub(crate) fn input(&self) -> Option<Arc<InputController>> {
        self.inner.browser.upgrade().map(|b| b.input.clone())
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
    async fn ensure_isolated_world(&self) -> Result<i64> {
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

    /// Detach the target session for this tab.
    pub async fn close(self) -> Result<()> {
        let sid = self.inner.session.session_id().to_string();
        self.inner
            .session
            .connection()
            .call_raw("Target.detachFromTarget", json!({ "sessionId": sid }), None)
            .await?;
        Ok(())
    }
}

impl Tab {
    /// Begin a chainable element query against this tab. Use `.css(...)` to
    /// supply a selector, then `.one()` / `.one_or_none()` to await a result.
    pub fn find(&self) -> crate::query::FindBuilder<'_> {
        crate::query::FindBuilder::new(self)
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
        let tab = Tab::new(sess, std::sync::Weak::new());

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
        let tab = Tab::new(sess, std::sync::Weak::new());

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
        let tab = Tab::new(sess, std::sync::Weak::new());

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
        let tab = Tab::new(sess, std::sync::Weak::new());

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
        let tab = Tab::new(sess, std::sync::Weak::new());

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
        let tab = Tab::new(sess, std::sync::Weak::new());

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
        let tab = Tab::new(sess, std::sync::Weak::new());

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
        let tab = Tab::new(sess, std::sync::Weak::new());

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
    async fn close_sends_target_detach_with_session_id() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S42");
        let tab = Tab::new(sess, std::sync::Weak::new());

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.close().await }
        });

        let id = mock.expect_cmd("Target.detachFromTarget").await;
        assert_eq!(mock.last_sent()["params"]["sessionId"], "S42");
        mock.reply(id, json!({})).await;
        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn title_returns_string_from_target_info() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());

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
