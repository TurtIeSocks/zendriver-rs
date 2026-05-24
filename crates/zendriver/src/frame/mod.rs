//! Handle to a single document frame within a [`crate::Tab`].
//!
//! A [`Frame`] wraps the CDP `frameId` plus the [`zendriver_transport::SessionHandle`]
//! that should be used to dispatch commands against that frame. For the
//! main frame the session is the owning tab's session (same-process); for
//! out-of-process iframes (OOPIFs) it's a distinct child session attached
//! via `Target.attachedToTarget`.
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! # let browser = zendriver::Browser::builder().launch().await?;
//! # let tab = browser.main_tab();
//! tab.goto("https://example.com").await?;
//! let main = tab.main_frame().await?;
//! let title: String = main.evaluate_main("document.title").await?;
//! # let _ = title;
//! # Ok(()) }
//! ```

use std::sync::{Arc, Weak};
use std::time::Duration;

use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use tokio::time::timeout;
use zendriver_transport::SessionHandle;

use crate::error::{Result, ZendriverError};
use crate::isolated_world::IsolatedWorldCache;
use crate::tab::{Tab, TabInner};

pub mod lifecycle;
pub mod oopif;

/// Default wait window for [`Frame::wait_for_load`]. Mirrors the constant
/// in [`crate::tab::Tab::wait_for_load`] so single-frame and tab-level
/// navigation share their stall budget.
const DEFAULT_LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Cheap-to-clone handle to a single document frame.
///
/// Construct via [`crate::tab::Tab::main_frame`] (top-level frame for a
/// tab); sub-frames and OOPIFs arrive via the lifecycle / OOPIF wiring
/// in later P4 tasks. All accessor methods operate on the inner `Arc`,
/// so cloning a `Frame` is a single refcount bump.
#[derive(Clone, Debug)]
pub struct Frame {
    inner: Arc<FrameInner>,
}

