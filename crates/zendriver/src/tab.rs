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

const DEFAULT_LOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Tab {
    pub(crate) inner: Arc<TabInner>,
}

pub(crate) struct TabInner {
    pub(crate) session: SessionHandle,
}

impl Tab {
    pub(crate) fn new(session: SessionHandle) -> Self {
        Self {
            inner: Arc::new(TabInner { session }),
        }
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

    /// Evaluate a JavaScript expression in the tab's main frame. The result
    /// is deserialized into `T`. Throws `JsException` if the expression
    /// raises.
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
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
        let tab = Tab::new(sess);

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
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.goto("https://bad.test").await }
        });

        let id_enable = mock.expect_cmd("Page.enable").await;
        mock.reply(id_enable, json!({})).await;

        let id_nav = mock.expect_cmd("Page.navigate").await;
        mock.reply(
            id_nav,
            json!({ "errorText": "net::ERR_NAME_NOT_RESOLVED" }),
        )
        .await;

        let res = fut.await.unwrap();
        match res {
            Err(ZendriverError::Navigation(m)) => assert!(m.contains("ERR_NAME_NOT_RESOLVED")),
            other => panic!("unexpected: {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn evaluate_returns_typed_value() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("1+1").await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["expression"], "1+1");
        mock.reply(id, json!({ "result": { "value": 2, "type": "number" } }))
            .await;
        let n = fut.await.unwrap().unwrap();
        assert_eq!(n, 2);
        conn.shutdown();
    }

    #[tokio::test]
    async fn evaluate_returns_js_exception_when_chrome_reports_one() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("throw new Error('boom')").await }
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
}
