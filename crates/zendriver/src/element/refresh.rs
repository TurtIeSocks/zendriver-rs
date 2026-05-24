//! Auto-refresh: re-resolve a stale `Element` via its memoized
//! `ElementOrigin` and retry the wrapped CDP op once.
//!
//! Coverage:
//!   - `ElementOrigin::Query { scope_kind: TabMain, .. }` — re-runs the
//!     stored `SelectorKind::resolve_many` against `QueryScope::Tab` and
//!     re-picks the `nth` index.
//!   - `ElementOrigin::Query { scope_kind: ElementSubtree, .. }` —
//!     `NotRefreshable`. Refreshing a subtree query requires
//!     reconstructing the parent element, which would mean carrying the
//!     parent's full origin chain; deferred past P4.
//!   - `ElementOrigin::Traversal { parent, kind }` — recursively
//!     resolves the parent's origin to a fresh `RemoteRef`, synthesizes
//!     a temporary `Element` from that ref, then re-traverses (Parent
//!     or NthChild) via `Runtime.callFunctionOn` against the fresh
//!     parent. Lands the P3 T17 deferral. Recursion depth = traversal
//!     chain length; `Box<ElementOrigin>` keeps storage off the stack.
//!   - `ElementOrigin::Evaluation` — `NotRefreshable` (no selector to
//!     replay).
//!
//! `with_refresh(op)` retries `op` exactly once when the first attempt
//! errors with a stale-node signature (see `is_stale_node_error`). The
//! second failure surfaces as-is.

use std::future::Future;
use std::pin::Pin;

use serde_json::{json, Value};

use crate::element::{Element, ElementOrigin, ScopeKind, TraversalKind};
use crate::error::{Result, ZendriverError};
use crate::query::selectors::{extract_node_ref, QueryScope, RemoteRef};
use crate::tab::Tab;

impl Element {
    /// Re-resolve this element's underlying CDP handle via its origin.
    /// Updates `backend_node_id` + `remote_object_id` in place on success.
    ///
    /// Returns `NotRefreshable` for `Evaluation` and
    /// `Query { ElementSubtree, .. }` origins. `Traversal` origins
    /// recursively re-resolve their parent chain.
    pub async fn refresh(&self) -> Result<()> {
        let r = resolve_origin(&self.inner.origin, &self.inner.tab).await?;
        *self.inner.backend_node_id.lock().await = Some(r.backend_node_id);
        *self.inner.remote_object_id.lock().await = Some(r.remote_object_id);
        Ok(())
    }

    /// Run `op`, retrying it once if the first attempt errors with a
    /// stale-node signature. Used by every `Element` read + action so
    /// the retry-on-stale logic stays centralized.
    pub(crate) async fn with_refresh<T, F, Fut>(&self, op: F) -> Result<T>
    where
        F: Fn() -> Fut + Send,
        Fut: Future<Output = Result<T>> + Send,
    {
        match op().await {
            Ok(v) => Ok(v),
            Err(e) if is_stale_node_error(&e) => {
                self.refresh().await?;
                op().await
            }
            Err(e) => Err(e),
        }
    }
}

