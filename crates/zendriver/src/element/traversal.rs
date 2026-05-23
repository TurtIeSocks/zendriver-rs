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

    /// Begin a subtree-scoped chainable query against this element.
    /// Pick a selector kind (`.css`, `.xpath`, `.text`, `.text_exact`,
    /// `.text_regex`, `.text_regex_with_flags`, `.role`, `.role_named`),
    /// optionally apply modifiers (`.nth`, `.visible_only`, `.in_frame`,
    /// `.timeout`), then terminate with `.one()` / `.one_or_none()`.
    ///
    /// Resolution runs as `this.querySelector(...)` (CSS) or the
    /// equivalent element-relative form for other selector kinds —
    /// matches outside this element's subtree are not considered. The
    /// terminal dispatches `Runtime.callFunctionOn` against this
    /// element's remote object, distinguishing it from `Tab::find` which
    /// dispatches `Runtime.evaluate` against the whole document.
    pub fn find(&self) -> crate::query::FindBuilder<'_> {
        crate::query::FindBuilder::new_for_element(self)
    }

    /// Begin a subtree-scoped chainable query that returns ALL matches
    /// within this element. Mirrors [`Element::find`] selectors +
    /// modifiers (no `.nth`); terminate with `.many()` (errors on empty)
    /// or `.many_or_empty()` (returns an empty `Vec`).
    pub fn find_all(&self) -> crate::query::FindAllBuilder<'_> {
        crate::query::FindAllBuilder::new_for_element(self)
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

    #[tokio::test]
    async fn find_css_one_dispatches_callfunctionon_against_element_scope() {
        // Element::find().css(...).one() must resolve via
        // Runtime.callFunctionOn against the element's remote object
        // (`this.querySelectorAll(...)`) rather than Runtime.evaluate
        // against `document` — that's the contract that distinguishes
        // subtree-scoped queries from tab-scoped queries.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let el = Element::from_jsret(tab, 9, "ROOT".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.find().css("button").one().await }
        });

        // 1. Element-scoped CSS many → Runtime.callFunctionOn on the
        //    element's remote object, NOT Runtime.evaluate.
        let id_q = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert_eq!(
            sent["params"]["objectId"], "ROOT",
            "element-scoped query must dispatch against the element's remote object"
        );
        let decl = sent["params"]["functionDeclaration"].as_str().unwrap();
        assert!(
            decl.contains("this.querySelectorAll"),
            "element-scoped CSS must use this.querySelectorAll, got: {decl}"
        );
        assert_eq!(sent["params"]["arguments"][0]["value"], "button");
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;

        // 2. Enumerate the array — one match at index 0.
        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RArr");
        mock.reply(
            id_p,
            json!({
                "result": [
                    { "name": "0", "value": { "objectId": "RB", "type": "object", "subtype": "node" } },
                    { "name": "length", "value": { "value": 1, "type": "number" } }
                ]
            }),
        )
        .await;

        // 3. DOM.describeNode resolves backendNodeId of the matched node.
        let id_d = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RB");
        mock.reply(id_d, json!({ "node": { "backendNodeId": 77 } }))
            .await;

        let found = fut.await.unwrap().unwrap();
        assert_eq!(found.remote_object_id_cloned().await.unwrap(), "RB");
        assert_eq!(found.backend_node_id_cloned().await.unwrap(), 77);
        // Element-scoped origin tracks ScopeKind::ElementSubtree so T17
        // refresh can re-resolve via this element's subtree.
        assert!(matches!(
            found.inner.origin,
            ElementOrigin::Query {
                scope_kind: crate::element::ScopeKind::ElementSubtree,
                ..
            }
        ));
        conn.shutdown();
    }
}
