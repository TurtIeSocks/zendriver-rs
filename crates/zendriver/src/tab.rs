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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::time::timeout;
use tracing::trace;
use zendriver_transport::{AccountedRawEvent, SessionHandle};

use crate::error::{Result, ZendriverError};
use crate::frame::Frame;
use crate::input::InputController;
use crate::isolated_world::IsolatedWorldCache;
use crate::network_idle::IdleLossPolicy;
use crate::screenshot::ScreenshotBuilder;

const DEFAULT_LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Poll cadence for [`Tab::wait_for_ready_state`]'s `document.readyState`
/// loop. Small enough to feel responsive, large enough not to spin the CDP
/// channel.
const READY_STATE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Fixed `(x, y)` viewport anchor for [`Tab::scroll_with`] gestures. A
/// constant in-viewport point keeps page scrolls deterministic and
/// single-dispatch (no `Page.getLayoutMetrics` round-trip to derive a
/// center) — the scroll *distance* is what matters, not the anchor.
const SCROLL_ANCHOR: (f64, f64) = (100.0, 100.0);

/// Per-call knobs for [`Tab::reload_with`].
///
/// `Default` reloads with `ignore_cache: false` and no injected script —
/// the same behavior as the plain [`Tab::reload`] shortcut. Set
/// `ignore_cache: true` for a hard refresh, and/or
/// `script_to_evaluate_on_load` to inject a script that runs on every frame
/// load triggered by the reload.
///
/// # Examples
///
/// ```no_run
/// # async fn ex() -> zendriver::Result<()> {
/// use zendriver::ReloadOptions;
/// # let browser = zendriver::Browser::builder().launch().await?;
/// # let tab = browser.main_tab();
/// tab.reload_with(ReloadOptions {
///     ignore_cache: true,
///     ..Default::default()
/// }).await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Default)]
pub struct ReloadOptions {
    /// Bypass the HTTP cache for the reload (`Page.reload.ignoreCache`).
    /// `false` by default — a soft refresh.
    pub ignore_cache: bool,
    /// Script source injected before any other page script on each frame
    /// loaded by the reload (`Page.reload.scriptToEvaluateOnLoad`). Omitted
    /// from the dispatch entirely when `None`.
    pub script_to_evaluate_on_load: Option<String>,
}

/// Per-call knobs for [`Tab::scroll_with`].
///
/// `dx` / `dy` are signed pixel distances forwarded verbatim to
/// `Input.synthesizeScrollGesture`'s `xDistance` / `yDistance`. Following
/// the CDP convention a **negative** `dy` scrolls the page *down* (content
/// moves up); a positive `dy` scrolls up. `speed` (px/s) plumbs through to
/// the gesture's `speed` field when `Some`, and is omitted otherwise (Chrome
/// picks its default).
///
/// For the common cases prefer the [`Tab::scroll_down`] / [`Tab::scroll_up`]
/// shortcuts, which take an unsigned pixel amount and pick the sign for you.
///
/// # Examples
///
/// ```no_run
/// # async fn ex() -> zendriver::Result<()> {
/// use zendriver::ScrollOptions;
/// # let browser = zendriver::Browser::builder().launch().await?;
/// # let tab = browser.main_tab();
/// // Scroll down 400px and right 50px at a fixed speed.
/// tab.scroll_with(ScrollOptions {
///     dx: 50.0,
///     dy: -400.0,
///     speed: Some(800),
/// }).await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Default)]
pub struct ScrollOptions {
    /// Horizontal scroll distance in pixels (`synthesizeScrollGesture.xDistance`).
    pub dx: f64,
    /// Vertical scroll distance in pixels (`synthesizeScrollGesture.yDistance`).
    /// Negative scrolls the page down (CDP convention); positive scrolls up.
    pub dy: f64,
    /// Optional gesture speed in pixels/second (`synthesizeScrollGesture.speed`).
    /// Omitted from the dispatch when `None`.
    pub speed: Option<i64>,
}

/// Runtime user-agent override for [`Tab::set_user_agent_with`].
///
/// `accept_language` / `platform` are optional refinements — leave them
/// `None` to override only the UA string. Each `None` field is omitted from
/// the `Emulation.setUserAgentOverride` dispatch entirely (not sent as
/// `null`).
///
/// # Stealth interaction (read before using under a stealth profile)
///
/// The active [`StealthProfile`](zendriver_stealth::StealthProfile) observer
/// already issues `Emulation.setUserAgentOverride` carrying a coherent
/// `userAgentMetadata` block (UA Client-Hints). This override is
/// **last-write-wins and sends NO `userAgentMetadata`**, so applying it under
/// the Spoofed profile *clobbers* that Client-Hints coherence and can
/// *increase* fingerprint detectability (the UA string and the UA-CH high
/// entropy values would disagree). For stealth, prefer setting the UA through
/// the stealth profile instead. Use this for non-stealth tabs or a deliberate
/// per-tab UA change where you accept the coherence trade-off.
///
/// # Examples
///
/// ```no_run
/// # async fn ex() -> zendriver::Result<()> {
/// use zendriver::UserAgentOverride;
/// # let browser = zendriver::Browser::builder().launch().await?;
/// # let tab = browser.main_tab();
/// tab.set_user_agent_with(UserAgentOverride {
///     user_agent: "Mozilla/5.0 (custom) Gecko/20100101 Firefox/123.0".into(),
///     accept_language: Some("en-US,en;q=0.9".into()),
///     platform: Some("Linux x86_64".into()),
/// }).await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Default)]
pub struct UserAgentOverride {
    /// Full `User-Agent` request-header / `navigator.userAgent` string
    /// (`Emulation.setUserAgentOverride.userAgent`). Required.
    pub user_agent: String,
    /// `Accept-Language` header + `navigator.language(s)` override
    /// (`Emulation.setUserAgentOverride.acceptLanguage`). Omitted when `None`.
    pub accept_language: Option<String>,
    /// `navigator.platform` override
    /// (`Emulation.setUserAgentOverride.platform`). Omitted when `None`.
    pub platform: Option<String>,
}

/// Document load milestone for [`Tab::wait_for_ready_state`].
///
/// Maps to the three values of the DOM `document.readyState` property and
/// orders them by progress: `Loading` < `Interactive` < `Complete`. Passing a
/// target to `wait_for_ready_state` polls until the document has reached *at
/// least* that milestone — e.g. waiting for `Interactive` also returns once
/// the page is fully `Complete`.
///
/// # Examples
///
/// ```no_run
/// # async fn ex() -> zendriver::Result<()> {
/// use zendriver::ReadyState;
/// # let browser = zendriver::Browser::builder().launch().await?;
/// # let tab = browser.main_tab();
/// tab.goto("https://example.com").await?;
/// tab.wait_for_ready_state(ReadyState::Complete).await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReadyState {
    /// `document.readyState === "loading"` — the document is still parsing.
    #[serde(rename = "loading")]
    Loading,
    /// `document.readyState === "interactive"` — parsed, but sub-resources
    /// (images, stylesheets, frames) may still be loading.
    #[serde(rename = "interactive")]
    Interactive,
    /// `document.readyState === "complete"` — the document and all
    /// sub-resources have finished loading (the `load` event has fired).
    #[serde(rename = "complete")]
    Complete,
}

impl ReadyState {
    /// Monotonic progress rank: `Loading` = 0 < `Interactive` = 1 <
    /// `Complete` = 2. Used by [`Tab::wait_for_ready_state`] to decide whether
    /// the observed state has reached the requested milestone.
    fn rank(self) -> u8 {
        match self {
            ReadyState::Loading => 0,
            ReadyState::Interactive => 1,
            ReadyState::Complete => 2,
        }
    }

    /// Parse a raw `document.readyState` string into a [`ReadyState`].
    /// Returns `None` for any value other than the three documented states.
    fn from_dom_str(s: &str) -> Option<Self> {
        match s {
            "loading" => Some(ReadyState::Loading),
            "interactive" => Some(ReadyState::Interactive),
            "complete" => Some(ReadyState::Complete),
            _ => None,
        }
    }
}

/// A single resource within a frame whose content matched a
/// [`Tab::search_frame_resources`] query.
///
/// `url` is the resource's request URL (the document, a script, a stylesheet,
/// …) and `frame_id` is the CDP `frameId` of the frame that owns it. Returned
/// only for resources whose body produced at least one match for the query.
///
/// # Examples
///
/// ```no_run
/// # async fn ex() -> zendriver::Result<()> {
/// # let browser = zendriver::Browser::builder().launch().await?;
/// # let tab = browser.main_tab();
/// tab.goto("https://example.com").await?;
/// for m in tab.search_frame_resources("apiKey").await? {
///     println!("match in {} (frame {})", m.url, m.frame_id);
/// }
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameResourceMatch {
    /// Request URL of the matched resource.
    pub url: String,
    /// CDP `frameId` of the frame that owns the resource.
    pub frame_id: String,
}

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
    /// Oneshot receiver published by [`Tab::goto`] (subscribes to
    /// `Page.frameStoppedLoading` BEFORE issuing `Page.navigate` so the
    /// event can't race past) and consumed by [`Tab::wait_for_load`].
    /// `None` when there is no pending navigation — `wait_for_load` falls
    /// back to a fresh subscribe + `document.readyState` short-circuit.
    pub(crate) pending_load: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// Whether a download path / behavior has been established for this Tab
    /// (via [`Tab::set_download_path`] or a prior [`Tab::download_file`]).
    /// Mirrors nodriver's `_download_behavior` guard: [`Tab::download_file`]
    /// only installs a default `cwd/downloads` path when this is still
    /// `false`, so it never clobbers a directory the caller chose explicitly.
    pub(crate) download_behavior_set: std::sync::atomic::AtomicBool,
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

/// Tunables for [`Tab::wait_for_idle_opts`].
///
/// Construct from [`IdleOptions::default`] and override the fields you care
/// about:
///
/// ```no_run
/// # use std::time::Duration;
/// # use zendriver::IdleOptions;
/// let opts = IdleOptions {
///     max_inflight_age: Some(Duration::from_secs(5)),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct IdleOptions {
    /// Outer bound on the whole wait. [`Tab::wait_for_idle_opts`] returns
    /// [`ZendriverError::Timeout`] once it elapses. Default: 30 s.
    pub timeout: Duration,
    /// The in-flight set must stay empty for this long to count as idle.
    /// Default: 500 ms.
    pub quiet_window: Duration,
    /// Requests that have been in flight *longer than* this are ignored when
    /// judging idleness — they are treated as stuck/background (a hung beacon,
    /// long-poll, SSE stream, …) rather than active page loading. This lets
    /// idle resolve even while such a request is still technically open.
    ///
    /// `None` (the default) waits for **every** request to terminate, which is
    /// the historical behavior: a single never-completing request keeps the
    /// tab non-idle until `timeout`.
    pub max_inflight_age: Option<Duration>,
    /// How the wait reacts to a CDP event-stream delivery gap (a lagging
    /// broadcast subscriber, a reconnect, or the WebSocket dying) observed
    /// during the wait. Default: [`IdleLossPolicy::Lenient`] — a gap is
    /// tolerated and the call still resolves best-effort, matching every
    /// prior release's behavior. Opt into [`IdleLossPolicy::Strict`] to fail
    /// loudly with [`ZendriverError::EventStreamIncomplete`] instead of
    /// silently trusting a possibly-incomplete observation.
    pub loss_policy: IdleLossPolicy,
}