/// Recursively re-resolve `origin` against `tab`, returning a fresh
/// `RemoteRef` for the element it describes.
///
/// - `Query { TabMain, selector, nth }` re-runs the selector against
///   the whole tab and re-picks `nth`.
/// - `Query { ElementSubtree, .. }` — not refreshable here: we'd need to
///   reconstruct the parent element + its full origin chain, which the
///   `Query` variant doesn't carry.
/// - `Traversal { parent, kind }` recursively resolves `parent`, then
///   synthesizes a temporary parent `Element` and dispatches
///   `Runtime.callFunctionOn` with `this.parentElement` (Parent) or
///   `this.children[idx]` (NthChild) against the parent's fresh
///   `objectId`. The follow-up `DOM.describeNode` round-trip lifts the
///   result to a `RemoteRef`.
/// - `Evaluation` — no selector to replay → `NotRefreshable`.
///
/// `BoxFuture` wraps the recursive call so the async fn type-checks for
/// arbitrary depths without monomorphizing a distinct future type per
/// level (Rust doesn't natively support `async fn` recursion).
fn resolve_origin<'a>(
    origin: &'a ElementOrigin,
    tab: &'a Tab,
) -> Pin<Box<dyn Future<Output = Result<RemoteRef>> + Send + 'a>> {
    Box::pin(async move {
        match origin {
            ElementOrigin::Query {
                scope_kind: ScopeKind::TabMain,
                selector,
                nth,
            } => {
                let scope = QueryScope::Tab(tab);
                let candidates = selector.resolve_many(&scope).await?;
                candidates
                    .into_iter()
                    .nth(*nth)
                    .ok_or_else(|| ZendriverError::ElementNotFound {
                        selector: format!("{selector:?}"),
                    })
            }
            ElementOrigin::Query {
                scope_kind: ScopeKind::ElementSubtree,
                ..
            }
            | ElementOrigin::Evaluation => Err(ZendriverError::NotRefreshable),
            ElementOrigin::Traversal { parent, kind } => {
                // Recursively re-resolve the parent's origin first.
                let parent_ref = resolve_origin(parent, tab).await?;
                // Synthesize a temporary Element bound to the fresh
                // parent ref so we can route the traversal call through
                // `Runtime.callFunctionOn { objectId: parent.objectId }`.
                let parent_el = Element::from_jsret(
                    tab.clone(),
                    parent_ref.backend_node_id,
                    parent_ref.remote_object_id,
                );
                let (function_decl, missing_selector) = match kind {
                    TraversalKind::Parent => (
                        "function(){ return this.parentElement; }".to_string(),
                        "parent".to_string(),
                    ),
                    TraversalKind::NthChild(idx) => (
                        format!("function(){{ return this.children[{idx}]; }}"),
                        format!("nth_child({idx})"),
                    ),
                };
                let object_id = parent_el.remote_object_id_cloned().await?;
                let raw = tab
                    .call(
                        "Runtime.callFunctionOn",
                        json!({
                            "objectId": object_id,
                            "functionDeclaration": function_decl,
                            "arguments": [],
                            "returnByValue": false,
                            "awaitPromise": true,
                        }),
                    )
                    .await?;
                extract_remote_ref(&raw["result"], tab).await?.ok_or(
                    ZendriverError::ElementNotFound {
                        selector: missing_selector,
                    },
                )
            }
        }
    })
}

/// Lift a `Runtime.callFunctionOn` *single-node* result into a
/// `RemoteRef`. Thin wrapper over [`extract_node_ref`] that takes the
/// `Tab` (rather than a raw `SessionHandle`) — keeps the call sites in
/// `resolve_origin` readable. Null / undefined results yield `Ok(None)`,
/// which callers map to `ElementNotFound` with a kind-specific selector.
async fn extract_remote_ref(value: &Value, tab: &Tab) -> Result<Option<RemoteRef>> {
    extract_node_ref(tab.session(), value).await
}

