//! Handle to a DOM node via CDP `RemoteObjectId` / `BackendNodeId`.
//!
//! [`Element`] is the result of a [`crate::Tab::find`] /
//! [`crate::Tab::find_all`] / [`crate::Element::find`] query (or any of the
//! traversal helpers). Actions sit on submodules:
//!
//! - [`mod@actions`] — click / hover / focus / scroll / set_value / clear /
//!   upload_files.
//! - [`mod@input`] — type_text / type_text_fast / press / press_with.
//! - [`mod@reads`] — attribute access, innerText, outerHTML, bounding box,
//!   visibility / enabled state.
//! - [`mod@traversal`] — parent / nth_child.
//! - [`mod@isolated_eval`] — true isolated-world `evaluate` (with the
//!   element bound as `el`).
//! - [`mod@screenshot`] — element-scoped PNG capture.
//! - [`mod@refresh`] — auto-refresh-on-stale-handle support.
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! # let browser = zendriver::Browser::builder().launch().await?;
//! # let tab = browser.main_tab();
//! tab.goto("https://example.com").await?;
//! let h1 = tab.find().css("h1").one().await?;
//! assert_eq!(h1.inner_text().await?, "Example Domain");
//! # Ok(()) }
//! ```

pub mod actions;
pub mod input;
pub mod isolated_eval;
pub mod reads;
pub mod refresh;
pub mod screenshot;
pub mod traversal;

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::error::{Result, ZendriverError};
use crate::query::selectors::{QueryScope, RemoteRef, SelectorKind};
use crate::tab::Tab;

/// Handle to a DOM node in a [`Tab`].
///
/// `Element` is `Clone` (cheap — wraps an `Arc`) and `Send + Sync`. Methods
/// are grouped into thematic submodules — see the [module-level docs](self)
/// for the map.
///
/// Get one via [`Tab::find`](crate::Tab::find) / [`Tab::find_all`](crate::Tab::find_all),
/// frame queries, or element traversal helpers.
#[derive(Clone, Debug)]
pub struct Element {
    pub(crate) inner: Arc<ElementInner>,
}

#[derive(Debug)]
pub(crate) struct ElementInner {
    pub(crate) tab: Tab,
    /// `None` once the element has been observed stale; refilled by
    /// `Element::refresh` (T17). Reads + actions lock briefly to clone
    /// the inner value, then proceed without holding the lock across
    /// `.await` on the CDP session.
    pub(crate) backend_node_id: Mutex<Option<i64>>,
    pub(crate) remote_object_id: Mutex<Option<String>>,
    /// How this element was first obtained — drives `Element::refresh`'s
    /// re-resolution path (`element::refresh::resolve_origin`).
    pub(crate) origin: ElementOrigin,
}

/// How an `Element` was obtained. Drives `Element::refresh`: a
/// `Query`-origin element re-runs its selector against its original
/// scope; a `Traversal`-origin element re-traverses from its parent
/// (which itself may need refreshing recursively); an `Evaluation`
/// origin has no way to re-resolve and surfaces `NotRefreshable`.
#[derive(Debug, Clone)]
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
/// originally resolved. Kept coarse — we only need to know "tab vs
/// subtree" to decide where refresh should run. Re-resolving an
/// element-subtree origin against a stale parent is not yet supported
/// (see `ElementOrigin::Query { scope_kind: ElementSubtree, .. }`'s
/// `NotRefreshable` arm in `element::refresh`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    TabMain,
    ElementSubtree,
}