// `tab` is populated at construction but consumed by later P4 tasks (frame
// lifecycle / OOPIF wiring in T15+T16). Silencing dead-code on that single
// field until those land keeps clippy clean without dropping the (already
// correct) plumbing. T13 enables `session` + `isolated_world` via the new
// `Frame::evaluate` / `evaluate_main` / `content` accessors.
#[derive(Debug)]
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
    /// Weak ref to the owning tab. Upgraded by
    /// [`Frame::tab_for_synthesize`] when the query layer needs to wrap
    /// a `RemoteRef` in an `Element` (the `Element` stores an owned
    /// `Tab` clone for the lifetime of the handle). `Weak` so that a
    /// long-held `Frame` clone does not pin the tab alive past its
    /// public lifetime.
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

    /// The frame's CDP `frameId`.
    ///
    /// Stable for the lifetime of the frame.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let main = tab.main_frame().await?;
    /// let _: &str = main.id();
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn id(&self) -> &str {
        &self.inner.frame_id
    }

    /// CDP session used to dispatch commands against this frame. Crate-
    /// internal — exposed so `query::selectors` can route
    /// `Runtime.evaluate` / `DOM.describeNode` etc. through the right
    /// session for [`crate::query::FindBuilder::new_for_frame`] queries.
    ///
    /// For the main frame and same-origin sub-frames this is identical
    /// to the parent tab's session; for out-of-process iframes (T16
    /// onward) it is a distinct child session attached via
    /// `Target.attachedToTarget`.
    pub(crate) fn session(&self) -> &SessionHandle {
        &self.inner.session
    }

    /// Upgrade the frame's `Weak<TabInner>` into an owned `Tab` for
    /// [`crate::element::Element::synthesize_query`] callers. Returns
    /// `None` if the owning Tab was dropped — in practice this should
    /// not happen because Frames are constructed by Tabs that hold a
    /// strong reference, but the contract is exposed honestly here so
    /// the caller (the `QueryScope::synthesize_tab` accessor in
    /// `query::selectors`) can decide between erroring and panicking.
    pub(crate) fn tab_for_synthesize(&self) -> Option<Tab> {
        self.inner.tab.upgrade().map(|inner| Tab { inner })
    }

    /// The frame's parent CDP `frameId`.
    ///
    /// `None` iff this is the main (top-level) frame for the owning tab.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// for f in tab.frames().await? {
    ///     if let Some(parent) = f.parent_id() {
    ///         println!("frame {} is a child of {parent}", f.id());
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn parent_id(&self) -> Option<&str> {
        self.inner.parent_frame_id.as_deref()
    }

    /// The frame's `name` attribute (`<iframe name="...">`).
    ///
    /// `None` for frames without an explicit name (including the main frame
    /// in most cases).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// for f in tab.frames().await? {
    ///     println!("name: {:?}", f.name());
    /// }
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.inner.name.as_deref()
    }

    /// `true` iff this is the main (top-level) frame for its owning tab.
    ///
    /// Equivalent to `parent_id().is_none()`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let main = tab.main_frame().await?;
    /// assert!(main.is_main());
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn is_main(&self) -> bool {
        self.inner.parent_frame_id.is_none()
    }

    /// The frame's current document URL.
    ///
    /// Snapshot under an `RwLock`; kept fresh by the frame lifecycle
    /// subscriber as `Page.frameNavigated` events arrive.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let main = tab.main_frame().await?;
    /// println!("{}", main.url().await);
    /// # Ok(()) }
    /// ```
    pub async fn url(&self) -> String {
        self.inner.url.read().await.clone()
    }

    /// Evaluate JS in an isolated world bound to **this frame**.
    ///
    /// Sandboxed — no page globals visible. Result deserialized into `T`.
    /// Mirrors [`crate::Tab::evaluate`] but at frame granularity: each
    /// `Frame` carries its own internal isolated-world cache, so contextIds
    /// are per-frame and don't collide across sibling frames.
    ///
    /// First call dispatches `Page.createIsolatedWorld { frameId, worldName:
    /// "zendriver-eval" }` on the frame's session and caches the returned
    /// `executionContextId`. Subsequent calls reuse the cached id.
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
    /// let frame = tab.main_frame().await?;
    /// let url: String = frame.evaluate("location.href").await?;
    /// # let _ = url;
    /// # Ok(()) }
    /// ```
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

    /// Evaluate JS in this frame's **main world** (page globals visible).
    ///
    /// Escape hatch when isolated-world semantics don't fit. Dispatches
    /// `Runtime.evaluate` *without* a `contextId` on the frame's session.
    /// For OOPIFs the frame's session is a distinct child session, so this
    /// lands in the OOPIF's own main world.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::JsException`] when the expression raises.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let frame = tab.main_frame().await?;
    /// let title: String = frame.evaluate_main("document.title").await?;
    /// # let _ = title;
    /// # Ok(()) }
    /// ```
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

    /// The frame's current document HTML.
    ///
    /// Returns the serialized `document.documentElement.outerHTML`. Backed
    /// by [`Frame::evaluate_main`]. For OOPIFs this returns the OOPIF's
    /// *own* HTML, not the parent page's.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let frame = tab.main_frame().await?;
    /// let html = frame.content().await?;
    /// assert!(html.contains("<html"));
    /// # Ok(()) }
    /// ```
    pub async fn content(&self) -> Result<String> {
        self.evaluate_main("document.documentElement.outerHTML")
            .await
    }

    /// Navigate **this frame** to `url`.
    ///
    /// Only supported for the main frame — sub-frame navigation must be
    /// driven by mutating the parent document's `<iframe src>`. Does NOT
    /// wait for the load to complete; pair with [`Frame::wait_for_load`].
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] when called on a sub-frame
    /// or when Chrome reports `errorText` on the response.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let main = tab.main_frame().await?;
    /// main.goto("https://example.org").await?;
    /// main.wait_for_load().await?;
    /// # Ok(()) }
    /// ```
    pub async fn goto(&self, url: impl AsRef<str>) -> Result<()> {
        if !self.is_main() {
            return Err(ZendriverError::Navigation(
                "sub-frame goto not supported; set iframe.src via parent evaluate_main".into(),
            ));
        }
        // Enable Page domain so we get FrameStoppedLoading events.
        self.inner.session.call("Page.enable", json!({})).await?;
        let url_s = url.as_ref().to_string();
        let res = self
            .inner
            .session
            .call("Page.navigate", json!({ "url": url_s }))
            .await?;
        if let Some(err) = res.get("errorText").and_then(|v| v.as_str()) {
            if !err.is_empty() {
                return Err(ZendriverError::Navigation(err.to_string()));
            }
        }
        Ok(())
    }

    /// Wait until **this frame's** `Page.frameStoppedLoading` fires.
    ///
    /// Subscribes on the frame's session and filters events by `frameId` so
    /// load notifications for sibling frames in the same tab don't satisfy
    /// the wait. Default timeout matches [`crate::Tab::wait_for_load`] (30s).
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Timeout`] on expiry.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let main = tab.main_frame().await?;
    /// main.goto("https://example.com").await?;
    /// main.wait_for_load().await?;
    /// # Ok(()) }
    /// ```
    pub async fn wait_for_load(&self) -> Result<()> {
        let mut stream = self
            .inner
            .session
            .subscribe::<Value>("Page.frameStoppedLoading");
        let deadline = tokio::time::Instant::now() + DEFAULT_LOAD_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(ZendriverError::Timeout(DEFAULT_LOAD_TIMEOUT));
            }
            let evt = timeout(remaining, stream.next())
                .await
                .map_err(|_| ZendriverError::Timeout(DEFAULT_LOAD_TIMEOUT))?
                .ok_or_else(|| ZendriverError::Navigation("page event stream closed".into()))?;
            if evt
                .get("frameId")
                .and_then(|v| v.as_str())
                .is_some_and(|fid| fid == self.inner.frame_id)
            {
                return Ok(());
            }
        }
    }

    /// Begin a chainable element query against this frame's document.
    ///
    /// Mirrors [`crate::Tab::find`]. Queries dispatch on **this frame's**
    /// CDP session; the query root is the frame's own `document` (matches
    /// in sibling frames or the parent document are not considered).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let frame = tab.main_frame().await?;
    /// let h1 = frame.find().css("h1").one().await?;
    /// # let _ = h1;
    /// # Ok(()) }
    /// ```
    pub fn find(&self) -> crate::query::FindBuilder<'_> {
        crate::query::FindBuilder::new_for_frame(self)
    }

    /// Begin a chainable element query against this frame's document that
    /// returns ALL matches.
    ///
    /// Mirror of [`Frame::find`] (no `.nth`); terminate with `.many()` or
    /// `.many_or_empty()`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let frame = tab.main_frame().await?;
    /// let links = frame.find_all().css("a").many_or_empty().await?;
    /// # let _ = links;
    /// # Ok(()) }
    /// ```
    pub fn find_all(&self) -> crate::query::FindAllBuilder<'_> {
        crate::query::FindAllBuilder::new_for_frame(self)
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