impl Default for IdleOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            quiet_window: Duration::from_millis(500),
            max_inflight_age: None,
            loss_policy: IdleLossPolicy::Lenient,
        }
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
        // in-flight set in response to `Network.requestWillBeSent` (insert)
        // and `loadingFailed` / `loadingFinished` (remove) events arriving on
        // this tab's session — `responseReceived` (headers only) is
        // deliberately not terminal. `wait_for_idle` reads from the same
        // `network_tracker` Arc.
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
                pending_load: tokio::sync::Mutex::new(None),
                download_behavior_set: std::sync::atomic::AtomicBool::new(false),
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

    /// Start a persistent network monitor over this tab's session.
    ///
    /// Returns a [`crate::monitor::MonitorBuilder`]; configure an optional URL
    /// filter via [`MonitorBuilder::url_pattern`](crate::monitor::MonitorBuilder::url_pattern)
    /// then call
    /// [`start()`](crate::monitor::MonitorBuilder::start) to obtain a
    /// [`crate::monitor::NetworkMonitor`] — a
    /// [`Stream`](futures::Stream)`<Item = `[`NetworkEvent`](crate::monitor::NetworkEvent)`>`
    /// over HTTP exchanges, WebSocket frames, and EventSource messages. The
    /// monitor is passive (CDP `Network` domain) — read-only; use the
    /// `interception` feature to modify requests.
    ///
    /// Dropping the returned monitor (or calling
    /// [`stop()`](crate::monitor::NetworkMonitor::stop)) cancels its background
    /// task.
    ///
    /// Gated by the `monitor` cargo feature.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use futures::StreamExt;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let mut monitor = tab.monitor().url_pattern("/api/").start().await?;
    /// tab.goto("https://example.com").await?;
    /// while let Some(event) = monitor.next().await {
    ///     if let zendriver::NetworkEvent::Http(exchange) = event {
    ///         println!("{} -> {:?}", exchange.request.url, exchange.status());
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    #[cfg(feature = "monitor")]
    pub fn monitor(&self) -> crate::monitor::MonitorBuilder {
        crate::monitor::MonitorBuilder::new(self.session().clone())
    }

    /// Make an HTTP request from the browser context (inherits cookies/CORS).
    ///
    /// Returns a [`RequestBuilder`][crate::request::RequestBuilder] that lets
    /// you set the method, URL, headers, and body. Call
    /// [`send()`][crate::request::RequestBuilder::send] to execute via
    /// in-page `fetch`, or chain
    /// [`bypass_cors()`][crate::request::RequestBuilder::bypass_cors] first to
    /// use the privileged `Network.loadNetworkResource` path (GET only).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use serde_json::json;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    ///
    /// // Simple GET
    /// let resp = tab.request().get("https://example.com/api/data").send().await?;
    /// println!("status={} body={}", resp.status(), resp.text()?);
    ///
    /// // POST with a JSON body
    /// let resp = tab
    ///     .request()
    ///     .post("https://example.com/api/echo")
    ///     .json(&json!({"key": "value"}))?
    ///     .send()
    ///     .await?;
    /// println!("status={} body={}", resp.status(), resp.text()?);
    /// # Ok(()) }
    /// ```
    pub fn request(&self) -> crate::request::RequestBuilder<'_> {
        crate::request::RequestBuilder::new(self)
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
    /// Sorted by [`Frame::id`] for a deterministic, run-to-run-stable order
    /// (the backing registry is a [`HashMap`], whose iteration order is not
    /// stable on its own) — callers relying on cross-frame result ordering
    /// (e.g. `FindBuilder::include_frames`) get consistent results across
    /// calls with the same frame set.
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
        let mut frames: Vec<Frame> = self.inner.frames.read().await.values().cloned().collect();
        frames.sort_by(|a, b| a.id().cmp(b.id()));
        Ok(frames)
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
        // Subscribe to Page.frameStoppedLoading BEFORE issuing Page.navigate.
        // The transport's event bus is a tokio broadcast channel, so a
        // subscriber created after the event has already been published can
        // never observe it. By subscribing first, then handing a oneshot to
        // `wait_for_load`, we guarantee the next load event lands in our
        // receiver regardless of how fast the page loads (e.g. localhost
        // wiremock fixtures in CI).
        let mut stream = self
            .inner
            .session
            .subscribe::<Value>("Page.frameStoppedLoading");
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            if stream.next().await.is_some() {
                let _ = tx.send(());
            }
        });
        *self.inner.pending_load.lock().await = Some(rx);

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
        // Preferred path: consume the oneshot stashed by `goto`, which
        // subscribed to `Page.frameStoppedLoading` BEFORE the navigation
        // request — guaranteed delivery.
        if let Some(rx) = self.inner.pending_load.lock().await.take() {
            timeout(DEFAULT_LOAD_TIMEOUT, rx)
                .await
                .map_err(|_| ZendriverError::Timeout(DEFAULT_LOAD_TIMEOUT))?
                .map_err(|_| ZendriverError::Navigation("page event stream closed".into()))?;
            return Ok(());
        }
        // Fallback: no pending navigation (e.g. caller invoked
        // `wait_for_load` without a preceding `goto`, or after the page
        // navigated itself). Subscribe + short-circuit on the current
        // `document.readyState` so an already-loaded page returns
        // immediately rather than blocking on a missed event.
        let mut stream = self
            .inner
            .session
            .subscribe::<Value>("Page.frameStoppedLoading");
        let ready: Option<String> = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": "document.readyState",
                    "returnByValue": true,
                }),
            )
            .await
            .ok()
            .and_then(|v| v.get("result")?.get("value")?.as_str().map(str::to_owned));
        if ready.as_deref() == Some("complete") {
            return Ok(());
        }
        timeout(DEFAULT_LOAD_TIMEOUT, stream.next())
            .await
            .map_err(|_| ZendriverError::Timeout(DEFAULT_LOAD_TIMEOUT))?
            .ok_or_else(|| ZendriverError::Navigation("page event stream closed".into()))?;
        Ok(())
    }

    /// Block until the document reaches at least the `until` load milestone.
    ///
    /// Polls `document.readyState` (via a main-world `Runtime.evaluate`) every
    /// [`READY_STATE_POLL_INTERVAL`] and returns as soon as the observed state
    /// ranks at or above `until` (see [`ReadyState`] for the
    /// `Loading < Interactive < Complete` ordering). Bounded by a 30s outer
    /// timeout, matching [`Tab::wait_for_load`].
    ///
    /// This is the finer-grained sibling of [`Tab::wait_for_load`]: use it when
    /// you want to proceed at `Interactive` (DOM parsed, scripts runnable)
    /// without waiting for every image / stylesheet that `Complete` implies.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Timeout`] if the requested state is not
    /// reached within 30s; propagates [`ZendriverError::JsException`] if the
    /// `document.readyState` read raises.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// use zendriver::ReadyState;
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// // Proceed as soon as the DOM is parsed, don't wait for sub-resources.
    /// tab.wait_for_ready_state(ReadyState::Interactive).await?;
    /// # Ok(()) }
    /// ```
    pub async fn wait_for_ready_state(&self, until: ReadyState) -> Result<()> {
        let deadline = tokio::time::Instant::now() + DEFAULT_LOAD_TIMEOUT;
        loop {
            let state: String = self.evaluate_main("document.readyState").await?;
            if let Some(observed) = ReadyState::from_dom_str(&state) {
                if observed.rank() >= until.rank() {
                    return Ok(());
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ZendriverError::Timeout(DEFAULT_LOAD_TIMEOUT));
            }
            tokio::time::sleep(READY_STATE_POLL_INTERVAL).await;
        }
    }

    /// Evaluate a JavaScript expression in an isolated world.
    ///
    /// Runs in a sandbox where page globals are NOT visible — the default for
    /// stealth-safe execution. The result is deserialized into `T`.
    ///
    /// If the cached isolated-world execution context was destroyed (e.g. by
    /// a page navigation), the cache is invalidated and the evaluation is
    /// retried once. One retry is enough: the failure mode this guards
    /// against is "navigation happened between cache-fetch and `Runtime.evaluate`",
    /// which is a one-shot race — recreating the world for the new context
    /// and re-issuing the call clears it. If the same call fails the
    /// second attempt the page has a real problem (target gone, isolated
    /// world refused to recreate) and further retries would only mask it.
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

    /// Dump a named JavaScript object (or any expression) as an untyped
    /// [`serde_json::Value`].
    ///
    /// Evaluates `obj_name` in the page main world with `returnByValue: true`
    /// and hands back the deep-serialized `result.value`. This is the untyped
    /// sibling of [`Tab::evaluate_main`] — useful for grabbing a whole object
    /// graph (`tab.js_dumps("window.performance").await?`) when you don't have
    /// a concrete Rust type to deserialize into and just want to inspect the
    /// structure.
    ///
    /// As nodriver's `js_dumps` notes: complex objects may not be fully
    /// serializable (functions, cyclic references, host objects), so the
    /// result is a best-effort snapshot, not a source of truth.
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
    /// tab.goto("https://example.com").await?;
    /// let perf = tab.js_dumps("window.performance.timing").await?;
    /// println!("{perf:#?}");
    /// # Ok(()) }
    /// ```
    pub async fn js_dumps(&self, obj_name: &str) -> Result<Value> {
        let res = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": obj_name,
                    "returnByValue": true,
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
        Ok(res
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
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

    /// Reload the tab's current page with explicit options.
    ///
    /// Dispatches `Page.reload` with the `ignoreCache` flag from `opts` and,
    /// when `opts.script_to_evaluate_on_load` is `Some`, a
    /// `scriptToEvaluateOnLoad` that runs before any other script on each
    /// frame the reload loads. The script field is omitted from the dispatch
    /// when `None`. For a plain soft refresh, use the [`Tab::reload`]
    /// shortcut.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// use zendriver::ReloadOptions;
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.reload_with(ReloadOptions {
    ///     ignore_cache: true,
    ///     script_to_evaluate_on_load: Some("window.__reloaded = true".into()),
    /// }).await?;
    /// # Ok(()) }
    /// ```
    pub async fn reload_with(&self, opts: ReloadOptions) -> Result<()> {
        let mut params = json!({ "ignoreCache": opts.ignore_cache });
        if let Some(script) = opts.script_to_evaluate_on_load {
            params["scriptToEvaluateOnLoad"] = Value::String(script);
        }
        self.call("Page.reload", params).await?;
        Ok(())
    }

    /// Full HTML source of the tab's current page.
    ///
    /// Dispatches `DOM.getDocument { depth: 0 }` to resolve the document's
    /// root `nodeId`, then `DOM.getOuterHTML { nodeId }` to serialize it.
    /// The result is the complete document markup including the doctype —
    /// the page-level analogue of [`crate::Frame::content`].
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] when Chrome's response is
    /// missing the root `nodeId` or the serialized `outerHTML`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let html = tab.content().await?;
    /// assert!(html.contains("<html"));
    /// # Ok(()) }
    /// ```
    pub async fn content(&self) -> Result<String> {
        let doc = self.call("DOM.getDocument", json!({ "depth": 0 })).await?;
        let node_id = doc["root"]["nodeId"].as_i64().ok_or_else(|| {
            ZendriverError::Navigation("DOM.getDocument missing root.nodeId".into())
        })?;
        let res = self
            .call("DOM.getOuterHTML", json!({ "nodeId": node_id }))
            .await?;
        res["outerHTML"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| ZendriverError::Navigation("DOM.getOuterHTML missing outerHTML".into()))
    }

    /// Scroll the page down by `pixels`.
    ///
    /// Dispatches `Input.synthesizeScrollGesture` anchored at a fixed
    /// viewport point with a **negative** `yDistance` of `pixels` — the CDP
    /// convention where a negative `yDistance` moves the page content up
    /// (i.e. scrolls down). For horizontal scrolling, a custom speed, or
    /// scrolling up, see [`Tab::scroll_with`] / [`Tab::scroll_up`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.scroll_down(500.0).await?;
    /// # Ok(()) }
    /// ```
    pub async fn scroll_down(&self, pixels: f64) -> Result<()> {
        self.scroll_with(ScrollOptions {
            dx: 0.0,
            dy: -pixels,
            speed: None,
        })
        .await
    }

    /// Scroll the page up by `pixels`.
    ///
    /// Mirror of [`Tab::scroll_down`] with a **positive** `yDistance` of
    /// `pixels`, which moves the page content down (scrolls up).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.scroll_down(500.0).await?;
    /// tab.scroll_up(200.0).await?;
    /// # Ok(()) }
    /// ```
    pub async fn scroll_up(&self, pixels: f64) -> Result<()> {
        self.scroll_with(ScrollOptions {
            dx: 0.0,
            dy: pixels,
            speed: None,
        })
        .await
    }

    /// Scroll the page by an explicit signed distance with optional speed.
    ///
    /// Dispatches `Input.synthesizeScrollGesture` anchored at a fixed
    /// viewport point (`x: 100, y: 100` — a stable, in-viewport anchor that
    /// avoids a `Page.getLayoutMetrics` round-trip), forwarding
    /// [`ScrollOptions::dx`] / [`ScrollOptions::dy`] to `xDistance` /
    /// `yDistance` and [`ScrollOptions::speed`] to `speed` when `Some`.
    /// Negative `dy` scrolls the page down (CDP convention).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// use zendriver::ScrollOptions;
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.scroll_with(ScrollOptions { dx: 0.0, dy: -300.0, speed: Some(1200) }).await?;
    /// # Ok(()) }
    /// ```
    pub async fn scroll_with(&self, opts: ScrollOptions) -> Result<()> {
        // Fixed in-viewport anchor for the gesture. Picking a constant point
        // keeps the call deterministic + single-dispatch (no
        // Page.getLayoutMetrics round-trip to compute a center); the scroll
        // distance, not the anchor, is what callers care about.
        let mut params = json!({
            "x": SCROLL_ANCHOR.0,
            "y": SCROLL_ANCHOR.1,
            "xDistance": opts.dx,
            "yDistance": opts.dy,
        });
        if let Some(speed) = opts.speed {
            params["speed"] = Value::from(speed);
        }
        self.call("Input.synthesizeScrollGesture", params).await?;
        Ok(())
    }

    /// Wait until the tab's network has been idle (0 in-flight requests)
    /// for 500ms, with a 30s outer timeout. Playwright `networkidle`
    /// semantics.
    ///
    /// Backed by a per-Tab in-flight network tracker that subscribes to
    /// `Network.requestWillBeSent` (insert) and the two terminal events
    /// (`loadingFinished` / `loadingFailed`, both remove).
    /// `Network.responseReceived` (headers arrived) is deliberately not
    /// terminal: headers arriving is not the same as the response body
    /// finishing, so treating it as terminal could report idle while a body
    /// was still streaming.
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
        self.wait_for_idle_opts(IdleOptions::default()).await
    }

    /// Wait until the tab's network has been idle for `quiet_window`,
    /// bounded by `timeout`. Convenience wrapper over
    /// [`Tab::wait_for_idle_opts`] with no stuck-request eviction
    /// (`max_inflight_age: None`).
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
        self.wait_for_idle_opts(IdleOptions {
            timeout,
            quiet_window,
            max_inflight_age: None,
            ..Default::default()
        })
        .await
    }

    /// Wait for network idle with full control over the policy via
    /// [`IdleOptions`].
    ///
    /// Algorithm: poll the in-flight set with a `Notify`-driven wake (or a
    /// 50ms fallback tick). Each iteration computes the number of *active*
    /// requests — every in-flight request when
    /// [`IdleOptions::max_inflight_age`] is `None`, otherwise only those in
    /// flight for less than that age (older ones are treated as stuck /
    /// background and ignored). Track `quiet_start = Some(now)` on the first
    /// observation of zero active requests; reset to `None` whenever the active
    /// count is non-zero or a membership change fires. Return once
    /// `now - quiet_start >= quiet_window`.
    ///
    /// The 50ms tick bounds latency both for the already-idle case (no further
    /// events fire) and for an age-out crossing (which emits no CDP event), so
    /// worst-case latency to detect "stayed idle long enough" is
    /// `quiet_window + 50ms`.
    ///
    /// Under [`IdleOptions::loss_policy`] = [`IdleLossPolicy::Strict`], the
    /// wait additionally races against this tab's connection's accounted
    /// event stream; the first `Lagged` / `Reconnected` / `Disconnected`
    /// boundary observed while the wait is in progress aborts it early with
    /// [`ZendriverError::EventStreamIncomplete`]. Under the default
    /// [`IdleLossPolicy::Lenient`] this extra stream is never subscribed to,
    /// so a Lenient wait costs nothing beyond what the tracker's own
    /// best-effort subscription already pays.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Timeout`] (carrying [`IdleOptions::timeout`])
    /// once the outer deadline elapses. Returns
    /// [`ZendriverError::EventStreamIncomplete`] under
    /// [`IdleLossPolicy::Strict`] if a delivery gap is observed before the
    /// network settles; [`IdleLossPolicy::Lenient`] (the default) never
    /// returns this variant.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # use zendriver::IdleOptions;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// // Resolve even if a beacon / long-poll stays open past 5s.
    /// tab.wait_for_idle_opts(IdleOptions {
    ///     max_inflight_age: Some(Duration::from_secs(5)),
    ///     ..Default::default()
    /// }).await?;
    /// # Ok(()) }
    /// ```
    pub async fn wait_for_idle_opts(&self, opts: IdleOptions) -> Result<()> {
        let IdleOptions {
            timeout,
            quiet_window,
            max_inflight_age,
            loss_policy,
        } = opts;
        let tracker = self.inner.network_tracker.clone();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut quiet_start: Option<tokio::time::Instant> = None;

        // Under `Strict`, subscribe to this tab's connection's accounted
        // event stream for the lifetime of this call so a delivery gap
        // during the wait aborts it instead of silently reporting a
        // possibly-wrong idle. Under `Lenient` we deliberately never call
        // `subscribe_raw_accounted` — the accounted bus only pays its
        // per-event clone/send cost when it has a live subscriber, so a
        // Lenient wait stays as cheap as it always was.
        let mut loss_boundary: std::pin::Pin<
            Box<dyn futures::Stream<Item = AccountedRawEvent> + Send>,
        > = match loss_policy {
            IdleLossPolicy::Strict => {
                Box::pin(self.session().connection().subscribe_raw_accounted())
            }
            IdleLossPolicy::Lenient => Box::pin(futures::stream::pending()),
        };

        loop {
            // Arm the notification interest BEFORE reading the in-flight set
            // so a notification fired between the read and the `select!`
            // below is still delivered. `Notify::notified()` only catches
            // notifications fired after the future has been `enable()`d, so
            // doing it the other way around would let a request that started
            // *and finished* inside the quiet window slip past us with a
            // sustained count of 0 — `wait_for_idle` would return early.
            let notif = tracker.notifier.notified();
            tokio::pin!(notif);
            notif.as_mut().enable();

            // "Active" = requests that still count toward busy-ness. With
            // `max_inflight_age` set, a request in flight longer than that age
            // is treated as stuck/background and excluded, so a never-
            // terminating request can no longer pin the tab non-idle forever.
            let active_count = {
                let set = tracker.in_flight.lock().await;
                match max_inflight_age {
                    None => set.len(),
                    Some(age) => {
                        let now = tokio::time::Instant::now();
                        set.values()
                            .filter(|inserted| now.duration_since(**inserted) < age)
                            .count()
                    }
                }
            };
            if active_count == 0 {
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
                () = notif => {
                    // A membership change fired since we armed `notif`. Reset
                    // the quiet window — even if the set is back to zero by
                    // the next iteration, real activity occurred during this
                    // window so it doesn't count as "idle".
                    quiet_start = None;
                }
                Some(boundary) = loss_boundary.next() => {
                    match boundary {
                        AccountedRawEvent::Lagged { .. }
                        | AccountedRawEvent::Reconnected { .. }
                        | AccountedRawEvent::Disconnected { .. } => {
                            // Under `Lenient` this branch is unreachable —
                            // `loss_boundary` is `stream::pending()`, which
                            // never yields. Under `Strict` we can no longer
                            // prove nothing relevant to idleness was missed,
                            // so refuse to report a possibly-wrong idle.
                            return Err(ZendriverError::EventStreamIncomplete);
                        }
                        AccountedRawEvent::Event { .. } => {
                            // Delivered in order — no loss, keep waiting.
                        }
                    }
                }
            }
        }
    }

    /// Override this tab's user-agent string at runtime.
    ///
    /// Dispatches `Emulation.setUserAgentOverride { userAgent }`. Convenience
    /// shortcut over [`Tab::set_user_agent_with`] for the UA-only case (no
    /// `acceptLanguage` / `platform`).
    ///
    /// # Stealth warning
    ///
    /// This is **last-write-wins** over the stealth observer's own UA override
    /// and sends NO `userAgentMetadata`, so under the Spoofed stealth profile
    /// it clobbers the UA Client-Hints coherence the profile set up and can
    /// *increase* detectability. Prefer the stealth profile's UA for stealth;
    /// use this for non-stealth tabs or a deliberate per-tab UA change. See
    /// [`UserAgentOverride`] for the full rationale.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.set_user_agent("Mozilla/5.0 (compatible; MyBot/1.0)").await?;
    /// # Ok(()) }
    /// ```
    pub async fn set_user_agent(&self, user_agent: impl Into<String>) -> Result<()> {
        self.set_user_agent_with(UserAgentOverride {
            user_agent: user_agent.into(),
            ..Default::default()
        })
        .await
    }

    /// Override this tab's user-agent (and optionally `Accept-Language` /
    /// platform) at runtime.
    ///
    /// Dispatches `Emulation.setUserAgentOverride` with `userAgent` always set
    /// and `acceptLanguage` / `platform` included only when the corresponding
    /// [`UserAgentOverride`] field is `Some` (omitted, not `null`, otherwise).
    /// For the UA-only case use the [`Tab::set_user_agent`] shortcut.
    ///
    /// # Stealth warning
    ///
    /// Same caveat as [`Tab::set_user_agent`]: this is last-write-wins and
    /// sends no `userAgentMetadata`, so under the Spoofed stealth profile it
    /// clobbers UA Client-Hints coherence. See [`UserAgentOverride`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// use zendriver::UserAgentOverride;
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.set_user_agent_with(UserAgentOverride {
    ///     user_agent: "Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/123.0".into(),
    ///     accept_language: Some("de-DE,de;q=0.9".into()),
    ///     platform: Some("Linux x86_64".into()),
    /// }).await?;
    /// # Ok(()) }
    /// ```
    pub async fn set_user_agent_with(&self, ovr: UserAgentOverride) -> Result<()> {
        let mut params = json!({ "userAgent": ovr.user_agent });
        if let Some(lang) = ovr.accept_language {
            params["acceptLanguage"] = Value::String(lang);
        }
        if let Some(platform) = ovr.platform {
            params["platform"] = Value::String(platform);
        }
        self.call("Emulation.setUserAgentOverride", params).await?;
        Ok(())
    }

    /// Move the cursor to `(x, y)` in viewport coordinates along a realistic
    /// Bezier path.
    ///
    /// Tab-level analogue of [`crate::Element::hover`] for an arbitrary
    /// coordinate (no element required) — useful for canvas/widget targets
    /// and CAPTCHA-checkbox flows where the hit point is a fixed pixel rather
    /// than a DOM node. Emits a sequence of `Input.dispatchMouseEvent
    /// { type: "mouseMoved" }` calls and advances the per-tab cursor state.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.mouse_move(120.0, 240.0).await?;
    /// # Ok(()) }
    /// ```
    pub async fn mouse_move(&self, x: f64, y: f64) -> Result<()> {
        let input = self.input().clone();
        crate::input::mouse::move_realistic(&input, self, x, y).await
    }

    /// Click at `(x, y)` in viewport coordinates: a left, single, realistic
    /// click.
    ///
    /// Moves the cursor to the point along a Bezier path, then emits the
    /// `mousePressed` + `mouseReleased` pair. Tab-level analogue of
    /// [`crate::Element::click`] for a raw coordinate. For right-click /
    /// modifier-held / double-click / raw-teleport variants use
    /// [`Tab::mouse_click_with`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.mouse_click(120.0, 240.0).await?;
    /// # Ok(()) }
    /// ```
    pub async fn mouse_click(&self, x: f64, y: f64) -> Result<()> {
        let input = self.input().clone();
        crate::input::mouse::click_at(
            &input,
            self,
            x,
            y,
            crate::input::mouse::MouseButton::Left,
            1,
            true,
        )
        .await
    }

    /// Click at `(x, y)` in viewport coordinates with explicit
    /// [`crate::ClickOptions`].
    ///
    /// Maps `opts.button` / `opts.click_count` / `opts.realistic` onto the
    /// dispatch. Unlike [`crate::Element::click_with`], there is no element to
    /// gate on, so `opts.force` and `opts.position` are ignored — the click
    /// lands at the supplied `(x, y)` regardless. Use this for right-clicks /
    /// modifier-held clicks / double-clicks / raw teleports at a coordinate.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// use zendriver::{ClickOptions, MouseButton};
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.mouse_click_with(50.0, 60.0, ClickOptions {
    ///     button: MouseButton::Right,
    ///     ..Default::default()
    /// }).await?;
    /// # Ok(()) }
    /// ```
    pub async fn mouse_click_with(
        &self,
        x: f64,
        y: f64,
        opts: crate::element::actions::ClickOptions,
    ) -> Result<()> {
        let input = self.input().clone();
        crate::input::mouse::click_at(
            &input,
            self,
            x,
            y,
            opts.button,
            opts.click_count,
            opts.realistic,
        )
        .await
    }

    /// Tap at `(x, y)` in viewport coordinates.
    ///
    /// Dispatches a bare `Input.dispatchTouchEvent` `touchStart` (one touch
    /// point at `(x, y)`) followed by `touchEnd` (empty `touchPoints` — the
    /// CDP contract for a lifted finger). Tab-level analogue of
    /// [`crate::Element::tap`] for a raw coordinate — same relationship
    /// [`Tab::mouse_click`] has to [`crate::Element::click`].
    ///
    /// No `Emulation.setTouchEmulationEnabled` call precedes the dispatch:
    /// the bare `dispatchTouchEvent` already fires `touchstart`/`touchend`
    /// page-side handlers (and, on a clickable element, the browser's own
    /// synthesized `click`), which is what a tap needs. Touch-*capability*
    /// emulation — `'ontouchstart' in window`, `navigator.maxTouchPoints`,
    /// `matchMedia('(pointer: coarse)')` — is a separate concern that
    /// belongs with mobile device emulation; not doing it here is
    /// intentional, not a bug.
    ///
    /// Scope is touch only: no pressure / pen / stylus / tilt input.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.tap(120.0, 240.0).await?;
    /// # Ok(()) }
    /// ```
    pub async fn tap(&self, x: f64, y: f64) -> Result<()> {
        crate::input::touch::tap_at(self, x, y).await
    }

    /// Flash a transient red dot at `(x, y)` for ~1 second — a visual debug
    /// aid.
    ///
    /// Injects a small absolutely-positioned `<div>` at the viewport
    /// coordinate via [`Tab::evaluate_main`], then schedules its own removal
    /// after roughly a second. Handy for eyeballing where [`Tab::mouse_click`]
    /// / [`Tab::mouse_move`] targets land when debugging coordinate math
    /// against a headful Chrome. Has no effect on input state and is not
    /// intended for production paths.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.flash_point(120.0, 240.0).await?;
    /// # Ok(()) }
    /// ```
    pub async fn flash_point(&self, x: f64, y: f64) -> Result<()> {
        // Build the dot in the page main world so it's painted into the real
        // document the user is watching. `returnByValue` short-circuits to
        // undefined; we only care about the side effect.
        let js = format!(
            "(() => {{ \
                const d = document.createElement('div'); \
                d.style.cssText = 'position:fixed;left:{x}px;top:{y}px;width:10px;height:10px;\
margin:-5px 0 0 -5px;border-radius:50%;background:red;z-index:2147483647;\
pointer-events:none;opacity:0.85;'; \
                document.body.appendChild(d); \
                setTimeout(() => d.remove(), 1000); \
            }})()"
        );
        let _: Value = self.evaluate_main(js).await?;
        Ok(())
    }

    /// Drag the mouse from `from` to `to` with the left button held, moving in
    /// `steps` interpolated hops.
    ///
    /// Ports nodriver's `Tab.mouse_drag`: emits a `mousePressed` (left button)
    /// at `from`, then a sequence of `mouseMoved` events linearly interpolated
    /// toward `to`, then a `mouseReleased` (left) at `to`. A larger `steps`
    /// makes the drag look smoother / more human (nodriver suggests 50–100 for
    /// "very smooth"); `steps` of 0 or 1 collapses to a single move straight to
    /// `to`.
    ///
    /// This is the raw-coordinate, linearly-interpolated drag (faithful to
    /// nodriver) — it does **not** use the realistic Bezier path that
    /// [`Tab::mouse_move`] / [`crate::Element::click`] do, and it dispatches
    /// directly rather than advancing the per-Tab cursor state.
    ///
    /// # Errors
    ///
    /// Propagates [`ZendriverError::Transport`] / `Cdp` from the underlying
    /// `Input.dispatchMouseEvent` calls.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// // Drag a slider handle 200px to the right over 40 smooth steps.
    /// tab.mouse_drag((120.0, 300.0), (320.0, 300.0), 40).await?;
    /// # Ok(()) }
    /// ```
    pub async fn mouse_drag(&self, from: (f64, f64), to: (f64, f64), steps: usize) -> Result<()> {
        // Press the left button at the source point.
        self.call(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mousePressed",
                "x": from.0, "y": from.1,
                "button": "left",
                "clickCount": 1,
            }),
        )
        .await?;

        // Interpolate the move. nodriver walks i in 0..=steps (steps+1 points,
        // the first coinciding with `from`); steps <= 1 collapses to a single
        // hop straight to `to`.
        let steps = steps.max(1);
        if steps == 1 {
            self.call(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseMoved", "x": to.0, "y": to.1 }),
            )
            .await?;
        } else {
            let step_x = (to.0 - from.0) / steps as f64;
            let step_y = (to.1 - from.1) / steps as f64;
            for i in 0..=steps {
                let x = from.0 + step_x * i as f64;
                let y = from.1 + step_y * i as f64;
                self.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": "mouseMoved", "x": x, "y": y }),
                )
                .await?;
            }
        }

        // Release at the destination.
        self.call(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseReleased",
                "x": to.0, "y": to.1,
                "button": "left",
                "clickCount": 1,
            }),
        )
        .await?;
        Ok(())
    }

    /// Search the text content of every loaded frame resource for `query`,
    /// returning the resources that contain at least one match.
    ///
    /// Ports nodriver's `search_frame_resources`. Fetches the page's resource
    /// tree (`Page.getResourceTree`), walks every frame and its resources
    /// (recursing into child frames), and for each resource runs
    /// `Page.searchInResource { frameId, url, query }`. Resources whose search
    /// returns a non-empty match list are collected into the result as
    /// [`FrameResourceMatch`] records (resource URL + owning frame id).
    ///
    /// `query` is treated as a plain substring by Chrome (the underlying
    /// `searchInResource` defaults to non-regex, case-sensitive matching).
    /// Resources that error on search (e.g. a body Chrome no longer retains)
    /// are skipped rather than failing the whole call.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] if `Page.getResourceTree`'s
    /// response is missing the frame tree; transport errors from
    /// `getResourceTree` itself propagate.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let hits = tab.search_frame_resources("__INITIAL_STATE__").await?;
    /// println!("{} resources matched", hits.len());
    /// # Ok(()) }
    /// ```
    pub async fn search_frame_resources(&self, query: &str) -> Result<Vec<FrameResourceMatch>> {
        let tree = self.call("Page.getResourceTree", json!({})).await?;
        let root = tree.get("frameTree").ok_or_else(|| {
            ZendriverError::Navigation("Page.getResourceTree missing frameTree".into())
        })?;

        // Flatten the tree into (frame_id, resource_url) pairs. The frame node
        // carries its id under `frame.id`; resources live in `resources[]`
        // (each with a `url`), and nested frames in `childFrames[]` (each a
        // FrameResourceTree of the same shape).
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let frame_id = node["frame"]["id"].as_str().unwrap_or("").to_string();
            if let Some(resources) = node.get("resources").and_then(Value::as_array) {
                for res in resources {
                    if let Some(url) = res.get("url").and_then(Value::as_str) {
                        pairs.push((frame_id.clone(), url.to_string()));
                    }
                }
            }
            if let Some(children) = node.get("childFrames").and_then(Value::as_array) {
                stack.extend(children.iter());
            }
        }

        // Search each resource; collect the ones with a non-empty match list.
        let mut matches = Vec::new();
        for (frame_id, url) in pairs {
            let res = self
                .call(
                    "Page.searchInResource",
                    json!({ "frameId": frame_id, "url": url, "query": query }),
                )
                .await;
            // Skip resources Chrome can't search (stale body, unsupported
            // type) rather than aborting the whole sweep.
            let Ok(res) = res else { continue };
            let has_match = res
                .get("result")
                .and_then(Value::as_array)
                .is_some_and(|m| !m.is_empty());
            if has_match {
                matches.push(FrameResourceMatch { url, frame_id });
            }
        }
        Ok(matches)
    }

    /// Bring this tab's page to the front of its browser window.
    ///
    /// Dispatches `Page.bringToFront` on this tab's session. Distinct from
    /// [`Tab::activate`], which sends the browser-scope
    /// `Target.activateTarget` to switch the active tab — `bring_to_front`
    /// raises the page within its window (focus + paint) at session scope.
    /// nodriver exposes both; they serve slightly different purposes, so
    /// zendriver keeps both.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.bring_to_front().await?;
    /// # Ok(()) }
    /// ```
    pub async fn bring_to_front(&self) -> Result<()> {
        self.call("Page.bringToFront", json!({})).await?;
        Ok(())
    }

    /// Dismiss Chrome's "Your connection is not private" interstitial by
    /// typing the magic bypass phrase.
    ///
    /// On the SSL/cert warning page, Chrome accepts the literal keystrokes
    /// `thisisunsafe` (typed anywhere with the page focused) as a proceed
    /// gesture. This focuses the page `<body>` and fast-types that phrase.
    /// No-op-ish on normal pages (the keystrokes go to the body and are
    /// harmless). Mirrors nodriver's `select("body").send_keys("thisisunsafe")`.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::ElementNotFound`] if the page has no `<body>`
    /// to focus (should not happen on a real interstitial).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://self-signed.badssl.com").await?;
    /// tab.bypass_insecure_connection_warning().await?;
    /// # Ok(()) }
    /// ```
    pub async fn bypass_insecure_connection_warning(&self) -> Result<()> {
        let body = self.find().css("body").one().await?;
        body.type_text_fast("thisisunsafe").await
    }

    /// Compose the DevTools front-end "inspector" URL for this tab.
    ///
    /// Returns a string of the form
    /// `http://{host}:{port}/devtools/inspector.html?ws={host}:{port}/devtools/page/{target_id}`,
    /// where `{host}:{port}` is the remote-debugging endpoint the owning
    /// [`crate::Browser`] connected to (parsed from Chrome's
    /// `DevTools listening on ws://HOST:PORT/...` launch line). Open the
    /// returned URL in any Chromium browser to attach the DevTools UI to this
    /// page. This returns the URL only — it does not launch a browser.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] when the owning browser has been
    /// dropped or its debug endpoint is unknown (e.g. a Tab constructed
    /// outside of a real `launch`, or a future transport that doesn't surface
    /// the endpoint).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let url = tab.inspector_url()?;
    /// println!("open this to inspect: {url}");
    /// # Ok(()) }
    /// ```
    pub fn inspector_url(&self) -> Result<String> {
        let browser = self.inner.browser.upgrade().ok_or_else(|| {
            ZendriverError::Navigation("inspector_url: owning Browser has been dropped".into())
        })?;
        let host_port = browser.debug_host_port.as_deref().ok_or_else(|| {
            ZendriverError::Navigation(
                "inspector_url: browser debug endpoint not known (not launched via Browser::builder?)".into(),
            )
        })?;
        let target_id = self.target_id();
        Ok(format!(
            "http://{host_port}/devtools/inspector.html?ws={host_port}/devtools/page/{target_id}"
        ))
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

    /// Find one element by CSS selector. Python-parity convenience for
    /// `find().css(sel).one()`. For modifiers (frames / nth / timeout) use the
    /// builder directly.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::ElementNotFound`] if no element matches.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let h1 = tab.select("h1").await?;
    /// # let _ = h1;
    /// # Ok(()) }
    /// ```
    pub async fn select(&self, css: &str) -> crate::error::Result<crate::Element> {
        self.find().css(css).one().await
    }

    /// Find all elements by CSS selector. Python-parity convenience for
    /// `find_all().css(sel).many()`.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::ElementNotFound`] if no elements match.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let links = tab.select_all("a").await?;
    /// println!("{} links", links.len());
    /// # Ok(()) }
    /// ```
    pub async fn select_all(&self, css: &str) -> crate::error::Result<Vec<crate::Element>> {
        self.find_all().css(css).many().await
    }

    /// Collect every linked URL on the page — the `href` of `[href]` elements
    /// (`<a>`, `<link>`, `<area>`, …) and the `src` of `[src]` elements
    /// (`<img>`, `<script>`, `<iframe>`, …).
    ///
    /// When `absolute` is `true` the URLs are read from each element's
    /// `.href` / `.src` DOM properties, which the browser has already resolved
    /// against the document base URL (so a relative `href="/a"` comes back as
    /// `https://host/a`). When `false` the raw attribute strings are returned
    /// verbatim (often relative, as authored). Empty / missing values are
    /// skipped.
    ///
    /// Mirrors nodriver's `get_all_urls`. This is the cheap string-list view;
    /// for live [`Element`](crate::Element) handles to those nodes use
    /// [`Tab::get_all_linked_sources`].
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::JsException`] if the collector script raises.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let urls = tab.get_all_urls(true).await?;
    /// for u in urls {
    ///     println!("{u}");
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn get_all_urls(&self, absolute: bool) -> Result<Vec<String>> {
        // Read `.href` / `.src` DOM props (browser-resolved → absolute) when
        // `absolute`, else the raw `getAttribute` values. A single main-world
        // collector walks `[href], [src]` once and filters out empties.
        let js = format!(
            "(() => {{ \
                const out = []; \
                const abs = {absolute}; \
                for (const el of document.querySelectorAll('[href], [src]')) {{ \
                    const v = abs \
                        ? (el.href || el.src || '') \
                        : (el.getAttribute('href') || el.getAttribute('src') || ''); \
                    if (v) out.push(v); \
                }} \
                return out; \
            }})()"
        );
        self.evaluate_main(js).await
    }

    /// Live [`Element`](crate::Element) handles for every linked-source node on
    /// the page (`[src], [href]`).
    ///
    /// Routes through [`Tab::find_all`] with a `[src], [href]` CSS selector and
    /// terminates with `many_or_empty`, so a page with no such elements yields
    /// an empty `Vec` rather than an error. Mirrors nodriver's
    /// `get_all_linked_sources`; unlike [`Tab::get_all_urls`] (which returns
    /// plain URL strings) this hands back interactable element handles you can
    /// click / screenshot / inspect.
    ///
    /// # Errors
    ///
    /// Propagates query/transport errors from the underlying
    /// [`Tab::find_all`] resolution.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let assets = tab.get_all_linked_sources().await?;
    /// println!("{} linked sources", assets.len());
    /// # Ok(()) }
    /// ```
    pub async fn get_all_linked_sources(&self) -> Result<Vec<crate::Element>> {
        self.find_all().css("[src], [href]").many_or_empty().await
    }

    /// Route this browser's downloads into `dir` at runtime, keeping each
    /// file's server-suggested name.
    ///
    /// Dispatches `Browser.setDownloadBehavior { behavior: "allow",
    /// downloadPath: <dir> }` at **browser scope** (no `sessionId`) — the
    /// connection beneath every tab is the same, and Chrome does not honor
    /// per-session download behavior reliably across versions, so the policy
    /// applies browser-wide. `dir` must already exist; Chrome writes files
    /// there under the names it would have used in the user's downloads
    /// folder.
    ///
    /// This is distinct from [`Tab::expect_download`]
    /// (gated by the `expect` feature), which configures `allowAndName`
    /// against a private tempdir to *capture* a single download for
    /// `await` + `save_to`. Use `set_download_path` when you just want
    /// downloads to land in a known directory with their natural names;
    /// use `expect_download` when you want to await and inspect one.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Transport`] / `Cdp` if the CDP call fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.set_download_path("/tmp/downloads").await?;
    /// # Ok(()) }
    /// ```
    pub async fn set_download_path(&self, dir: impl Into<PathBuf>) -> Result<()> {
        let dir = dir.into();
        self.inner
            .session
            .connection()
            .call_raw(
                "Browser.setDownloadBehavior",
                json!({
                    "behavior": "allow",
                    "downloadPath": dir.to_string_lossy().to_string(),
                }),
                None,
            )
            .await?;
        self.inner
            .download_behavior_set
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Download the resource at `url` into the tab's download directory,
    /// driven entirely from the page itself.
    ///
    /// Ports nodriver's `download_file` mechanism: it injects a main-world
    /// script that `fetch`es `url`, wraps the response body in a `Blob`,
    /// creates an object URL, and clicks a synthetic `<a download>` anchor —
    /// so the bytes flow through the page's own network context (cookies,
    /// referer, same-origin credentials) and Chrome saves them via the
    /// configured download behavior. The anchor is removed and the object URL
    /// revoked shortly after the click.
    ///
    /// If no download directory has been set on this tab (via
    /// [`Tab::set_download_path`] or an earlier `download_file`), a default of
    /// `<cwd>/downloads` is created and installed first — matching nodriver.
    /// When `filename` is `None` the saved name is derived from the URL's last
    /// path segment (query string stripped).
    ///
    /// Returns once the injection script has been dispatched — it does **not**
    /// wait for the download to finish. For await/inspect semantics use
    /// [`Tab::expect_download`] (gated by the `expect` feature).
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Io`] if the default download directory cannot
    /// be created; propagates [`ZendriverError::JsException`] if the injected
    /// fetch/anchor script raises (e.g. a CORS-blocked `fetch`).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.set_download_path("/tmp/dl").await?;
    /// tab.download_file("https://example.com/file.pdf", None).await?;
    /// # Ok(()) }
    /// ```
    pub async fn download_file(
        &self,
        url: impl Into<String>,
        filename: Option<PathBuf>,
    ) -> Result<()> {
        let url = url.into();

        // Establish a default download directory if the caller never set one
        // (mirrors nodriver's `_download_behavior` guard so we don't clobber
        // an explicitly-chosen directory).
        if !self
            .inner
            .download_behavior_set
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let dir = std::env::current_dir()?.join("downloads");
            std::fs::create_dir_all(&dir)?;
            self.set_download_path(dir).await?;
        }

        // Derive the saved filename from the URL tail (query stripped) when
        // not supplied, matching nodriver.
        let filename = filename
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                url.rsplit('/')
                    .next()
                    .unwrap_or("")
                    .split('?')
                    .next()
                    .unwrap_or("")
                    .to_string()
            });

        // Inject the fetch→blob→anchor[download]→click sequence into the page
        // main world (the document's own network context). Values are JSON-
        // encoded so quotes/specials in the URL or filename can't break out.
        let url_lit = serde_json::to_string(&url)?;
        let name_lit = serde_json::to_string(&filename)?;
        let js = format!(
            "(async () => {{ \
                const response = await fetch({url_lit}); \
                const blob = await response.blob(); \
                const href = URL.createObjectURL(blob); \
                const a = document.createElement('a'); \
                a.href = href; \
                a.download = {name_lit}; \
                document.body.appendChild(a); \
                a.click(); \
                setTimeout(() => {{ document.body.removeChild(a); URL.revokeObjectURL(href); }}, 500); \
            }})()"
        );
        let _: Value = self.evaluate_main(js).await?;
        Ok(())
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

    /// Register a one-shot expectation for the first `Page.fileChooserOpened`
    /// on this tab, and answer it with `paths` — bypassing the OS file
    /// dialog the same way [`Element::upload_files`](crate::Element::upload_files)
    /// does, but for **button/label-triggered** pickers (a hidden
    /// `<input type="file">` clicked from a JS handler, or any custom
    /// widget that ultimately opens one) that `upload_files` can't reach
    /// because it only knows how to target a direct `<input type="file">`'s
    /// backend node.
    ///
    /// `paths` is captured up front (converted via `to_string_lossy()`,
    /// matching CDP's UTF-8 string contract) and applied automatically the
    /// instant the chooser opens.
    ///
    /// `async` because `Page.setInterceptFileChooserDialog { enabled: true
    /// }` must reach Chrome before the caller's next line — usually the
    /// click that opens the picker — or that click could race Chrome into
    /// showing the real OS dialog instead of firing the intercept event.
    /// Same reasoning as [`Tab::expect_download`]'s own setup call.
    ///
    /// The returned
    /// [`FileChooserExpectation`](crate::expect::file_chooser::FileChooserExpectation)
    /// is awaitable directly (`expectation.await`) or via the
    /// Playwright-style `expectation.matched().await`; configure the
    /// timeout via
    /// [`timeout`](crate::expect::file_chooser::FileChooserExpectation::timeout)
    /// before awaiting. If the expectation is dropped before a chooser
    /// opens (timeout, early return, panic unwind), the intercept is
    /// disabled best-effort so a later real dialog isn't silently
    /// swallowed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let fc = tab.expect_file_chooser(&["/tmp/photo.jpg"]).await?;
    /// let button = tab.find().css("#upload-btn").one().await?;
    /// button.click().await?; // opens the picker via a hidden input
    /// fc.await?; // intercepts fileChooserOpened + sets the files
    /// # Ok(()) }
    /// ```
    ///
    /// Gated by the `expect` cargo feature.
    pub async fn expect_file_chooser<P: AsRef<std::path::Path>>(
        &self,
        paths: &[P],
    ) -> Result<crate::expect::file_chooser::FileChooserExpectation> {
        let files: Vec<String> = paths
            .iter()
            .map(|p| p.as_ref().to_string_lossy().into_owned())
            .collect();
        crate::expect::file_chooser::register(self.session(), files).await
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
    /// **Stealth recommended.** Cloudflare Turnstile is somewhat forgiving
    /// of non-stealth Chrome, but `BrowserBuilder::stealth` significantly
    /// raises the clearance success rate.
    ///
    /// Gated by the `cloudflare` cargo feature.
    #[must_use]
    pub fn cloudflare(&self) -> zendriver_cloudflare::CloudflareBypass<'_> {
        zendriver_cloudflare::CloudflareBypass::new(self.session())
    }
}

