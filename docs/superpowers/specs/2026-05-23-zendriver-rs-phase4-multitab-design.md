# zendriver-rs — Phase 4: Multi-tab + Cookies + Storage + Frames

**Date:** 2026-05-23
**Status:** Approved (delegate-mode brainstorming complete, ready for implementation plan)
**Phase:** 4 of 6 — see [Roadmap](#roadmap)
**Depends on:** Phase 1 (`12bf170`), Phase 2 (`7c10c8e`), Phase 3 (`b1208f0`), all on `main`

## Summary

Round out the public surface for real-world scraping: open + manage multiple Tabs in a single Browser, traverse iframes via a first-class `Frame` type that owns its own CDP session (so out-of-process iframes work), read/write cookies with full persistence via `CookieJar`, get/set `localStorage` + `sessionStorage`, navigate history (back/forward/reload), wait for network idle on SPAs, and finally land the Traversal-origin auto-refresh chain that P3 T17 deferred. Refactors `InputController` from per-Browser to per-Tab so each tab tracks its own cursor.

Phase 4 exit criterion: 5 more Python `zendriver/examples/*.py` scripts that exercise multi-tab + cookies + iframes port to Rust 1:1 and run green under `cargo test --features integration-tests`.

## Goals

- `Browser::new_tab()` / `Browser::tabs()` / `Tab::activate()` / `Tab::close()` complete multi-tab.
- `Tab::main_frame()` / `Tab::frames()` + first-class `Frame` type owning its own `SessionHandle`. `Frame::find()` / `evaluate` / `evaluate_main` / `goto` (where applicable) mirror Tab's surface.
- `FindBuilder::in_frame(&Frame)` becomes type-safe (replaces P3's `&str` placeholder).
- OOPIF (cross-origin iframes) handled via `Target.attachedToTarget` flow — already wired in P2 for stealth; P4 makes it produce Frame instances.
- `Browser::cookies() -> &CookieJar` with full CRUD + persistence: `all()`, `for_url(&Url)`, `set(Cookie)`, `set_many(&[Cookie])`, `delete(name, domain)`, `clear()`, `save_to_file(path)` (JSON), `load_from_file(path)`.
- `Tab::local_storage() -> Storage` + `Tab::session_storage() -> Storage` with HashMap-like CRUD: `get(key)`, `get_all() -> HashMap`, `set(key, value)`, `set_many(&HashMap)`, `remove(key)`, `clear()`.
- `Tab::back()` / `Tab::forward()` / `Tab::reload()` navigation history.
- `Tab::wait_for_idle()` resolves when 0 in-flight network requests sustain for 500ms (Playwright `networkidle` semantics). Default timeout 30s.
- `Element::refresh()` Traversal-origin chain now actually re-resolves the parent + re-traverses (P3 T17 deferral).
- `InputController` refactored from per-Browser to per-Tab. Each Tab tracks its own pointer + modifier state. Required for realistic multi-tab automation.
- `ScreenshotBuilder` for full-page + format options (PNG/JPEG/WebP). Existing `Tab::screenshot()` becomes `Tab::screenshot().bytes().await`.

## Non-goals

Explicitly **out of scope** for Phase 4:
- Network interception (`Fetch.*` domain wrapper) — P5.
- Cloudflare bypass — P5.
- `expect()` API for waiting on requests/responses/dialogs/downloads — P5.
- `zendriver-fetcher` (download Chrome binary) — P5.
- Cross-tab drag-and-drop — defer until proven needed.
- Per-tab BrowserContext (Playwright-style isolated cookie/storage groupings) — P5+ if demand surfaces.
- Tab grouping / window management — not in Python upstream either.
- Service worker / shared worker first-class types — accessed via `tab.cdp()` escape hatch only.

## Architecture

### Crate + file layout delta

```
crates/zendriver/src/
├── lib.rs                  # +pub mod frame; +re-exports Frame, CookieJar, Cookie, Storage, ScreenshotBuilder
├── browser.rs              # +tabs registry (TabRegistry), new_tab/tabs/close_tab impls
├── tab.rs                  # +InputController moves here (TabInner gains input field); +activate/back/forward/reload/wait_for_idle/screenshot_builder methods; +cookies access methods
├── element/                # P3 directory unchanged structurally
│   └── refresh.rs          # Traversal-origin chain refresh now actually implemented
├── frame/                  # NEW directory
│   ├── mod.rs              # Frame struct + Arc<Inner> + module re-exports + main_frame discovery
│   ├── lifecycle.rs        # Page.frameAttached/frameDetached/frameNavigated event handling
│   └── ooopif.rs           # Out-of-process iframe attach flow (re-uses TargetObserver if helpful)
├── cookies/                # NEW directory
│   ├── mod.rs              # CookieJar + Cookie + cookie ops
│   └── persistence.rs      # save_to_file / load_from_file (JSON)
├── storage/                # NEW directory
│   └── mod.rs              # Storage handle (local + session variants) + CRUD via DOMStorage CDP domain
├── network_idle/           # NEW directory (or just network_idle.rs)
│   └── mod.rs              # InFlightTracker + wait_for_idle impl
└── screenshot/             # NEW directory
    └── mod.rs              # ScreenshotBuilder + Format enum
```

### Dependency graph delta

```
zendriver
  ├─ zendriver-transport
  ├─ zendriver-stealth
  └─ chromiumoxide_cdp
```

No new external crate deps in P4. `base64` already in workspace from P3. `regex` already there.

### Per-Tab `InputController` refactor

Migrate from P3's `BrowserInner { input: Arc<InputController> }` to `TabInner { input: Arc<InputController> }`. Each Tab constructs its own InputController at attachment time, derived from the Browser's stealth profile.

```rust
// crates/zendriver/src/tab.rs
pub(crate) struct TabInner {
    pub(crate) session: SessionHandle,
    pub(crate) isolated_world: tokio::sync::Mutex<IsolatedWorldCache>,
    pub(crate) browser: std::sync::Weak<crate::browser::BrowserInner>,
    pub(crate) input: Arc<InputController>,                              // moved from Browser
    pub(crate) main_frame: tokio::sync::OnceCell<Frame>,                 // lazy-init main frame
    pub(crate) network_tracker: Arc<network_idle::InFlightTracker>,     // per-tab
}

impl Tab {
    pub fn input(&self) -> &Arc<InputController> {
        &self.inner.input
    }
}
```

`Browser::input()` accessor goes away. Callers update to `tab.input()`. P3's `Element::*` methods that called `self.tab().browser().upgrade().map(|b| b.input.clone())` simplify to `self.tab().input().clone()`.

`Browser::launch` constructs the main_tab's InputController from `StealthProfile::input_profile()`. `Browser::new_tab` does the same — each fresh Tab gets a fresh InputController with the same profile.

## Components — Multi-tab management

### `Browser` extension

```rust
impl Browser {
    /// Open a new tab via `Target.createTarget { url: "about:blank" }`.
    /// The new tab attaches via the existing observer flow so stealth applies.
    pub async fn new_tab(&self) -> Result<Tab>;

    /// Open a new tab navigating to `url` immediately.
    pub async fn new_tab_at(&self, url: impl AsRef<str>) -> Result<Tab>;

    /// All currently-attached tabs (snapshot — does not auto-update).
    pub fn tabs(&self) -> Vec<Tab>;

    /// Number of currently-attached tabs.
    pub fn tab_count(&self) -> usize;

    // `main_tab` is unchanged — initial tab created at launch.
}
```

### Tab registry inside `BrowserInner`

```rust
pub(crate) struct BrowserInner {
    pub(crate) conn: Connection,
    pub(crate) main_tab: Tab,
    pub(crate) child: tokio::sync::Mutex<Option<Child>>,
    pub(crate) _user_data: Option<TempDir>,
    pub(crate) tabs: tokio::sync::RwLock<HashMap<String, Tab>>,         // NEW; key = sessionId
    pub(crate) stealth_input_profile: zendriver_stealth::InputProfile,  // NEW; cached for new_tab
}
```

Tabs registered on `Target.attachedToTarget` event (handled by a new TabRegistrar observer that runs after the StealthObserver). Removed on `Target.detachedFromTarget`.

### `Tab` extension

```rust
impl Tab {
    /// Bring this tab to the foreground via `Target.activateTarget`.
    /// Required before clicks/keyboard on this tab when running multi-tab.
    pub async fn activate(&self) -> Result<()>;

    /// Navigate back one entry. Errors if no history.
    pub async fn back(&self) -> Result<()>;

    /// Navigate forward one entry. Errors if no history.
    pub async fn forward(&self) -> Result<()>;

    /// Reload the current page.
    pub async fn reload(&self) -> Result<()>;

    /// Wait until network is idle (0 in-flight requests for 500ms) OR timeout.
    pub async fn wait_for_idle(&self) -> Result<()>;
    pub async fn wait_for_idle_with(&self, timeout: Duration, quiet_window: Duration) -> Result<()>;

    /// Browser-wide cookie jar (shared across all tabs).
    pub fn cookies(&self) -> CookieJar;

    /// localStorage for this tab's origin.
    pub fn local_storage(&self) -> Storage;

    /// sessionStorage for this tab's origin.
    pub fn session_storage(&self) -> Storage;

    /// Main frame of this tab.
    pub async fn main_frame(&self) -> Result<Frame>;

    /// All frames (main + descendants) currently attached to this tab.
    pub async fn frames(&self) -> Result<Vec<Frame>>;

    /// Find a frame by URL substring or name attribute.
    pub async fn frame_by_url(&self, url_substr: &str) -> Result<Option<Frame>>;
    pub async fn frame_by_name(&self, name: &str) -> Result<Option<Frame>>;

    /// New screenshot builder (replaces P3's parameterless `screenshot()`).
    pub fn screenshot_builder(&self) -> ScreenshotBuilder<'_>;

    /// Convenience: full-page PNG (calls screenshot_builder().full_page(true).png().bytes()).
    pub async fn screenshot(&self) -> Result<Vec<u8>>;
}
```

## Components — Frame as first-class type

### Frame struct

```rust
// crates/zendriver/src/frame/mod.rs

#[derive(Clone)]
pub struct Frame { pub(crate) inner: Arc<FrameInner> }

pub(crate) struct FrameInner {
    pub(crate) frame_id: String,
    pub(crate) parent_frame_id: Option<String>,
    pub(crate) url: tokio::sync::RwLock<String>,            // mutates on navigation
    pub(crate) name: Option<String>,
    /// Session for this frame. For same-origin frames this is the parent
    /// Tab's session. For OOPIF this is a separate session attached via
    /// Target.attachToTarget.
    pub(crate) session: SessionHandle,
    /// Isolated-world contextId cache per frame.
    pub(crate) isolated_world: tokio::sync::Mutex<IsolatedWorldCache>,
    pub(crate) tab: std::sync::Weak<crate::tab::TabInner>,
}

impl Frame {
    pub fn id(&self) -> &str;
    pub fn url(&self) -> impl std::future::Future<Output = String> + '_;
    pub fn name(&self) -> Option<&str>;
    pub fn is_main(&self) -> bool;       // parent_frame_id.is_none()
    pub fn parent_id(&self) -> Option<&str>;

    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T>;
    pub async fn evaluate_main<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T>;

    pub fn find(&self) -> FindBuilder<'_>;
    pub fn find_all(&self) -> FindAllBuilder<'_>;

    pub async fn content(&self) -> Result<String>;          // outerHTML of <html>
    pub async fn goto(&self, url: impl AsRef<str>) -> Result<()>;   // for main frame; errors for sub-frames
    pub async fn wait_for_load(&self) -> Result<()>;
}
```

### Frame discovery + lifecycle

```
Browser::launch → main_tab attached → tab.main_frame() lazily calls Page.getFrameTree → constructs Frame

Page.frameAttached event → record new frame_id in Tab's frame registry (same-session)
Page.frameDetached event → remove frame_id from registry
Page.frameNavigated event → update Frame::url
Target.attachedToTarget event with type=iframe → register OOPIF Frame with its own session
Target.detachedFromTarget event → remove OOPIF Frame
```

`TabInner` gains a `frames: tokio::sync::RwLock<HashMap<String, Frame>>` registry similar to BrowserInner's tab registry. Frames track lifecycle via Page events on their owning session.

### `FindBuilder::in_frame` type-safety upgrade

```rust
impl<'scope> FindBuilder<'scope> {
    /// P3 took `frame_id: String`. P4 takes `&Frame` and scopes queries to
    /// the frame's session/contextId.
    pub fn in_frame<'a>(self, frame: &'a Frame) -> FindBuilder<'a>
    where 'scope: 'a;
}
```

Behavior change: query scope shifts from Tab's main frame to the specified Frame. SelectorKind::resolve_* uses the Frame's session instead of the Tab's session.

## Components — CookieJar

```rust
// crates/zendriver/src/cookies/mod.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<f64>,        // unix epoch seconds; None = session cookie
    pub http_only: bool,
    pub secure: bool,
    pub same_site: Option<SameSite>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SameSite { Strict, Lax, None }

/// Browser-wide cookie jar. Operations dispatch to `Network.*` CDP commands
/// on the Browser's underlying connection (not tab-scoped).
#[derive(Clone)]
pub struct CookieJar { pub(crate) inner: Arc<CookieJarInner> }

pub(crate) struct CookieJarInner {
    pub(crate) conn: Connection,
}

impl CookieJar {
    /// All cookies in the browser.
    pub async fn all(&self) -> Result<Vec<Cookie>>;

    /// Cookies for a specific URL (matches domain + path + secure).
    pub async fn for_url(&self, url: &url::Url) -> Result<Vec<Cookie>>;

    /// Set a single cookie via `Network.setCookie`.
    pub async fn set(&self, cookie: Cookie) -> Result<()>;

    /// Set many cookies via `Network.setCookies`.
    pub async fn set_many(&self, cookies: &[Cookie]) -> Result<()>;

    /// Delete by name + optional domain/path.
    pub async fn delete(&self, name: &str, domain: Option<&str>, path: Option<&str>) -> Result<()>;

    /// Clear ALL cookies. Equivalent to "log out everywhere".
    pub async fn clear(&self) -> Result<()>;

    /// Persist all cookies to JSON file.
    pub async fn save_to_file(&self, path: impl AsRef<Path>) -> Result<()>;

    /// Load cookies from JSON file + apply via set_many.
    pub async fn load_from_file(&self, path: impl AsRef<Path>) -> Result<()>;
}
```

## Components — Storage

```rust
// crates/zendriver/src/storage/mod.rs

#[derive(Clone)]
pub struct Storage { pub(crate) inner: Arc<StorageInner> }

pub(crate) struct StorageInner {
    pub(crate) session: SessionHandle,
    pub(crate) is_local: bool,    // true = localStorage, false = sessionStorage
}

impl Storage {
    pub async fn get(&self, key: &str) -> Result<Option<String>>;
    pub async fn get_all(&self) -> Result<HashMap<String, String>>;
    pub async fn set(&self, key: &str, value: &str) -> Result<()>;
    pub async fn set_many(&self, items: &HashMap<String, String>) -> Result<()>;
    pub async fn remove(&self, key: &str) -> Result<()>;
    pub async fn clear(&self) -> Result<()>;
    pub async fn len(&self) -> Result<usize>;
}
```

CDP wires:
- `DOMStorage.getDOMStorageItems` for read
- `DOMStorage.setDOMStorageItem` for set
- `DOMStorage.removeDOMStorageItem` for remove
- `DOMStorage.clear` for clear

Each call requires `StorageId { securityOrigin, isLocalStorage: bool }`. The Storage handle fetches the current tab origin via `Tab::url()` on each call (cheap CDP roundtrip; storage operations are infrequent).

## Components — Navigation history + wait_for_idle

```rust
impl Tab {
    pub async fn back(&self) -> Result<()> {
        let entries = self.call("Page.getNavigationHistory", json!({})).await?;
        let current_idx = entries["currentIndex"].as_i64()
            .ok_or_else(|| ZendriverError::Navigation("no currentIndex".into()))?;
        if current_idx <= 0 {
            return Err(ZendriverError::Navigation("no back history".into()));
        }
        let entry_id = entries["entries"][(current_idx - 1) as usize]["id"].clone();
        self.call("Page.navigateToHistoryEntry", json!({ "entryId": entry_id })).await?;
        Ok(())
    }

    pub async fn forward(&self) -> Result<()> { /* symmetric */ }

    pub async fn reload(&self) -> Result<()> {
        self.call("Page.reload", json!({ "ignoreCache": false })).await?;
        Ok(())
    }
}
```

### `wait_for_idle`

```rust
// crates/zendriver/src/network_idle/mod.rs

pub(crate) struct InFlightTracker {
    in_flight: tokio::sync::Mutex<HashSet<String>>,    // request_ids
    notifier: tokio::sync::Notify,
}

impl InFlightTracker {
    pub fn new() -> Arc<Self>;

    /// Spawn a background task that subscribes to Network.requestWillBeSent /
    /// responseReceived / loadingFailed / loadingFinished and maintains the
    /// in_flight set. Returns when the cancellation token fires.
    pub async fn run(self: Arc<Self>, session: SessionHandle, cancel: CancellationToken);
}

impl Tab {
    pub async fn wait_for_idle(&self) -> Result<()> {
        self.wait_for_idle_with(Duration::from_secs(30), Duration::from_millis(500)).await
    }

    pub async fn wait_for_idle_with(&self, timeout: Duration, quiet_window: Duration) -> Result<()> {
        let tracker = self.inner.network_tracker.clone();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Wait for in_flight to reach 0.
            let mut quiet_start = None;
            loop {
                let in_flight_count = tracker.in_flight.lock().await.len();
                if in_flight_count == 0 {
                    let now = tokio::time::Instant::now();
                    match quiet_start {
                        None => quiet_start = Some(now),
                        Some(start) if now.duration_since(start) >= quiet_window => return Ok(()),
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
                    () = tracker.notifier.notified() => {}    // wake immediately on state change
                }
            }
        }
    }
}
```

`InFlightTracker::run` task is spawned at Tab construction time. Calls `Network.enable` on the session, subscribes to events, increments on `requestWillBeSent`, decrements on `responseReceived` / `loadingFailed` / `loadingFinished`. Notifies on every change.

## Components — Traversal-origin refresh chain

P3 T17 left Traversal origin returning `NotRefreshable`. P4 implements:

```rust
// crates/zendriver/src/element/refresh.rs

async fn resolve_origin(origin: &ElementOrigin, tab: &Tab) -> Result<RemoteRef> {
    match origin {
        ElementOrigin::Query { ... } => { /* P3 impl */ }
        ElementOrigin::Traversal { parent, kind } => {
            // Recursively resolve parent first.
            let parent_ref = resolve_origin(parent, tab).await?;
            // Synthesize a temporary Element with the resolved parent ref so we
            // can call traversal methods on it.
            let parent_el = Element::from_jsret(tab.clone(),
                parent_ref.backend_node_id,
                parent_ref.remote_object_id);
            // Re-traverse to the child.
            match kind {
                TraversalKind::Parent => {
                    let result = parent_el.call_on_main("function(){return this.parentElement;}",
                        json!([])).await?;
                    extract_remote_ref(&result, tab).await?
                        .ok_or(ZendriverError::ElementNotFound { selector: "parent".into() })
                }
                TraversalKind::NthChild(idx) => {
                    let result = parent_el.call_on_main(
                        &format!("function(){{return this.children[{idx}];}}"),
                        json!([])).await?;
                    extract_remote_ref(&result, tab).await?
                        .ok_or(ZendriverError::ElementNotFound { selector: format!("nth_child({idx})") })
                }
            }
        }
        ElementOrigin::Evaluation => Err(ZendriverError::NotRefreshable),
    }
}
```

`extract_remote_ref` helper takes the JS-call result and turns it into a `RemoteRef` via `DOM.describeNode`.

## Components — `ScreenshotBuilder`

```rust
// crates/zendriver/src/screenshot/mod.rs

pub struct ScreenshotBuilder<'tab> {
    tab: &'tab Tab,
    format: Format,
    full_page: bool,
    clip: Option<BoundingBox>,
    quality: Option<u8>,    // for JPEG only
    omit_background: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format { Png, Jpeg, Webp }

impl<'tab> ScreenshotBuilder<'tab> {
    pub fn new(tab: &'tab Tab) -> Self;
    #[must_use] pub fn png(mut self) -> Self;
    #[must_use] pub fn jpeg(mut self) -> Self;
    #[must_use] pub fn webp(mut self) -> Self;
    #[must_use] pub fn full_page(mut self, on: bool) -> Self;
    #[must_use] pub fn clip(mut self, bbox: BoundingBox) -> Self;
    #[must_use] pub fn quality(mut self, q: u8) -> Self;    // panics if format != Jpeg
    #[must_use] pub fn omit_background(mut self, on: bool) -> Self;
    pub async fn bytes(self) -> Result<Vec<u8>>;
    pub async fn save(self, path: impl AsRef<Path>) -> Result<()>;
}
```

`Tab::screenshot()` becomes shorthand for `tab.screenshot_builder().png().bytes()`.

Full-page mode uses `Page.getLayoutMetrics` to learn document dimensions, then `Page.captureScreenshot { captureBeyondViewport: true, clip: { x:0, y:0, width:doc_width, height:doc_height, scale:1 } }`.

## Error handling

New variants:

```rust
#[non_exhaustive]
pub enum ZendriverError {
    // existing variants ...

    #[error("frame not found: {0}")]
    FrameNotFound(String),

    #[error("cookie operation failed: {0}")]
    Cookie(String),

    #[error("storage operation failed: {0}")]
    Storage(String),

    #[error("history navigation failed: {0}")]
    HistoryNavigation(String),
}
```

`TabNotFound(String)` was already declared in P1's spec section 4 — verify it's actually in the enum and add if missing.

## Testing

### Tier 1 — Unit (mocked CDP)
- TabRegistrar observer: emits Target.attachedToTarget event → tab registered with correct sessionId
- new_tab: sends Target.createTarget + waits for attachedToTarget + returns Tab with new sessionId
- close_tab: sends Target.closeTarget + removes from registry
- Frame::main: Page.getFrameTree response → top-level Frame extracted
- Frame::evaluate (isolated): per-frame contextId cached
- CookieJar.all: Network.getAllCookies response parsed into Vec<Cookie>
- CookieJar.set: Network.setCookie payload shape
- CookieJar.save_to_file / load_from_file: JSON round-trip on tmp file
- Storage.get: DOMStorage.getDOMStorageItems request + response parse
- Storage.set: DOMStorage.setDOMStorageItem request shape
- back: Page.getNavigationHistory + navigateToHistoryEntry sequence
- back: errors on no-history
- InFlightTracker: requestWillBeSent + responseReceived round-trip → in_flight set diff
- wait_for_idle: quiet_window enforced (in_flight goes 0 then becomes 1 within 500ms → does NOT return early)
- Traversal-refresh: stale parent triggers recursive resolve_origin
- ScreenshotBuilder: PNG default, JPEG with quality, full_page sets captureBeyondViewport
- Per-Tab InputController: two tabs have independent cursors

### Tier 2 — Integration (real Chrome + wiremock)
- new_tab opens a second tab; tabs() returns 2
- Tab::close removes from tabs() list
- cookies.set + cookies.all round-trip
- cookies.save_to_file + load_from_file round-trip across browser restart (simulated)
- local_storage.set + get + clear round-trip
- back/forward navigation history
- wait_for_idle resolves on SPA fixture with delayed XHR
- Frame::find inside iframe fixture
- Cross-origin iframe attaches via OOPIF flow; Frame::evaluate works
- Traversal-refresh: el.parent() returns Element that survives location.reload()

### Tier 3 — Snapshot
- CookieJar JSON serialization (round-trip of known fixture)
- ScreenshotBuilder default flag set
- Frame TargetInfo deserialization

### Tier 4 — Exit criterion
Port 5 more Python `zendriver/examples/*.py` that exercise P4 features. Candidates:
- `multi_tab.py` → open + manage multiple tabs
- `iframe.py` → query inside iframe
- `cookies.py` → save/load cookies across sessions
- `storage.py` → localStorage round-trip
- `navigation.py` → back/forward/reload

If specific Python examples don't exist, synthesize equivalents from spec.

### CI matrix
No new jobs. Existing matrix covers unit/integration/snapshot.

## Assumptions (delegate mode — judgement calls)

1. **InputController per-Tab refactor is mandatory in P4** — not optional. Even if a user only ever uses main_tab, refactoring later (in P5+) would be more disruptive.
2. **Browser-wide cookie jar, not per-tab.** Chrome cookies are inherently browser-scoped; per-tab is an artificial constraint. CookieJar lives on Browser.
3. **Storage is per-Tab** (per-origin) — matches browser semantics. Two tabs on different origins have isolated storage.
4. **Frame::goto only works on main frames.** Sub-frames navigate via parent setting `iframe.src` from JS; CDP doesn't directly navigate sub-frames. Documented; errors with `Navigation("sub-frame goto not supported; set iframe.src via parent evaluate_main")`.
5. **Storage round-trip fetches tab URL on every call.** Cheap (1 CDP roundtrip); avoids caching invalidation problems on navigation. If profiling shows it's a bottleneck, cache + invalidate on `Page.frameNavigated`.
6. **`Tab::activate` is required before keyboard/mouse on background tabs.** Chrome routes input to the active tab. Element actions on inactive tabs may silently no-op. We don't auto-activate; user controls.
7. **Tab registration via a TabRegistrar observer** that runs after StealthObserver. Order matters: stealth must apply before user code sees the tab. TabRegistrar just records sessionId → Tab handle.
8. **OOPIF Frames discovered via same TargetObserver flow as Tabs** but with `target_info.kind == "iframe"`. The TabRegistrar observer differentiates and routes to either Tab or Frame registries.
9. **wait_for_idle quiet_window default is 500ms.** Matches Playwright. Configurable via `wait_for_idle_with`.
10. **Cookie persistence format is JSON.** Cross-language compatible (Python can read), human-inspectable. Not a language-specific binary format — JSON survives across stack changes.
11. **`CookieJar` is a wrapper around `Connection`, not a stateful local cache.** Each query hits Chrome. Avoids cache invalidation. Save/load reads/writes Chrome's state.
12. **ScreenshotBuilder is a builder, not options-struct.** Matches P3 FindBuilder pattern. Chains read well: `tab.screenshot_builder().jpeg().quality(80).full_page(true).bytes().await`.
13. **No per-call retry on Cookie/Storage operations.** They're idempotent; transport-level retries already handled by CallError → ZendriverError mapping.
14. **No `BrowserContext` separation.** Playwright has BrowserContexts for isolated cookie/storage groupings within one Browser. zendriver-Python doesn't have this either. Defer to P5+ if demand surfaces.
15. **Network domain stays enabled per-tab once `wait_for_idle` is called.** Don't toggle on/off (cost + race with the tracker). Trade-off: slightly more event traffic; user can disable via `tab.cdp()` if needed.
16. **Traversal refresh recursion is unbounded.** Deep traversal chains (el.parent().parent().parent()...) refresh O(depth). Acceptable — real DOMs rarely traverse > 10 levels. If a user constructs a 1000-deep chain and hits stale, they'll get a slow refresh, not a stack overflow (Box<ElementOrigin> uses heap).
17. **`Browser::close` waits for all tabs to detach.** Per-tab InFlightTracker tasks must shut down cleanly. Uses the existing CancellationToken pattern; each tracker has its own child token of `BrowserInner::shutdown`.
18. **Existing `Tab::screenshot() -> Vec<u8>` from P3 keeps working.** It becomes `tab.screenshot_builder().png().bytes().await` under the hood. No breaking change for that call site even though ScreenshotBuilder is new.

## Roadmap

| Phase | Status | Goal |
|---|---|---|
| 1 | DONE | Foundation |
| 2 | DONE | Stealth |
| 3 | DONE | Element + input + actionability |
| **4 (this spec)** | IN PROGRESS | Multi-tab + cookies + storage + frames + nav history + wait_for_idle + traversal refresh + per-Tab input refactor |
| 5 | planned | Optional gated features: interception, cloudflare, expect, fetcher |
| 6 | planned | Polish + crates.io publish |

Sizing: 3-4 weeks solo (comparable to P3).

## Brainstorm cross-ref

Decisions locked during brainstorming:
- **InputController scope**: per-Tab (refactor from P3's per-Browser).
- **Frame model**: first-class type with own session for OOPIF; main_frame + frames() on Tab.
- **Cookies**: Browser-wide jar with CRUD + JSON persistence.
- **Network idle**: 0 in-flight for 500ms quiet window (Playwright `networkidle` semantics), default 30s timeout.
- **Traversal-origin refresh**: implemented; P3 T17 deferral lands here.
- **Multi-tab API**: Browser::new_tab / new_tab_at / tabs / tab_count + Tab::activate / close.
- **Storage**: per-Tab; Tab::local_storage / session_storage returning Storage handle with HashMap-like CRUD.
- **Nav history**: back/forward/reload via Page.getNavigationHistory + navigateToHistoryEntry.
- **ScreenshotBuilder**: full-page + format (PNG/JPEG/WebP) + quality + clip + omit_background.
- **No new external deps** (already have base64, regex, rand, etc).
