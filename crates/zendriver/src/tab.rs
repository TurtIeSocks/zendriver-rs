//! Per-page handle to a single CDP target session.
//!
//! [`Tab`] is the primary interaction surface in zendriver — most workflows
//! are some sequence of `goto`, `find().css(...).one()`, `evaluate`,
//! `screenshot`, and `wait_for_idle`. Each [`Tab`] owns its own
//! [`InputController`] (cursor + held-modifier state), its own per-tab
//! frame registry, and its own in-flight network tracker, so multiple tabs
//! in the same [`crate::Browser`] don't interfere with one another.
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! let browser = zendriver::Browser::builder().launch().await?;
//! let tab = browser.main_tab();
//! tab.goto("https://example.com").await?;
//! tab.wait_for_load().await?;
//! let title: String = tab.evaluate_main("document.title").await?;
//! assert_eq!(title, "Example Domain");
//! # Ok(()) }
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::time::timeout;
use tracing::trace;
use zendriver_transport::SessionHandle;

use crate::error::{Result, ZendriverError};
use crate::frame::Frame;
use crate::input::InputController;
use crate::isolated_world::IsolatedWorldCache;
use crate::screenshot::ScreenshotBuilder;

const DEFAULT_LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Handle to a single CDP target session — one open page in Chrome.
///
/// `Tab` is `Clone` (cheap — wraps an `Arc`) and `Send + Sync`, so the same
/// handle can be passed across `tokio::spawn` boundaries freely. Dropping
/// the last clone tears down the per-Tab background tasks (network tracker,
/// frame lifecycle subscriber) but does NOT close the page in Chrome — call
/// [`Tab::close`] for an explicit teardown.
///
/// Obtain a `Tab` from [`crate::Browser::main_tab`], [`crate::Browser::new_tab`],
/// or [`crate::Browser::tabs`].
#[derive(Clone, Debug)]
pub struct Tab {
    pub(crate) inner: Arc<TabInner>,
}

#[derive(Debug)]
pub(crate) struct TabInner {
    pub(crate) session: SessionHandle,
    pub(crate) isolated_world: tokio::sync::Mutex<IsolatedWorldCache>,
    /// Weak ref to the owning `BrowserInner`. Used by [`Tab::cookies`] to
    /// hand back a [`crate::CookieJar`] bound to the browser's root
    /// connection (Chrome's cookie store is browser-scoped, so per-tab jars
    /// would all dispatch the same way). Reserved for future P4 tasks
    /// (tabs registry walks, storage). `Weak` breaks the Browser→Tab→Browser
    /// cycle.
    pub(crate) browser: std::sync::Weak<crate::browser::BrowserInner>,
    /// Per-Tab input controller. Each tab owns its own cursor + held-modifier
    /// state — distinct tabs in the same Browser have independent pointers.
    /// `Element` actions clone this `Arc` to drive `mouse::*` / `keyboard::*`
    /// dispatch helpers; the shared mutex inside `InputController` serializes
    /// per-tab writes without crossing tab boundaries.
    pub(crate) input: Arc<InputController>,
    /// CDP `targetId` for the page target this tab wraps. Cached at Tab
    /// construction time (from `Target.attachedToTarget`'s `target_info`)
    /// so multi-tab orchestration (`Browser::new_tab` correlation,
    /// `Tab::activate`, `Tab::close`'s `Target.closeTarget` upgrade) can
    /// dispatch by `targetId` without re-querying `Target.getTargetInfo`
    /// per call.
    pub(crate) target_id: String,
    /// Per-Tab in-flight network request tracker. Constructed in
    /// [`Tab::new`] alongside a background task (spawned via
    /// [`crate::network_idle::InFlightTracker::run`]) that subscribes to
    /// `Network.*` events and maintains the set. Consulted by
    /// [`Tab::wait_for_idle`] / [`Tab::wait_for_idle_with`] for Playwright
    /// `networkidle` semantics.
    pub(crate) network_tracker: Arc<crate::network_idle::InFlightTracker>,
    /// Cancellation token for the background tracker task. Fires on
    /// [`Drop`] so the spawned task exits cleanly when the last clone of
    /// this Tab goes away. Cloned by the spawned task at construction
    /// time; cancelling here propagates to the task's `tokio::select!`
    /// loop within one event tick.
    pub(crate) network_cancel: tokio_util::sync::CancellationToken,
    /// Lazily-discovered main [`Frame`] for this tab. First call to
    /// [`Tab::main_frame`] sends `Page.getFrameTree`, extracts the top-level
    /// frame id/url/name, constructs a `Frame` (sharing this tab's
    /// session — the main frame is always same-process), and stores it
    /// here. Subsequent calls return the cached `Frame` clone without
    /// another round-trip.
    pub(crate) main_frame: tokio::sync::OnceCell<Frame>,
    /// Per-Tab download coordinator. Lazily initialized on the first
    /// [`Tab::expect_download`] call (gated `expect`) — the constructor
    /// allocates a tempdir, dispatches `Browser.setDownloadBehavior` once,
    /// and spawns a long-running `Page.downloadProgress` subscriber. Held
    /// behind a [`tokio::sync::OnceCell`] so the wiring happens exactly
    /// once per Tab; subsequent `expect_download` calls reuse the same
    /// coordinator (and therefore the same tempdir + subscriber).
    ///
    /// `Arc` because both the [`Tab`] (via this cell) and the spawned
    /// progress subscriber task hold references to the same coordinator
    /// state for the Tab's entire lifetime.
    #[cfg(feature = "expect")]
    pub(crate) download_setup:
        tokio::sync::OnceCell<Arc<crate::expect::download::DownloadCoordinator>>,
    /// Per-Tab frames registry keyed by CDP `frameId`. Populated by the
    /// background subscriber spawned in [`Tab::new`] via
    /// [`crate::frame::lifecycle::run`] which mutates the map in response
    /// to `Page.frameAttached` / `Page.frameDetached` /
    /// `Page.frameNavigated` events on this tab's session. Read by
    /// [`Tab::frames`] / [`Tab::frame_by_url`] / [`Tab::frame_by_name`].
    ///
    /// Same-origin sub-frames go in this map directly; out-of-process
    /// iframes (OOPIFs) take the `Target.attachedToTarget` path wired in
    /// T16 and land here only after that observer registers them.
    pub(crate) frames: Arc<tokio::sync::RwLock<HashMap<String, Frame>>>,
    /// Cancellation token for the frame lifecycle subscriber task. Mirror
    /// of [`TabInner::network_cancel`]: fires on [`Drop`] so the spawned
    /// task exits cleanly when the last clone of this Tab goes away. The
    /// task selects on this token alongside the three `Page.frame*`
    /// subscriber streams so cancellation unblocks the select even if no
    /// events are arriving.
    pub(crate) frame_lifecycle_cancel: tokio_util::sync::CancellationToken,
}

impl Drop for TabInner {
    fn drop(&mut self) {
        // Signal the spawned `InFlightTracker::run` task to exit. The task
        // selects on this token alongside the four `Network.*` subscriber
        // streams; cancellation unblocks the select even if no events are
        // arriving. Without this the task would leak per Tab on shutdown.
        self.network_cancel.cancel();
        // Signal the spawned `frame::lifecycle::run` task to exit. Same
        // posture as `network_cancel` above — the task selects on this
        // token alongside the three `Page.frame*` subscriber streams.
        self.frame_lifecycle_cancel.cancel();
    }
}