impl crate::traits::Queryable for Frame {
    fn find(&self) -> crate::query::FindBuilder<'_> {
        Frame::find(self)
    }
    fn find_all(&self) -> crate::query::FindAllBuilder<'_> {
        Frame::find_all(self)
    }
}

#[async_trait::async_trait]
impl crate::traits::Evaluable for Frame {
    async fn evaluate<T>(&self, js: &str) -> Result<T>
    where
        T: DeserializeOwned + Send + 'static,
    {
        Frame::evaluate(self, js).await
    }
    async fn evaluate_main<T>(&self, js: &str) -> Result<T>
    where
        T: DeserializeOwned + Send + 'static,
    {
        Frame::evaluate_main(self, js).await
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
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

    /// `Frame::find().css(...).one()` dispatches `Runtime.evaluate` on the
    /// frame's session (same MockConnection in tests — so the assertion is
    /// "the call was made", not "it landed on a specific session id"). The
    /// resolved `Element` is wrapped via `synthesize_query`, which upgrades
    /// the frame's `Weak<TabInner>` — so the test frame is constructed with
    /// a live Tab backing rather than the `Weak::new()` shortcut used by
    /// `frame_on` in the evaluate-only tests above.
    ///
    /// We assert the full CDP trace mirrors the Tab/Element resolve_many
    /// path (Runtime.evaluate → Runtime.getProperties → DOM.describeNode)
    /// because Frame scope routes through the same `resolve_css_many` →
    /// `extract_array_refs` pipeline as Tab/Element scope, only swapping
    /// the underlying `SessionHandle`.
    #[tokio::test]
    async fn find_dispatches_runtime_evaluate_on_frames_session() {
        use crate::tab::Tab;

        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess.clone());
        // Construct the Frame with a live `Weak<TabInner>` so that
        // `synthesize_query` → `tab_for_synthesize` succeeds. We can't
        // use `Tab::main_frame()` here because that would consume the
        // mock connection's first inbound `Page.getFrameTree` slot.
        let frame = Frame::new(
            "FRAME_FIND".to_string(),
            None,
            String::new(),
            None,
            sess,
            std::sync::Arc::downgrade(&tab.inner),
        );

        let fut = tokio::spawn({
            let f = frame.clone();
            async move { f.find().css("button").one().await }
        });

        // First dispatch: Runtime.evaluate with the document.querySelectorAll
        // expression (resolve_css_many goes through the array path).
        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent()["params"]["expression"]
            .as_str()
            .expect("expression should be a string")
            .to_string();
        assert!(
            sent.contains("document.querySelectorAll") && sent.contains("button"),
            "frame.find().css('button').one() must dispatch document.querySelectorAll, got: {sent}"
        );
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArrF", "type": "object", "subtype": "array" } }),
        )
        .await;