/// The traversal step that produced a `Traversal`-origin element from
/// its parent. Covers `Parent` + `NthChild`; richer relationships
/// (sibling indices, etc.) can extend the enum without churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            // Frame queries are document-level within a frame's own
            // session — refresh-wise they look like TabMain (re-resolve
            // the selector against the frame's document root) rather
            // than an element-subtree walk. The dedicated FrameMain
            // variant lands when T17 wires Frame-aware refresh.
            QueryScope::Tab(_) | QueryScope::Frame(_) => ScopeKind::TabMain,
            QueryScope::Element(_) => ScopeKind::ElementSubtree,
        };
        Self {
            inner: Arc::new(ElementInner {
                tab: scope.synthesize_tab(),
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
    /// `Runtime.evaluate` that yielded a node handle, or a predicate
    /// query's match — see `Resolver::synthesize` in `query::mod`). No
    /// selector to replay → `Element::refresh` errors with
    /// `NotRefreshable` for elements built this way.
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

    /// The parent [`Tab`] this element was queried from.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let el = tab.find().css("button").one().await?;
    /// let _: &zendriver::Tab = el.tab();
    /// # Ok(()) }
    /// ```
    #[must_use]
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

    /// Dispatch `Runtime.callFunctionOn` against `object_id` and unwrap the
    /// response into its `result` RemoteObject.
    ///
    /// World-agnostic: the JS runs in whatever world `object_id` belongs to.
    /// Picking that world is the caller's job — [`Element::call_on`] resolves
    /// a fresh isolated-world handle first, [`Element::call_on_main`] passes
    /// the element's own page-world handle.
    async fn call_function_on(
        &self,
        object_id: &str,
        function: &str,
        args: Value,
    ) -> Result<Value> {
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

    /// Call a JS function on this element **in the tab's isolated world**.
    ///
    /// The function receives the element as `this`, so its signature takes
    /// only the extra arguments: `function(a, b){ ... this ... }`. `args` is
    /// forwarded verbatim and must hold plain values — a page-world
    /// `objectId` means nothing inside the isolated world.
    ///
    /// Each call re-resolves the node through `DOM.resolveNode
    /// { backendNodeId, executionContextId }` so the handle we invoke on
    /// belongs to the isolated world, then releases it (best effort, so a
    /// long scrape doesn't accumulate handles). That costs two extra
    /// round-trips over reusing the page-world handle, which is the price of
    /// the guarantee: a page that has monkeypatched `getAttribute`,
    /// `innerText`, `getBoundingClientRect` or `getComputedStyle` can
    /// neither feed the automation false values nor observe it reading.
    /// Isolated worlds share the DOM, so the values are the same ones the
    /// page actually renders.
    ///
    /// Use [`Element::call_on_main`] instead when the JS has to see
    /// page-defined state (page globals, framework expandos on the node)
    /// rather than DOM state.
    pub(crate) async fn call_on(&self, function: &str, args: Value) -> Result<Value> {
        // A navigation destroys the isolated world while the tab still holds
        // its cached context id, so a read that straddles one resolves
        // against a dead context. Drop the cache and rebuild it once — the
        // same recovery `Tab::evaluate` performs, without which the very
        // common goto → read → goto → read flow would fail from the second
        // page onwards.
        match self.call_in_isolated_world(function, args.clone()).await {
            Err(ref e) if is_dead_context_error(e) => {
                self.inner.tab.inner.isolated_world.lock().await.context_id = None;
                self.call_in_isolated_world(function, args).await
            }
            other => other,
        }
    }

    /// One isolated-world attempt: resolve the node into the world, invoke,
    /// release. Split out so [`Element::call_on`] can retry it after
    /// invalidating a destroyed context.
    async fn call_in_isolated_world(&self, function: &str, args: Value) -> Result<Value> {
        let ctx_id = self.inner.tab.ensure_isolated_world().await?;
        let backend_node_id = self.backend_node_id_cloned().await?;
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

        let result = self
            .call_function_on(&isolated_object_id, function, args)
            .await;

        // Release on both the success and the JS-exception path — a leaked
        // handle would otherwise outlive the call until the isolated world
        // itself is replaced. Failures here are non-fatal.
        let _ = self
            .inner
            .tab
            .call(
                "Runtime.releaseObject",
                json!({ "objectId": isolated_object_id }),
            )
            .await;

        result
    }

    /// Invoke a JS function in the main world with this element bound as
    /// the first positional argument. Accepts a function declaration whose
    /// first parameter is the element handle (`function(el, ...rest){...}`)
    /// and an `args` JSON array of additional `Runtime.callFunctionOn`
    /// argument descriptors that follow the element. Returns the raw
    /// `result` RemoteObject (caller picks `value` if `returnByValue`).
    ///
    /// Reserved for JS that must observe page-defined state — page globals,
    /// or framework expandos living on the node's page-world wrapper (e.g.
    /// React's per-instance `_valueTracker`). Anything reading plain DOM
    /// state belongs on [`Element::call_on`], where the page can neither
    /// see the read nor forge its answer.
    pub(crate) async fn call_on_main(&self, function: &str, args: Value) -> Result<Value> {
        let object_id = self.remote_object_id_cloned().await?;
        let mut full_args = vec![json!({ "objectId": object_id })];
        if let Some(extra) = args.as_array() {
            full_args.extend(extra.iter().cloned());
        }
        self.call_function_on(&object_id, function, Value::Array(full_args))
            .await
    }

    /// Evaluate a JS expression in the main world with `el` bound to this
    /// element handle.
    ///
    /// Uses `Runtime.callFunctionOn` against the element's remote object,
    /// which lives in whatever world it was created in (main world if found
    /// via `document.querySelector`).
    ///
    /// For stealth-safe isolated-world evaluation, see [`Element::evaluate`].
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::JsException`] when the expression raises;
    /// [`ZendriverError::Serde`] when the result cannot be decoded into `T`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let el = tab.find().css("input").one().await?;
    /// let value: String = el.evaluate_main("el.value").await?;
    /// # let _ = value;
    /// # Ok(()) }
    /// ```
    pub async fn evaluate_main<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let function = format!("function(el){{ return ({}) }}", js.as_ref());
        let result = self.call_on_main(&function, json!([])).await?;
        let value = result.get("value").cloned().unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(ZendriverError::Serde)
    }
}

/// `true` for Chrome's "the execution context you named is gone" error,
/// which is what a destroyed isolated world looks like from the outside.
///
/// Chrome reports it as -32000 "Cannot find context with specified id";
/// `From<CallError>` maps that to [`ZendriverError::Navigation`] (see
/// `error.rs`), but the raw [`ZendriverError::Cdp`] shape reaches us too, so
/// both are matched.
fn is_dead_context_error(e: &ZendriverError) -> bool {
    let message = match e {
        ZendriverError::Navigation(m) => m.as_str(),
        ZendriverError::Cdp { message, .. } => message.as_str(),
        _ => return false,
    };
    message.contains("Cannot find context")
}

impl crate::traits::Queryable for Element {
    fn find(&self) -> crate::query::FindBuilder<'_> {
        Element::find(self)
    }
    fn find_all(&self) -> crate::query::FindAllBuilder<'_> {
        Element::find_all(self)
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use super::*;
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

    /// `expect_cmd` silently discards non-matching frames and has no built-in
    /// timeout, so a dispatch that vanished or moved would hang the suite
    /// instead of failing it. Bound every wait.
    async fn expect(mock: &mut MockConnection, method: &str) -> u64 {
        match tokio::time::timeout(Duration::from_secs(5), mock.expect_cmd(method)).await {
            Ok(id) => id,
            Err(_) => panic!("timed out waiting for {method}"),
        }
    }

    /// Serve the two-frame isolated-world handshake, yielding context 42.
    async fn serve_isolated_world(mock: &mut MockConnection) {
        let id = expect(mock, "Page.getFrameTree").await;
        mock.reply(id, json!({ "frameTree": { "frame": { "id": "F1" } } }))
            .await;
        let id = expect(mock, "Page.createIsolatedWorld").await;
        mock.reply(id, json!({ "executionContextId": 42 })).await;
    }

    /// The read/probe funnel must re-resolve the node into the isolated world
    /// and invoke the *isolated* handle — a page that shadowed `innerText` or
    /// `getBoundingClientRect` can then neither lie to the read nor see it.
    #[tokio::test]
    async fn call_on_resolves_into_the_isolated_world() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 314, "R_MAIN".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move {
                e.call_on("function(){ return this.innerText; }", json!([]))
                    .await
            }
        });

        serve_isolated_world(&mut mock).await;

        let id = expect(&mut mock, "DOM.resolveNode").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["backendNodeId"], 314);
        assert_eq!(
            sent["params"]["executionContextId"], 42,
            "element reads must be re-resolved into the isolated world",
        );
        mock.reply(id, json!({ "object": { "objectId": "R_ISO" } }))
            .await;

        let id = expect(&mut mock, "Runtime.callFunctionOn").await;
        assert_eq!(
            mock.last_sent()["params"]["objectId"],
            "R_ISO",
            "the call must target the isolated handle, not the page-world one",
        );
        mock.reply(
            id,
            json!({ "result": { "value": "hello", "type": "string" } }),
        )
        .await;

        let id = expect(&mut mock, "Runtime.releaseObject").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R_ISO");
        mock.reply(id, json!({})).await;

        let res = fut.await.unwrap().unwrap();
        assert_eq!(res["value"], "hello");
        conn.shutdown();
    }

    /// A JS exception still frees the isolated handle — otherwise a scraper
    /// hitting throwing pages leaks one `RemoteObject` per failed read.
    #[tokio::test]
    async fn call_on_releases_the_isolated_handle_after_a_js_exception() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 1, "R_MAIN".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move {
                e.call_on("function(){ throw new Error('boom'); }", json!([]))
                    .await
            }
        });

        serve_isolated_world(&mut mock).await;
        let id = expect(&mut mock, "DOM.resolveNode").await;
        mock.reply(id, json!({ "object": { "objectId": "R_ISO" } }))
            .await;

        let id = expect(&mut mock, "Runtime.callFunctionOn").await;
        mock.reply(
            id,
            json!({
                "result": { "type": "object", "subtype": "error" },
                "exceptionDetails": { "exception": { "description": "Error: boom" } },
            }),
        )
        .await;

        let id = expect(&mut mock, "Runtime.releaseObject").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R_ISO");
        mock.reply(id, json!({})).await;

        match fut.await.unwrap() {
            Err(ZendriverError::JsException(m)) => assert!(m.contains("boom")),
            other => panic!("unexpected: {other:?}"),
        }
        conn.shutdown();
    }

    /// A navigation destroys the isolated world but leaves its id cached, so
    /// the next read resolves against a dead context. It must rebuild the
    /// world and carry on — otherwise the first navigation would poison
    /// every subsequent read on the tab.
    #[tokio::test]
    async fn call_on_rebuilds_the_isolated_world_after_a_navigation_destroyed_it() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 5, "R_MAIN".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move {
                e.call_on("function(){ return this.innerText; }", json!([]))
                    .await
            }
        });

        serve_isolated_world(&mut mock).await;

        // First resolve lands on the context the navigation just destroyed.
        let id = expect(&mut mock, "DOM.resolveNode").await;
        mock.reply_err(id, -32000, "Cannot find context with specified id")
            .await;

        // Recovery: the cache is dropped, so the whole discovery handshake
        // runs again and yields a live context.
        let id = expect(&mut mock, "Page.getFrameTree").await;
        mock.reply(id, json!({ "frameTree": { "frame": { "id": "F1" } } }))
            .await;
        let id = expect(&mut mock, "Page.createIsolatedWorld").await;
        mock.reply(id, json!({ "executionContextId": 99 })).await;

        let id = expect(&mut mock, "DOM.resolveNode").await;
        assert_eq!(
            mock.last_sent()["params"]["executionContextId"],
            99,
            "the retry must use the rebuilt context, not the dead one",
        );
        mock.reply(id, json!({ "object": { "objectId": "R_ISO2" } }))
            .await;

        let id = expect(&mut mock, "Runtime.callFunctionOn").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R_ISO2");
        mock.reply(
            id,
            json!({ "result": { "value": "recovered", "type": "string" } }),
        )
        .await;

        let id = expect(&mut mock, "Runtime.releaseObject").await;
        mock.reply(id, json!({})).await;

        let res = fut.await.unwrap().unwrap();
        assert_eq!(res["value"], "recovered");
        conn.shutdown();
    }

    /// The escape hatch stays an escape hatch: JS that needs page-defined
    /// state runs against the element's own page-world handle, with no
    /// isolated-world handshake in front of it.
    #[tokio::test]
    async fn call_on_main_stays_in_the_page_world() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 1, "R_MAIN".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move {
                e.call_on_main("function(el){ return el.value; }", json!([]))
                    .await
            }
        });

        // No Page.getFrameTree / DOM.resolveNode is served here: if the main-
        // world path ever grew one, this wait would trip the timeout.
        let id = expect(&mut mock, "Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["objectId"], "R_MAIN");
        assert_eq!(
            sent["params"]["arguments"][0]["objectId"], "R_MAIN",
            "the element is bound as the first positional argument",
        );
        mock.reply(id, json!({ "result": { "value": "v", "type": "string" } }))
            .await;

        let res = fut.await.unwrap().unwrap();
        assert_eq!(res["value"], "v");
        conn.shutdown();
    }

    #[tokio::test]
    async fn from_jsret_yields_evaluation_origin() {
        let (_mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 7, "R7".to_string());
        assert!(matches!(el.inner.origin, ElementOrigin::Evaluation));
        conn.shutdown();
    }

    #[tokio::test]
    async fn remote_object_id_cloned_errors_after_clear() {
        let (_mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
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
