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
use tokio::sync::Mutex;

use crate::error::{Result, ZendriverError};
use crate::query::selectors::{QueryScope, RemoteRef, SelectorKind};
use crate::tab::Tab;

#[derive(Clone)]
pub struct Element {
    pub(crate) inner: Arc<ElementInner>,
}

pub(crate) struct ElementInner {
    pub(crate) tab: Tab,
    /// `None` once the element has been observed stale; refilled by
    /// `Element::refresh` (T17). Reads + actions lock briefly to clone
    /// the inner value, then proceed without holding the lock across
    /// `.await` on the CDP session.
    pub(crate) backend_node_id: Mutex<Option<i64>>,
    pub(crate) remote_object_id: Mutex<Option<String>>,
    /// How this element was first obtained — drives T17's
    /// `Element::refresh` re-resolution path.
    #[allow(dead_code)] // First reader is T17 (refresh.rs).
    pub(crate) origin: ElementOrigin,
}

/// How an `Element` was obtained. Drives `Element::refresh` (T17): a
/// `Query`-origin element re-runs its selector against its original
/// scope; a `Traversal`-origin element re-traverses from its parent
/// (which itself may need refreshing recursively); an `Evaluation`
/// origin has no way to re-resolve and surfaces `NotRefreshable`.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants consumed by T17 (refresh).
pub(crate) enum ElementOrigin {
    Query {
        scope_kind: ScopeKind,
        selector: SelectorKind,
        nth: usize,
    },
    Traversal {
        parent: Box<ElementOrigin>,
        kind: TraversalKind,
    },
    /// Returned from a raw JS expression (e.g. `Tab::evaluate` that
    /// yields a node handle). No selector to replay → not refreshable.
    Evaluation,
}

/// The root context against which a `Query` origin's selector was
/// originally resolved. P3 keeps this coarse — we only need to know
/// "tab vs subtree" to decide where refresh should run. Re-resolving
/// an element-subtree origin against a stale parent is deferred to P4
/// (full traversal-chain refresh).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Variants consumed by T17 (refresh).
pub(crate) enum ScopeKind {
    TabMain,
    ElementSubtree,
}

/// The traversal step that produced a `Traversal`-origin element from
/// its parent. P3 lands `Parent` + `NthChild`; richer relationships
/// (sibling indices, etc.) can extend the enum without churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Variants consumed by T17 (refresh).
pub(crate) enum TraversalKind {
    Parent,
    NthChild(usize),
}

impl Element {
    /// Construct an `Element` whose origin is a tracked query against
    /// `scope`. T17's `Element::refresh` re-runs `selector` against
    /// that scope and re-picks `nth` to recover from stale handles.
    pub(crate) fn synthesize_query(
        r: RemoteRef,
        scope: &QueryScope<'_>,
        selector: &SelectorKind,
        nth: usize,
    ) -> Self {
        let scope_kind = match scope {
            QueryScope::Tab(_) => ScopeKind::TabMain,
            QueryScope::Element(_) => ScopeKind::ElementSubtree,
        };
        Self {
            inner: Arc::new(ElementInner {
                tab: scope.tab().clone(),
                backend_node_id: Mutex::new(Some(r.backend_node_id)),
                remote_object_id: Mutex::new(Some(r.remote_object_id)),
                origin: ElementOrigin::Query {
                    scope_kind,
                    selector: selector.clone(),
                    nth,
                },
            }),
        }
    }

    /// Construct an `Element` returned from a JS expression (e.g. a
    /// `Runtime.evaluate` that yielded a node handle). No selector to
    /// replay → `Element::refresh` will error with `NotRefreshable`
    /// once T17 lands. This is the constructor P2's `Element::new`
    /// becomes — the P2 semantics of "raw remote handle, no provenance"
    /// match the new `Evaluation` origin exactly.
    #[allow(dead_code)] // First public callers land with isolated_eval/traversal in T23+.
    pub(crate) fn from_jsret(tab: Tab, backend_node_id: i64, remote_object_id: String) -> Self {
        Self {
            inner: Arc::new(ElementInner {
                tab,
                backend_node_id: Mutex::new(Some(backend_node_id)),
                remote_object_id: Mutex::new(Some(remote_object_id)),
                origin: ElementOrigin::Evaluation,
            }),
        }
    }