/// Returns `true` if `e` looks like a stale-node failure from Chrome —
/// either the DOM domain reporting an unknown node id, or the Runtime
/// domain reporting a missing execution context (navigation race).
///
/// Matches against:
///   - `ZendriverError::ElementStale` (set by the inner-id accessors
///     when they observe a cleared id mid-flight).
///   - `ZendriverError::Navigation(m)` where `m` contains
///     `"No node with given id"` or `"Cannot find context"`. The
///     `From<CallError>` impl in `error.rs` maps `-32000 "Cannot find
///     context"` into the Navigation variant.
///   - `ZendriverError::Cdp { message, .. }` where `message` contains
///     either of the above substrings (covers DOM-domain stale errors
///     that don't get pre-mapped to Navigation).
pub(crate) fn is_stale_node_error(e: &ZendriverError) -> bool {
    match e {
        ZendriverError::ElementStale => true,
        ZendriverError::Navigation(m) => {
            m.contains("No node with given id") || m.contains("Cannot find context")
        }
        ZendriverError::Cdp { message, .. } => {
            message.contains("No node with given id") || message.contains("Cannot find context")
        }
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::Mutex;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    use crate::element::ElementInner;
    use crate::tab::Tab;

    /// Build a Traversal-origin Element whose `parent` is a tracked
    /// Query/TabMain origin. Mirrors what `Element::parent()` produces
    /// against a query-rooted element, so the refresh test exercises a
    /// realistic recursion (parent re-resolves via selector → child
    /// re-traverses via `this.parentElement`).
    fn make_traversal_element(
        tab: Tab,
        backend: i64,
        remote: &str,
        parent_origin: ElementOrigin,
        kind: TraversalKind,
    ) -> Element {
        Element {
            inner: Arc::new(ElementInner {
                tab,
                backend_node_id: Mutex::new(Some(backend)),
                remote_object_id: Mutex::new(Some(remote.to_string())),
                origin: ElementOrigin::Traversal {
                    parent: Box::new(parent_origin),
                    kind,
                },
            }),
        }
    }

    #[tokio::test]
    async fn traversal_parent_refresh_reresolves_parent_then_retraverses() {
        // A Traversal { Parent }-origin Element whose parent's origin is
        // Query/TabMain { Css("#root"), nth=0 } should, on refresh:
        //   1. Re-run document.querySelector('#root') (parent re-resolve)
        //   2. DOM.describeNode on the parent's fresh objectId
        //   3. Runtime.callFunctionOn with `this.parentElement` against
        //      that parent objectId (re-traverse)
        //   4. DOM.describeNode on the parentElement objectId
        // and update the Element's backend_node_id + remote_object_id.
        use crate::element::ScopeKind;
        use crate::query::selectors::SelectorKind;

        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let parent_origin = ElementOrigin::Query {
            scope_kind: ScopeKind::TabMain,
            selector: SelectorKind::Css("#root".into()),
            nth: 0,
        };
        let el = make_traversal_element(
            tab.clone(),
            10, // stale backend id
            "R_STALE",
            parent_origin,
            TraversalKind::Parent,
        );

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.refresh().await }
        });

        // 1. Parent re-resolve via `resolve_many` → Runtime.evaluate with
        //    `Array.from(document.querySelectorAll('#root'))`.
        let id_eval = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            sent.contains("document.querySelectorAll") && sent.contains("#root"),
            "parent re-resolve must call querySelectorAll('#root'); got: {sent}"
        );
        mock.reply(
            id_eval,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;

        // 2. Enumerate the candidate array — one match at index 0.
        let id_props = mock.expect_cmd("Runtime.getProperties").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RArr");
        mock.reply(
            id_props,
            json!({
                "result": [
                    { "name": "0", "value": { "objectId": "R_PARENT", "type": "object", "subtype": "node" } },
                    { "name": "length", "value": { "value": 1, "type": "number" } }
                ]
            }),
        )
        .await;

        // 3. DOM.describeNode on the new parent objectId → backendNodeId.
        let id_d1 = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R_PARENT");
        mock.reply(id_d1, json!({ "node": { "backendNodeId": 200 } }))
            .await;

        // 4. Re-traversal: Runtime.callFunctionOn { objectId: R_PARENT,
        //    functionDeclaration: function(){ return this.parentElement; } }.
        let id_call = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert_eq!(
            sent["params"]["objectId"], "R_PARENT",
            "re-traversal must dispatch against the fresh parent objectId"
        );
        let decl = sent["params"]["functionDeclaration"].as_str().unwrap();
        assert!(
            decl.contains("this.parentElement"),
            "Parent traversal must call `this.parentElement`; got: {decl}"
        );
        assert_eq!(sent["params"]["returnByValue"], false);
        mock.reply(
            id_call,
            json!({ "result": { "objectId": "R_FRESH", "type": "object", "subtype": "node" } }),
        )
        .await;

        // 5. DOM.describeNode on the new target objectId.
        let id_d2 = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R_FRESH");
        mock.reply(id_d2, json!({ "node": { "backendNodeId": 314 } }))
            .await;

        fut.await.unwrap().unwrap();

        // Element's in-place handles updated to the fresh ids.
        assert_eq!(el.remote_object_id_cloned().await.unwrap(), "R_FRESH");
        assert_eq!(el.backend_node_id_cloned().await.unwrap(), 314);
        conn.shutdown();
    }

    #[tokio::test]
    async fn traversal_chain_two_level_refresh_recurses() {
        // A Traversal { Parent }-origin Element whose parent itself is a
        // Traversal { NthChild(2) }-origin (grandparent = Query/TabMain
        // for `.list`) should refresh both levels in turn:
        //   - grandparent re-resolve via Runtime.evaluate
        //   - grandparent describeNode
        //   - parent re-traverse via callFunctionOn { this.children[2] }
        //   - parent describeNode
        //   - self re-traverse via callFunctionOn { this.parentElement }
        //   - self describeNode
        use crate::element::ScopeKind;
        use crate::query::selectors::SelectorKind;

        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let grandparent_origin = ElementOrigin::Query {
            scope_kind: ScopeKind::TabMain,
            selector: SelectorKind::Css(".list".into()),
            nth: 0,
        };
        let parent_origin = ElementOrigin::Traversal {
            parent: Box::new(grandparent_origin),
            kind: TraversalKind::NthChild(2),
        };
        let el = make_traversal_element(
            tab.clone(),
            5,
            "R_STALE",
            parent_origin,
            TraversalKind::Parent,
        );

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.refresh().await }
        });

        // Level 2: grandparent re-resolve.
        let id_eval = mock.expect_cmd("Runtime.evaluate").await;
        assert!(mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .contains(".list"));
        mock.reply(
            id_eval,
            json!({ "result": { "objectId": "GP_ARR", "type": "object", "subtype": "array" } }),
        )
        .await;

        let id_props = mock.expect_cmd("Runtime.getProperties").await;
        mock.reply(
            id_props,
            json!({
                "result": [
                    { "name": "0", "value": { "objectId": "GP", "type": "object", "subtype": "node" } },
                    { "name": "length", "value": { "value": 1, "type": "number" } }
                ]
            }),
        )
        .await;

        let id_d = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "GP");
        mock.reply(id_d, json!({ "node": { "backendNodeId": 100 } }))
            .await;

        // Level 1: parent re-traversal via `this.children[2]`.
        let id_call = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["objectId"], "GP");
        let decl = sent["params"]["functionDeclaration"].as_str().unwrap();
        assert!(
            decl.contains("this.children[2]"),
            "NthChild(2) must call `this.children[2]`; got: {decl}"
        );
        mock.reply(
            id_call,
            json!({ "result": { "objectId": "P", "type": "object", "subtype": "node" } }),
        )
        .await;

        let id_d = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "P");
        mock.reply(id_d, json!({ "node": { "backendNodeId": 200 } }))
            .await;

        // Level 0: self re-traversal via `this.parentElement`.
        let id_call = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["objectId"], "P");
        assert!(sent["params"]["functionDeclaration"]
            .as_str()
            .unwrap()
            .contains("this.parentElement"));
        mock.reply(
            id_call,
            json!({ "result": { "objectId": "SELF", "type": "object", "subtype": "node" } }),
        )
        .await;

        let id_d = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "SELF");
        mock.reply(id_d, json!({ "node": { "backendNodeId": 300 } }))
            .await;

        fut.await.unwrap().unwrap();

        assert_eq!(el.remote_object_id_cloned().await.unwrap(), "SELF");
        assert_eq!(el.backend_node_id_cloned().await.unwrap(), 300);
        conn.shutdown();
    }

    #[test]
    fn is_stale_node_error_matches_expected_shapes() {
        // ElementStale: explicit stale signal.
        assert!(is_stale_node_error(&ZendriverError::ElementStale));

        // Navigation with "Cannot find context" — the `From<CallError>`
        // mapping in error.rs lands stale-context CDP errors here.
        assert!(is_stale_node_error(&ZendriverError::Navigation(
            "Cannot find context with specified id".into(),
        )));

        // Cdp variant with "No node with given id" — typical DOM-domain
        // stale error that isn't pre-mapped to Navigation.
        assert!(is_stale_node_error(&ZendriverError::Cdp {
            code: -32000,
            message: "No node with given id".into(),
            data: None,
        }));

        // Unrelated error: a plain timeout should NOT trigger refresh.
        assert!(!is_stale_node_error(&ZendriverError::Timeout(
            Duration::from_secs(1)
        )));
    }
}
