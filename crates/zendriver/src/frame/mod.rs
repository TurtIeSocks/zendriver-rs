//! Frame — handle to a single document frame within a [`crate::tab::Tab`].
//!
//! A [`Frame`] wraps the CDP `frameId` plus the [`zendriver_transport::SessionHandle`]
//! that should be used to dispatch commands against that frame. For the
//! main frame the session is the owning tab's session (same-process); for
//! out-of-process iframes (OOPIFs) it's a distinct child session attached
//! via `Target.attachedToTarget` (wired in T16).
//!
//! P4 Task 12 ships the bare struct + accessors + main-frame discovery via
//! [`crate::tab::Tab::main_frame`]. Evaluate / find / lifecycle / OOPIF /
//! navigation land in T13–T18.

use std::sync::{Arc, Weak};

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use zendriver_transport::SessionHandle;

use crate::error::{Result, ZendriverError};
use crate::isolated_world::IsolatedWorldCache;
use crate::tab::TabInner;

pub mod lifecycle;
pub mod oopif;

/// Cheap-to-clone handle to a single document frame.
///
/// Construct via [`crate::tab::Tab::main_frame`] (top-level frame for a
/// tab); sub-frames and OOPIFs arrive via the lifecycle / OOPIF wiring
/// in later P4 tasks. All accessor methods operate on the inner `Arc`,
/// so cloning a `Frame` is a single refcount bump.
#[derive(Clone)]
pub struct Frame {
    inner: Arc<FrameInner>,
}

// `tab` is populated at construction but consumed by later P4 tasks (frame
// lifecycle / OOPIF wiring in T15+T16). Silencing dead-code on that single
// field until those land keeps clippy clean without dropping the (already
// correct) plumbing. T13 enables `session` + `isolated_world` via the new
// `Frame::evaluate` / `evaluate_main` / `content` accessors.
pub(crate) struct FrameInner {
    /// CDP `frameId` (e.g. `"F0"`, a hex string at runtime). Stable for the
    /// lifetime of the frame.
    pub(crate) frame_id: String,
    /// Parent frame's CDP `frameId`. `None` for the main (top-level) frame;
    /// `Some` for every sub-frame and OOPIF.
    pub(crate) parent_frame_id: Option<String>,
    /// Last-known document URL for this frame. Behind an [`RwLock`] because
    /// the lifecycle subscriber task (T15) mutates it on
    /// `Page.frameNavigated` while readers concurrently call [`Frame::url`].
    pub(crate) url: RwLock<String>,
    /// `<frame name>` / `<iframe name>` attribute if present. Captured at
    /// construction time; the spec does not currently track renames after
    /// the fact since `Page.frameNavigated` does not carry the name field.
    pub(crate) name: Option<String>,
    /// CDP session used to dispatch commands against this frame. The main
    /// frame shares the owning tab's session; OOPIFs (T16) attach to a
    /// distinct child session whose handle is plumbed in at construction.
    pub(crate) session: SessionHandle,
    /// Per-frame isolated-world cache. Same shape as the tab-level cache —
    /// `Frame::evaluate` (T13) populates `executionContextId` on first call
    /// via `Page.createIsolatedWorld { frameId: self.frame_id }` and reuses
    /// it on subsequent calls. Distinct from the tab-level cache so that
    /// per-frame contexts don't collide when the tab has multiple frames.
    pub(crate) isolated_world: Mutex<IsolatedWorldCache>,
    /// Weak ref to the owning tab. Used by later P4 tasks (lifecycle
    /// updates, frame-tree walks). `Weak` so that a long-held `Frame`
    /// clone does not pin the tab alive past its public lifetime.
    #[allow(dead_code)]
    pub(crate) tab: Weak<TabInner>,
}

impl Frame {
    /// Construct a `Frame` from its CDP identity + the session that should
    /// dispatch commands against it.
    ///
    /// Called by [`crate::tab::Tab::main_frame`] (main-frame path, shares
    /// the tab's session) and — in later P4 tasks — by the lifecycle
    /// subscriber (sub-frame attach) and the OOPIF attach observer
    /// (distinct child session).
    pub(crate) fn new(
        frame_id: String,
        parent_frame_id: Option<String>,
        url: String,
        name: Option<String>,
        session: SessionHandle,
        tab: Weak<TabInner>,
    ) -> Self {
        Self {
            inner: Arc::new(FrameInner {
                frame_id,
                parent_frame_id,
                url: RwLock::new(url),
                name,
                session,
                isolated_world: Mutex::new(IsolatedWorldCache::default()),
                tab,
            }),
        }
    }