impl Tab {
    pub(crate) fn new(
        session: SessionHandle,
        browser: std::sync::Weak<crate::browser::BrowserInner>,
        input: Arc<InputController>,
        target_id: String,
    ) -> Self {
        // Build the per-Tab network tracker + spawn its background subscriber
        // task. The task calls `Network.enable` once, then maintains the
        // in-flight set in response to `Network.requestWillBeSent` /
        // `responseReceived` / `loadingFailed` / `loadingFinished` events
        // arriving on this tab's session. `wait_for_idle` reads from the
        // same `network_tracker` Arc.
        let network_tracker = crate::network_idle::InFlightTracker::new();
        let network_cancel = tokio_util::sync::CancellationToken::new();
        tokio::spawn({
            let tracker = network_tracker.clone();
            let session_for_task = session.clone();
            let cancel_for_task = network_cancel.clone();
            async move {
                tracker.run(session_for_task, cancel_for_task).await;
            }
        });

        // Build the per-Tab frames registry + spawn the lifecycle
        // subscriber. The task calls `Page.enable` once, then mutates the
        // registry in response to `Page.frameAttached` (insert),
        // `Page.frameNavigated` (update url / insert if unseen) and
        // `Page.frameDetached` (remove). The `Arc<RwLock<_>>` lives on
        // `TabInner::frames` so `Tab::frames` / `frame_by_url` /
        // `frame_by_name` can take snapshots without going through the
        // tracker task. The `Weak<TabInner>` is wired in via
        // `Arc::new_cyclic` below so every `Frame` constructed by the
        // subscriber can upgrade back to the owning Tab.
        let frames: Arc<tokio::sync::RwLock<HashMap<String, Frame>>> =
            Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        let frame_lifecycle_cancel = tokio_util::sync::CancellationToken::new();

        let inner = Arc::new_cyclic(|weak: &std::sync::Weak<TabInner>| {
            tokio::spawn({
                let session_for_task = session.clone();
                let frames_for_task = frames.clone();
                let weak_for_task = weak.clone();
                let cancel_for_task = frame_lifecycle_cancel.clone();
                async move {
                    crate::frame::lifecycle::run(
                        session_for_task,
                        frames_for_task,
                        weak_for_task,
                        cancel_for_task,
                    )
                    .await;
                }
            });
            TabInner {
                session,
                isolated_world: tokio::sync::Mutex::new(IsolatedWorldCache::default()),
                browser,
                input,
                target_id,
                network_tracker,
                network_cancel,
                main_frame: tokio::sync::OnceCell::new(),
                #[cfg(feature = "expect")]
                download_setup: tokio::sync::OnceCell::new(),
                frames,
                frame_lifecycle_cancel,
            }
        });

        Self { inner }
    }

    /// Test-only constructor: builds a `Tab` with a deterministic seeded
    /// [`InputController`] (native input profile, seed `42`) and an empty
    /// `Weak` browser ref. Replaces the P3 `Tab::new(sess, Weak::new())`
    /// pattern that paired with `Tab::input() -> Option<_>`; now that
    /// `Tab::input()` returns `&Arc<InputController>` unconditionally, tests
    /// must seed a controller at construction time.
    ///
    /// The synthetic `target_id` is derived from the session_id — tests that
    /// need a specific `targetId` should use [`Tab::new_for_test_with_target`].
    #[cfg(test)]
    pub(crate) fn new_for_test(session: SessionHandle) -> Self {
        let target_id = format!("test-target-{}", session.session_id());
        Self::new(
            session,
            std::sync::Weak::new(),
            crate::input::InputController::new_with_seed(
                zendriver_stealth::InputProfile::native(),
                42,
            ),
            target_id,
        )
    }

    /// CDP `targetId` for the page target this tab wraps.
    ///
    /// Stable for the lifetime of the underlying target — used by
    /// [`crate::Browser::new_tab`] to correlate a `Target.createTarget`
    /// response with the [`Tab`] that the internal `TabRegistrar`
    /// subsequently registers.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// let browser = zendriver::Browser::builder().launch().await?;
    /// let tab = browser.main_tab();
    /// let id = tab.target_id();
    /// assert!(!id.is_empty());
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn target_id(&self) -> &str {
        &self.inner.target_id
    }

    /// Per-Tab [`InputController`].
    ///
    /// Each tab carries its own cursor + modifier state; [`crate::Element`]
    /// actions ([`crate::Element::click`], [`crate::Element::hover`],
    /// [`crate::Element::type_text`], [`crate::Element::press`]) call this
    /// to drive internal mouse / keyboard dispatch helpers. Always returns a
    /// valid handle.
    #[must_use]
    pub fn input(&self) -> &Arc<InputController> {
        &self.inner.input
    }

    /// Raw [`SessionHandle`] escape hatch.
    ///
    /// For advanced users who need to send CDP commands the high-level API
    /// doesn't expose. Returns the underlying transport session bound to
    /// this tab's `sessionId`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let session = tab.session();
    /// // Send a CDP command the high-level API doesn't wrap.
    /// session.call("Page.bringToFront", serde_json::json!({})).await?;
    /// # Ok(()) }
    /// ```
    pub fn session(&self) -> &SessionHandle {
        &self.inner.session
    }

    /// The top-level [`Frame`] for this tab.
    ///
    /// First call dispatches `Page.getFrameTree` on the tab's session,
    /// extracts the top-level frame's `id` / `url` / `name`, and constructs
    /// a [`Frame`] whose session is this tab's session (the main frame is
    /// always same-process). The result is cached internally so subsequent
    /// calls return the same `Frame` clone without a round-trip.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] if Chrome's response is
    /// missing the top-level frame id.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let main = tab.main_frame().await?;
    /// assert!(main.url().await.contains("example.com"));
    /// # Ok(()) }
    /// ```
    pub async fn main_frame(&self) -> Result<Frame> {
        let frame = self
            .inner
            .main_frame
            .get_or_try_init(|| async {
                let tree = self.call("Page.getFrameTree", json!({})).await?;
                let frame_node = &tree["frameTree"]["frame"];
                let frame_id = frame_node["id"]
                    .as_str()
                    .ok_or_else(|| {
                        ZendriverError::Navigation(
                            "Page.getFrameTree missing frameTree.frame.id".into(),
                        )
                    })?
                    .to_string();
                let url = frame_node["url"].as_str().unwrap_or("").to_string();
                let name = frame_node["name"].as_str().map(str::to_string);
                Ok::<_, ZendriverError>(Frame::new(
                    frame_id,
                    None,
                    url,
                    name,
                    self.inner.session.clone(),
                    Arc::downgrade(&self.inner),
                ))
            })
            .await?;
        Ok(frame.clone())
    }

    /// Snapshot of all currently-registered frames for this tab.
    ///
    /// The registry is maintained by an internal lifecycle subscriber spawned
    /// when the Tab is constructed (see the [`crate::frame::lifecycle`]
    /// module). Includes the top-level frame (once Chrome has emitted at
    /// least one `Page.frameAttached` or `Page.frameNavigated` for it) plus
    /// every same-origin sub-frame. Out-of-process iframes (OOPIFs) land in
    /// this map via the [`crate::frame::oopif`] observer path.
    ///
    /// Order is unspecified ([`HashMap`] iteration); callers that need a
    /// stable order should sort by [`Frame::id`] or [`Frame::url`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// for f in tab.frames().await? {
    ///     println!("frame {}: {}", f.id(), f.url().await);
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn frames(&self) -> Result<Vec<Frame>> {
        Ok(self.inner.frames.read().await.values().cloned().collect())
    }

    /// First frame in [`Tab::frames`] whose URL contains `url_substr`.
    ///
    /// Linear scan over the registry. Useful for picking a frame by its
    /// origin (e.g. `tab.frame_by_url("docs.google.com")`) without knowing
    /// the exact path. Returns `Ok(None)` if no frame matches; the registry
    /// lock is released before returning so concurrent updates can land.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// if let Some(iframe) = tab.frame_by_url("youtube.com").await? {
    ///     println!("found iframe: {}", iframe.url().await);
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn frame_by_url(&self, url_substr: &str) -> Result<Option<Frame>> {
        let map = self.inner.frames.read().await;
        for frame in map.values() {
            if frame.url().await.contains(url_substr) {
                return Ok(Some(frame.clone()));
            }
        }
        Ok(None)
    }

    /// First frame in [`Tab::frames`] whose `name` attribute equals `name`.
    ///
    /// Linear scan. Frames without a name attribute (the common case for
    /// the top-level frame and unnamed iframes) are skipped. Returns
    /// `Ok(None)` if no frame matches.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// if let Some(content) = tab.frame_by_name("content").await? {
    ///     content.evaluate::<()>("document.body.scrollTop = 0").await?;
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn frame_by_name(&self, name: &str) -> Result<Option<Frame>> {
        let map = self.inner.frames.read().await;
        Ok(map.values().find(|f| f.name() == Some(name)).cloned())
    }

    /// Browser-wide cookie store handle.
    ///
    /// Convenience accessor that delegates to the owning [`crate::Browser`]'s
    /// root [`zendriver_transport::Connection`] — Chrome's cookie store is
    /// browser-scoped, so this jar is functionally identical to
    /// [`crate::Browser::cookies`] for the same browser.
    ///
    /// If the owning Browser has already been dropped (which shouldn't happen
    /// in practice because Drop ordering keeps it alive while any Tab clone
    /// exists, but is handled defensively here), the jar falls back to the
    /// Tab's session-level connection.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let jar = tab.cookies();
    /// let all = jar.all().await?;
    /// println!("{} cookies set", all.len());
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn cookies(&self) -> crate::CookieJar {
        let conn = self.inner.browser.upgrade().map_or_else(
            || self.inner.session.connection().clone(),
            |b| b.conn.clone(),
        );
        crate::CookieJar::new(conn)
    }