#[cfg(feature = "imperva")]
impl Tab {
    /// Construct an
    /// [`ImpervaBypass`](zendriver_imperva::ImpervaBypass) bound to this
    /// tab's session.
    ///
    /// Chain
    /// [`timeout`](zendriver_imperva::ImpervaBypass::timeout) /
    /// [`poll_interval`](zendriver_imperva::ImpervaBypass::poll_interval) /
    /// [`with_interception`](zendriver_imperva::ImpervaBypass::with_interception) /
    /// [`on_captcha`](zendriver_imperva::ImpervaBypass::on_captcha)
    /// builder methods, then call
    /// [`wait_for_clearance`](zendriver_imperva::ImpervaBypass::wait_for_clearance)
    /// to detect the active Imperva surface (modern reese84, legacy
    /// Incapsula, or CAPTCHA escalation) and poll until clearance.
    ///
    /// **Stealth required.** Without `BrowserBuilder::stealth`, the
    /// bypass will fail on nearly all real Imperva-protected sites.
    ///
    /// Gated by the `imperva` cargo feature.
    #[must_use]
    pub fn imperva(&self) -> zendriver_imperva::ImpervaBypass<'_> {
        zendriver_imperva::ImpervaBypass::new(self.session())
    }
}

#[cfg(feature = "datadome")]
impl Tab {
    /// Construct a [`DataDomeBypass`](zendriver_datadome::DataDomeBypass) bound
    /// to this tab's session.
    ///
    /// Chain `timeout` / `poll_interval` / `with_interception` / `on_captcha`,
    /// then `wait_for_clearance` to detect the active DataDome surface
    /// (device-check, captcha, or block) and poll until the `datadome`
    /// clearance cookie lands.
    ///
    /// **Stealth strongly recommended.** DataDome's device-check scores the
    /// browser fingerprint; without `BrowserBuilder::stealth` (including the
    /// `Surface::Webgpu` coherence patch) the device-check will not clear.
    ///
    /// Gated by the `datadome` cargo feature.
    #[must_use]
    pub fn datadome(&self) -> zendriver_datadome::DataDomeBypass<'_> {
        zendriver_datadome::DataDomeBypass::new(self.session())
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

/// Live-probe seam for [`zendriver_stealth::Persona::from_browser`].
///
/// Implemented here (rather than in `zendriver-stealth`) so the stealth crate
/// stays free of a `zendriver` dependency. The `ZendriverError` from
/// [`Tab::evaluate`] is mapped into [`zendriver_stealth::StealthError::Probe`]
/// — going the other direction (a `From<ZendriverError>` in stealth) would
/// require stealth to depend on this crate, which is a cycle.
#[async_trait::async_trait]
impl zendriver_stealth::JsProbe for Tab {
    async fn eval_json(
        &self,
        js: &str,
    ) -> std::result::Result<Value, zendriver_stealth::StealthError> {
        self.evaluate::<Value>(js)
            .await
            .map_err(|e| zendriver_stealth::StealthError::Probe(e.to_string()))
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

    #[tokio::test]
    async fn reload_with_sets_ignore_cache_and_script() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.reload_with(ReloadOptions {
                    ignore_cache: true,
                    script_to_evaluate_on_load: Some("x".into()),
                })
                .await
            }
        });

        let id = mock.expect_cmd("Page.reload").await;
        assert_eq!(mock.last_sent()["params"]["ignoreCache"], true);
        assert_eq!(mock.last_sent()["params"]["scriptToEvaluateOnLoad"], "x");
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn reload_with_omits_script_when_none() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.reload_with(ReloadOptions {
                    ignore_cache: false,
                    script_to_evaluate_on_load: None,
                })
                .await
            }
        });

        let id = mock.expect_cmd("Page.reload").await;
        assert_eq!(mock.last_sent()["params"]["ignoreCache"], false);
        // scriptToEvaluateOnLoad must be omitted entirely when None.
        assert!(
            mock.last_sent()["params"]
                .get("scriptToEvaluateOnLoad")
                .is_none()
        );
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    // --- Tab::content (B1) ---------------------------------------------

    #[tokio::test]
    async fn content_dispatches_get_document_then_outer_html() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.content().await }
        });

        // 1. DOM.getDocument { depth: 0 } → root nodeId.
        let id_doc = mock.expect_cmd("DOM.getDocument").await;
        assert_eq!(mock.last_sent()["params"]["depth"], 0);
        mock.reply(id_doc, json!({ "root": { "nodeId": 7 } })).await;

        // 2. DOM.getOuterHTML { nodeId } → outerHTML string.
        let id_html = mock.expect_cmd("DOM.getOuterHTML").await;
        assert_eq!(mock.last_sent()["params"]["nodeId"], 7);
        mock.reply(
            id_html,
            json!({ "outerHTML": "<!DOCTYPE html><html><body>hi</body></html>" }),
        )
        .await;

        let html = fut.await.unwrap().unwrap();
        assert_eq!(html, "<!DOCTYPE html><html><body>hi</body></html>");
        conn.shutdown();
    }

    // --- Tab scroll (B3) -----------------------------------------------

    #[tokio::test]
    async fn scroll_down_synthesizes_negative_y_gesture() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.scroll_down(300.0).await }
        });

        let id = mock.expect_cmd("Input.synthesizeScrollGesture").await;
        let p = mock.last_sent();
        // scroll_down(px) maps to a NEGATIVE yDistance (CDP: negative
        // yDistance scrolls the page down / content up).
        assert_eq!(p["params"]["yDistance"], -300.0);
        assert_eq!(p["params"]["xDistance"], 0.0);
        // Anchored at the fixed viewport point (100, 100).
        assert_eq!(p["params"]["x"], 100.0);
        assert_eq!(p["params"]["y"], 100.0);
        // No speed override by default.
        assert!(p["params"].get("speed").is_none());
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn scroll_up_synthesizes_positive_y_gesture() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.scroll_up(150.0).await }
        });

        let id = mock.expect_cmd("Input.synthesizeScrollGesture").await;
        // scroll_up(px) maps to a POSITIVE yDistance.
        assert_eq!(mock.last_sent()["params"]["yDistance"], 150.0);
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn scroll_with_forwards_speed() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.scroll_with(ScrollOptions {
                    dx: 25.0,
                    dy: -400.0,
                    speed: Some(800),
                })
                .await
            }
        });

        let id = mock.expect_cmd("Input.synthesizeScrollGesture").await;
        let p = mock.last_sent();
        // dx/dy forward verbatim to xDistance/yDistance; speed plumbs through.
        assert_eq!(p["params"]["xDistance"], 25.0);
        assert_eq!(p["params"]["yDistance"], -400.0);
        assert_eq!(p["params"]["speed"], 800);
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn scroll_with_omits_speed_when_none() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.scroll_with(ScrollOptions {
                    dx: 0.0,
                    dy: 100.0,
                    speed: None,
                })
                .await
            }
        });

        let id = mock.expect_cmd("Input.synthesizeScrollGesture").await;
        assert!(mock.last_sent()["params"].get("speed").is_none());
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    // --- Tab::cookies (P4 T10) ----------------------------------------

    /// [`Tab::cookies`] returns a [`crate::CookieJar`] bound to the owning
    /// browser's root connection — discovered via the cached `Weak<BrowserInner>`
    /// upgrade. The test builds a synthetic `BrowserInner` with a known
    /// connection, attaches a Tab whose Weak ref points at it, and asserts
    /// that calling `.set(...)` dispatches `Storage.setCookies` on that
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
                job: crate::browser::ProcessJob::none(),
                _user_data: None,
                _extension_dirs: Vec::new(),
                owns_process: false,
                tabs: tokio::sync::RwLock::new(map),
                debug_host_port: None,
                ws_url: None,
                tabs_changed: tokio::sync::Notify::new(),
                #[cfg(feature = "interception")]
                proxy_auth_handle: std::sync::OnceLock::new(),
                #[cfg(feature = "interception")]
                context_proxy_auth: tokio::sync::Mutex::new(HashMap::new()),
                #[cfg(feature = "tracker-blocking")]
                tracker_matcher: None,
                #[cfg(feature = "interception")]
                session_intercept_handles: tokio::sync::Mutex::new(std::collections::HashMap::new()),
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
                ..Default::default()
            })
            .await
        });

        let id = mock.expect_cmd("Storage.setCookies").await;
        let params = &mock.last_sent()["params"];
        let arr = params["cookies"]
            .as_array()
            .expect("setCookies payload must carry a cookies array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "sid");
        // Browser-scope command — no session_id (jar dispatches against
        // the browser's connection, not the tab's session).
        assert!(mock.last_sent().get("sessionId").is_none());
        mock.reply(id, json!({})).await;

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

    /// `Tab::frames()` sorts by [`Frame::id`] regardless of registry
    /// (insertion/`HashMap`) order, so cross-frame callers like
    /// `FindBuilder::include_frames` see a deterministic, run-to-run-stable
    /// order instead of `HashMap::values()`'s unspecified iteration order.
    #[tokio::test]
    async fn frames_are_sorted_by_id_regardless_of_attach_order() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let id_enable =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Page.enable"))
                .await
                .expect("frame lifecycle did not send Page.enable within 2s");
        mock.reply(id_enable, json!({})).await;

        // Attach frames out of lexical order: "FC", "FA", "FB". Each gets
        // its own `parentFrameId` — sharing one would trip the lifecycle
        // subscriber's same-parent/empty-url "stale provisional sibling"
        // sweep (see `frame::lifecycle::run`), which is unrelated to what
        // this test is exercising.
        for (frame_id, parent_id) in [("FC", "FROOT1"), ("FA", "FROOT2"), ("FB", "FROOT3")] {
            mock.emit_event_for_session(
                "Page.frameAttached",
                json!({
                    "frameId": frame_id,
                    "parentFrameId": parent_id,
                }),
                "S1",
            )
            .await;
        }

        for _ in 0..50 {
            if tab.inner.frames.read().await.len() == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let frames = tab.frames().await.unwrap();
        let ids: Vec<&str> = frames.iter().map(Frame::id).collect();
        assert_eq!(
            ids,
            vec!["FA", "FB", "FC"],
            "frames() should be sorted by id, not HashMap/attach order"
        );

        conn.shutdown();
    }

    // --- wait_for_idle quiet-window enforcement ------------------------

    /// End-to-end: emit a `Network.requestWillBeSent` event, then 100ms
    /// later emit `Network.loadingFinished` for the same id. With a 500ms
    /// quiet window + 2s outer timeout, `wait_for_idle_with` should
    /// resolve `Ok(())` within ~600ms of the completion (500ms quiet +
    /// scheduling slack). Asserts the call returns within 1.5s of the
    /// completion event — a generous bound that still rejects "never
    /// resolves" without flaking on a loaded CI machine.
    #[tokio::test]
    async fn wait_for_idle_resolves_after_quiet_window_post_completion() {
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
        let completed_at = tokio::time::Instant::now();
        mock.emit_event_for_session(
            "Network.loadingFinished",
            json!({ "requestId": "R1" }),
            "S1",
        )
        .await;

        let res = tokio::time::timeout(Duration::from_millis(1500), fut)
            .await
            .expect("wait_for_idle did not resolve within 1500ms after completion");
        res.unwrap().unwrap();
        let elapsed = completed_at.elapsed();
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
        let r1_completed_at = tokio::time::Instant::now();
        mock.emit_event_for_session(
            "Network.loadingFinished",
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
                .contains_key("R2")
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
                .contains_key("R2"),
            "R2 did not register inside quiet window",
        );

        // Hold R2 in-flight briefly, then close it. A new quiet window
        // starts from this point — wait_for_idle must wait it out.
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock.emit_event_for_session(
            "Network.loadingFinished",
            json!({ "requestId": "R2" }),
            "S1",
        )
        .await;

        let res = tokio::time::timeout(Duration::from_secs(2), fut)
            .await
            .expect("wait_for_idle did not resolve within 2s after R2 completed");
        res.unwrap().unwrap();
        let total_elapsed = r1_completed_at.elapsed();

        // Lower bound: R1-completion (T0) → 100ms gap → R2 starts → 50ms
        // hold → R2 completion → 200ms quiet window → resolve. Total ≥
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

    /// Regression for the "burst within tick" race: a request that fires
    /// *and finishes* inside one 50ms poll tick takes the in-flight set
    /// 0 → 1 → 0 without ever being observed at a >0 read. A naive
    /// implementation that only checks the set len would see sustained
    /// 0 and resolve early. The fix arms a `Notify::notified()` future
    /// before each count read so the two membership events from the burst
    /// both hit an armed waker; on the next iteration we wake via the
    /// notifier arm and reset `quiet_start`.
    #[tokio::test]
    async fn wait_for_idle_burst_inside_tick_resets_quiet_window() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let id_enable =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Network.enable"))
                .await
                .expect("tracker did not send Network.enable within 2s");
        mock.reply(id_enable, json!({})).await;

        // Start with the set already at 0 — wait_for_idle should accumulate
        // a quiet window from T0. The inner `wait_for_idle_with` timeout is
        // deliberately generous (30s) so a slow / loaded CI runner doesn't
        // flake the test; the correctness assertion below uses a strict
        // lower bound on `elapsed` and an outer 10s tokio::time::timeout to
        // catch a too-early resolve or a hang.
        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.wait_for_idle_with(Duration::from_secs(30), Duration::from_millis(300))
                    .await
            }
        });
        let started_at = tokio::time::Instant::now();

        // Let the wait_for_idle loop tick once on an empty set so
        // `quiet_start` is firmly armed.
        tokio::time::sleep(Duration::from_millis(75)).await;

        // Burst: fire R-burst's requestWillBeSent + loadingFinished
        // back-to-back. The tracker should observe both transitions and
        // notify on each; the wait_for_idle loop should wake on the notif
        // arm and reset quiet_start even though the set's instantaneous
        // value returns to 0.
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({ "requestId": "Rburst" }),
            "S1",
        )
        .await;
        mock.emit_event_for_session(
            "Network.loadingFinished",
            json!({ "requestId": "Rburst" }),
            "S1",
        )
        .await;

        // Outer timeout deliberately generous (10s) so a slow / loaded
        // CI runner doesn't flake the test. The correctness assertion
        // below uses a strict lower bound on `elapsed` to catch a
        // too-early resolve regardless of how long the slack window is.
        let res = tokio::time::timeout(Duration::from_secs(10), fut)
            .await
            .expect("wait_for_idle did not resolve within 10s");
        res.unwrap().unwrap();

        // Lower bound: 75ms initial sleep + 300ms quiet window after the
        // burst's last notification = 375ms. A bug that ignored the burst
        // would resolve at started_at + 300ms = 300ms.
        let elapsed = started_at.elapsed();
        assert!(
            elapsed >= Duration::from_millis(355),
            "wait_for_idle resolved too early ({elapsed:?}); 0→1→0 burst inside \
             quiet window must reset quiet_start",
        );

        conn.shutdown();
    }

    /// A request that never receives a terminal CDP event (a hung beacon /
    /// long-poll / stuck XHR) must not pin `wait_for_idle` forever when the
    /// caller sets [`IdleOptions::max_inflight_age`]: once it has been in
    /// flight longer than that age it stops counting toward "active network",
    /// so the quiet window can elapse and the call resolves.
    #[tokio::test]
    async fn wait_for_idle_opts_evicts_stuck_request_past_max_inflight_age() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let id_enable =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Network.enable"))
                .await
                .expect("tracker did not send Network.enable within 2s");
        mock.reply(id_enable, json!({})).await;

        // Insert a request and NEVER emit a terminal event for it.
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({ "requestId": "STUCK" }),
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
            "stuck request did not register before wait_for_idle starts",
        );

        // age 300ms + 200ms window ⇒ resolves ~500ms even though the request
        // never completes. The 5s inner timeout is never reached on success;
        // the 3s outer timeout below fails the test if eviction is missing
        // (the stub hangs to the inner timeout instead of resolving).
        let start = tokio::time::Instant::now();
        let res = tokio::time::timeout(
            Duration::from_secs(3),
            tab.wait_for_idle_opts(IdleOptions {
                timeout: Duration::from_secs(5),
                quiet_window: Duration::from_millis(200),
                max_inflight_age: Some(Duration::from_millis(300)),
                ..Default::default()
            }),
        )
        .await
        .expect("wait_for_idle_opts did not resolve despite max_inflight_age eviction");
        res.unwrap();
        let elapsed = start.elapsed();

        // Must clear the eviction age (300ms) + quiet window (200ms).
        assert!(
            elapsed >= Duration::from_millis(450),
            "resolved too early ({elapsed:?}); eviction age + quiet window not enforced",
        );
        // Eviction is a counting filter, not a removal: the never-terminated id
        // still lingers in the map (so a `None` waiter would still block on it).
        assert_eq!(
            tab.inner.network_tracker.in_flight.lock().await.len(),
            1,
            "stuck id should remain in the map (age filter, not prune)",
        );

        conn.shutdown();
    }

    // --- wait_for_idle IdleLossPolicy (Task 4) --------------------------

    /// Under `IdleLossPolicy::Strict`, a delivery gap on the connection's
    /// accounted event stream (a lagging broadcast subscriber) observed
    /// while `wait_for_idle_opts` is waiting must abort the call with
    /// `ZendriverError::EventStreamIncomplete` — once that happens the wait
    /// can no longer prove nothing relevant to idleness was missed, so it
    /// must refuse to report a possibly-wrong idle.
    ///
    /// Forces the gap deterministically:
    /// `MockConnection::pair_with_accounted_capacity(2)` gives the accounted
    /// bus a 2-slot capacity, so pushing 5 unrelated events (after the
    /// Strict wait has subscribed) overflows it and the next accounted poll
    /// reports `Lagged`. R1 is left in-flight throughout — deliberately
    /// never completed — so the only way this test resolves before its
    /// outer timeout is via the `Lagged` abort path, not a coincidental
    /// idle.
    #[tokio::test]
    async fn wait_for_idle_opts_strict_aborts_on_lagged_boundary() {
        let (mut mock, conn) = MockConnection::pair_with_accounted_capacity(2);
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let id_enable =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Network.enable"))
                .await
                .expect("tracker did not send Network.enable within 2s");
        mock.reply(id_enable, json!({})).await;

        // Keep a request in flight (never completed) so the only path to
        // resolution is the Strict abort, not a real idle.
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
                t.wait_for_idle_opts(IdleOptions {
                    timeout: Duration::from_secs(5),
                    quiet_window: Duration::from_millis(200),
                    max_inflight_age: None,
                    loss_policy: IdleLossPolicy::Strict,
                })
                .await
            }
        });

        // Give the Strict wait a chance to reach its first poll and
        // subscribe via `subscribe_raw_accounted` (a synchronous call made
        // once, before the loop starts) before overflowing the bus.
        tokio::time::sleep(Duration::from_millis(75)).await;

        // Overflow the 2-slot accounted bus: 5 unrelated events, none of
        // which touch the `Network.*` domain the in-flight tracker cares
        // about (so R1 staying in-flight is unaffected).
        for i in 0..5u32 {
            mock.emit_event("Test.dummy", json!({ "i": i })).await;
        }

        let res = tokio::time::timeout(Duration::from_secs(2), fut)
            .await
            .expect("wait_for_idle_opts (Strict) did not resolve within 2s after the lag");
        assert!(
            matches!(res.unwrap(), Err(ZendriverError::EventStreamIncomplete)),
            "Strict must abort with EventStreamIncomplete on a Lagged boundary",
        );

        conn.shutdown();
    }

    /// Sibling of the Strict test above: under `IdleLossPolicy::Lenient`
    /// (the default), the exact same injected `Lagged` boundary must have
    /// zero effect. Lenient never subscribes to the accounted stream at
    /// all, so it can't observe the gap — the wait proceeds purely off the
    /// in-flight tracker's own best-effort subscription and resolves
    /// normally once the request genuinely completes.
    #[tokio::test]
    async fn wait_for_idle_opts_lenient_still_resolves_after_lagged_boundary() {
        let (mut mock, conn) = MockConnection::pair_with_accounted_capacity(2);
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let id_enable =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Network.enable"))
                .await
                .expect("tracker did not send Network.enable within 2s");
        mock.reply(id_enable, json!({})).await;

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

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.wait_for_idle_opts(IdleOptions {
                    timeout: Duration::from_secs(5),
                    quiet_window: Duration::from_millis(200),
                    max_inflight_age: None,
                    loss_policy: IdleLossPolicy::Lenient,
                })
                .await
            }
        });

        tokio::time::sleep(Duration::from_millis(75)).await;

        // Same overflow as the Strict test — Lenient must not even notice.
        for i in 0..5u32 {
            mock.emit_event("Test.dummy", json!({ "i": i })).await;
        }

        // Now genuinely finish R1 — Lenient must still resolve normally,
        // unaffected by the earlier flood.
        mock.emit_event_for_session(
            "Network.loadingFinished",
            json!({ "requestId": "R1" }),
            "S1",
        )
        .await;

        let res = tokio::time::timeout(Duration::from_secs(2), fut)
            .await
            .expect(
                "wait_for_idle_opts (Lenient) did not resolve within 2s despite the injected gap",
            );
        res.unwrap()
            .expect("Lenient must resolve Ok once the request completes, gap notwithstanding");

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

    // --- B4: set_user_agent --------------------------------------------

    #[tokio::test]
    async fn set_user_agent_dispatches_emulation_override() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.set_user_agent("Mozilla/5.0 (compatible; MyBot/1.0)")
                    .await
            }
        });

        let id = mock.expect_cmd("Emulation.setUserAgentOverride").await;
        let sent = mock.last_sent();
        assert_eq!(
            sent["params"]["userAgent"],
            "Mozilla/5.0 (compatible; MyBot/1.0)"
        );
        // UA-only shortcut omits the optional fields entirely.
        assert!(sent["params"].get("acceptLanguage").is_none());
        assert!(sent["params"].get("platform").is_none());
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn set_user_agent_with_includes_lang_and_platform() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.set_user_agent_with(UserAgentOverride {
                    user_agent: "UA/1.0".into(),
                    accept_language: Some("en-US,en;q=0.9".into()),
                    platform: Some("Linux x86_64".into()),
                })
                .await
            }
        });

        let id = mock.expect_cmd("Emulation.setUserAgentOverride").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["userAgent"], "UA/1.0");
        assert_eq!(sent["params"]["acceptLanguage"], "en-US,en;q=0.9");
        assert_eq!(sent["params"]["platform"], "Linux x86_64");
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    // --- B5: raw mouse + flash_point -----------------------------------

    #[tokio::test]
    async fn mouse_click_emits_pressed_released_at_coords() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.mouse_click(123.0, 456.0).await }
        });

        // Realistic click: N mouseMoved frames (Bezier), then exactly one
        // mousePressed + one mouseReleased. Drain every dispatch; the final
        // two must be mousePressed then mouseReleased at (123, 456).
        let mut saw_pressed = false;
        let mut saw_released = false;
        let mut last_two: Vec<String> = Vec::new();
        loop {
            let next = tokio::time::timeout(
                Duration::from_millis(500),
                mock.expect_cmd("Input.dispatchMouseEvent"),
            )
            .await;
            match next {
                Ok(id) => {
                    let sent = mock.last_sent();
                    let kind = sent["params"]["type"].as_str().unwrap_or("").to_string();
                    if kind == "mousePressed" || kind == "mouseReleased" {
                        // Press/release land at the exact target coordinate.
                        assert_eq!(sent["params"]["x"], 123.0);
                        assert_eq!(sent["params"]["y"], 456.0);
                        assert_eq!(sent["params"]["button"], "left");
                        if kind == "mousePressed" {
                            saw_pressed = true;
                        } else {
                            saw_released = true;
                        }
                    }
                    last_two.push(kind);
                    mock.reply(id, json!({})).await;
                }
                Err(_) => break,
            }
        }

        fut.await.unwrap().unwrap();
        assert!(saw_pressed, "expected a mousePressed dispatch");
        assert!(saw_released, "expected a mouseReleased dispatch");
        let tail: Vec<&str> = last_two.iter().rev().take(2).map(String::as_str).collect();
        // Reversed: [released, pressed] — i.e. the final two in order are
        // mousePressed then mouseReleased.
        assert_eq!(
            tail,
            vec!["mouseReleased", "mousePressed"],
            "final two dispatches must be mousePressed then mouseReleased"
        );
        conn.shutdown();
    }

    #[tokio::test]
    async fn mouse_move_emits_mousemoved() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.mouse_move(50.0, 60.0).await }
        });

        // Realistic move emits one-or-more mouseMoved dispatches and NO
        // press/release. Drain them all, asserting type along the way.
        let mut saw_moved = false;
        loop {
            let next = tokio::time::timeout(
                Duration::from_millis(500),
                mock.expect_cmd("Input.dispatchMouseEvent"),
            )
            .await;
            match next {
                Ok(id) => {
                    let kind = mock.last_sent()["params"]["type"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    assert_eq!(kind, "mouseMoved", "mouse_move must only emit mouseMoved");
                    saw_moved = true;
                    mock.reply(id, json!({})).await;
                }
                Err(_) => break,
            }
        }

        fut.await.unwrap().unwrap();
        assert!(saw_moved, "expected at least one mouseMoved dispatch");
        conn.shutdown();
    }

    #[tokio::test]
    async fn tap_dispatches_touchstart_with_point_then_touchend_empty() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.tap(10.0, 20.0).await }
        });

        // touchStart carries the single touch point at (10, 20).
        let id = mock.expect_cmd("Input.dispatchTouchEvent").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["type"], "touchStart");
        let points = sent["params"]["touchPoints"].as_array().unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0]["x"], 10.0);
        assert_eq!(points[0]["y"], 20.0);
        mock.reply(id, json!({})).await;

        // touchEnd carries an empty touchPoints array — finger lifted.
        let id = mock.expect_cmd("Input.dispatchTouchEvent").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["type"], "touchEnd");
        assert_eq!(sent["params"]["touchPoints"].as_array().unwrap().len(), 0);
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn flash_point_dispatches_evaluate() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.flash_point(12.0, 34.0).await }
        });

        // flash_point injects a transient dot via a single main-world
        // Runtime.evaluate. Assert the JS references the coordinates and a
        // self-removal timer.
        let id = mock.expect_cmd("Runtime.evaluate").await;
        let expr = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert!(expr.contains("createElement"), "should build a dot element");
        assert!(
            expr.contains("12px") && expr.contains("34px"),
            "should position at (x, y)"
        );
        assert!(
            expr.contains("setTimeout") && expr.contains("remove"),
            "should self-remove"
        );
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    // --- B8: bring_to_front / bypass_insecure_connection_warning / inspector_url

    #[tokio::test]
    async fn bring_to_front_dispatches_page_bring_to_front() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.bring_to_front().await }
        });

        let id = mock.expect_cmd("Page.bringToFront").await;
        // Session-scope command (unlike activate's browser-scope
        // Target.activateTarget): the MockConnection session call path is used.
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn bypass_insecure_connection_warning_focuses_body_and_types_phrase() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.bypass_insecure_connection_warning().await }
        });

        // Step 1: find().css("body").one() resolves via querySelectorAll →
        // getProperties → describeNode.
        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        let expr = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert!(
            expr.contains("document.querySelectorAll") && expr.contains("body"),
            "expected querySelectorAll for body, got: {expr}"
        );
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;
        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        mock.reply(
            id_p,
            json!({
                "result": [
                    { "name": "0", "value": { "objectId": "RBody", "type": "object", "subtype": "node" } },
                    { "name": "length", "value": { "value": 1, "type": "number" } }
                ]
            }),
        )
        .await;
        let id_d = mock.expect_cmd("DOM.describeNode").await;
        mock.reply(id_d, json!({ "node": { "backendNodeId": 7 } }))
            .await;

        // Step 2: type_text_fast focuses the body first — actionability gate
        // (TEXT_INPUT = visible → enabled, 2 callFunctionOn) then this.focus()
        // (1 callFunctionOn). Reply truthy/undefined to each.
        for _ in 0..2 {
            let id = mock.expect_cmd("Runtime.callFunctionOn").await;
            mock.reply(
                id,
                json!({ "result": { "value": true, "type": "boolean" } }),
            )
            .await;
        }
        let id_focus = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(id_focus, json!({ "result": { "type": "undefined" } }))
            .await;

        // Step 3: each of the 12 chars of "thisisunsafe" emits a keyDown +
        // keyUp via Input.dispatchKeyEvent. Drain them all and reconstruct
        // the typed string from the keyDown events.
        let phrase = "thisisunsafe";
        let mut typed = String::new();
        loop {
            let next = tokio::time::timeout(
                Duration::from_millis(500),
                mock.expect_cmd("Input.dispatchKeyEvent"),
            )
            .await;
            match next {
                Ok(id) => {
                    let sent = mock.last_sent();
                    if sent["params"]["type"] == "keyDown" {
                        if let Some(k) = sent["params"]["key"].as_str() {
                            typed.push_str(k);
                        }
                    }
                    mock.reply(id, json!({})).await;
                }
                Err(_) => break,
            }
        }

        fut.await.unwrap().unwrap();
        assert_eq!(typed, phrase, "must type the literal bypass phrase");
        conn.shutdown();
    }

    #[tokio::test]
    async fn inspector_url_composes_expected_string() {
        // `inspector_url` reaches the owning browser's `debug_host_port` via
        // the Tab's Weak<BrowserInner>. Build a real BrowserInner (test
        // helper) and set the endpoint, then mint a Tab bound to it.
        let (_mock, conn) = MockConnection::pair();
        let inner = crate::browser::test_only_inner_from_conn(conn.clone());
        // Endpoint is `None` for the test helper by default → error path.
        let tab = inner.main_tab.clone();
        let err = tab.inspector_url().unwrap_err();
        assert!(
            matches!(err, ZendriverError::Navigation(_)),
            "missing endpoint should surface Navigation error, got {err:?}"
        );
        conn.shutdown();
    }

    #[tokio::test]
    async fn inspector_url_with_endpoint_builds_devtools_frontend_url() {
        let (_mock, conn) = MockConnection::pair();
        // Construct a BrowserInner with a known debug endpoint + main tab
        // target id so we can assert the exact composed URL.
        let inner =
            std::sync::Arc::new_cyclic(|weak: &std::sync::Weak<crate::browser::BrowserInner>| {
                let session = SessionHandle::new(conn.clone(), "S1");
                let input =
                    crate::input::InputController::new(zendriver_stealth::InputProfile::native());
                let main_tab = Tab::new(session, weak.clone(), input, "TARGET-XYZ".to_string());
                let mut map = std::collections::HashMap::new();
                map.insert("S1".to_string(), main_tab.clone());
                crate::browser::BrowserInner {
                    conn: conn.clone(),
                    main_tab,
                    child: tokio::sync::Mutex::new(None),
                    job: crate::browser::ProcessJob::none(),
                    _user_data: None,
                    _extension_dirs: Vec::new(),
                    owns_process: false,
                    tabs: tokio::sync::RwLock::new(map),
                    debug_host_port: Some("127.0.0.1:9222".to_string()),
                    ws_url: None,
                    tabs_changed: tokio::sync::Notify::new(),
                    #[cfg(feature = "interception")]
                    proxy_auth_handle: std::sync::OnceLock::new(),
                    #[cfg(feature = "interception")]
                    context_proxy_auth: tokio::sync::Mutex::new(HashMap::new()),
                    #[cfg(feature = "tracker-blocking")]
                    tracker_matcher: None,
                    #[cfg(feature = "interception")]
                    session_intercept_handles: tokio::sync::Mutex::new(
                        std::collections::HashMap::new(),
                    ),
                }
            });
        let url = inner.main_tab.inspector_url().unwrap();
        assert_eq!(
            url,
            "http://127.0.0.1:9222/devtools/inspector.html?ws=127.0.0.1:9222/devtools/page/TARGET-XYZ"
        );
        conn.shutdown();
    }

    /// [`Tab::set_download_path`] dispatches `Browser.setDownloadBehavior`
    /// with `behavior: "allow"` (keeps suggested filenames — distinct from
    /// the `expect_download` coordinator's `allowAndName`) and the chosen
    /// `downloadPath`, at browser scope.
    #[tokio::test]
    async fn tab_set_download_path_dispatches_set_download_behavior_allow() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.set_download_path("/tmp/x").await }
        });

        let id = mock.expect_cmd("Browser.setDownloadBehavior").await;
        let params = &mock.last_sent()["params"];
        assert_eq!(params["behavior"], "allow");
        assert_eq!(params["downloadPath"], "/tmp/x");
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    // --- E1: js_dumps --------------------------------------------------

    #[tokio::test]
    async fn js_dumps_evaluates_with_return_by_value_and_returns_value() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.js_dumps("window.foo").await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["expression"], "window.foo");
        // Must request a by-value deep serialization (untyped dump).
        assert_eq!(sent["params"]["returnByValue"], true);
        mock.reply(
            id,
            json!({ "result": { "type": "object", "value": { "a": 1, "b": [2, 3] } } }),
        )
        .await;

        let val = fut.await.unwrap().unwrap();
        assert_eq!(val, json!({ "a": 1, "b": [2, 3] }));
        conn.shutdown();
    }

    // --- E2: get_all_urls + get_all_linked_sources ---------------------

    #[tokio::test]
    async fn get_all_urls_evaluates_collector_reading_href_and_src() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.get_all_urls(true).await }
        });

        // Single main-world collector walks [href], [src]; the JS must read
        // both attribute kinds and (absolute=true) the resolved DOM props.
        let id = mock.expect_cmd("Runtime.evaluate").await;
        let expr = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert!(
            expr.contains("href") && expr.contains("src"),
            "collector must read both href and src, got: {expr}"
        );
        assert!(
            expr.contains("querySelectorAll"),
            "collector must query the DOM, got: {expr}"
        );
        mock.reply(
            id,
            json!({ "result": { "type": "object", "value": ["https://x.test/a", "https://x.test/b.png"] } }),
        )
        .await;

        let urls = fut.await.unwrap().unwrap();
        assert_eq!(urls, vec!["https://x.test/a", "https://x.test/b.png"]);
        conn.shutdown();
    }

    #[tokio::test]
    async fn get_all_linked_sources_routes_through_find_all_css() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.get_all_linked_sources().await }
        });

        // find_all().css("[src], [href]") resolves via a querySelectorAll
        // Runtime.evaluate carrying that exact selector, then getProperties,
        // then a describeNode per node. Return one node so `many()` resolves
        // on the first poll (an empty result would re-poll until the 10s
        // default timeout — see `many_or_empty_returns_empty_vec_on_timeout`).
        let id = mock.expect_cmd("Runtime.evaluate").await;
        let expr = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert!(
            expr.contains("querySelectorAll") && expr.contains("[src], [href]"),
            "must route through find_all css with the linked-source selector, got: {expr}"
        );
        mock.reply(
            id,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;
        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        mock.reply(
            id_p,
            json!({ "result": [
                { "name": "0", "value": { "objectId": "R0", "type": "object", "subtype": "node" } },
                { "name": "length", "value": { "value": 1, "type": "number" } }
            ] }),
        )
        .await;
        let id_d = mock.expect_cmd("DOM.describeNode").await;
        mock.reply(id_d, json!({ "node": { "backendNodeId": 20 } }))
            .await;

        let els = fut.await.unwrap().unwrap();
        assert_eq!(els.len(), 1, "should return the one linked-source element");
        conn.shutdown();
    }

    // --- E3: wait_for_ready_state --------------------------------------

    #[tokio::test]
    async fn wait_for_ready_state_polls_until_target_reached() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.wait_for_ready_state(ReadyState::Complete).await }
        });

        // Poll 1: still "loading" (rank 0 < Complete) → must keep polling.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(
            mock.last_sent()["params"]["expression"],
            "document.readyState"
        );
        mock.reply(
            id1,
            json!({ "result": { "type": "string", "value": "loading" } }),
        )
        .await;

        // Poll 2: now "complete" → resolves Ok.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            json!({ "result": { "type": "string", "value": "complete" } }),
        )
        .await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_ready_state_returns_when_observed_state_exceeds_target() {
        // Asking for Interactive must also resolve when the page is already
        // fully Complete (Complete ⊇ Interactive ordering).
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.wait_for_ready_state(ReadyState::Interactive).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            json!({ "result": { "type": "string", "value": "complete" } }),
        )
        .await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    // --- E4: download_file ---------------------------------------------

    #[tokio::test]
    async fn download_file_sets_behavior_then_injects_fetch_anchor_script() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.download_file("https://x.test/path/file.pdf?token=abc", None)
                    .await
            }
        });

        // No path set yet → download_file installs a default download
        // directory first (browser-scope setDownloadBehavior, behavior allow).
        let id_dl = mock.expect_cmd("Browser.setDownloadBehavior").await;
        let dl = mock.last_sent();
        assert_eq!(dl["params"]["behavior"], "allow");
        assert!(
            dl["params"]["downloadPath"]
                .as_str()
                .unwrap_or("")
                .ends_with("downloads"),
            "default download path should end with /downloads"
        );
        mock.reply(id_dl, json!({})).await;

        // Then the page-driven fetch→blob→anchor[download]→click injection.
        let id_eval = mock.expect_cmd("Runtime.evaluate").await;
        let expr = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert!(expr.contains("fetch("), "must fetch the url");
        assert!(
            expr.contains("createElement('a')") && expr.contains(".download"),
            "must build a download anchor"
        );
        assert!(expr.contains(".click()"), "must click the anchor");
        // URL is carried into the script; filename derived from the tail with
        // the query string stripped.
        assert!(
            expr.contains("https://x.test/path/file.pdf?token=abc"),
            "url must be injected"
        );
        assert!(
            expr.contains("\"file.pdf\""),
            "filename should be derived from url tail (query stripped), got: {expr}"
        );
        mock.reply(id_eval, json!({ "result": { "type": "undefined" } }))
            .await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn download_file_skips_default_path_when_already_set() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        // Pre-set a download path; download_file must NOT re-install a default.
        let set_fut = tokio::spawn({
            let t = tab.clone();
            async move { t.set_download_path("/tmp/chosen").await }
        });
        let id_set = mock.expect_cmd("Browser.setDownloadBehavior").await;
        assert_eq!(mock.last_sent()["params"]["downloadPath"], "/tmp/chosen");
        mock.reply(id_set, json!({})).await;
        set_fut.await.unwrap().unwrap();

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.download_file("https://x.test/a.bin", Some(PathBuf::from("renamed.bin")))
                    .await
            }
        });

        // Next command must be the injection evaluate directly — no second
        // setDownloadBehavior.
        let id_eval = mock.expect_cmd("Runtime.evaluate").await;
        let expr = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap_or("")
            .to_string();
        // Explicit filename wins over the url-derived one.
        assert!(
            expr.contains("\"renamed.bin\""),
            "explicit filename must be used"
        );
        mock.reply(id_eval, json!({ "result": { "type": "undefined" } }))
            .await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    // --- E5: mouse_drag ------------------------------------------------

    #[tokio::test]
    async fn mouse_drag_emits_pressed_moves_released_in_order() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.mouse_drag((10.0, 20.0), (110.0, 20.0), 4).await }
        });

        // Drain every dispatch: expect mousePressed(left) first, then ≥1
        // mouseMoved, then mouseReleased(left) last.
        let mut kinds: Vec<String> = Vec::new();
        let mut first_button = String::new();
        let mut last_button = String::new();
        loop {
            let next = tokio::time::timeout(
                Duration::from_millis(500),
                mock.expect_cmd("Input.dispatchMouseEvent"),
            )
            .await;
            match next {
                Ok(id) => {
                    let sent = mock.last_sent();
                    let kind = sent["params"]["type"].as_str().unwrap_or("").to_string();
                    if kind == "mousePressed" {
                        first_button = sent["params"]["button"].as_str().unwrap_or("").to_string();
                        // Press at the source point.
                        assert_eq!(sent["params"]["x"], 10.0);
                        assert_eq!(sent["params"]["y"], 20.0);
                    }
                    if kind == "mouseReleased" {
                        last_button = sent["params"]["button"].as_str().unwrap_or("").to_string();
                        // Release at the destination point.
                        assert_eq!(sent["params"]["x"], 110.0);
                        assert_eq!(sent["params"]["y"], 20.0);
                    }
                    kinds.push(kind);
                    mock.reply(id, json!({})).await;
                }
                Err(_) => break,
            }
        }

        fut.await.unwrap().unwrap();
        assert_eq!(kinds.first().map(String::as_str), Some("mousePressed"));
        assert_eq!(kinds.last().map(String::as_str), Some("mouseReleased"));
        assert!(
            kinds.iter().any(|k| k == "mouseMoved"),
            "expected at least one mouseMoved between press and release"
        );
        assert_eq!(first_button, "left", "drag presses the left button");
        assert_eq!(last_button, "left", "drag releases the left button");
        conn.shutdown();
    }

    // --- E6: search_frame_resources ------------------------------------

    #[tokio::test]
    async fn search_frame_resources_walks_tree_then_searches_each_resource() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.search_frame_resources("needle").await }
        });

        // 1. Page.getResourceTree → one frame with two resources.
        let id_tree = mock.expect_cmd("Page.getResourceTree").await;
        mock.reply(
            id_tree,
            json!({
                "frameTree": {
                    "frame": { "id": "FRAME_A" },
                    "resources": [
                        { "url": "https://x.test/app.js", "type": "Script" },
                        { "url": "https://x.test/style.css", "type": "Stylesheet" },
                    ],
                }
            }),
        )
        .await;

        // 2. Page.searchInResource for resource #1 → a match.
        let id_s1 = mock.expect_cmd("Page.searchInResource").await;
        let s1 = mock.last_sent();
        assert_eq!(s1["params"]["frameId"], "FRAME_A");
        assert_eq!(s1["params"]["query"], "needle");
        let first_url = s1["params"]["url"].as_str().unwrap_or("").to_string();
        mock.reply(
            id_s1,
            json!({ "result": [ { "lineNumber": 3, "lineContent": "var x = needle" } ] }),
        )
        .await;

        // 3. searchInResource for resource #2 → no match (empty result).
        let id_s2 = mock.expect_cmd("Page.searchInResource").await;
        let second_url = mock.last_sent()["params"]["url"]
            .as_str()
            .unwrap_or("")
            .to_string();
        mock.reply(id_s2, json!({ "result": [] })).await;

        let matches = fut.await.unwrap().unwrap();
        // Only the first resource matched.
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].frame_id, "FRAME_A");
        assert_eq!(matches[0].url, first_url);
        // Sanity: both resources were searched (the two URLs differ).
        assert_ne!(first_url, second_url);
        conn.shutdown();
    }
}
