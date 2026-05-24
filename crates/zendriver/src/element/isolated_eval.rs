//! [`Element::evaluate`] — true isolated-world evaluation.
//!
//! Flow:
//!
//! 1. Resolve the tab's isolated `executionContextId` (creating it on the
//!    first call).
//! 2. `DOM.resolveNode { backendNodeId, executionContextId }` returns a
//!    fresh `RemoteObject` whose `objectId` is bound to that isolated world.
//! 3. `Runtime.callFunctionOn { objectId, functionDeclaration, arguments,
//!    returnByValue, awaitPromise }` invokes the user's JS with `el` bound
//!    to the isolated-world handle. Page globals (`window.foo`, monkeypatches
//!    on `HTMLElement.prototype`, etc.) are NOT visible from inside.
//! 4. Best-effort `Runtime.releaseObject { objectId }` frees the handle so
//!    long-running scrapers don't leak isolated-world `RemoteObject`s.
//!
//! Internal refresh-on-stale recovery handles a stale `backendNodeId`
//! (post-navigation) by re-resolving once and retrying.
//! [`Element::evaluate_main`] (page-world, no re-resolution) lives in
//! `element/mod.rs`.

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::element::Element;
use crate::error::{Result, ZendriverError};

impl Element {
    /// Re-resolve this element inside the tab's isolated world and run JS.
    ///
    /// Wraps `js` as `function(el){ return (<js>) }` against the isolated-
    /// world handle. The returned value is deserialized into `T`.
    ///
    /// Use this for stealth-safe reads — the isolated world doesn't share
    /// scope with page scripts, so monkeypatched DOM prototypes don't lie
    /// to you. For cases where you need page globals (e.g. reading a custom
    /// element's JS-only state), use [`Element::evaluate_main`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::JsException`] when the expression raises;
    /// [`ZendriverError::Serde`] when the result cannot be decoded.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let h1 = tab.find().css("h1").one().await?;
    /// let tag: String = h1.evaluate("el.tagName").await?;
    /// assert_eq!(tag, "H1");
    /// # Ok(()) }
    /// ```
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let js = js.as_ref();
        self.with_refresh(|| async move {
            let ctx_id = self.inner.tab.ensure_isolated_world().await?;
            let backend_node_id = self.backend_node_id_cloned().await?;

            // Re-resolve our node in the isolated world. The returned
            // RemoteObject lives in `ctx_id`, not the main world.
            let resolved = self
                .inner
                .tab
                .call(
                    "DOM.resolveNode",
                    json!({
                        "backendNodeId": backend_node_id,
                        "executionContextId": ctx_id,
                    }),
                )
                .await?;
            let isolated_object_id = resolved["object"]["objectId"]
                .as_str()
                .ok_or_else(|| {
                    ZendriverError::Navigation(
                        "DOM.resolveNode returned no objectId for isolated world".into(),
                    )
                })?
                .to_string();

            let function = format!("function(el){{ return ({js}) }}");
            let result = self
                .inner
                .tab
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": isolated_object_id,
                        "functionDeclaration": function,
                        "arguments": [{ "objectId": isolated_object_id }],
                        "returnByValue": true,
                        "awaitPromise": true,
                    }),
                )
                .await?;

            // Surface page-script exceptions before we deserialize.
            if let Some(details) = result.get("exceptionDetails") {
                let msg = details
                    .get("exception")
                    .and_then(|e| e.get("description"))
                    .and_then(|d| d.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                // Release the handle even on JS exception — best effort.
                let _ = self
                    .inner
                    .tab
                    .call(
                        "Runtime.releaseObject",
                        json!({ "objectId": isolated_object_id }),
                    )
                    .await;
                return Err(ZendriverError::JsException(msg));
            }

            let value = result
                .get("result")
                .and_then(|r| r.get("value"))
                .cloned()
                .unwrap_or(Value::Null);

            // Best-effort cleanup so isolated-world handles don't pile up
            // across many evaluate() calls. Failures are non-fatal — the
            // worst case is a short-lived leak until the isolated world
            // itself is replaced.
            let _ = self
                .inner
                .tab
                .call(
                    "Runtime.releaseObject",
                    json!({ "objectId": isolated_object_id }),
                )
                .await;

            serde_json::from_value(value).map_err(ZendriverError::Serde)
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tab::Tab;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn evaluate_dispatches_full_isolated_world_sequence() {
        // Verifies the canonical 4-step CDP flow:
        //   1. Page.getFrameTree            (ensure_isolated_world prep)
        //   2. Page.createIsolatedWorld     (ensure_isolated_world create)
        //   3. DOM.resolveNode              (rebind node in ctx 42)
        //   4. Runtime.callFunctionOn       (run user JS on isolated handle)
        //   5. Runtime.releaseObject        (best-effort cleanup)
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 314, "R_MAIN".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.evaluate::<String>("el.tagName").await }
        });

        // 1. Page.getFrameTree — discovers main frame id.
        let id_tree = mock.expect_cmd("Page.getFrameTree").await;
        mock.reply(
            id_tree,
            json!({ "frameTree": { "frame": { "id": "FRAME_1" } } }),
        )
        .await;

        // 2. Page.createIsolatedWorld — gets us executionContextId 42.
        let id_world = mock.expect_cmd("Page.createIsolatedWorld").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["frameId"], "FRAME_1");
        assert_eq!(sent["params"]["worldName"], "zendriver-eval");
        mock.reply(id_world, json!({ "executionContextId": 42 }))
            .await;

        // 3. DOM.resolveNode — re-binds backendNodeId 314 into ctx 42.
        let id_resolve = mock.expect_cmd("DOM.resolveNode").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["backendNodeId"], 314);
        assert_eq!(sent["params"]["executionContextId"], 42);
        mock.reply(
            id_resolve,
            json!({ "object": { "objectId": "R_ISO", "type": "object", "subtype": "node" } }),
        )
        .await;

        // 4. Runtime.callFunctionOn — uses isolated objectId for BOTH the
        //    target object AND the el argument; wraps user js in
        //    `function(el){ return (...) }`.
        let id_call = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert_eq!(
            sent["params"]["objectId"], "R_ISO",
            "callFunctionOn must target the isolated-world handle, not the main-world one",
        );
        assert_eq!(
            sent["params"]["arguments"][0]["objectId"], "R_ISO",
            "the bound `el` argument must be the isolated handle",
        );
        let decl = sent["params"]["functionDeclaration"].as_str().unwrap();
        assert!(
            decl.contains("function(el)") && decl.contains("el.tagName"),
            "function declaration must wrap user JS; got: {decl}",
        );
        assert_eq!(sent["params"]["returnByValue"], true);
        assert_eq!(sent["params"]["awaitPromise"], true);
        mock.reply(
            id_call,
            json!({ "result": { "type": "string", "value": "DIV" } }),
        )
        .await;

        // 5. Runtime.releaseObject — best-effort cleanup of the isolated handle.
        let id_release = mock.expect_cmd("Runtime.releaseObject").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R_ISO");
        mock.reply(id_release, json!({})).await;

        let value = fut.await.unwrap().unwrap();
        assert_eq!(value, "DIV");
        conn.shutdown();
    }
}