    /// Per-tab `localStorage` accessor.
    ///
    /// The returned [`crate::Storage`] is configured with `is_local: true`
    /// and dispatches against this tab's session; each operation re-resolves
    /// the tab's current origin via a [`Tab::url`] round-trip (since
    /// DOMStorage is origin-keyed and a navigation between calls would shift
    /// the target storage area).
    ///
    /// `DOMStorage.enable` fires lazily on the first op per handle so
    /// re-using the same handle across many calls pays the enable cost
    /// exactly once.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let ls = tab.local_storage();
    /// ls.set("theme", "dark").await?;
    /// let v = ls.get("theme").await?;
    /// assert_eq!(v.as_deref(), Some("dark"));
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn local_storage(&self) -> crate::Storage {
        crate::Storage::new(
            self.inner.session.clone(),
            true,
            Arc::downgrade(&self.inner),
        )
    }

    /// Per-tab `sessionStorage` accessor.
    ///
    /// Mirror of [`Tab::local_storage`] with `is_local: false` — backs the
    /// per-tab, per-origin `sessionStorage` area instead of the persistent
    /// localStorage.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.session_storage().set("draft", "hello").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn session_storage(&self) -> crate::Storage {
        crate::Storage::new(
            self.inner.session.clone(),
            false,
            Arc::downgrade(&self.inner),
        )
    }

    /// Helper: call a CDP method on this tab's session, parsing transport
    /// errors into `ZendriverError`.
    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value> {
        trace!(%method, "tab.call");
        let res = self.inner.session.call(method, params).await?;
        Ok(res)
    }

    /// Navigate the tab to `url`.
    ///
    /// Does NOT wait for the load to complete — call [`Tab::wait_for_load`]
    /// (or [`Tab::wait_for_idle`]) afterward to block on the navigation.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] when Chrome reports
    /// `errorText` on the `Page.navigate` response (e.g. DNS failure,
    /// connection refused, invalid URL).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.wait_for_load().await?;
    /// # Ok(()) }
    /// ```
    pub async fn goto(&self, url: impl AsRef<str>) -> Result<()> {
        // Enable Page domain so we get FrameStoppedLoading events.
        self.call("Page.enable", json!({})).await?;
        let url_s = url.as_ref().to_string();
        let res = self.call("Page.navigate", json!({ "url": url_s })).await?;
        if let Some(err) = res.get("errorText").and_then(|v| v.as_str()) {
            if !err.is_empty() {
                return Err(ZendriverError::Navigation(err.to_string()));
            }
        }
        Ok(())
    }

    /// Wait until the main frame's load event fires.
    ///
    /// Subscribes to `Page.frameStoppedLoading` and waits for the first
    /// event. Bounded by a 30s timeout.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Timeout`] when no load event arrives
    /// within 30s; [`ZendriverError::Navigation`] if the event stream
    /// closes (transport teardown).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.wait_for_load().await?;
    /// # Ok(()) }
    /// ```
    pub async fn wait_for_load(&self) -> Result<()> {
        // Subscribe before any `goto` to avoid missing the event; in P1 we
        // accept that callers may have a small race. P3+ revisits.
        let mut stream = self
            .inner
            .session
            .subscribe::<Value>("Page.frameStoppedLoading");
        timeout(DEFAULT_LOAD_TIMEOUT, stream.next())
            .await
            .map_err(|_| ZendriverError::Timeout(DEFAULT_LOAD_TIMEOUT))?
            .ok_or_else(|| ZendriverError::Navigation("page event stream closed".into()))?;
        Ok(())
    }

    /// Evaluate a JavaScript expression in an isolated world.
    ///
    /// Runs in a sandbox where page globals are NOT visible — the default for
    /// stealth-safe execution. The result is deserialized into `T`.
    ///
    /// If the cached isolated-world execution context was destroyed (e.g. by
    /// a page navigation), the cache is invalidated and the evaluation is
    /// retried once.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::JsException`] when the expression raises;
    /// [`ZendriverError::Serde`] when the result cannot be decoded into `T`;
    /// [`ZendriverError::Navigation`] when the execution context is missing.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let n: i32 = tab.evaluate("1 + 2").await?;
    /// assert_eq!(n, 3);
    /// # Ok(()) }
    /// ```
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let js = js.as_ref();
        for attempt in 0..2 {
            let ctx_id = self.ensure_isolated_world().await?;
            let res = self
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": js,
                        "contextId": ctx_id,
                        "returnByValue": true,
                        "awaitPromise": true,
                    }),
                )
                .await;
            match res {
                Ok(v) => {
                    if let Some(details) = v.get("exceptionDetails") {
                        let msg = details
                            .get("exception")
                            .and_then(|e| e.get("description"))
                            .and_then(|d| d.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        return Err(ZendriverError::JsException(msg));
                    }
                    let value = v
                        .get("result")
                        .and_then(|r| r.get("value"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    return serde_json::from_value(value).map_err(ZendriverError::Serde);
                }
                // Chrome returns -32000 "Cannot find context with specified
                // id" when the execution context we cached was destroyed
                // (typically by a navigation). `From<CallError>` maps that
                // to `Navigation` (see `error.rs`), so we match on that
                // variant here — not on `Cdp` as the original P2 plan
                // suggested.
                Err(ZendriverError::Navigation(ref m))
                    if attempt == 0 && m.contains("Cannot find context") =>
                {
                    self.inner.isolated_world.lock().await.context_id = None;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    /// Evaluate a JavaScript expression in the page main world.
    ///
    /// Page globals (e.g. `window.foo` set by page scripts) ARE visible.
    /// Escape hatch for cases where isolated-world semantics don't fit; for
    /// stealth-sensitive contexts prefer [`Tab::evaluate`].
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
    /// tab.goto("https://example.com").await?;
    /// let title: String = tab.evaluate_main("document.title").await?;
    /// println!("{title}");
    /// # Ok(()) }
    /// ```
    pub async fn evaluate_main<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let res = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": js.as_ref(),
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
        let value = res
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(ZendriverError::Serde)
    }

    /// Ensure an isolated-world execution context exists for this tab's main
    /// frame, returning its `executionContextId`. Cached after first call.
    pub(crate) async fn ensure_isolated_world(&self) -> Result<i64> {
        let mut cache = self.inner.isolated_world.lock().await;
        if let Some(ctx) = cache.context_id {
            return Ok(ctx);
        }
        // Discover the main frame id.
        let tree = self.call("Page.getFrameTree", json!({})).await?;
        let frame_id = tree["frameTree"]["frame"]["id"]
            .as_str()
            .ok_or_else(|| ZendriverError::Navigation("no main frame in Page.getFrameTree".into()))?
            .to_string();
        let res = self
            .call(
                "Page.createIsolatedWorld",
                json!({
                    "frameId": frame_id,
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
        cache.main_frame_id = Some(frame_id);
        cache.context_id = Some(ctx_id);
        Ok(ctx_id)
    }

    /// Get the tab's current URL.
    ///
    /// Returns a parsed [`url::Url`]. Reads from `Target.getTargetInfo`'s
    /// `targetInfo.url`.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] when Chrome returns no URL or
    /// the URL is unparseable.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com/foo").await?;
    /// let u = tab.url().await?;
    /// assert_eq!(u.path(), "/foo");
    /// # Ok(()) }
    /// ```
    pub async fn url(&self) -> Result<url::Url> {
        let res = self.call("Target.getTargetInfo", json!({})).await?;
        let s = res["targetInfo"]["url"]
            .as_str()
            .ok_or_else(|| ZendriverError::Navigation("target has no url".into()))?;
        url::Url::parse(s).map_err(|e| ZendriverError::Navigation(e.to_string()))
    }

    /// Get the tab's `<title>`.
    ///
    /// Reads from `Target.getTargetInfo`'s `targetInfo.title`. Returns an
    /// empty string when the page has no title.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// assert_eq!(tab.title().await?, "Example Domain");
    /// # Ok(()) }
    /// ```
    pub async fn title(&self) -> Result<String> {
        let res = self.call("Target.getTargetInfo", json!({})).await?;
        Ok(res["targetInfo"]["title"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    /// Construct a [`ScreenshotBuilder`] bound to this tab.
    ///
    /// Chain format / clip / quality / full-page options, then call
    /// [`ScreenshotBuilder::bytes`] or [`ScreenshotBuilder::save`] to
    /// execute the capture.
    ///
    /// For element-scoped screenshots, see [`crate::Element::screenshot`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.screenshot_builder()
    ///     .full_page(true)
    ///     .jpeg()
    ///     .quality(85)
    ///     .save("page.jpg").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn screenshot_builder(&self) -> ScreenshotBuilder<'_> {
        ScreenshotBuilder::new(self)
    }

    /// Capture a full-viewport PNG screenshot of this tab.
    ///
    /// Convenience wrapper over `self.screenshot_builder().png().bytes().await`.
    /// For JPEG / WebP / full-page / clipped captures, drive
    /// [`Tab::screenshot_builder`] directly. For element-scoped screenshots,
    /// see [`crate::Element::screenshot`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let png_bytes = tab.screenshot().await?;
    /// tokio::fs::write("page.png", png_bytes).await?;
    /// # Ok(()) }
    /// ```
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        self.screenshot_builder().png().bytes().await
    }

    /// Close this tab in Chrome.
    ///
    /// Sends `Target.closeTarget { targetId }` at browser scope (no
    /// `session_id`) using the cached `targetId`. Chrome destroys the page
    /// target, which in turn produces a `Target.detachedFromTarget` event
    /// whose internal handler removes this tab from the browser's tab
    /// registry.
    ///
    /// Consumes `self` — the [`Tab`] handle is gone after this returns.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// let tab = browser.new_tab().await?;
    /// tab.goto("https://example.com").await?;
    /// tab.close().await?;
    /// # Ok(()) }
    /// ```
    pub async fn close(self) -> Result<()> {
        let target_id = self.target_id().to_string();
        self.inner
            .session
            .connection()
            .call_raw("Target.closeTarget", json!({ "targetId": target_id }), None)
            .await?;
        Ok(())
    }

    /// Bring this tab to the foreground in Chrome.
    ///
    /// Sends `Target.activateTarget { targetId }` at browser scope (no
    /// `session_id`) using the cached `targetId`. Chrome focuses the page
    /// target so it becomes the visible/active tab.
    ///
    /// Unlike [`Tab::close`], this borrows `&self` — the tab remains usable
    /// after activation. Useful in multi-tab workflows where you want to
    /// surface a specific tab without tearing it down.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// let tab1 = browser.main_tab();
    /// let tab2 = browser.new_tab().await?;
    /// // Bring the first tab back to focus.
    /// tab1.activate().await?;
    /// # let _ = tab2;
    /// # Ok(()) }
    /// ```
    pub async fn activate(&self) -> Result<()> {
        let target_id = self.target_id().to_string();
        self.inner
            .session
            .connection()
            .call_raw(
                "Target.activateTarget",
                json!({ "targetId": target_id }),
                None,
            )
            .await?;
        Ok(())
    }

    /// Navigate one step backward in the tab's session history.
    ///
    /// Fetches the history list via `Page.getNavigationHistory`, then
    /// dispatches `Page.navigateToHistoryEntry { entryId }` for the entry at
    /// `currentIndex - 1`.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::HistoryNavigation`] with `"no back history"`
    /// when `currentIndex <= 0`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.goto("https://example.org").await?;
    /// tab.back().await?;
    /// # Ok(()) }
    /// ```
    pub async fn back(&self) -> Result<()> {
        let history = self.call("Page.getNavigationHistory", json!({})).await?;
        let current_idx = history["currentIndex"].as_i64().ok_or_else(|| {
            ZendriverError::HistoryNavigation(
                "Page.getNavigationHistory missing currentIndex".into(),
            )
        })?;
        if current_idx <= 0 {
            return Err(ZendriverError::HistoryNavigation("no back history".into()));
        }
        let entry_id = history["entries"][(current_idx - 1) as usize]["id"].clone();
        self.call(
            "Page.navigateToHistoryEntry",
            json!({ "entryId": entry_id }),
        )
        .await?;
        Ok(())
    }

    /// Navigate one step forward in the tab's session history.
    ///
    /// Fetches the history list via `Page.getNavigationHistory`, then
    /// dispatches `Page.navigateToHistoryEntry { entryId }` for the entry at
    /// `currentIndex + 1`.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::HistoryNavigation`] with `"no forward history"`
    /// when `currentIndex` is already at the last entry.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.goto("https://example.org").await?;
    /// tab.back().await?;
    /// tab.forward().await?;
    /// # Ok(()) }
    /// ```
    pub async fn forward(&self) -> Result<()> {
        let history = self.call("Page.getNavigationHistory", json!({})).await?;
        let current_idx = history["currentIndex"].as_i64().ok_or_else(|| {
            ZendriverError::HistoryNavigation(
                "Page.getNavigationHistory missing currentIndex".into(),
            )
        })?;
        let entries = history["entries"].as_array().ok_or_else(|| {
            ZendriverError::HistoryNavigation("Page.getNavigationHistory missing entries".into())
        })?;
        if (current_idx + 1) as usize >= entries.len() {
            return Err(ZendriverError::HistoryNavigation(
                "no forward history".into(),
            ));
        }
        let entry_id = entries[(current_idx + 1) as usize]["id"].clone();
        self.call(
            "Page.navigateToHistoryEntry",
            json!({ "entryId": entry_id }),
        )
        .await?;
        Ok(())
    }

    /// Reload the tab's current page.
    ///
    /// Dispatches `Page.reload` with `ignoreCache: false` — equivalent to a
    /// soft refresh.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.reload().await?;
    /// # Ok(()) }
    /// ```
    pub async fn reload(&self) -> Result<()> {
        self.call("Page.reload", json!({ "ignoreCache": false }))
            .await?;
        Ok(())
    }

    /// Wait until the tab's network has been idle (0 in-flight requests)
    /// for 500ms, with a 30s outer timeout. Playwright `networkidle`
    /// semantics.
    ///
    /// Backed by a per-Tab in-flight network tracker that subscribes to
    /// `Network.requestWillBeSent` (insert) and the three terminal events
    /// (`responseReceived` / `loadingFailed` / `loadingFinished`, all
    /// remove).
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Timeout`] with the configured timeout
    /// duration when the network does not stay idle within the deadline.
    ///
    /// See [`Tab::wait_for_idle_with`] for tunable timeout + quiet window.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.wait_for_idle().await?;
    /// # Ok(()) }
    /// ```
    pub async fn wait_for_idle(&self) -> Result<()> {
        self.wait_for_idle_with(Duration::from_secs(30), Duration::from_millis(500))
            .await
    }

    /// Wait until the tab's network has been idle for `quiet_window`,
    /// bounded by `timeout`.
    ///
    /// Algorithm: poll the in-flight set with a `Notify`-driven wake (or a
    /// 50ms fallback tick). Track `quiet_start = Some(now)` on the first
    /// observation of an empty set; reset to `None` on any observation
    /// where the set is non-empty. Return once `now - quiet_start
    /// >= quiet_window`.
    ///
    /// The 50ms tick is a safety net for the case where the tracker is
    /// already at 0 in-flight requests and no further events fire to wake
    /// the notifier. Worst-case latency to detect "stayed idle long enough"
    /// is `quiet_window + 50ms`.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Timeout`] (carrying the supplied `timeout`)
    /// once the outer deadline elapses.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.wait_for_idle_with(
    ///     Duration::from_secs(60),
    ///     Duration::from_secs(1),
    /// ).await?;
    /// # Ok(()) }
    /// ```
    pub async fn wait_for_idle_with(
        &self,
        timeout: Duration,
        quiet_window: Duration,
    ) -> Result<()> {
        let tracker = self.inner.network_tracker.clone();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut quiet_start: Option<tokio::time::Instant> = None;
        loop {
            let in_flight_count = tracker.in_flight.lock().await.len();
            if in_flight_count == 0 {
                let now = tokio::time::Instant::now();
                match quiet_start {
                    None => quiet_start = Some(now),
                    Some(start) if now.duration_since(start) >= quiet_window => {
                        return Ok(());
                    }
                    _ => {}
                }
            } else {
                quiet_start = None;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ZendriverError::Timeout(timeout));
            }
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(50)) => {}
                () = tracker.notifier.notified() => {}
            }
        }
    }
}

impl Tab {
    /// Begin a chainable element query against this tab.
    ///
    /// Pick a selector kind (`.css`, `.xpath`, `.text`, `.text_exact`,
    /// `.text_regex`, `.text_regex_with_flags`, `.role`, `.role_named`),
    /// optionally apply modifiers (`.nth`, `.visible_only`, `.in_frame`,
    /// `.timeout`), then terminate with `.one()` or `.one_or_none()`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let h1 = tab.find().css("h1").one().await?;
    /// h1.click().await?;
    /// # Ok(()) }
    /// ```
    pub fn find(&self) -> crate::query::FindBuilder<'_> {
        crate::query::FindBuilder::new_for_tab(self)
    }

    /// Begin a chainable element query against this tab that returns
    /// ALL matches.
    ///
    /// Mirrors [`Tab::find`] selectors + modifiers (no `nth`); terminate
    /// with `.many()` (errors on empty) or `.many_or_empty()` (returns
    /// empty `Vec` instead).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let links = tab.find_all().css("a").many_or_empty().await?;
    /// println!("{} links", links.len());
    /// # Ok(()) }
    /// ```
    pub fn find_all(&self) -> crate::query::FindAllBuilder<'_> {
        crate::query::FindAllBuilder::new_for_tab(self)
    }
}

#[cfg(feature = "expect")]
impl Tab {
    /// Register a one-shot expectation for the first
    /// `Network.requestWillBeSent` whose URL matches `pattern`.
    ///
    /// `pattern` is anything convertible to a [`crate::expect::UrlMatcher`]:
    /// `&str` / `String` build a substring matcher; [`regex::Regex`] builds
    /// a regex matcher. The returned
    /// [`RequestExpectation`](crate::expect::request::RequestExpectation)
    /// is awaitable directly (`expectation.await`) or via the
    /// Playwright-style `expectation.matched().await`; configure the
    /// timeout via
    /// [`timeout`](crate::expect::request::RequestExpectation::timeout)
    /// before awaiting.
    ///
    /// The subscriber task is spawned synchronously inside this call —
    /// the subscription is live by the time you receive the
    /// `RequestExpectation`, so a trigger action issued immediately
    /// after cannot race past us. `Network.enable` is already on per-Tab
    /// via the P4 in-flight tracker; this call does not re-enable.
    ///
    /// Gated by the `expect` cargo feature.
    #[must_use]
    pub fn expect_request(
        &self,
        pattern: impl Into<crate::expect::UrlMatcher>,
    ) -> crate::expect::request::RequestExpectation {
        crate::expect::request::register(self.session(), pattern.into())
    }

    /// Register a one-shot expectation for the first
    /// `Network.responseReceived` whose URL matches `pattern`.
    ///
    /// `pattern` is anything convertible to a [`crate::expect::UrlMatcher`]:
    /// `&str` / `String` build a substring matcher; [`regex::Regex`] builds
    /// a regex matcher. The returned
    /// [`ResponseExpectation`](crate::expect::response::ResponseExpectation)
    /// is awaitable directly (`expectation.await`) or via the
    /// Playwright-style `expectation.matched().await`; configure the
    /// timeout via
    /// [`timeout`](crate::expect::response::ResponseExpectation::timeout)
    /// before awaiting.
    ///
    /// Resolves with a
    /// [`MatchedResponse`](crate::expect::response::MatchedResponse) whose
    /// [`body`](crate::expect::response::MatchedResponse::body) method
    /// fetches the response payload via `Network.getResponseBody`. Bodies
    /// are only retained for a short window after the response completes —
    /// call `body()` promptly.
    ///
    /// The subscriber task is spawned synchronously inside this call —
    /// the subscription is live by the time you receive the
    /// `ResponseExpectation`, so a trigger action issued immediately after
    /// cannot race past us. `Network.enable` is already on per-Tab via the
    /// P4 in-flight tracker; this call does not re-enable.
    ///
    /// Gated by the `expect` cargo feature.
    #[must_use]
    pub fn expect_response(
        &self,
        pattern: impl Into<crate::expect::UrlMatcher>,
    ) -> crate::expect::response::ResponseExpectation {
        crate::expect::response::register(self.session(), pattern.into())
    }

    /// Register a one-shot expectation for the first
    /// `Page.javascriptDialogOpened` event on this tab.
    ///
    /// There is no URL pattern: dialogs don't carry a request URL the way
    /// requests/responses do — any dialog opened during the expectation
    /// window matches. The page URL is captured on the resolved
    /// [`MatchedDialog`](crate::expect::dialog::MatchedDialog) for context.
    ///
    /// The returned
    /// [`DialogExpectation`](crate::expect::dialog::DialogExpectation) is
    /// awaitable directly (`expectation.await`) or via the
    /// Playwright-style `expectation.matched().await`; configure the
    /// timeout via
    /// [`timeout`](crate::expect::dialog::DialogExpectation::timeout) before
    /// awaiting.
    ///
    /// Resolves with a
    /// [`MatchedDialog`](crate::expect::dialog::MatchedDialog) whose
    /// [`accept`](crate::expect::dialog::MatchedDialog::accept) /
    /// [`dismiss`](crate::expect::dialog::MatchedDialog::dismiss) methods
    /// dispatch `Page.handleJavaScriptDialog`.
    ///
    /// The subscriber task is spawned synchronously inside this call — the
    /// subscription is live by the time you receive the
    /// `DialogExpectation`, so a trigger action issued immediately after
    /// cannot race past us. `Page.enable` is already on per-Tab via P1's
    /// `Tab::goto`; this call does not re-enable.
    ///
    /// Gated by the `expect` cargo feature.
    #[must_use]
    pub fn expect_dialog(&self) -> crate::expect::dialog::DialogExpectation {
        crate::expect::dialog::register(self.session())
    }

    /// Register a one-shot expectation for the first `Page.downloadWillBegin`
    /// on this tab.
    ///
    /// First call on a Tab also allocates a per-Tab tempdir, dispatches
    /// `Browser.setDownloadBehavior { behavior: "allowAndName", downloadPath
    /// }` at browser scope, and spawns a long-running `Page.downloadProgress`
    /// subscriber. The coordinator is reused across every subsequent
    /// `expect_download` call on the same tab. `Page.enable` is already on
    /// per-Tab via P1's `Tab::goto` / the frame lifecycle subscriber, so
    /// this call does not re-enable.
    ///
    /// Returned [`MatchedDownload`](crate::expect::download::MatchedDownload)
    /// exposes [`path`](crate::expect::download::MatchedDownload::path) /
    /// [`save_to`](crate::expect::download::MatchedDownload::save_to) for
    /// reaching the downloaded bytes once Chrome reports completion.
    ///
    /// Gated by the `expect` cargo feature.
    pub async fn expect_download(&self) -> Result<crate::expect::download::DownloadExpectation> {
        let coord = crate::expect::download::ensure_download_setup(
            &self.inner.download_setup,
            self.session(),
        )
        .await?;
        Ok(crate::expect::download::register(self.session(), coord))
    }
}

#[cfg(feature = "cloudflare")]
impl Tab {
    /// Construct a
    /// [`CloudflareBypass`](zendriver_cloudflare::CloudflareBypass) bound to
    /// this tab's session.
    ///
    /// Chain
    /// [`poll_interval`](zendriver_cloudflare::CloudflareBypass::poll_interval)
    /// to tune the polling cadence, then call
    /// [`wait_for_clearance`](zendriver_cloudflare::CloudflareBypass::wait_for_clearance)
    /// to detect the Turnstile checkbox, click it at the canonical 15%
    /// offset, and poll until either the `cf-turnstile-response` token
    /// appears, the challenge container disappears, or the supplied timeout
    /// elapses. Use
    /// [`is_challenge_present`](zendriver_cloudflare::CloudflareBypass::is_challenge_present)
    /// for a one-shot probe without driving a click.
    ///
    /// Gated by the `cloudflare` cargo feature.
    #[must_use]
    pub fn cloudflare(&self) -> zendriver_cloudflare::CloudflareBypass<'_> {
        zendriver_cloudflare::CloudflareBypass::new(self.session())
    }
}

#[cfg(feature = "interception")]
impl Tab {
    /// Construct a fluent
    /// [`InterceptBuilder`](zendriver_interception::InterceptBuilder) for
    /// this tab's session.
    ///
    /// Chain rule registration (`.block(...)` / `.redirect(...)` /
    /// `.respond(...)` / `.modify_request(...)`) and optional CDP
    /// `RequestPattern` filters (`.pattern(...)` / `.at_request()` /
    /// `.at_response()` / `.resource(...)`), then call
    /// [`start`](zendriver_interception::InterceptBuilder::start) to spawn
    /// the rule-driven actor (returns an
    /// [`InterceptHandle`](zendriver_interception::InterceptHandle) whose
    /// `Drop` tears it down), or
    /// [`subscribe`](zendriver_interception::InterceptBuilder::subscribe)
    /// to receive raw
    /// [`PausedRequest`](zendriver_interception::PausedRequest)s on a
    /// stream you drive manually.
    ///
    /// Gated by the `interception` cargo feature.
    #[must_use]
    pub fn intercept(&self) -> zendriver_interception::InterceptBuilder<'_> {
        zendriver_interception::InterceptBuilder::new(self.session())
    }
}