        // Enumerate the array — one match at index 0.
        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RArrF");
        mock.reply(
            id_p,
            json!({
                "result": [
                    {
                        "name": "0",
                        "value": { "objectId": "RFN0", "type": "object", "subtype": "node" }
                    },
                    {
                        "name": "length",
                        "value": { "value": 1, "type": "number" }
                    }
                ]
            }),
        )
        .await;

        // describeNode resolves the backendNodeId for the picked element.
        let id_d = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RFN0");
        mock.reply(id_d, json!({ "node": { "backendNodeId": 77 } }))
            .await;

        let el = fut.await.expect("task should not panic").expect("one() ok");
        assert_eq!(
            el.inner.remote_object_id.lock().await.as_deref(),
            Some("RFN0")
        );
        assert_eq!(*el.inner.backend_node_id.lock().await, Some(77));
        conn.shutdown();
    }

    /// Main-frame `goto` mirrors `Tab::goto`: `Page.enable` first (so a
    /// subsequent `wait_for_load` sees `Page.frameStoppedLoading`), then
    /// `Page.navigate { url }` on the frame's session.
    #[tokio::test]
    async fn main_frame_goto_dispatches_page_navigate() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let frame = frame_on(sess, "FRAME_MAIN");

        let fut = tokio::spawn({
            let f = frame.clone();
            async move { f.goto("https://example.com").await }
        });

        let id_enable = mock.expect_cmd("Page.enable").await;
        mock.reply(id_enable, json!({})).await;

        let id_nav = mock.expect_cmd("Page.navigate").await;
        assert_eq!(mock.last_sent()["params"]["url"], "https://example.com");
        mock.reply(id_nav, json!({ "frameId": "FRAME_MAIN" })).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    /// Sub-frame `goto` is rejected up-front (no CDP traffic) with a
    /// `Navigation` error pointing callers at `evaluate_main` on the
    /// parent.
    #[tokio::test]
    async fn sub_frame_goto_returns_navigation_error() {
        let (_mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        // parent_frame_id = Some(...) → is_main() == false.
        let frame = Frame::new(
            "FRAME_CHILD".to_string(),
            Some("FRAME_PARENT".to_string()),
            String::new(),
            None,
            sess,
            Weak::new(),
        );

        let res = frame.goto("https://example.com").await;
        match res {
            Err(ZendriverError::Navigation(m)) => {
                assert!(
                    m.contains("sub-frame goto not supported"),
                    "unexpected message: {m}"
                );
            }
            other => panic!("expected Navigation error, got: {other:?}"),
        }
        conn.shutdown();
    }
}
