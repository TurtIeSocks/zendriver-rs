# zendriver-rs Phase 4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use `- [ ]` checkbox syntax.

**Goal:** Multi-tab + Frame-as-first-class-type + CookieJar + Storage + nav history + wait_for_idle + Traversal-origin refresh chain + InputController per-Tab refactor + ScreenshotBuilder. Land Python-zendriver parity for real-world scraping.

**Architecture:** New `frame/`, `cookies/`, `storage/`, `network_idle/`, `screenshot/` directories under `crates/zendriver/src/`. `BrowserInner` gains a tabs registry; `TabInner` gains an InputController (moved from Browser) + main_frame cache + InFlightTracker + frames registry. Frame is a first-class type with its own SessionHandle (own session for OOPIF, shared with parent Tab for same-origin). CookieJar is browser-wide; Storage is per-Tab.

**Tech Stack:** Same as P3 — Rust + tokio + serde + rand + bitflags + regex + base64. No new external deps.

**Spec:** [docs/superpowers/specs/2026-05-23-zendriver-rs-phase4-multitab-design.md](../specs/2026-05-23-zendriver-rs-phase4-multitab-design.md)

---

## File structure

### `crates/zendriver/src/` modify
- `lib.rs` — `pub mod frame; pub mod cookies; pub mod storage; pub mod network_idle; pub mod screenshot;` + re-exports `Frame, CookieJar, Cookie, SameSite, Storage, ScreenshotBuilder, Format`
- `browser.rs` — `BrowserInner` gains `tabs: RwLock<HashMap<String, Tab>>` + `stealth_input_profile: InputProfile`. Drop `Browser::input()`. Add `Browser::new_tab/new_tab_at/tabs/tab_count`. Build new `TabRegistrar` observer.
- `tab.rs` — `TabInner` gains `input: Arc<InputController>` (moved from BrowserInner) + `main_frame: OnceCell<Frame>` + `network_tracker: Arc<InFlightTracker>` + `frames: RwLock<HashMap<String, Frame>>`. Add `Tab::input()`, `Tab::activate/back/forward/reload/wait_for_idle/wait_for_idle_with`, `Tab::cookies/local_storage/session_storage`, `Tab::main_frame/frames/frame_by_url/frame_by_name`, `Tab::screenshot_builder`. Replace P3 `Tab::screenshot()` with delegate to ScreenshotBuilder.
- `error.rs` — add `FrameNotFound(String)`, `Cookie(String)`, `Storage(String)`, `HistoryNavigation(String)`, `TabNotFound(String)` (if missing).
- `element/refresh.rs` — implement Traversal-origin chain refresh (P3 T17 deferral).
- `element/actions.rs` + `element/input.rs` — drop `tab.browser().upgrade()...input` pattern; use `tab.input()` directly.

### `crates/zendriver/src/frame/` NEW
- `mod.rs` — Frame + FrameInner + main_frame discovery via Page.getFrameTree
- `lifecycle.rs` — Page.frameAttached/Detached/Navigated event handling, frames registry maintenance
- `oopif.rs` — Out-of-process iframe handling via Target.attachedToTarget with type=iframe

### `crates/zendriver/src/cookies/` NEW
- `mod.rs` — Cookie + SameSite + CookieJar + CRUD ops
- `persistence.rs` — save_to_file / load_from_file (JSON)

### `crates/zendriver/src/storage/` NEW
- `mod.rs` — Storage handle + DOMStorage CDP wrappers

### `crates/zendriver/src/network_idle/` NEW
- `mod.rs` — InFlightTracker (subscriber task + in-flight set + Notify)

### `crates/zendriver/src/screenshot/` NEW
- `mod.rs` — ScreenshotBuilder + Format enum

### `crates/zendriver/tests/` modify
- `integration_phase1.rs` / `integration_phase2.rs` / `integration_phase3.rs` — touch only if Tab/Browser/Element signature changes break them
- `integration_phase4.rs` NEW — multi-tab + cookies + storage + frames + nav + wait_for_idle + traversal refresh

### `crates/zendriver/examples/` NEW
- 5 Rust ports of multi-tab/cookies/storage/iframe/nav Python examples

---

## Task list

