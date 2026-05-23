//! `Element` traversal: walk to parents + children, preserving origin so
//! P4's chain-refresh can re-resolve the result against its parent.
//!
//! Both methods bypass `Element::call_on_main` (which forces
//! `returnByValue: true`) and dispatch `Runtime.callFunctionOn` with
//! `returnByValue: false` so the result carries an `objectId` we can
//! follow up on. The reused `extract_node_ref` / `extract_array_refs`
//! helpers from `query::selectors` handle the standard
//! `objectId → DOM.describeNode → backendNodeId` dance.

use serde_json::json;

use crate::element::{Element, TraversalKind};
use crate::error::Result;
use crate::query::selectors::{extract_array_refs, extract_node_ref};

impl Element {
    /// Return this element's `parentElement`, or `Ok(None)` if it has no
    /// parent (root `<html>`, detached, or a text node with no parent).
    ///
    /// The returned `Element` is constructed with
    /// `ElementOrigin::Traversal { parent: <self.origin>, kind: Parent }`
    /// so once P4 lands the full chain-refresh, a stale parent will
    /// recursively re-resolve via this element's `Element::refresh`.
    pub async fn parent(&self) -> Result<Option<Element>> {
        self.with_refresh(|| async move {
            let object_id = self.remote_object_id_cloned().await?;
            let raw = self
                .inner
                .tab
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": "function(){ return this.parentElement; }",
                        "arguments": [],
                        "returnByValue": false,
                        "awaitPromise": true,
                    }),
                )
                .await?;
            let Some(remote_ref) = extract_node_ref(&self.inner.tab, &raw["result"]).await? else {
                return Ok(None);
            };
            let parent_origin = self.inner.origin.clone();
            Ok(Some(Element::synthesize_traversal(
                self.inner.tab.clone(),
                remote_ref.backend_node_id,
                remote_ref.remote_object_id,
                parent_origin,
                TraversalKind::Parent,
            )))
        })
        .await
    }

    /// Return this element's `children` (HTMLCollection materialized via
    /// `Array.from`) as `Element`s tagged with `TraversalKind::NthChild(i)`
    /// so P4's chain-refresh can re-pick the same index after a
    /// re-render. Empty children yield an empty `Vec`, not an error.
    pub async fn children(&self) -> Result<Vec<Element>> {
        self.with_refresh(|| async move {
            let object_id = self.remote_object_id_cloned().await?;
            let raw = self
                .inner
                .tab
                .call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": "function(){ return Array.from(this.children); }",
                        "arguments": [],
                        "returnByValue": false,
                        "awaitPromise": true,
                    }),
                )
                .await?;
            let refs = extract_array_refs(&self.inner.tab, &raw["result"]).await?;
            let parent_origin = self.inner.origin.clone();
            let out = refs
                .into_iter()
                .enumerate()
                .map(|(idx, r)| {
                    Element::synthesize_traversal(
                        self.inner.tab.clone(),
                        r.backend_node_id,
                        r.remote_object_id,
                        parent_origin.clone(),
                        TraversalKind::NthChild(idx),
                    )
                })
                .collect();
            Ok(out)
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::element::ElementOrigin;
    use crate::tab::Tab;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn parent_calls_runtime_callfunctionon_with_parent_element() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let el = Element::from_jsret(tab, 7, "R7".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.parent().await }
        });

        // 1. Runtime.callFunctionOn with this.parentElement.
        let id_q = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["objectId"], "R7");
        let decl = sent["params"]["functionDeclaration"].as_str().unwrap();
        assert!(
            decl.contains("this.parentElement"),
            "parent() must call this.parentElement, got: {decl}"
        );
        assert_eq!(sent["params"]["returnByValue"], false);
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RP", "type": "object", "subtype": "node" } }),
        )
        .await;

        // 2. DOM.describeNode resolves backendNodeId.
        let id_d = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RP");
        mock.reply(id_d, json!({ "node": { "backendNodeId": 314 } }))
            .await;

        let parent = fut.await.unwrap().unwrap().expect("parent present");
        assert_eq!(parent.remote_object_id_cloned().await.unwrap(), "RP");
        assert_eq!(parent.backend_node_id_cloned().await.unwrap(), 314);
        assert!(matches!(
            parent.inner.origin,
            ElementOrigin::Traversal {
                kind: TraversalKind::Parent,
                ..
            }
        ));
        conn.shutdown();
    }
}
