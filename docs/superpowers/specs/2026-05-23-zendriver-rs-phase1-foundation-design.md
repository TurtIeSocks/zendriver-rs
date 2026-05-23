# zendriver-rs — Phase 1: Foundation

**Date:** 2026-05-23
**Status:** Approved (brainstorming complete, ready for implementation plan)
**Phase:** 1 of 6 (foundation only — see [Roadmap](#roadmap))

## Summary

Port [zendriver](https://github.com/cdpdriver/zendriver) (Python, AGPL-3) to Rust as an idiomatic, async-first CDP browser automation library. Build the transport layer, minimal `Browser` / `Tab` / `Element` surface, and unit-test infrastructure. Stealth, full element API, multi-tab, interception, and Cloudflare bypass are out of scope for this phase — they ship in P2–P5.

Goal of Phase 1: prove the CDP loop works end-to-end with a small, complete vertical slice. End state: a user can write `Browser::builder().launch().await?`, navigate to a URL, find an element by CSS, click it, and read text back.

## Goals

- A working CDP transport (WebSocket I/O, command/response routing, event broadcast) built on `tokio-tungstenite` and `tokio` primitives.
- Public API: `Browser`, `BrowserBuilder`, `Tab`, `Element`, `FindBuilder` (subset).
- Typed error hierarchy (`ZendriverError`) per Apollo best practices.
- A `MockConnection` for unit-testing driver code without spawning Chrome.
- Workspace skeleton accommodating all six phases (crates exist as empty stubs where P1 doesn't touch them).
- CI matrix: unit + doc + clippy + fmt on every PR; one integration test against `wiremock`.
- Dual MIT / Apache-2.0 license.

## Non-goals

Explicitly **out of scope** for Phase 1; deferred to later phases:

- Stealth patches, launch-flag hardening, `Target.setAutoAttach { waitForDebuggerOnStart }` flow (P2).
- xpath / text / role selectors, `nth`, `visible`, iframe scoping, element traversal, hover / focus / scroll / press / file upload (P3).
- Cookies, storage, screenshots, multi-tab, `wait_for_idle`, navigation history (P4).
- Network interception, Cloudflare bypass, `expect()` API, Chrome fetcher (P5).
- mdBook docs site, examples directory beyond `hello.rs`, crates.io publish (P6).
- Anti-detection testing against `bot.sannysoft.com` (P2 exit criterion, not P1).

## Architecture

### Workspace layout

```
zendriver-rs/
├── Cargo.toml                          # workspace root
├── crates/
│   ├── zendriver/                      # high-level driver (public face)
│   ├── zendriver-transport/            # WS + CDP routing actor (internal)
│   ├── zendriver-stealth/              # empty stub in P1; populated in P2
│   ├── zendriver-cloudflare/           # empty stub; populated in P5
│   ├── zendriver-interception/         # empty stub; populated in P5
│   └── zendriver-fetcher/              # empty stub; populated in P5
├── examples/
│   └── hello.rs                        # Phase 1 exit example
├── docs/superpowers/specs/             # design + spec markdown
└── tests/integration/                  # cross-crate integration tests, gated
```

Phase 1 implements only `zendriver/` and `zendriver-transport/`. The other crate directories exist with skeleton `Cargo.toml` + `src/lib.rs` containing `// populated in phase N` so the workspace builds cleanly and future phases land without restructuring.

### Dependency graph

```
zendriver
  ├─ zendriver-transport
  └─ chromiumoxide_cdp (external; CDP type bindings only)
```

No dependency on `chromiumoxide` itself — only on its `chromiumoxide_cdp` subcrate for generated CDP `Command` / `Event` / type definitions.

### External dependencies (Phase 1)

| Crate | Purpose | Notes |
|---|---|---|
| `chromiumoxide_cdp` | CDP type bindings (~60k LOC of generated code) | Pinned to a specific version; review on every bump |
| `tokio` (features: `full`) | Async runtime + process spawning | |
| `tokio-tungstenite` | WebSocket client | TLS not needed for local Chrome WS; `native-tls` feature only if `connect(ws_url)` to remote |
| `serde` / `serde_json` | CDP frame encoding | Already required transitively by `chromiumoxide_cdp` |
| `thiserror` | Error hierarchy | Library convention |
| `tracing` | Instrumentation | All actor + connection ops emit spans |
| `futures` | Stream combinators | Specifically `BroadcastStream`, `stream::select` |
| `tokio-util` | `CancellationToken` | Graceful shutdown |
| `url` | URL parsing / validation | Public `Tab::url()` return type |

Dev-dependencies: `tokio-test`, `wiremock`, `insta`, `serial_test`.

## Components

### `zendriver-transport`

The hardest part of the system. Owns the WebSocket, frame serialization, command/response correlation, event fan-out, target session lifecycle. Pure async, no shared mutable state, no locks across `.await`.

#### Frame model

```rust
// Outbound: caller → Chrome
#[derive(Serialize)]
struct CdpCommand<'a, P: Serialize> {
    id: u64,
    method: &'a str,           // e.g. "Page.navigate"
    params: P,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>, // for target-attached calls
}

// Inbound: Chrome → caller
#[derive(Deserialize)]
#[serde(untagged)]
enum CdpInbound {
    Response { id: u64, result: Option<Value>, error: Option<CdpRpcError> },
    Event   { method: String, params: Value, session_id: Option<String> },
}
```

`chromiumoxide_cdp` already exposes typed `Command` impls (`Page::Navigate`, etc.). We serialize their `params` field and deserialize responses into their `Response` types. The transport owns just the envelope (`id`, `method`, `session_id`).

#### Connection actor

One `tokio::task` owns the WebSocket. Communicates via two channels — no shared mutable state, no `Mutex` across `.await`.

```rust
pub struct Connection {
    cmd_tx: mpsc::Sender<OutboundCmd>,
    event_bus: broadcast::Sender<RawEvent>,
    shutdown: CancellationToken,
}

struct ConnectionActor {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    pending: HashMap<u64, oneshot::Sender<Result<Value, CdpRpcError>>>,
    next_id: u64,
    cmd_rx: mpsc::Receiver<OutboundCmd>,
    event_tx: broadcast::Sender<RawEvent>,
    shutdown: CancellationToken,
}

struct OutboundCmd {
    method: &'static str,
    params: Value,
    session_id: Option<String>,
    reply: oneshot::Sender<Result<Value, CdpRpcError>>,
}
```

Actor loop:

```rust
loop {
    tokio::select! {
        biased;  // shutdown wins on tie
        _ = self.shutdown.cancelled() => break,
        cmd = self.cmd_rx.recv() => self.send_cmd(cmd).await?,
        frame = self.ws.next() => self.dispatch_frame(frame).await?,
    }
}
```

On exit: drain pending into `Err(TransportError::Shutdown)` so callers see a typed error rather than hanging.

#### Command flow

```
caller                  Connection                ConnectionActor               Chrome
  │                         │                            │                        │
  │ call(Page::Navigate)    │                            │                        │
  ├────────────────────────▶│                            │                        │
  │                         │ OutboundCmd{reply: tx}     │                        │
  │                         ├───────────────────────────▶│                        │
  │                         │                            │ id=N; pending[N]=tx    │
  │                         │                            ├──── WS write JSON ────▶│
  │                         │                            │◀──── WS read JSON ─────┤
  │                         │                            │ pending.remove(N).send │
  │ oneshot rx.await        │                            │                        │
  │◀────────────────────────┴────────────────────────────┘                        │
```

#### Event subscription

```rust
impl Connection {
    /// Generic event stream typed by the CDP event marker.
    pub fn subscribe<E>(&self) -> impl Stream<Item = E>
    where
        E: chromiumoxide_cdp::IntoEventKind + DeserializeOwned + 'static,
    {
        BroadcastStream::new(self.event_bus.subscribe())
            .filter_map(|res| async move {
                let raw = res.ok()?;
                if raw.method == E::EVENT_NAME {
                    serde_json::from_value(raw.params).ok()
                } else {
                    None
                }
            })
    }
}
```

`broadcast::Sender` (not `mpsc`) because a single CDP event may have N consumers (stealth, interception, user observer). Lagged subscribers drop frames — that's the right behavior; slow handlers must not backpressure Chrome.

#### Target / session model

P1 only handles **one** target: the initial blank page after `Browser::launch()`. We attach via `Target.attachToTarget { flatten: true }` and store the resulting `sessionId` in the single `Tab`.

P2 generalizes this to `Target.setAutoAttach { waitForDebuggerOnStart, flatten }` plus the `TargetObserver` trait for stealth injection. P1 reserves the public hook (a `Vec<Arc<dyn TargetObserver>>` field in `ConnectionActor` initialized empty) so P2 doesn't break ABI.

#### Cancellation and Drop

- `Browser` owns the `CancellationToken`.
- `Browser::drop` (or explicit `Browser::close`) → cancel → actor exits → WS closes → subprocess gets `SIGTERM` with a 5s grace period before `SIGKILL`.
- Tab drops mid-`await`: the user's awaited `oneshot::Receiver` is dropped; actor's `pending[N]` becomes a dead sender; actor skips on next response (or removes lazily during pending-map drain).
- No `tokio::spawn` of orphan tasks. All lifetimes tied to `Browser`.

### `zendriver`

The public-facing crate. Owns `Browser`, `Tab`, `Element`, `FindBuilder`, `Config`, error types.

#### Public surface (Phase 1)

```rust
// crates/zendriver/src/lib.rs

pub use crate::browser::{Browser, BrowserBuilder};
pub use crate::tab::Tab;
pub use crate::element::Element;
pub use crate::query::FindBuilder;
pub use crate::error::{ZendriverError, Result};

/// Convenience entry point. Equivalent to `Browser::builder().launch().await`.
pub async fn start() -> Result<Browser>;
```

#### `Browser`

```rust
pub struct Browser { /* Arc<Inner>; cheap to clone */ }

impl Browser {
    pub fn builder() -> BrowserBuilder;

    /// Returns the initial blank tab attached at launch time.
    /// In P1, `Browser` exposes exactly one tab.
    pub fn main_tab(&self) -> Tab;

    pub async fn close(self) -> Result<()>;

    /// Escape hatch: raw CDP connection for commands not exposed by the high-level API.
    pub fn cdp(&self) -> &Connection;
}

pub struct BrowserBuilder { /* … */ }

impl BrowserBuilder {
    pub fn headless(self, on: bool) -> Self;             // default: true
    pub fn executable(self, path: impl Into<PathBuf>) -> Self;
    pub fn user_data_dir(self, path: impl Into<PathBuf>) -> Self;
    pub fn arg(self, flag: impl Into<String>) -> Self;
    pub fn args(self, flags: impl IntoIterator<Item = String>) -> Self;

    /// Spawn a new Chrome process and attach.
    pub async fn launch(self) -> Result<Browser>;

    /// Attach to an already-running Chrome at the given WS endpoint.
    pub async fn connect(self, ws_url: impl Into<String>) -> Result<Browser>;
}
```

Defaults in P1: `headless = true`, no stealth, user-data-dir = `tempfile::TempDir`, executable auto-discovered via PATH lookups (`google-chrome`, `chromium`, `chrome`, `Chromium`, `Google Chrome` on macOS). Auto-discovery failure → `BrowserError::ExecutableNotFound`.

#### `Tab`

```rust
pub struct Tab { /* Arc<Inner>, holds session_id + Connection */ }

impl Tab {
    // Navigation (subset for P1)
    pub async fn goto(&self, url: impl AsRef<str>) -> Result<()>;
    pub async fn wait_for_load(&self) -> Result<()>;

    // Query
    pub fn find(&self) -> FindBuilder<'_>;

    // JS evaluation (Runtime.evaluate)
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T>;

    // Page state (subset)
    pub async fn url(&self) -> Result<url::Url>;
    pub async fn title(&self) -> Result<String>;

    // Close
    pub async fn close(self) -> Result<()>;

    // Escape hatch
    pub fn session(&self) -> SessionHandle<'_>;
}
```

#### `FindBuilder` (Phase 1 — CSS selector only)

```rust
pub struct FindBuilder<'tab> { tab: &'tab Tab, /* selector accumulator */ }

impl<'tab> FindBuilder<'tab> {
    pub fn css(self, selector: impl Into<String>) -> Self;
    pub fn timeout(self, dur: Duration) -> Self;        // default 10s

    pub async fn one(self) -> Result<Element>;
    pub async fn one_or_none(self) -> Result<Option<Element>>;
}
```

xpath, text, text_regex, role, in_frame, nth, visible, `many()` deferred to P3. In P1 the `FindBuilder` type literally has only the methods shown above — adding more is the work of P3. Type-level rejection at call site; no runtime "unsupported selector" error path.

#### `Element` (Phase 1 minimal)

```rust
pub struct Element { /* Arc<Inner>: backend_node_id + remote_object_id + Tab ref */ }

impl Element {
    pub async fn click(&self) -> Result<()>;            // DOM dispatch via Runtime.callFunctionOn
    pub async fn inner_text(&self) -> Result<String>;
    pub async fn outer_html(&self) -> Result<String>;
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T>;
}
```

Click in P1: `Runtime.callFunctionOn` with `function: "function(){ this.click(); }"`. P3 upgrades to realistic mouse-move + `Input.dispatchMouseEvent`. (P1 click works for most use cases; doesn't simulate user gestures convincingly enough to defeat advanced bot checks, which is fine because stealth is P2 anyway.)

### `MockConnection` (testing infrastructure)

Lives at `zendriver-transport/src/testing.rs`, gated behind `cfg(any(test, feature = "testing"))`. The `testing` feature is also exposed by the `zendriver` crate so downstream users can mock their integrations.

```rust
pub struct MockConnection {
    pub server_tx: mpsc::Sender<serde_json::Value>,
    pub client_rx: mpsc::Receiver<serde_json::Value>,
    inner: Arc<ConnectionInner>,
}

impl MockConnection {
    /// Create a paired (mock-side, driver-side) connection.
    pub fn pair() -> (Self, Connection);

    /// Block until the driver sends a command whose `method` field matches.
    /// Returns the command id (for use with `reply`). Wrap in
    /// `tokio::time::timeout` at test sites — there is no built-in timeout.
    pub async fn expect_cmd(&mut self, method: &str) -> u64;

    /// Inspect the most recent outbound frame (for assertions on params).
    pub fn last_sent(&self) -> &serde_json::Value;

    /// Reply to a previously-expected command with a successful result.
    pub async fn reply(&self, id: u64, result: serde_json::Value);

    /// Reply to a command with a CDP-style RPC error.
    pub async fn reply_err(&self, id: u64, code: i32, msg: &str);

    /// Inject an unsolicited event into the driver's event bus.
    pub async fn emit_event(&self, method: &str, params: serde_json::Value);
}
```

## Data flow

### Browser launch sequence

1. `BrowserBuilder::launch` resolves the Chrome executable (PATH lookup, or builder override).
2. Construct args: `--remote-debugging-port=0`, `--user-data-dir=<temp>`, `--headless=new` if headless, plus user-supplied flags.
3. Spawn Chrome via `tokio::process::Command`, capture stderr.
4. Watch stderr for the `DevTools listening on ws://127.0.0.1:<port>/devtools/browser/<uuid>` line; parse out the WS URL. Timeout: 10s.
5. Open WS via `tokio_tungstenite::connect_async`.
6. Spawn `ConnectionActor`; return `Connection` handle.
7. Send `Target.attachToTarget { targetId: <main>, flatten: true }`; receive `Target.attachedToTarget { sessionId }`.
8. Wrap session in a `Tab`; return `Browser` with the tab as `main_tab`.

### Navigation + element interaction

```
user                     Tab                Connection             Chrome
  │  goto(url)            │                       │                  │
  ├──────────────────────▶│ Page.navigate         │                  │
  │                       ├──────────────────────▶│──── WS send ────▶│
  │                       │◀─────── { frameId } ──┤◀──── WS recv ────┤
  │                       │                       │                  │
  │  wait_for_load()      │                       │                  │
  ├──────────────────────▶│ subscribe<Page.FrameStoppedLoading>      │
  │                       │  wait for frameId match                  │
  │                       │◀── Page.FrameStoppedLoading ─────────────┤
  │◀──── Ok(()) ──────────┤                                          │
  │                       │                                          │
  │  find().css(...)      │                                          │
  │   .one()              │ DOM.querySelector { selector }           │
  ├──────────────────────▶├──────────────────────▶│──── WS send ────▶│
  │                       │◀──── { nodeId } ──────┤◀──── WS recv ────┤
  │                       │ DOM.resolveNode { nodeId }               │
  │                       ├──────────────────────▶│──── WS send ────▶│
  │                       │◀── { remoteObjectId } ┤◀──── WS recv ────┤
  │◀── Element ───────────┤                                          │
  │  .click()             │ Runtime.callFunctionOn                   │
  ├──────────────────────▶├──────────────────────▶│──── WS send ────▶│
  │                       │◀── Ok ────────────────┤◀──── WS recv ────┤
  │◀── Ok(()) ────────────┤                                          │
```

### Shutdown sequence

```
Browser::drop or Browser::close
   │
   ├── shutdown.cancel()
   │     │
   │     └── ConnectionActor: tokio::select! picks shutdown branch → loop break
   │             │
   │             ├── drain pending → reply Err(TransportError::Shutdown) on every oneshot
   │             ├── ws.close() with normal close code
   │             └── return Ok from actor task
   │
   ├── child.kill() with SIGTERM
   ├── tokio::time::timeout(5s, child.wait()) — graceful exit
   ├── on timeout: child.kill() with SIGKILL
   └── TempDir drop → cleanup user_data_dir
```

## Error handling

Top-level error type, per [Section 4 of the brainstorm](#brainstorm-cross-ref):

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ZendriverError {
    #[error("browser process failed: {0}")]
    Browser(#[from] BrowserError),

    #[error("transport: {0}")]
    Transport(#[from] TransportError),

    #[error("CDP RPC error [{code}] {message}")]
    Cdp { code: i32, message: String, data: Option<serde_json::Value> },

    #[error("element not found: {selector}")]
    ElementNotFound { selector: String },

    #[error("timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("navigation failed: {0}")]
    Navigation(String),

    #[error("javascript exception: {0}")]
    JsException(String),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = ZendriverError> = std::result::Result<T, E>;
```

Per-crate error types (`BrowserError`, `TransportError`) own their narrow domain and convert into `ZendriverError` via `#[from]`. `#[non_exhaustive]` on the public enum so P2+ can add variants (e.g. `Stealth(StealthError)`) without a breaking change.

Conventions (apply to all P1 code):

- No `panic!` / `unwrap` / `expect` outside `#[cfg(test)]` modules.
- `anyhow` never in the dep tree of any published crate.
- Every wrapped error uses `#[source]` (or `#[from]` which implies `#[source]`) to preserve the chain.
- Timeouts always typed `ZendriverError::Timeout(Duration)` — never collapsed into a generic error.
- Element-not-found is its own variant for clean `match` arms in user code.

## Testing

Four tiers — Phase 1 implements all four in skeleton form so later phases just add tests, not infrastructure.

### Tier 1 — Unit tests (mocked CDP, every commit)

Use `MockConnection::pair()` to test driver code without spawning Chrome.

Phase 1 unit-test coverage targets:
- Connection actor: command id assignment, pending-map eviction on response, broadcast fan-out, shutdown ordering, pending-drain on shutdown, lagged subscriber behavior.
- Frame serialization: `Page.navigate` payload shape, `sessionId` routing, params skipped when None.
- Error translation: CDP `-32602` → `ZendriverError::Cdp`, `"Cannot find context"` → `ZendriverError::Navigation`, `Disconnected` propagates as `TransportError`.
- `Tab::goto` happy path + invalid URL.
- `FindBuilder::css(...).one()` sends `DOM.querySelector` with correct payload; missing element → `ZendriverError::ElementNotFound`.
- `Element::click` invokes `Runtime.callFunctionOn` on the correct `remoteObjectId`.
- `Element::evaluate<T>` correctly deserializes the `Runtime.evaluate` result.
- Browser process lifecycle: WS URL parsed from stderr, executable-not-found error, spawn failure mapped, devtools timeout.
- All `ZendriverError` variants' `Display` output (one assertion per test).

### Tier 2 — Integration test (real Chrome, gated by `integration-tests` feature)

Single Phase 1 integration test: spawn Chrome via `BrowserBuilder`, navigate to a `wiremock` fixture, find an element by CSS, click it, read state back via `Tab::evaluate`.

```rust
#[tokio::test]
#[cfg(feature = "integration-tests")]
async fn phase1_click_dispatches_event_to_dom_listener() {
    let mock = MockServer::start().await;
    Mock::given(method("GET")).and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"
            <!doctype html>
            <button id="b" onclick="window.clicked = true">x</button>
        "#))
        .mount(&mock).await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.find().css("#b").one().await.unwrap().click().await.unwrap();

    let clicked: bool = tab.evaluate("window.clicked").await.unwrap();
    assert!(clicked);
}
```

### Tier 3 — Snapshot tests (`cargo insta`)

Phase 1 snapshots:
- Default launch flag list (output of `BrowserBuilder::default().build_flags()` — a private helper exposed under `#[cfg(test)]`).
- Default `ZendriverError` variant `Display` outputs.

### Tier 4 — Doctests

Every public method on `Browser`, `Tab`, `Element`, `FindBuilder` carries a `no_run` doctest demonstrating realistic usage. `cargo test --doc` type-checks all of them.

### CI matrix (Phase 1 baseline)

```yaml
test-unit:        cargo test --workspace --lib
test-doc:         cargo test --workspace --doc
test-integration: cargo test --workspace --features integration-tests --test '*' -- --test-threads=1
test-snapshot:    cargo test --workspace --test snapshots
clippy:           cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
fmt:              cargo fmt --all --check
```

Unit + doc + clippy + fmt on every PR. Integration on PRs touching `zendriver` or `zendriver-transport` (path filter).

### Determinism rules

| Source of nondeterminism | Mitigation |
|---|---|
| Real Chrome timing variance | `wait_for_load` over `sleep`; never sleep in tests |
| External network | `wiremock` for HTML fixtures; no internet dependency in Phase 1 |
| Cargo `--test-threads` default N | Integration test marked `#[serial_test::serial]` |
| User-data-dir cleanup | `tempfile::TempDir` per test; `Browser` `Drop` waits |
| Snapshot drift from `chromiumoxide_cdp` bumps | `cargo insta review` step in the bump PR |

## Roadmap

Phase 1 is one of six. Each subsequent phase gets its own brainstorm + spec cycle when work on it begins.

| Phase | Goal | Exit criterion |
|---|---|---|
| **1 (this spec)** | Foundation: transport + minimal Tab/Element + unit infra | `examples/hello.rs` runs against `example.com` |
| 2 | Stealth: launch flags + JS patches + `TargetObserver` + chaser-oxide real-OS detection | nightly stealth test against `bot.sannysoft.com` passes |
| 3 | Element API completeness: xpath/text/role selectors, hover/focus/scroll, type_text, attrs | 5 random Python `examples/*.py` ported 1:1 and pass |
| 4 | Tab/Browser completeness: cookies, storage, screenshots, multi-tab, iframe scoping | `multi_tab.py`, `iframe.py`, `cookies.py` ported and pass |
| 5 | Optional gated features: interception, cloudflare, `expect()`, fetcher | each subcrate ships its own example + integration test |
| 6 | Polish + 0.1 release | `cargo publish` succeeds; docs.rs builds |

Rough solo sizing: P1 = 2–3 weeks; total P1–P6 = 12–18 weeks.

## Open questions

None blocking Phase 1 implementation. Items punted to later phases (already noted in roadmap):

- Exact `TargetObserver` trait shape (P2 will revisit; P1 reserves an empty `Vec<Arc<dyn TargetObserver>>` field).
- How to handle iframe `Frame` typing (P3).
- Cookie jar persistence format (P4).

## Brainstorm cross-ref

This spec captures the brainstorm session of 2026-05-23. Decisions locked during brainstorming:

- **Scope:** Rust-native redesign, not 1:1 Python API port.
- **CDP bindings:** depend on `chromiumoxide_cdp` directly (not vendored, not re-generated).
- **Stealth (future P2):** port zendriver's surface + absorb chaser-oxide's protocol patches.
- **License:** Dual MIT / Apache-2.0.
- **v0.15 parity target:** yes, but phased across six releases.
- **API style:** builder pattern (`tab.find().css(...).timeout(...).one()`).
- **Workspace layout:** Cargo workspace, multiple crates.
- **Testing:** tiered (unit + integration + snapshot + doctest).
- **Connection model:** own WebSocket + actor (`tokio-tungstenite`), no dependency on `chromiumoxide` proper.