| # | Title | Files |
|---|---|---|
| 0 | InputController per-Tab refactor | browser.rs, tab.rs, element/actions.rs, element/input.rs, integration_phase1.rs |
| 1 | New ZendriverError variants | error.rs |
| 2 | Browser tabs registry + TabRegistrar observer | browser.rs, tab.rs |
| 3 | Browser::new_tab + new_tab_at + tabs + tab_count | browser.rs |
| 4 | Tab::close upgrade for registry + Target.detachedFromTarget | tab.rs, browser.rs |
| 5 | Tab::activate | tab.rs |
| 6 | Tab::back + forward + reload | tab.rs |
| 7 | InFlightTracker + wait_for_idle | network_idle/mod.rs, tab.rs |
| 8 | CookieJar + Cookie + SameSite types + ops | cookies/mod.rs |
| 9 | Cookie persistence JSON save/load | cookies/persistence.rs |
| 10 | Tab::cookies accessor + Browser::cookies | tab.rs, browser.rs |
| 11 | Storage handle + DOMStorage CRUD | storage/mod.rs, tab.rs |
| 12 | Frame struct + main_frame discovery | frame/mod.rs, tab.rs |
| 13 | Frame::evaluate + evaluate_main + content | frame/mod.rs |
| 14 | Frame::find + find_all (element-scoped wraps) | frame/mod.rs, query/mod.rs |
| 15 | Frame lifecycle events (frameAttached/Detached/Navigated) | frame/lifecycle.rs, tab.rs |
| 16 | OOPIF Frame attach | frame/oopif.rs, browser.rs |
| 17 | FindBuilder::in_frame(&Frame) type-safety upgrade | query/mod.rs, query/selectors.rs |
| 18 | Frame::goto + wait_for_load (main only) | frame/mod.rs |
| 19 | Traversal-origin refresh chain | element/refresh.rs |
| 20 | ScreenshotBuilder + Format enum | screenshot/mod.rs |
| 21 | Tab::screenshot delegates to ScreenshotBuilder | tab.rs |
| 22 | P4 integration tests | tests/integration_phase4.rs |
| 23 | Port 5 Python examples | examples/*.rs |
| 24 | Snapshot regen + README updates | various |

---

## Task 0: InputController per-Tab refactor

This is the largest refactor in P4. Move `Arc<InputController>` from `BrowserInner` to `TabInner`. Drop `Browser::input()`. Element methods use `tab.input()` instead of going through `tab.browser().upgrade()`.

**Files:**
- Modify: `crates/zendriver/src/browser.rs`
- Modify: `crates/zendriver/src/tab.rs`
- Modify: `crates/zendriver/src/element/actions.rs`
- Modify: `crates/zendriver/src/element/input.rs`
- Modify: `crates/zendriver/tests/integration_phase*.rs` (if any use Browser::input)

- [ ] **Step 1: Add `input: Arc<InputController>` field to TabInner + factory wiring**

In `crates/zendriver/src/tab.rs`, modify `TabInner`:

```rust
pub(crate) struct TabInner {
    pub(crate) session: SessionHandle,
    pub(crate) isolated_world: tokio::sync::Mutex<IsolatedWorldCache>,
    pub(crate) browser: std::sync::Weak<crate::browser::BrowserInner>,
    pub(crate) input: std::sync::Arc<crate::input::InputController>,  // NEW
    #[cfg(test)]
    pub(crate) test_input: Option<std::sync::Arc<crate::input::InputController>>,  // P3 T19 leftover; can drop after this refactor
}
```

Update `Tab::new` signature to take the input controller:

```rust
pub(crate) fn new(
    session: SessionHandle,
    browser: std::sync::Weak<crate::browser::BrowserInner>,
    input: std::sync::Arc<crate::input::InputController>,
) -> Self {
    Self {
        inner: std::sync::Arc::new(TabInner {
            session,
            isolated_world: tokio::sync::Mutex::new(IsolatedWorldCache::default()),
            browser,
            input,
            #[cfg(test)]
            test_input: None,
        }),
    }
}
```

Update `Tab::input()`:

```rust
impl Tab {
    pub fn input(&self) -> &std::sync::Arc<crate::input::InputController> {
        &self.inner.input
    }
}
```

Remove the P3 `Tab::input()` impl that returned `Option<Arc<InputController>>` via Weak::upgrade.

For tests that use `Tab::new(sess, Weak::new())` (the P3 pattern), create a test-only helper:

```rust
#[cfg(test)]
impl Tab {
    pub(crate) fn new_for_test(session: SessionHandle) -> Self {
        Self::new(
            session,
            std::sync::Weak::new(),
            crate::input::InputController::new_with_seed(
                zendriver_stealth::InputProfile::native(),
                42,
            ),
        )
    }
}
```

Update all `Tab::new(sess, Weak::new())` call sites to `Tab::new_for_test(sess)`. `grep -rn "Tab::new" crates/zendriver/src crates/zendriver/tests` for the list.

- [ ] **Step 2: Move InputController out of BrowserInner**

In `crates/zendriver/src/browser.rs`, modify `BrowserInner`:

```rust
pub(crate) struct BrowserInner {
    pub(crate) conn: Connection,
    pub(crate) main_tab: Tab,
    pub(crate) child: tokio::sync::Mutex<Option<Child>>,
    pub(crate) _user_data: Option<TempDir>,
    pub(crate) stealth_input_profile: zendriver_stealth::InputProfile,  // NEW; cached for new_tab
    // REMOVED: pub(crate) input: Arc<InputController>
}
```

Drop `impl Browser { pub fn input(&self) -> ... }`.

In `BrowserBuilder::launch`, change the construction:

```rust
// OLD: build input here, pass to BrowserInner
// NEW: build input here, pass to Tab::new instead
let input_profile = self.stealth.as_ref()
    .map_or_else(zendriver_stealth::InputProfile::native, |sp| sp.input_profile());
let input = crate::input::InputController::new(input_profile.clone());

// In the Arc::new_cyclic closure:
let main_tab = Tab::new(session, weak.clone(), input);

BrowserInner {
    conn,
    main_tab,
    child: tokio::sync::Mutex::new(Some(child)),
    _user_data: owned_tmp,
    stealth_input_profile: input_profile,
}
```

- [ ] **Step 3: Update Element methods to use `tab.input()` directly**

Find every `self.tab().browser().upgrade().map(|b| b.input.clone())` pattern in `element/actions.rs` and `element/input.rs`. Replace with `self.tab().input().clone()`.

The `Option`-handling code that errored "no input controller available" goes away — `tab.input()` always returns a valid `&Arc<InputController>` now.

- [ ] **Step 4: Run all P1+P2+P3 tests + integration test builds**

```bash
cargo build --workspace --locked
cargo test --workspace --lib --locked
cargo build --tests --workspace --features integration-tests --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

Expect 154 unit tests still pass. Integration builds clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
refactor(zendriver): move InputController from per-Browser to per-Tab

P3 placed InputController on BrowserInner; P4 multi-tab needs per-Tab
cursor state (each tab tracks its own pointer position). Move the field,
drop Browser::input(), simplify Element method input-controller access
(no more tab.browser().upgrade() dance — tab.input() is direct).

Tests that constructed Tabs with std::sync::Weak::new() switch to a
new Tab::new_for_test helper that wires a deterministic seeded
InputController.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: New ZendriverError variants

**Files:** `crates/zendriver/src/error.rs`

- [ ] **Step 1: TDD — add tests first**

Append to `#[cfg(test)] mod tests` in `crates/zendriver/src/error.rs`:

```rust
    #[test]
    fn display_frame_not_found() {
        let e = ZendriverError::FrameNotFound("F1".into());
        assert_eq!(e.to_string(), "frame not found: F1");
    }

    #[test]
    fn display_cookie() {
        let e = ZendriverError::Cookie("bad domain".into());
        assert_eq!(e.to_string(), "cookie operation failed: bad domain");
    }

    #[test]
    fn display_storage() {
        let e = ZendriverError::Storage("origin mismatch".into());
        assert_eq!(e.to_string(), "storage operation failed: origin mismatch");
    }

    #[test]
    fn display_history_navigation() {
        let e = ZendriverError::HistoryNavigation("no back history".into());
        assert_eq!(e.to_string(), "history navigation failed: no back history");
    }
```

- [ ] **Step 2: Add variants**

In `pub enum ZendriverError`, before the `Serde`/`Io` variants:

```rust
    #[error("frame not found: {0}")]
    FrameNotFound(String),

    #[error("cookie operation failed: {0}")]
    Cookie(String),

    #[error("storage operation failed: {0}")]
    Storage(String),

    #[error("history navigation failed: {0}")]
    HistoryNavigation(String),
```

Also verify `TabNotFound(String)` exists (P1 spec promised it). If missing, add:

```rust
    #[error("tab not found: {0}")]
    TabNotFound(String),
```

- [ ] **Step 3: Verify + commit**

```bash
cargo test -p zendriver --lib error
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
git add crates/zendriver/src/error.rs
git commit -m "feat(zendriver): P4 error variants (FrameNotFound, Cookie, Storage, HistoryNavigation)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

# Tasks 2-24 — Compact form

**Note to implementer:** Tasks 2-24 reference the spec for full code. Patterns from P1-P3 carry forward: TDD with MockConnection, file-scoped allows for test modules, full code via `cargo insta accept` for snapshot tests, `cargo fmt --all` between tasks. The spec at `docs/superpowers/specs/2026-05-23-zendriver-rs-phase4-multitab-design.md` is the source of truth.

For each task: implement per spec, add 1-3 tests covering happy path + 1 error path, verify build + clippy clean, commit with the listed message.

## Task 2: Browser tabs registry + TabRegistrar observer

**Files:** `crates/zendriver/src/browser.rs`, `crates/zendriver/src/tab.rs`

**Spec:** Section "Tab registry inside BrowserInner" + "Multi-tab management"

**Implement:**
- `BrowserInner` gains `tabs: tokio::sync::RwLock<HashMap<String, Tab>>` (key = sessionId).
- New `TabRegistrar` struct in `browser.rs` implementing `TargetObserver`. On `Target.attachedToTarget` event with `target_info.kind == "page"`, register the new Tab. On `Target.detachedFromTarget`, remove. Construct Tab inside the observer with the cached `stealth_input_profile`.
- `BrowserBuilder::launch` registers `TabRegistrar` AFTER `StealthObserver` (order matters; stealth applies first).
- The initial main_tab is special-cased: it's attached BEFORE the observer (P2 flow), so add it to the tabs map manually after construction.

**Test:** mock-driven: emit `Target.attachedToTarget` with `type=page, sessionId=S2`; assert Browser's tabs map now contains 2 entries (main + new).

**Commit:** `feat(zendriver): TabRegistrar observer + Browser tabs registry`

## Task 3: Browser::new_tab + new_tab_at + tabs + tab_count

**Files:** `crates/zendriver/src/browser.rs`

**Spec:** Section "Browser extension" + "Multi-tab management"

**Implement:**
- `Browser::new_tab() -> Result<Tab>` — send `Target.createTarget { url: "about:blank" }`, wait for the `Target.attachedToTarget` event with that targetId, look up the resulting Tab in the registry, return it. Use `tokio::time::timeout(5s)` for the wait.
- `Browser::new_tab_at(url) -> Result<Tab>` — same but with the url param.
- `Browser::tabs() -> Vec<Tab>` — snapshot from the RwLock.
- `Browser::tab_count() -> usize`.

**Test:** mock-driven new_tab: emit Target.attachedToTarget with the right sessionId; assert tabs() returns 2.

**Commit:** `feat(zendriver): Browser::new_tab + new_tab_at + tabs + tab_count`

## Task 4: Tab::close upgrade + Target.detachedFromTarget handling

**Files:** `crates/zendriver/src/tab.rs`, `crates/zendriver/src/browser.rs`

**Implement:**
- P1's `Tab::close(self)` already exists (Target.detachFromTarget). Upgrade so it ALSO sends `Target.closeTarget { targetId }` to actually close the tab in Chrome (not just detach the CDP session). Use `Target.getTargetInfo` to look up our targetId first.
- TabRegistrar handles `Target.detachedFromTarget` event → removes from registry.

**Test:** mock-driven close: assert Target.closeTarget sent + tabs() returns N-1.

**Commit:** `feat(zendriver): Tab::close fully closes the tab + registry cleanup`

## Task 5: Tab::activate

**Files:** `crates/zendriver/src/tab.rs`

**Spec:** Section "Tab extension" — `activate`

**Implement:** `Tab::activate() -> Result<()>` sends `Target.activateTarget { targetId: <self's targetId> }`. Look up targetId via `Target.getTargetInfo` on the tab's session.

**Test:** mock-driven: activate dispatches Target.activateTarget with correct targetId.

**Commit:** `feat(zendriver): Tab::activate brings tab to foreground`

## Task 6: Tab::back + forward + reload

**Files:** `crates/zendriver/src/tab.rs`

**Spec:** Section "Navigation history + wait_for_idle"

**Implement:** Per spec — `Page.getNavigationHistory` to get currentIndex + entries; `Page.navigateToHistoryEntry { entryId }` for back/forward. `Page.reload { ignoreCache: false }` for reload. Errors with `HistoryNavigation("no back history")` if `currentIndex <= 0`.

**Test:** 3 mock-driven tests (back happy path, back errors on no history, reload sends Page.reload).

**Commit:** `feat(zendriver): Tab::back + forward + reload via Page.navigateToHistoryEntry`

## Task 7: InFlightTracker + wait_for_idle

**Files:** `crates/zendriver/src/network_idle/mod.rs`, `crates/zendriver/src/tab.rs`

**Spec:** Section "wait_for_idle"

**Implement:** Per spec — `InFlightTracker` with `in_flight: Mutex<HashSet<String>>` + `notifier: Notify`. `run()` spawns a task subscribing to Network.* events, increments on requestWillBeSent, decrements on responseReceived/loadingFailed/loadingFinished, notifies on every state change. Tab spawns the tracker at construction (in `Tab::new`). `wait_for_idle` + `wait_for_idle_with` poll the tracker with the quiet-window logic.

**Tests:**
- mock-driven InFlightTracker: emit requestWillBeSent + responseReceived → in_flight set transitions through 1 then 0.
- wait_for_idle quiet_window: in_flight goes 0 then becomes 1 within 500ms → does NOT return early; goes back to 0 + stays for 500ms → returns Ok.

**Commit:** `feat(zendriver): InFlightTracker + Tab::wait_for_idle / wait_for_idle_with`

## Task 8: CookieJar + Cookie + SameSite + ops

**Files:** `crates/zendriver/src/cookies/mod.rs`

**Spec:** Section "CookieJar"

**Implement:** Per spec — Cookie struct (serde Serialize+Deserialize), SameSite enum, CookieJar wrapper around Connection. Methods all/for_url/set/set_many/delete/clear → CDP Network.getAllCookies / setCookie / setCookies / deleteCookies / clearBrowserCookies.

**Tests:**
- CookieJar.all parses Network.getAllCookies response
- CookieJar.set sends Network.setCookie with correct payload
- CookieJar.delete sends Network.deleteCookies with name/domain/path filters

**Commit:** `feat(zendriver): CookieJar + Cookie + SameSite + CRUD via Network domain`

## Task 9: Cookie persistence JSON save/load

**Files:** `crates/zendriver/src/cookies/persistence.rs`

**Implement:** `CookieJar::save_to_file(path)` calls all() then serializes Vec<Cookie> to JSON via serde_json + writes file via tokio::fs. `load_from_file(path)` reads + deserializes + calls set_many.

**Tests:**
- save+load round-trip on a tempfile preserves the cookie set
- save_to_file errors gracefully on bad path (delegated to io::Error via thiserror From)

**Commit:** `feat(zendriver): CookieJar JSON save_to_file + load_from_file`

## Task 10: Tab::cookies accessor + Browser::cookies

**Files:** `crates/zendriver/src/tab.rs`, `crates/zendriver/src/browser.rs`

**Implement:**
- `Browser::cookies() -> CookieJar` — constructs a CookieJar wrapping the Browser's Connection.
- `Tab::cookies() -> CookieJar` — delegates to Browser's cookies via Weak upgrade. If browser already dropped → empty `CookieJar::dummy()` or just unwrap Weak (Drop ordering should mean Tabs outlive Browser).

**Test:** mock-driven: tab.cookies().set(...) dispatches Network.setCookie on the Browser's connection.

**Commit:** `feat(zendriver): Browser::cookies + Tab::cookies accessors`

## Task 11: Storage handle + DOMStorage CRUD

**Files:** `crates/zendriver/src/storage/mod.rs`, `crates/zendriver/src/tab.rs`

**Spec:** Section "Storage"

**Implement:** Storage struct + StorageInner wrapping SessionHandle + is_local flag. Methods get/get_all/set/set_many/remove/clear/len dispatch to DOMStorage.* CDP commands. Each call fetches current tab origin via Tab::url() to build the StorageId.

Tab::local_storage() and Tab::session_storage() return Storage handles configured with the matching is_local value.

**Tests:**
- Storage.get dispatches DOMStorage.getDOMStorageItems with isLocalStorage=true
- Storage.set dispatches DOMStorage.setDOMStorageItem with key/value

**Commit:** `feat(zendriver): Tab::local_storage + session_storage via DOMStorage domain`

## Task 12: Frame struct + main_frame discovery

**Files:** `crates/zendriver/src/frame/mod.rs`, `crates/zendriver/src/tab.rs`

**Spec:** Section "Frame struct"

**Implement:**
- Frame struct + FrameInner per spec (frame_id, parent_frame_id, url, name, session, isolated_world cache, tab Weak ref).
- Frame::id/url/name/is_main/parent_id accessors.
- `Tab::main_frame() -> Result<Frame>` — uses tokio::sync::OnceCell. On first call: `Page.getFrameTree` → extract top-level frame → construct Frame with `session = self.session.clone()` (main frame shares Tab's session).

**Test:** mock-driven main_frame: Page.getFrameTree response → Frame with correct frame_id + url + no parent.

**Commit:** `feat(zendriver): Frame struct + Tab::main_frame discovery via Page.getFrameTree`

## Task 13: Frame::evaluate + evaluate_main + content

**Files:** `crates/zendriver/src/frame/mod.rs`

**Implement:**
- `Frame::evaluate<T>(js)` — same isolated-world pattern as Tab::evaluate but per-frame contextId. Uses `Page.createIsolatedWorld { frameId: self.id, worldName: "zendriver-eval" }` for first call, caches contextId in `inner.isolated_world`.
- `Frame::evaluate_main<T>(js)` — `Runtime.evaluate` with contextId=null but using Frame's session (works for both same-origin and OOPIF frames since each has its own session in OOPIF case).
- `Frame::content() -> Result<String>` — `evaluate_main` of `document.documentElement.outerHTML`.

**Tests:**
- Frame::evaluate caches contextId across calls
- Frame::evaluate_main does not call Page.createIsolatedWorld

**Commit:** `feat(zendriver): Frame::evaluate + evaluate_main + content`

## Task 14: Frame::find + find_all

**Files:** `crates/zendriver/src/frame/mod.rs`, `crates/zendriver/src/query/mod.rs`

**Implement:**
- New `QueryScope::Frame(&'a Frame)` variant in `query::selectors`.
- `FindBuilder::new_for_frame(&Frame)` constructor.
- `Frame::find()` / `Frame::find_all()` accessors.
- SelectorKind::resolve_* dispatch updated to handle QueryScope::Frame (uses frame's session + isolated context).

**Test:** mock-driven: frame.find().css("button").one() dispatches Runtime.evaluate on the Frame's session.

**Commit:** `feat(zendriver): Frame::find + find_all (QueryScope::Frame)`

## Task 15: Frame lifecycle events

**Files:** `crates/zendriver/src/frame/lifecycle.rs`, `crates/zendriver/src/tab.rs`

**Implement:** TabInner gains `frames: RwLock<HashMap<String, Frame>>`. On Tab construction, spawn a background task that subscribes to `Page.frameAttached`, `Page.frameDetached`, `Page.frameNavigated` events on the tab's session. Maintains the frames registry. Updates Frame::url on navigation. Tab::frames() returns snapshot of registry; Tab::frame_by_url/frame_by_name look up by predicate.

**Test:** mock-driven: emit Page.frameAttached → tab.frames() includes the new frame.

**Commit:** `feat(zendriver): Frame lifecycle events + Tab::frames/frame_by_url/frame_by_name`

## Task 16: OOPIF Frame attach

**Files:** `crates/zendriver/src/frame/oopif.rs`, `crates/zendriver/src/browser.rs`

**Implement:** Extend TabRegistrar observer (from T2): on `Target.attachedToTarget` with `target_info.kind == "iframe"`, construct a Frame with the new SessionHandle and register it in the *parent Tab's* frames map. Parent Tab is identified by walking up the frame tree or via `target_info.openerFrameId` (CDP-dependent).

**Test:** mock-driven: emit Target.attachedToTarget with type=iframe → parent tab's frames() includes the OOPIF Frame.

**Commit:** `feat(zendriver): OOPIF Frame attach via Target.attachedToTarget`

## Task 17: FindBuilder::in_frame(&Frame) type-safety upgrade

**Files:** `crates/zendriver/src/query/mod.rs`, `crates/zendriver/src/query/selectors.rs`

**Implement:** P3's `FindBuilder::in_frame(frame_id: String)` becomes `FindBuilder::in_frame<'a>(self, frame: &'a Frame) -> FindBuilder<'a>`. Internally stores the Frame's session and contextId; SelectorKind::resolve_* routes to that session.

**Test:** mock-driven: find().in_frame(&frame).css("...").one() dispatches on the frame's session.

**Commit:** `feat(zendriver): FindBuilder::in_frame(&Frame) type-safe upgrade`

## Task 18: Frame::goto + wait_for_load (main only)

**Files:** `crates/zendriver/src/frame/mod.rs`

**Implement:**
- `Frame::goto(url)` — errors if !is_main with `Navigation("sub-frame goto not supported")`. For main frame, identical to Tab::goto using Frame's session.
- `Frame::wait_for_load()` — subscribes to Page.frameStoppedLoading on Frame's session, waits for the event with matching frameId.

**Tests:**
- main frame goto sends Page.navigate
- sub-frame goto errors

**Commit:** `feat(zendriver): Frame::goto + wait_for_load (main frame only)`

## Task 19: Traversal-origin refresh chain

**Files:** `crates/zendriver/src/element/refresh.rs`

**Spec:** Section "Traversal-origin refresh chain"

**Implement:** Replace P3's `Err(NotRefreshable)` for Traversal origin with the recursive resolve_origin per spec: recursively resolve parent's origin, synthesize temporary parent Element via from_jsret, re-traverse via the TraversalKind (Parent or NthChild(idx)).

Add `extract_remote_ref(value, tab)` helper to extract a RemoteRef from a JS call result (objectId → DOM.describeNode → backendNodeId).

**Tests:**
- Traversal refresh with stale parent re-resolves recursively
- 2-level traversal chain (parent.parent) refreshes both levels

**Commit:** `feat(zendriver): Element refresh now handles Traversal origin (P3 T17 deferral)`

## Task 20: ScreenshotBuilder + Format enum

**Files:** `crates/zendriver/src/screenshot/mod.rs`

**Spec:** Section "ScreenshotBuilder"

**Implement:** Per spec — ScreenshotBuilder struct + Format enum + builder methods (png/jpeg/webp/full_page/clip/quality/omit_background) + bytes()/save() terminals.

`bytes()`:
- If full_page=true: call Page.getLayoutMetrics, compute doc dimensions, send Page.captureScreenshot { format, captureBeyondViewport: true, clip: { x:0, y:0, width:doc_w, height:doc_h, scale:1 }, optionalParameters... }.
- Else: Page.captureScreenshot { format, clip: <if Some>, optionalParameters... }.
- Decode base64 from response.data.

`save(path)` calls bytes() + writes via tokio::fs.

`quality(q)` errors at build time only valid for JPEG — runtime check, panics on misuse.

**Tests:**
- ScreenshotBuilder default = PNG, no clip, no full_page
- jpeg().quality(80) round-trip sets format + quality
- full_page sends captureBeyondViewport=true with clip set to layout metrics dims

**Commit:** `feat(zendriver): ScreenshotBuilder with PNG/JPEG/WebP + full-page + clip`

## Task 21: Tab::screenshot delegates to ScreenshotBuilder

**Files:** `crates/zendriver/src/tab.rs`

**Implement:**
- `Tab::screenshot_builder() -> ScreenshotBuilder<'_>` constructor.
- `Tab::screenshot() -> Result<Vec<u8>>` becomes `self.screenshot_builder().png().bytes().await`.

**Test:** verify Tab::screenshot calls ScreenshotBuilder chain (mock dispatches Page.captureScreenshot).

**Commit:** `feat(zendriver): Tab::screenshot delegates to ScreenshotBuilder`

## Task 22: P4 integration tests

**Files:** `crates/zendriver/tests/integration_phase4.rs` (NEW)

**Spec:** Section "Tier 2 — Integration"

**Implement:** ~10 tokio tests `#![cfg(feature = "integration-tests")]` + `#[serial]`:
1. new_tab opens a second tab; tabs().len() == 2
2. tab.close removes from registry
3. cookies.set + cookies.all round-trip
4. cookies save+load round-trip on tempfile
5. local_storage.set + get + clear
6. back/forward/reload nav history
7. wait_for_idle on SPA fixture with delayed XHR
8. Frame::find inside iframe fixture
9. Traversal refresh after location.reload()
10. ScreenshotBuilder.full_page captures beyond viewport

**Build only — don't run locally.**

**Commit:** `test(zendriver): P4 integration tests for multi-tab/cookies/storage/frames/nav/idle`

## Task 23: Port 5 Python examples

**Files:** `crates/zendriver/examples/*.rs`

**Implement:** Pick 5 Python examples exercising P4 surface:
- multi_tab (Browser::new_tab + iterate)
- iframe (Frame::find inside iframe)
- cookies (save/load across sessions)
- storage (localStorage round-trip)
- navigation (back/forward/reload)

If specific Python examples don't exist, synthesize equivalents from spec. Each compiles via `cargo build --examples --workspace --locked`.

**Commit:** `examples(zendriver): port 5 P4-flavored Python examples to Rust`

## Task 24: Snapshot regen + README updates

**Implement:**
- Run `cargo test --workspace --lib --locked` — if snapshots drifted, `cargo insta accept`
- Run `cargo fmt --all`
- Update README: Phase 4 → DONE; example shows multi-tab + cookies usage
- Final gate: `cargo test --workspace --lib --doc --locked`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo fmt --all --check` all clean

**Commit:** `chore: post-P4 snapshot regen + README updates`

---

## Self-review checklist

**Spec coverage:** every section in the spec maps to T0-T24. Per-Tab input → T0. Errors → T1. Multi-tab → T2-T5. Nav history → T6. wait_for_idle → T7. Cookies → T8-T10. Storage → T11. Frames → T12-T18. Traversal refresh → T19. Screenshots → T20-T21. Integration → T22-T23. Polish → T24.

**Placeholder scan:** none. T2-T24 are compact but reference spec sections + show explicit verification commands + commit messages.

**Type consistency:** Cookie/SameSite/CookieJar/Storage/Frame/FrameInner/InFlightTracker/ScreenshotBuilder/Format names consistent. `Tab::new` signature change from T0 propagates through all later tasks (use Tab::new_for_test in tests).

---

## Notes for the implementing engineer

1. **T0 is the riskiest task.** All P3 tests that constructed Tabs need migration to `Tab::new_for_test`. Grep first; expect ~16 callsites. Run full test suite after migration.
2. **The TabRegistrar observer (T2) must run AFTER StealthObserver.** Add to the launch flow's observers Vec in the right order.
3. **OOPIF (T16) is tricky in practice.** Determining the parent Tab for a new iframe target may require `Page.frameAttached` correlation or `targetInfo.openerFrameId`. Test with a real cross-origin iframe fixture to verify the parent-attribution logic.
4. **wait_for_idle (T7) needs Network.enable on the session.** Tab construction needs to send this before the InFlightTracker task starts. Add to Tab::new (or to the TabRegistrar observer that constructs the Tab).
5. **Storage::get_all returns HashMap<String, String>.** Order from DOMStorage is not guaranteed; if a test depends on order, sort first.
6. **For tasks marked "compact form", aim for 1-3 unit tests per task.** Don't over-test; integration tests in T22 cover end-to-end.
7. **`#[allow(clippy::result_large_err)]`, `#[allow(clippy::panic, clippy::unwrap_used)]` on test modules** — established patterns from P1-P3 carry forward.
8. **Branch is `worktree-phase4-multitab`** in worktree under `.claude/worktrees/phase4-multitab/`.