    /// The frame's CDP `frameId`. Stable for the lifetime of the frame.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.inner.frame_id
    }

    /// The frame's parent CDP `frameId`. `None` iff this is the main
    /// (top-level) frame for the owning tab.
    #[must_use]
    pub fn parent_id(&self) -> Option<&str> {
        self.inner.parent_frame_id.as_deref()
    }

    /// The frame's `name` attribute (`<iframe name="...">`). `None` for
    /// frames without an explicit name (including the main frame in most
    /// cases).
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.inner.name.as_deref()
    }

    /// `true` iff this is the main (top-level) frame for its owning tab —
    /// equivalent to `parent_id().is_none()`.
    #[must_use]
    pub fn is_main(&self) -> bool {
        self.inner.parent_frame_id.is_none()
    }

    /// The frame's current document URL. Snapshot under an `RwLock`; cheap
    /// to clone the resulting `String`. The lifecycle subscriber (T15)
    /// keeps this fresh on `Page.frameNavigated` events; until T15 lands
    /// the value reflects the construction-time URL only.
    pub async fn url(&self) -> String {
        self.inner.url.read().await.clone()
    }

    /// Evaluate a JavaScript expression in an isolated world bound to **this
    /// frame** (sandboxed; no page globals visible). Result deserialized into
    /// `T`. Throws [`ZendriverError::JsException`] when the expression raises.
    ///
    /// Mirrors [`crate::tab::Tab::evaluate`] but at frame granularity: each
    /// `Frame` carries its own [`IsolatedWorldCache`], so contextIds are
    /// per-frame and don't collide across sibling frames or between a frame
    /// and its owning tab's main-frame world.
    ///
    /// First call dispatches `Page.createIsolatedWorld { frameId: self.id,
    /// worldName: "zendriver-eval" }` on the frame's session and caches the
    /// returned `executionContextId`. Subsequent calls reuse the cached id
    /// and go straight to `Runtime.evaluate`.
    ///
    /// Unlike `Tab::evaluate`, this does *not* currently retry on
    /// `-32000 Cannot find context with specified id` — frame-level context
    /// invalidation is handled by the lifecycle subscriber (T15) clearing
    /// the cache on `Page.frameNavigated`. Retry-on-stale parity with the
    /// Tab path can be added once lifecycle is in.
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let ctx_id = self.ensure_isolated_world().await?;
        let res = self
            .inner
            .session
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": js.as_ref(),
                    "contextId": ctx_id,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        Self::extract_value(&res)
    }

    /// Evaluate a JavaScript expression in this frame's **main world** (page
    /// globals visible). Escape hatch when isolated-world semantics don't
    /// fit. Result deserialized into `T`. Throws
    /// [`ZendriverError::JsException`] when the expression raises.
    ///
    /// Dispatches `Runtime.evaluate` *without* a `contextId` on the frame's
    /// session. For the main frame and same-origin sub-frames this targets
    /// the parent tab's session (which routes to the frame's main world);
    /// for OOPIFs (T16) the frame's session is a distinct child session,
    /// so `Runtime.evaluate` lands in the OOPIF's own main world.
    pub async fn evaluate_main<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let res = self
            .inner
            .session
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": js.as_ref(),
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        Self::extract_value(&res)
    }

    /// The frame's current document HTML (serialized
    /// `document.documentElement.outerHTML`).
    ///
    /// Backed by [`Frame::evaluate_main`] — runs in the frame's main world
    /// so the serializer sees the actual DOM nodes, not an isolated-world
    /// proxy view. For OOPIFs this returns the OOPIF's *own* HTML, not the
    /// parent page's.
    pub async fn content(&self) -> Result<String> {
        self.evaluate_main("document.documentElement.outerHTML")
            .await
    }

    /// Ensure an isolated-world execution context exists for *this frame*,
    /// returning its `executionContextId`. Cached after first call.
    ///
    /// Unlike [`crate::tab::Tab::ensure_isolated_world`], we skip the
    /// `Page.getFrameTree` round-trip — the frame already knows its own
    /// CDP `frameId`, so we go straight to `Page.createIsolatedWorld`.
    async fn ensure_isolated_world(&self) -> Result<i64> {
        let mut cache = self.inner.isolated_world.lock().await;
        if let Some(ctx) = cache.context_id {
            return Ok(ctx);
        }
        let res = self
            .inner
            .session
            .call(
                "Page.createIsolatedWorld",
                json!({
                    "frameId": self.inner.frame_id,
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
        cache.main_frame_id = Some(self.inner.frame_id.clone());
        cache.context_id = Some(ctx_id);
        Ok(ctx_id)
    }

    /// Shared post-processing for `Runtime.evaluate` responses — checks
    /// `exceptionDetails` first (raising [`ZendriverError::JsException`])
    /// then deserializes `result.value` into `T`. Mirrors the inline tail
    /// of [`crate::tab::Tab::evaluate`] / `evaluate_main`.
    #[allow(clippy::result_large_err)] // ZendriverError variance is the project-wide return type
    fn extract_value<T: DeserializeOwned>(res: &Value) -> Result<T> {
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
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    /// Build a synthetic `Frame` whose session sits on the supplied mock
    /// connection. Mirrors `Tab::new_for_test` ergonomics — no parent tab,
    /// no parent frame, fixed frameId / url, ready for evaluate dispatch.
    fn frame_on(session: SessionHandle, frame_id: &str) -> Frame {
        Frame::new(
            frame_id.to_string(),
            None,
            String::new(),
            None,
            session,
            Weak::new(),
        )
    }

    /// First `Frame::evaluate` call dispatches `Page.createIsolatedWorld`
    /// (using the frame's own `frameId`, skipping the `Page.getFrameTree`
    /// round-trip that `Tab::evaluate` needs) and caches the returned
    /// `executionContextId`. Second call must reuse the cached id — next
    /// outbound frame is `Runtime.evaluate`, with NO intervening
    /// `Page.createIsolatedWorld`.
    #[tokio::test]
    async fn evaluate_caches_context_id_across_calls() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let frame = frame_on(sess, "FRAME_A");

        // --- Call 1: full handshake + eval. ---
        let fut1 = tokio::spawn({
            let f = frame.clone();
            async move { f.evaluate::<i32>("1").await }
        });
        let id_world = mock.expect_cmd("Page.createIsolatedWorld").await;
        assert_eq!(mock.last_sent()["params"]["frameId"], "FRAME_A");
        assert_eq!(mock.last_sent()["params"]["worldName"], "zendriver-eval");
        mock.reply(id_world, json!({ "executionContextId": 7 }))
            .await;
        let id_eval1 = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["contextId"], 7);
        assert_eq!(mock.last_sent()["params"]["expression"], "1");
        mock.reply(
            id_eval1,
            json!({ "result": { "value": 1, "type": "number" } }),
        )
        .await;
        assert_eq!(fut1.await.unwrap().unwrap(), 1);

        // --- Call 2: cached → straight to Runtime.evaluate. No second
        //     Page.createIsolatedWorld is expected; the next outbound
        //     frame is Runtime.evaluate itself.
        let fut2 = tokio::spawn({
            let f = frame.clone();
            async move { f.evaluate::<i32>("2").await }
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

    /// `Frame::evaluate_main` dispatches `Runtime.evaluate` directly on the
    /// frame's session with NO `contextId` parameter — proving the
    /// main-world path skips the isolated-world handshake entirely (no
    /// `Page.createIsolatedWorld` in the trace).
    #[tokio::test]
    async fn evaluate_main_dispatches_runtime_evaluate_without_context_id() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let frame = frame_on(sess, "FRAME_B");

        let fut = tokio::spawn({
            let f = frame.clone();
            async move { f.evaluate_main::<i32>("1+1").await }
        });

        // Next outbound frame is Runtime.evaluate itself — no isolated-world
        // bootstrap. `last_sent()` after `expect_cmd` confirms there is no
        // `contextId` field on the params.
        let id = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["expression"], "1+1");
        assert!(mock.last_sent()["params"].get("contextId").is_none());
        mock.reply(id, json!({ "result": { "value": 2, "type": "number" } }))
            .await;

        assert_eq!(fut.await.unwrap().unwrap(), 2);
        conn.shutdown();
    }
}