impl crate::traits::Queryable for Tab {
    fn find(&self) -> crate::query::FindBuilder<'_> {
        Tab::find(self)
    }
    fn find_all(&self) -> crate::query::FindAllBuilder<'_> {
        Tab::find_all(self)
    }
}

#[async_trait::async_trait]
impl crate::traits::Evaluable for Tab {
    async fn evaluate<T>(&self, js: &str) -> crate::error::Result<T>
    where
        T: serde::de::DeserializeOwned + Send + 'static,
    {
        Tab::evaluate(self, js).await
    }
    async fn evaluate_main<T>(&self, js: &str) -> crate::error::Result<T>
    where
        T: serde::de::DeserializeOwned + Send + 'static,
    {
        Tab::evaluate_main(self, js).await
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn goto_sends_page_enable_then_page_navigate_with_url() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.goto("https://example.com").await }
        });

        let id_enable = mock.expect_cmd("Page.enable").await;
        mock.reply(id_enable, json!({})).await;

        let id_nav = mock.expect_cmd("Page.navigate").await;
        assert_eq!(mock.last_sent()["params"]["url"], "https://example.com");
        mock.reply(id_nav, json!({ "frameId": "F1" })).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn goto_returns_navigation_error_when_chrome_reports_errortext() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.goto("https://bad.test").await }
        });

        let id_enable = mock.expect_cmd("Page.enable").await;
        mock.reply(id_enable, json!({})).await;

        let id_nav = mock.expect_cmd("Page.navigate").await;
        mock.reply(id_nav, json!({ "errorText": "net::ERR_NAME_NOT_RESOLVED" }))
            .await;

        let res = fut.await.unwrap();
        match res {
            Err(ZendriverError::Navigation(m)) => assert!(m.contains("ERR_NAME_NOT_RESOLVED")),
            other => panic!("unexpected: {other:?}"),
        }
        conn.shutdown();
    }

    // --- main-world evaluate (escape hatch) ----------------------------

    #[tokio::test]
    async fn evaluate_main_returns_typed_value() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate_main::<i32>("1+1").await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["expression"], "1+1");
        // Main-world evaluate must NOT pass a contextId.
        assert!(mock.last_sent()["params"].get("contextId").is_none());
        mock.reply(id, json!({ "result": { "value": 2, "type": "number" } }))
            .await;
        let n = fut.await.unwrap().unwrap();
        assert_eq!(n, 2);
        conn.shutdown();
    }

    #[tokio::test]
    async fn evaluate_main_returns_js_exception_when_chrome_reports_one() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate_main::<i32>("throw new Error('boom')").await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            json!({
                "result": { "type": "object", "subtype": "error" },
                "exceptionDetails": {
                    "exception": { "description": "Error: boom\n    at <anonymous>:1:7" }
                }
            }),
        )
        .await;
        let res = fut.await.unwrap();
        match res {
            Err(ZendriverError::JsException(m)) => assert!(m.contains("Error: boom")),
            other => panic!("unexpected: {other:?}"),
        }
        conn.shutdown();
    }

    // --- isolated-world evaluate ---------------------------------------

    #[tokio::test]
    async fn evaluate_isolated_creates_world_then_evaluates() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("1+1").await }
        });

        // 1. Page.getFrameTree → main frame id.
        let id_tree = mock.expect_cmd("Page.getFrameTree").await;
        mock.reply(
            id_tree,
            json!({ "frameTree": { "frame": { "id": "MAIN_FRAME" } } }),
        )
        .await;

        // 2. Page.createIsolatedWorld → executionContextId.
        let id_world = mock.expect_cmd("Page.createIsolatedWorld").await;
        assert_eq!(mock.last_sent()["params"]["frameId"], "MAIN_FRAME");
        assert_eq!(mock.last_sent()["params"]["worldName"], "zendriver-eval");
        mock.reply(id_world, json!({ "executionContextId": 42 }))
            .await;

        // 3. Runtime.evaluate with that contextId.
        let id_eval = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["expression"], "1+1");
        assert_eq!(mock.last_sent()["params"]["contextId"], 42);
        mock.reply(
            id_eval,
            json!({ "result": { "value": 2, "type": "number" } }),
        )
        .await;

        let n = fut.await.unwrap().unwrap();
        assert_eq!(n, 2);
        conn.shutdown();
    }

    #[tokio::test]
    async fn evaluate_caches_context_id_across_calls() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        // First call: full handshake + eval.
        let fut1 = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("1").await }
        });
        let id_tree = mock.expect_cmd("Page.getFrameTree").await;
        mock.reply(
            id_tree,
            json!({ "frameTree": { "frame": { "id": "MAIN_FRAME" } } }),
        )
        .await;
        let id_world = mock.expect_cmd("Page.createIsolatedWorld").await;
        mock.reply(id_world, json!({ "executionContextId": 7 }))
            .await;
        let id_eval1 = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["contextId"], 7);
        mock.reply(
            id_eval1,
            json!({ "result": { "value": 1, "type": "number" } }),
        )
        .await;
        assert_eq!(fut1.await.unwrap().unwrap(), 1);

        // Second call: must reuse the cached contextId → next outbound
        // frame should be Runtime.evaluate, with NO Page.getFrameTree or
        // Page.createIsolatedWorld in between.
        let fut2 = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("2").await }
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

    #[tokio::test]
    async fn evaluate_recreates_world_after_context_destroyed_error() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        // --- Call 1: establishes cache, succeeds. ---
        let fut1 = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("1").await }
        });
        let id_tree = mock.expect_cmd("Page.getFrameTree").await;
        mock.reply(
            id_tree,
            json!({ "frameTree": { "frame": { "id": "MAIN_FRAME" } } }),
        )
        .await;
        let id_world = mock.expect_cmd("Page.createIsolatedWorld").await;
        mock.reply(id_world, json!({ "executionContextId": 7 }))
            .await;
        let id_eval1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_eval1,
            json!({ "result": { "value": 1, "type": "number" } }),
        )
        .await;
        assert_eq!(fut1.await.unwrap().unwrap(), 1);

        // --- Call 2: cached contextId is now stale. Runtime.evaluate
        //     returns -32000 "Cannot find context with specified id";
        //     evaluate must invalidate the cache, re-run the discovery
        //     handshake with a NEW contextId, then re-issue Runtime.evaluate.
        let fut2 = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("2").await }
        });
        // First Runtime.evaluate uses cached id 7 → CDP returns error.
        let id_eval_fail = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["contextId"], 7);
        mock.reply_err(
            id_eval_fail,
            -32000,
            "Cannot find context with specified id",
        )
        .await;

        // Cache invalidated → discovery handshake re-runs.
        let id_tree2 = mock.expect_cmd("Page.getFrameTree").await;
        mock.reply(
            id_tree2,
            json!({ "frameTree": { "frame": { "id": "MAIN_FRAME_2" } } }),
        )
        .await;
        let id_world2 = mock.expect_cmd("Page.createIsolatedWorld").await;
        assert_eq!(mock.last_sent()["params"]["frameId"], "MAIN_FRAME_2");
        mock.reply(id_world2, json!({ "executionContextId": 99 }))
            .await;

        // Retried Runtime.evaluate uses the fresh contextId.
        let id_eval_retry = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["contextId"], 99);
        mock.reply(
            id_eval_retry,
            json!({ "result": { "value": 2, "type": "number" } }),
        )
        .await;
        assert_eq!(fut2.await.unwrap().unwrap(), 2);

        // --- Call 3: cache is fresh again → straight to Runtime.evaluate.
        let fut3 = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("3").await }
        });
        let id_eval3 = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["contextId"], 99);
        mock.reply(
            id_eval3,
            json!({ "result": { "value": 3, "type": "number" } }),
        )
        .await;
        assert_eq!(fut3.await.unwrap().unwrap(), 3);

        conn.shutdown();
    }

    // --- main_frame discovery (P4 T12) --------------------------------

    /// First [`Tab::main_frame`] call dispatches `Page.getFrameTree`, parses
    /// the top-level frame, and constructs a [`Frame`] with `is_main() ==
    /// true`. Second call must NOT round-trip — the `OnceCell` caches the
    /// `Frame` so further outbound traffic is empty for the same tab.
    #[tokio::test]
    async fn main_frame_discovers_top_level_frame_and_caches() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.main_frame().await }
        });

        let id = mock.expect_cmd("Page.getFrameTree").await;
        mock.reply(
            id,
            json!({
                "frameTree": {
                    "frame": {
                        "id": "F0",
                        "url": "https://x.test",
                    }
                }
            }),
        )
        .await;

        let frame = fut.await.unwrap().unwrap();
        assert_eq!(frame.id(), "F0");
        assert!(frame.is_main());
        assert!(frame.parent_id().is_none());
        assert!(frame.name().is_none());
        assert_eq!(frame.url().await, "https://x.test");

        // Second call: must hit cache, no further outbound CDP traffic.
        let frame2 = tab.main_frame().await.unwrap();
        assert_eq!(frame2.id(), "F0");
        // Verify the mock saw no additional commands — `expect_cmd` would
        // time out internally on the next call. We check via the lighter
        // `try_next` shape: a follow-up request would be queued already.
        // Drop the connection to assert nothing else is in-flight.
        conn.shutdown();
    }

    #[tokio::test]
    async fn url_returns_parsed_url_from_target_info() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.url().await }
        });

        let id = mock.expect_cmd("Target.getTargetInfo").await;
        mock.reply(
            id,
            json!({ "targetInfo": { "url": "https://example.com/x", "title": "ok" } }),
        )
        .await;
        let u = fut.await.unwrap().unwrap();
        assert_eq!(u.as_str(), "https://example.com/x");
        conn.shutdown();
    }

    #[tokio::test]
    async fn close_sends_target_close_target_with_target_id() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S42");
        // `Tab::new_for_test` derives a deterministic target_id from the
        // session_id: `test-target-S42` here.
        let tab = Tab::new_for_test(sess);
        assert_eq!(tab.target_id(), "test-target-S42");

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.close().await }
        });

        let id = mock.expect_cmd("Target.closeTarget").await;
        assert_eq!(mock.last_sent()["params"]["targetId"], "test-target-S42");
        // Browser-scope command — no session_id.
        assert!(mock.last_sent().get("sessionId").is_none());
        mock.reply(id, json!({ "success": true })).await;
        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn activate_sends_target_activate_target_with_target_id() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S99");
        // `Tab::new_for_test` derives a deterministic target_id from the
        // session_id: `test-target-S99` here.
        let tab = Tab::new_for_test(sess);
        assert_eq!(tab.target_id(), "test-target-S99");

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.activate().await }
        });

        let id = mock.expect_cmd("Target.activateTarget").await;
        assert_eq!(mock.last_sent()["params"]["targetId"], "test-target-S99");
        // Browser-scope command — no session_id.
        assert!(mock.last_sent().get("sessionId").is_none());
        mock.reply(id, json!({})).await;
        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn screenshot_sends_page_capturescreenshot_without_clip_and_decodes_base64() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.screenshot().await }
        });

        let id = mock.expect_cmd("Page.captureScreenshot").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["format"], "png");
        // Tab::screenshot must NOT pass a clip — that's Element::screenshot.
        assert!(sent["params"].get("clip").is_none());
        // "PNG!" → b"PNG!" once base64-decoded.
        mock.reply(id, json!({ "data": "UE5HIQ==" })).await;

        let bytes = fut.await.unwrap().unwrap();
        assert_eq!(bytes, b"PNG!");
        conn.shutdown();
    }

    #[tokio::test]
    async fn title_returns_string_from_target_info() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.title().await }
        });

        let id = mock.expect_cmd("Target.getTargetInfo").await;
        mock.reply(
            id,
            json!({ "targetInfo": { "url": "https://x", "title": "Hello" } }),
        )
        .await;
        let s = fut.await.unwrap().unwrap();
        assert_eq!(s, "Hello");
        conn.shutdown();
    }

    // --- nav history: back / forward / reload --------------------------

    #[tokio::test]
    async fn back_dispatches_navigate_to_history_entry_at_prev_index() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.back().await }
        });

        let id_hist = mock.expect_cmd("Page.getNavigationHistory").await;
        mock.reply(
            id_hist,
            json!({
                "currentIndex": 1,
                "entries": [
                    { "id": 10, "url": "https://a.test" },
                    { "id": 11, "url": "https://b.test" },
                ],
            }),
        )
        .await;

        let id_nav = mock.expect_cmd("Page.navigateToHistoryEntry").await;
        // Should target the entry at currentIndex - 1 (id=10).
        assert_eq!(mock.last_sent()["params"]["entryId"], 10);
        mock.reply(id_nav, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn back_errors_when_current_index_is_zero() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.back().await }
        });

        let id_hist = mock.expect_cmd("Page.getNavigationHistory").await;
        mock.reply(
            id_hist,
            json!({
                "currentIndex": 0,
                "entries": [{ "id": 10, "url": "https://a.test" }],
            }),
        )
        .await;

        let res = fut.await.unwrap();
        match res {
            Err(ZendriverError::HistoryNavigation(m)) => assert!(m.contains("no back history")),
            other => panic!("unexpected: {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn reload_dispatches_page_reload_with_ignore_cache_false() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.reload().await }
        });

        let id = mock.expect_cmd("Page.reload").await;
        assert_eq!(mock.last_sent()["params"]["ignoreCache"], false);
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    // --- Tab::cookies (P4 T10) ----------------------------------------

    /// [`Tab::cookies`] returns a [`crate::CookieJar`] bound to the owning
    /// browser's root connection — discovered via the cached `Weak<BrowserInner>`
    /// upgrade. The test builds a synthetic `BrowserInner` with a known
    /// connection, attaches a Tab whose Weak ref points at it, and asserts
    /// that calling `.set(...)` dispatches `Network.setCookie` on that
    /// browser-level connection (not the Tab's session channel).
    #[tokio::test]
    async fn tab_cookies_dispatches_through_browser_connection_via_weak_upgrade() {
        use crate::browser::BrowserInner;
        use crate::cookies::Cookie;
        use std::collections::HashMap;
        use std::sync::{Arc, Weak};

        let input_profile = zendriver_stealth::InputProfile::native();
        let (mut mock, conn) = MockConnection::pair();

        let inner = Arc::new_cyclic(|weak: &Weak<BrowserInner>| {
            let main_session = SessionHandle::new(conn.clone(), "S1");
            let main_input = crate::input::InputController::new(input_profile.clone());
            let main_tab = Tab::new(main_session, weak.clone(), main_input, "T1".to_string());
            let mut map = HashMap::new();
            map.insert("S1".to_string(), main_tab.clone());
            BrowserInner {
                conn: conn.clone(),
                main_tab,
                child: tokio::sync::Mutex::new(None),
                _user_data: None,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
            }
        });
        let tab = inner.main_tab.clone();
        let jar = tab.cookies();

        let fut = tokio::spawn(async move {
            jar.set(Cookie {
                name: "sid".into(),
                value: "abc".into(),
                domain: ".example.com".into(),
                path: "/".into(),
                expires: None,
                http_only: false,
                secure: false,
                same_site: None,
                url: None,
            })
            .await
        });

        let id = mock.expect_cmd("Network.setCookie").await;
        assert_eq!(mock.last_sent()["params"]["name"], "sid");
        // Browser-scope command — no session_id (jar dispatches against
        // the browser's connection, not the tab's session).
        assert!(mock.last_sent().get("sessionId").is_none());
        mock.reply(id, json!({ "success": true })).await;

        fut.await.unwrap().unwrap();
        // Keep `inner` alive until after the dispatch so the Weak upgrade
        // succeeds — that's the path under test.
        drop(inner);
        conn.shutdown();
    }

    // --- frame lifecycle subscriber (P4 T15) ---------------------------

    /// End-to-end: emit `Page.frameAttached` for a new same-origin
    /// sub-frame; `tab.frames()` should expose it. Then emit
    /// `Page.frameDetached` for the same `frameId` and assert the
    /// registry shrinks back to empty.
    ///
    /// Mirrors the [`InFlightTracker`] test pattern — synchronize on the
    /// subscriber's outbound `Page.enable` call before driving events,
    /// then poll the registry shape (the lifecycle task processes events
    /// asynchronously).
    #[tokio::test]
    async fn frame_lifecycle_attach_then_detach_round_trip() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        // Synchronize: wait until the background lifecycle task has run
        // far enough to issue `Page.enable`. Once that command lands in
        // the mock's outbound queue, the three `Page.frame*` subscriptions
        // are already registered, so any subsequent
        // `emit_event_for_session` will be routed to them.
        let id_enable =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Page.enable"))
                .await
                .expect("frame lifecycle did not send Page.enable within 2s");
        mock.reply(id_enable, json!({})).await;

        // Emit a Page.frameAttached event for a child frame.
        mock.emit_event_for_session(
            "Page.frameAttached",
            json!({
                "frameId": "FCHILD",
                "parentFrameId": "FROOT",
            }),
            "S1",
        )
        .await;

        // Poll until the subscriber processes the event (async).
        for _ in 0..50 {
            if !tab.inner.frames.read().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let frames = tab.frames().await.unwrap();
        assert_eq!(frames.len(), 1, "expected one frame after attach event");
        let attached = &frames[0];
        assert_eq!(attached.id(), "FCHILD");
        assert_eq!(attached.parent_id(), Some("FROOT"));
        assert!(!attached.is_main());

        // Emit a Page.frameDetached for the same frame.
        mock.emit_event_for_session("Page.frameDetached", json!({ "frameId": "FCHILD" }), "S1")
            .await;

        for _ in 0..50 {
            if tab.inner.frames.read().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let frames_after = tab.frames().await.unwrap();
        assert!(
            frames_after.is_empty(),
            "expected registry to drain after detach event",
        );

        conn.shutdown();
    }

    // --- wait_for_idle quiet-window enforcement ------------------------

    /// End-to-end: emit a `Network.requestWillBeSent` event, then 100ms
    /// later emit `Network.responseReceived` for the same id. With a 500ms
    /// quiet window + 2s outer timeout, `wait_for_idle_with` should
    /// resolve `Ok(())` within ~600ms of the response (500ms quiet +
    /// scheduling slack). Asserts the call returns within 1.5s of the
    /// response event — a generous bound that still rejects "never
    /// resolves" without flaking on a loaded CI machine.
    #[tokio::test]
    async fn wait_for_idle_resolves_after_quiet_window_post_response() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        // Synchronize: wait until the background tracker task has run far
        // enough to issue `Network.enable`. Once that command lands in the
        // mock's outbound queue, the subscriptions are already registered
        // (created in `InFlightTracker::run` before the enable spawn) — so
        // any subsequent `emit_event_for_session` will be routed to them.
        let id_enable =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Network.enable"))
                .await
                .expect("tracker did not send Network.enable within 2s");
        mock.reply(id_enable, json!({})).await;

        // Insert via requestWillBeSent.
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({ "requestId": "R1" }),
            "S1",
        )
        .await;
        // Wait for the tracker to actually observe the insert before
        // starting the wait — otherwise wait_for_idle could see an empty
        // set on its first poll and resolve immediately.
        for _ in 0..50 {
            if tab.inner.network_tracker.in_flight.lock().await.len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            tab.inner.network_tracker.in_flight.lock().await.len(),
            1,
            "request did not register before wait_for_idle starts",
        );

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.wait_for_idle_with(Duration::from_secs(2), Duration::from_millis(500))
                    .await
            }
        });

        // Hold the request in-flight briefly, then close it. After this
        // emit, the tracker drains to empty and the 500ms quiet window
        // starts ticking.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let response_at = tokio::time::Instant::now();
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({ "requestId": "R1" }),
            "S1",
        )
        .await;

        let res = tokio::time::timeout(Duration::from_millis(1500), fut)
            .await
            .expect("wait_for_idle did not resolve within 1500ms after response");
        res.unwrap().unwrap();
        let elapsed = response_at.elapsed();
        // 500ms quiet window + slack; must be at least 500ms.
        assert!(
            elapsed >= Duration::from_millis(450),
            "resolved too early ({elapsed:?}) — quiet window not enforced",
        );
        assert!(
            elapsed < Duration::from_millis(1500),
            "resolved too late ({elapsed:?})",
        );

        conn.shutdown();
    }

    /// Regression: the in-flight set going 1 → 0 → 1 within the quiet
    /// window must NOT cause `wait_for_idle_with` to resolve early. The
    /// quiet window measures sustained idleness, not a single
    /// instantaneous touch-of-zero.
    ///
    /// Sequence:
    /// 1. R1 starts (in_flight = 1).
    /// 2. R1 completes (in_flight = 0). Quiet window starts.
    /// 3. ~100ms later, well inside the 200ms quiet window, R2 starts
    ///    (in_flight = 1). Quiet window MUST reset to `None`.
    /// 4. R2 completes (in_flight = 0). New quiet window starts.
    /// 5. `wait_for_idle_with` resolves only after R2's quiet window
    ///    closes.
    ///
    /// Assertion: elapsed time from R1's response (step 2) to the future
    /// resolving is at least `delay-between-completions (~100ms) +
    /// quiet_window (200ms)`. A buggy implementation that ignored the
    /// in-window R2 burst would resolve at ~200ms and fail the lower
    /// bound.
    #[tokio::test]
    async fn wait_for_idle_does_not_return_early_if_new_request_arrives_in_quiet_window() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let id_enable =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Network.enable"))
                .await
                .expect("tracker did not send Network.enable within 2s");
        mock.reply(id_enable, json!({})).await;

        // Insert R1 and wait for the tracker to observe it before
        // starting wait_for_idle (mirrors the sibling test).
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({ "requestId": "R1" }),
            "S1",
        )
        .await;
        for _ in 0..50 {
            if tab.inner.network_tracker.in_flight.lock().await.len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            tab.inner.network_tracker.in_flight.lock().await.len(),
            1,
            "R1 did not register before wait_for_idle starts",
        );

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                // 5s outer timeout: plenty of headroom for the worst-case
                // scheduling on a loaded CI. 200ms quiet window: small
                // enough that the test finishes fast, large enough that
                // the 100ms gap fits comfortably inside it.
                t.wait_for_idle_with(Duration::from_secs(5), Duration::from_millis(200))
                    .await
            }
        });

        // Drain R1 — quiet window opens here.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let r1_response_at = tokio::time::Instant::now();
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({ "requestId": "R1" }),
            "S1",
        )
        .await;

        // ~100ms later (still inside the 200ms quiet window), insert R2.
        // A correct implementation resets quiet_start; a buggy one would
        // already be near the 200ms threshold and resolve any moment.
        tokio::time::sleep(Duration::from_millis(100)).await;
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({ "requestId": "R2" }),
            "S1",
        )
        .await;
        // Wait for the tracker to actually observe the insert before
        // closing it — otherwise R2 could complete before the tracker
        // even noticed it started, defeating the test.
        for _ in 0..50 {
            if tab
                .inner
                .network_tracker
                .in_flight
                .lock()
                .await
                .contains("R2")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            tab.inner
                .network_tracker
                .in_flight
                .lock()
                .await
                .contains("R2"),
            "R2 did not register inside quiet window",
        );

        // Hold R2 in-flight briefly, then close it. A new quiet window
        // starts from this point — wait_for_idle must wait it out.
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({ "requestId": "R2" }),
            "S1",
        )
        .await;

        let res = tokio::time::timeout(Duration::from_secs(2), fut)
            .await
            .expect("wait_for_idle did not resolve within 2s after R2 completed");
        res.unwrap().unwrap();
        let total_elapsed = r1_response_at.elapsed();

        // Lower bound: R1-response (T0) → 100ms gap → R2 starts → 50ms
        // hold → R2 response → 200ms quiet window → resolve. Total ≥
        // 350ms. A bug that ignored R2's in-window arrival would resolve
        // at T0 + 200ms = 200ms.
        assert!(
            total_elapsed >= Duration::from_millis(330),
            "wait_for_idle resolved too early ({total_elapsed:?}); R2 inside quiet \
             window must have reset quiet_start, requiring a fresh post-R2 quiet \
             window before resolving",
        );

        conn.shutdown();
    }

    // --- Tab::intercept (P5 T7, feature = "interception") -------------

    /// `tab.intercept().block("*").start()` should spawn the rule actor on
    /// the tab's session: assert `Fetch.enable` lands and a matching
    /// `Fetch.requestPaused` triggers `Fetch.failRequest`. Verifies the
    /// `Tab::intercept` shim plumbs into `InterceptBuilder` end-to-end.
    #[cfg(feature = "interception")]
    #[tokio::test]
    async fn intercept_block_all_dispatches_fail_request_via_tab_shim() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let handle = tab.intercept().block("*").unwrap().start();

        // Side-task `Fetch.enable` must land first; default match-all
        // pattern is injected when none was registered explicitly.
        let enable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.enable"))
                .await
                .expect("intercept did not send Fetch.enable within 2s");
        let enable_params = mock.last_sent()["params"].clone();
        assert_eq!(enable_params["handleAuthRequests"], false);
        assert_eq!(enable_params["patterns"][0]["urlPattern"], "*");
        mock.reply(enable_id, json!({})).await;

        // Any paused URL matches the `block("*")` rule.
        mock.emit_event_for_session(
            "Fetch.requestPaused",
            json!({
                "requestId": "REQ-1",
                "request": {
                    "url": "https://any.test/whatever",
                    "method": "GET",
                    "headers": {},
                },
                "resourceType": "Document",
            }),
            "S1",
        )
        .await;

        let fail_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.failRequest"))
                .await
                .expect("actor did not send Fetch.failRequest within 2s");
        let fail_params = mock.last_sent()["params"].clone();
        assert_eq!(fail_params["requestId"], "REQ-1");
        assert_eq!(fail_params["errorReason"], "BlockedByClient");
        mock.reply(fail_id, json!({})).await;

        let stop_fut = tokio::spawn(handle.stop());
        let disable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.disable"))
                .await
                .expect("actor did not send Fetch.disable on stop()");
        mock.reply(disable_id, json!({})).await;
        stop_fut
            .await
            .expect("stop() task panicked")
            .expect("stop() returned Err");

        conn.shutdown();
    }
}
