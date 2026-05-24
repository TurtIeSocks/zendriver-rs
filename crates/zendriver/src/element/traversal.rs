//! [`Element`] traversal: walk to parents + children.
//!
//! Both methods preserve origin so the auto-refresh chain can re-resolve
//! the result against its parent. They dispatch `Runtime.callFunctionOn`
//! with `returnByValue: false` so the result carries an `objectId` we can
//! follow up on.

use serde_json::json;

use crate::element::{Element, TraversalKind};
use crate::error::Result;
use crate::query::selectors::{extract_array_refs, extract_node_ref};

impl Element {
    /// Return this element's `parentElement`, or `Ok(None)` if absent.
    ///
    /// Returns `None` for the root `<html>`, detached nodes, or text nodes
    /// with no parent. The returned `Element`'s origin tracks back to this
    /// element so a stale parent will recursively re-resolve via
    /// [`Element::refresh`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let el = tab.find().css("h1").one().await?;
    /// if let Some(parent) = el.parent().await? {
    ///     println!("parent: {}", parent.inner_text().await?);
    /// }
    /// # Ok(()) }
    /// ```
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
            let Some(remote_ref) =
                extract_node_ref(self.inner.tab.session(), &raw["result"]).await?
            else {
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
    ///
    /// Pick a selector kind, optionally apply modifiers, then terminate
    /// with `.one()` or `.one_or_none()`. Matches outside this element's
    /// subtree are not considered. The terminal dispatches
    /// `Runtime.callFunctionOn` against this element's remote object,
    /// distinguishing it from [`crate::Tab::find`] which scans the whole
    /// document.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let card = tab.find().css(".card").one().await?;
    /// let title = card.find().css("h2").one().await?;
    /// # let _ = title;
    /// # Ok(()) }
    /// ```
    pub fn find(&self) -> crate::query::FindBuilder<'_> {
        crate::query::FindBuilder::new_for_element(self)
    }

    /// Begin a subtree-scoped chainable query that returns ALL matches.
    ///
    /// Mirrors [`Element::find`] selectors + modifiers (no `.nth`);
    /// terminate with `.many()` (errors on empty) or `.many_or_empty()`
    /// (returns an empty `Vec`).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let list = tab.find().css("ul").one().await?;
    /// let items = list.find_all().css("li").many_or_empty().await?;
    /// println!("{} items", items.len());
    /// # Ok(()) }
    /// ```
    pub fn find_all(&self) -> crate::query::FindAllBuilder<'_> {
        crate::query::FindAllBuilder::new_for_element(self)
    }

    /// Return this element's child elements as a `Vec<Element>`.
    ///
    /// HTMLCollection is materialized via `Array.from`. The returned
    /// elements are tagged with `NthChild(i)` so auto-refresh can re-pick
    /// the same index after a re-render. Empty children yield an empty
    /// `Vec`, not an error.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let nav = tab.find().css("nav").one().await?;
    /// for link in nav.children().await? {
    ///     println!("{}", link.inner_text().await?);
    /// }
    /// # Ok(()) }
    /// ```
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
            let refs = extract_array_refs(self.inner.tab.session(), &raw["result"]).await?;
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
        let tab = Tab::new_for_test(sess);
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
        let tab = Tab::new_for_test(sess);
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
