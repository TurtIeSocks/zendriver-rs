//! `Element` — handle to a DOM node via CDP `RemoteObjectId` / `BackendNodeId`.

pub mod actions;
pub mod input;
pub mod isolated_eval;
pub mod reads;
pub mod refresh;
pub mod screenshot;
pub mod traversal;

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::error::{Result, ZendriverError};
use crate::tab::Tab;

#[derive(Clone)]
pub struct Element {
    pub(crate) inner: Arc<ElementInner>,
}

pub(crate) struct ElementInner {
    pub(crate) tab: Tab,
    // Held for future tasks (e.g. DOM-domain calls keyed by backendNodeId).
    #[allow(dead_code)]
    pub(crate) backend_node_id: i64,
    pub(crate) remote_object_id: String,
}

impl Element {
    // Constructor consumed by Task 23 (`FindBuilder` materializes Elements);
    // no Phase 1 caller yet.
    #[allow(dead_code)]
    pub(crate) fn new(tab: Tab, backend_node_id: i64, remote_object_id: String) -> Self {
        Self {
            inner: Arc::new(ElementInner {
                tab,
                backend_node_id,
                remote_object_id,
            }),
        }
    }

    /// Constructor used by `FindBuilder::one()` to materialize an Element
    /// from a resolved query match. T16 will replace this stub with the
    /// real `ElementOrigin::Query { ... }` tracking so that
    /// `Element::refresh()` can re-resolve. Until then this just delegates
    /// to `Element::new` and the resulting Element has no origin metadata
    /// (refresh will be a no-op / NotRefreshable when T17 lands).
    //
    // TODO(T16): wire the `ElementOrigin::Query { scope_kind, selector, nth }`
    // payload through so auto-refresh works.
    #[allow(dead_code)]
    pub(crate) fn synthesize_query(
        tab: Tab,
        backend_node_id: i64,
        remote_object_id: String,
    ) -> Self {
        Self::new(tab, backend_node_id, remote_object_id)
    }

    /// Accessor for the parent `Tab` this element was queried from.
    pub fn tab(&self) -> &Tab {
        &self.inner.tab
    }

    /// Call a JS function on this element's remote object. The function
    /// signature MUST take exactly one parameter (the element); use
    /// `function(el){ ... }`.
    pub(crate) async fn call_on(&self, function: &str, args: Value) -> Result<Value> {
        let res = self
            .inner
            .tab
            .call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": self.inner.remote_object_id,
                    "functionDeclaration": function,
                    "arguments": args,
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
        Ok(res["result"].clone())
    }

    /// Evaluate a JS expression in the main world where `el` is bound to this
    /// element handle. Uses `Runtime.callFunctionOn` against the element's
    /// remote object, which lives in whatever world it was created in (main
    /// world if found via `document.querySelector`).
    pub async fn evaluate_main<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let function = format!("function(el){{ return ({}) }}", js.as_ref());
        let result = self
            .call_on(
                &function,
                json!([{ "objectId": self.inner.remote_object_id }]),
            )
            .await?;
        let value = result.get("value").cloned().unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(ZendriverError::Serde)
    }

    /// Element evaluation in an isolated world. Currently delegates to
    /// `evaluate_main`; true isolated-world Element evaluation requires
    /// re-resolving the element via `DOM.resolveNode { executionContextId }`,
    /// which is more invasive than P2 needs.
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        // TODO(P3): true isolated-world via DOM.resolveNode { executionContextId: <isolated> }
        self.evaluate_main(js).await
    }

    /// Click this element via DOM `el.click()`. Phase 1 uses the simple DOM
    /// dispatch; Phase 3 will upgrade to realistic mouse-move + `Input.dispatchMouseEvent`.
    pub async fn click(&self) -> Result<()> {
        let _ = self
            .call_on("function(){ this.click(); }", json!([]))
            .await?;
        Ok(())
    }

    pub async fn inner_text(&self) -> Result<String> {
        let res = self
            .call_on("function(){ return this.innerText; }", json!([]))
            .await?;
        Ok(res["value"].as_str().unwrap_or("").to_string())
    }

    pub async fn outer_html(&self) -> Result<String> {
        let res = self
            .call_on("function(){ return this.outerHTML; }", json!([]))
            .await?;
        Ok(res["value"].as_str().unwrap_or("").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn click_calls_runtime_callfunctionon_with_this_dot_click() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let el = Element::new(tab.clone(), 99, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.click().await }
        });

        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let last = mock.last_sent();
        assert_eq!(last["params"]["objectId"], "R1");
        assert!(last["params"]["functionDeclaration"]
            .as_str()
            .unwrap()
            .contains("this.click()"));
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;
        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn inner_text_returns_value_field() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let el = Element::new(tab, 1, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.inner_text().await }
        });

        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(
            id,
            json!({ "result": { "value": "hello", "type": "string" } }),
        )
        .await;
        let s = fut.await.unwrap().unwrap();
        assert_eq!(s, "hello");
        conn.shutdown();
    }

    #[tokio::test]
    async fn outer_html_returns_value_field() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let el = Element::new(tab, 1, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.outer_html().await }
        });

        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(
            id,
            json!({ "result": { "value": "<button>x</button>", "type": "string" } }),
        )
        .await;
        let s = fut.await.unwrap().unwrap();
        assert_eq!(s, "<button>x</button>");
        conn.shutdown();
    }
}