    /// Construct an `Element` produced by traversing from `parent_origin`
    /// via `kind` (e.g. `Parent` or `NthChild(i)`). P3 stores the origin
    /// for completeness; full chain-refresh lands in P4 (today, T17's
    /// `refresh` returns `NotRefreshable` for `Traversal` origins).
    pub(crate) fn synthesize_traversal(
        tab: Tab,
        backend_node_id: i64,
        remote_object_id: String,
        parent_origin: ElementOrigin,
        kind: TraversalKind,
    ) -> Self {
        Self {
            inner: Arc::new(ElementInner {
                tab,
                backend_node_id: Mutex::new(Some(backend_node_id)),
                remote_object_id: Mutex::new(Some(remote_object_id)),
                origin: ElementOrigin::Traversal {
                    parent: Box::new(parent_origin),
                    kind,
                },
            }),
        }
    }

    /// Accessor for the parent `Tab` this element was queried from.
    pub fn tab(&self) -> &Tab {
        &self.inner.tab
    }

    /// Lock + clone the current `remote_object_id`, erroring with
    /// `ElementStale` if it has been cleared (which T17's refresh path
    /// does between a stale-error observation and the re-resolve).
    /// Used everywhere a CDP call needs the raw object id.
    pub(crate) async fn remote_object_id_cloned(&self) -> Result<String> {
        self.inner
            .remote_object_id
            .lock()
            .await
            .clone()
            .ok_or(ZendriverError::ElementStale)
    }

    /// Lock + clone the current `backend_node_id`, erroring with
    /// `ElementStale` if it has been cleared. Symmetric with
    /// `remote_object_id_cloned`; used by DOM-domain calls keyed by
    /// backend id (e.g. `DOM.setFileInputFiles`, `DOM.getBoxModel`).
    pub(crate) async fn backend_node_id_cloned(&self) -> Result<i64> {
        self.inner
            .backend_node_id
            .lock()
            .await
            .as_ref()
            .copied()
            .ok_or(ZendriverError::ElementStale)
    }

    /// Call a JS function on this element's remote object. The function
    /// signature MUST take exactly one parameter (the element); use
    /// `function(el){ ... }`.
    pub(crate) async fn call_on(&self, function: &str, args: Value) -> Result<Value> {
        let object_id = self.remote_object_id_cloned().await?;
        let res = self
            .inner
            .tab
            .call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
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

    /// Invoke a JS function in the main world with this element bound as
    /// the first positional argument. Accepts a function declaration whose
    /// first parameter is the element handle (`function(el, ...rest){...}`)
    /// and an `args` JSON array of additional `Runtime.callFunctionOn`
    /// argument descriptors that follow the element. Returns the raw
    /// `result` RemoteObject (caller picks `value` if `returnByValue`).
    ///
    /// Locks the remote-object Mutex once at the top, then routes
    /// through `call_on` (which re-locks once more). The double-lock is
    /// cheap — `tokio::sync::Mutex` is uncontended in the common case
    /// and the guard is dropped before any `.await`.
    #[allow(dead_code)] // First callers (actionability predicates) wire up in T15.
    pub(crate) async fn call_on_main(&self, function: &str, args: Value) -> Result<Value> {
        let object_id = self.remote_object_id_cloned().await?;
        let mut full_args = vec![json!({ "objectId": object_id })];
        if let Some(extra) = args.as_array() {
            full_args.extend(extra.iter().cloned());
        }
        self.call_on(function, Value::Array(full_args)).await
    }

    /// Evaluate a JS expression in the main world where `el` is bound to this
    /// element handle. Uses `Runtime.callFunctionOn` against the element's
    /// remote object, which lives in whatever world it was created in (main
    /// world if found via `document.querySelector`).
    pub async fn evaluate_main<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let function = format!("function(el){{ return ({}) }}", js.as_ref());
        let result = self.call_on_main(&function, json!([])).await?;
        let value = result.get("value").cloned().unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(ZendriverError::Serde)
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn from_jsret_yields_evaluation_origin() {
        let (_mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let el = Element::from_jsret(tab, 7, "R7".to_string());
        assert!(matches!(el.inner.origin, ElementOrigin::Evaluation));
        conn.shutdown();
    }

    #[tokio::test]
    async fn remote_object_id_cloned_errors_after_clear() {
        let (_mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let el = Element::from_jsret(tab, 1, "R1".to_string());

        // Initially OK.
        assert_eq!(el.remote_object_id_cloned().await.unwrap(), "R1");

        // Clear → simulates the T17 refresh path mid-flight.
        *el.inner.remote_object_id.lock().await = None;
        let err = el.remote_object_id_cloned().await.unwrap_err();
        assert!(matches!(err, ZendriverError::ElementStale));
        conn.shutdown();
    }
}
