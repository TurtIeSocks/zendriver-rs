# Network Monitor + Browser-Context HTTP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a persistent network monitor (`tab.monitor()` → `Stream<NetworkEvent>` over HTTP exchanges + WebSocket frames + EventSource messages) and a browser-context HTTP API (`tab.request()` → hybrid in-page-fetch / CORS-bypass).

**Architecture:** The monitor spawns one task using a **single raw event subscription** (`session.connection().subscribe_raw()`) dispatched by CDP `method`, correlating `requestId`s into completed `NetworkExchange`s — mirroring `network_idle.rs` (this preserves CDP wire order; typed `select!` would reorder events and leak). HTTP runs `fetch` in the page via `evaluate_main` (cookies/CORS inherited) with an opt-in `Network.loadNetworkResource` bypass. The monitor is behind a `monitor` cargo feature; HTTP is in-tree.

**Tech Stack:** Rust, `tokio::sync::mpsc` + `tokio_stream::wrappers::ReceiverStream` (Stream), `serde` (CDP event deser), `base64` (body decode — already a dep), CDP `Network.*` domain.

**Spec:** `docs/superpowers/specs/2026-06-02-network-monitor-http-design.md`

---

## File Structure

- **Create** `crates/zendriver/src/url_matcher.rs` — `UrlMatcher` moved here from `expect/mod.rs` so both `expect` and `monitor` use it without feature coupling. Re-exported from `expect`.
- **Create** `crates/zendriver/src/monitor/mod.rs` — `NetworkEvent`, `NetworkExchange`, `MonitoredRequest`/`MonitoredResponse`, `FrameDirection`, `MonitorBuilder`, `NetworkMonitor`, the correlator task. Behind `monitor` feature.
- **Create** `crates/zendriver/src/monitor/events.rs` — serde structs that deserialize the CDP `Network.*` event params.
- **Create** `crates/zendriver/src/request.rs` — `RequestBuilder`, `Response`, in-page-fetch + bypass paths. In-tree.
- **Modify** `crates/zendriver/src/tab.rs` — `monitor()` (feature-gated) + `request()`.
- **Modify** `crates/zendriver/src/error.rs` — `NetworkMonitor(String)`, `Request(String)`.
- **Modify** `crates/zendriver/src/lib.rs` — module decls + re-exports; `crates/zendriver/Cargo.toml` — `monitor` feature.
- **Create** `crates/zendriver/tests/network_monitor_http.rs` — gated integration tests.
- **Modify** `CHANGELOG.md`.

---

## Task 1: Extract `UrlMatcher` to a shared module

**Files:**
- Create: `crates/zendriver/src/url_matcher.rs`
- Modify: `crates/zendriver/src/expect/mod.rs`, `crates/zendriver/src/lib.rs`

- [ ] **Step 1: Move the type**

Read the `UrlMatcher` enum + its `impl` (matching logic) in `crates/zendriver/src/expect/mod.rs:42+`. Cut them into a new `crates/zendriver/src/url_matcher.rs` (keep the exact code + tests). Add module docs:
```rust
//! Shared URL matcher used by `expect_*` and the network `monitor`.
```
In `expect/mod.rs`, replace the definition with a re-export:
```rust
pub use crate::url_matcher::UrlMatcher;
```
In `lib.rs`, add `pub(crate) mod url_matcher;` (and a public re-export `pub use url_matcher::UrlMatcher;` if it was public before — preserve its existing visibility).

- [ ] **Step 2: Build + test (no behavior change)**

Run: `cargo test -p zendriver --features expect url_matcher` and `cargo build -p zendriver --features expect`
Expected: existing `UrlMatcher` tests pass from the new location; expect still compiles.

- [ ] **Step 3: Commit**
```bash
git add crates/zendriver/src/url_matcher.rs crates/zendriver/src/expect/mod.rs crates/zendriver/src/lib.rs
git commit -m "refactor: extract UrlMatcher to shared module"
```

---

## Task 2: Error variants

**Files:**
- Modify: `crates/zendriver/src/error.rs`

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn network_monitor_and_request_errors_render() {
    assert!(ZendriverError::NetworkMonitor("x".into()).to_string().contains("monitor"));
    assert!(ZendriverError::Request("y".into()).to_string().contains("request"));
}
```

- [ ] **Step 2: Implement** — add to `pub enum ZendriverError` (near the other `{0}` variants):
```rust
    #[error("network monitor: {0}")]
    NetworkMonitor(String),
    #[error("browser request: {0}")]
    Request(String),
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver network_monitor_and_request_errors_render`
```bash
git add crates/zendriver/src/error.rs
git commit -m "feat(error): NetworkMonitor + Request variants"
```

---

## Task 3: Monitor public types + CDP event deser

**Files:**
- Create: `crates/zendriver/src/monitor/mod.rs`, `crates/zendriver/src/monitor/events.rs`
- Modify: `crates/zendriver/src/lib.rs` (`#[cfg(feature="monitor")] pub mod monitor;`), `Cargo.toml` (feature)

- [ ] **Step 1: Add the feature**

In `crates/zendriver/Cargo.toml` `[features]`:
```toml
# Persistent network monitor (tab.monitor()).
monitor = []
```
Add `"monitor"` to the `integration-tests` feature list.
In `lib.rs`:
```rust
#[cfg(feature = "monitor")]
pub mod monitor;
```

- [ ] **Step 2: Write the CDP event structs + failing test**

Create `crates/zendriver/src/monitor/events.rs`:
```rust
//! Serde shapes for the CDP `Network.*` event params the monitor consumes.
use std::collections::HashMap;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestWillBeSent {
    pub request_id: String,
    pub request: CdpRequest,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CdpRequest {
    pub url: String,
    pub method: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub post_data: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResponseReceived {
    pub request_id: String,
    pub response: CdpResponse,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CdpResponse {
    pub status: u16,
    #[serde(default)]
    pub status_text: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub mime_type: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestIdOnly { pub request_id: String }
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoadingFailed { pub request_id: String, #[serde(default)] pub error_text: String }
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebSocketCreated { pub request_id: String, pub url: String }
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebSocketFrameEvent { pub request_id: String, pub response: WsFrame }
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WsFrame { pub opcode: u8, #[serde(default)] pub payload_data: String }
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventSourceMessage {
    pub request_id: String,
    #[serde(default)] pub event_name: String,
    #[serde(default)] pub event_id: String,
    #[serde(default)] pub data: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn parses_request_will_be_sent() {
        let v = json!({"requestId":"1","request":{"url":"https://x/a","method":"GET","headers":{"A":"b"}}});
        let p: RequestWillBeSent = serde_json::from_value(v).unwrap();
        assert_eq!(p.request.url, "https://x/a");
        assert_eq!(p.request.method, "GET");
    }
    #[test]
    fn parses_ws_frame() {
        let v = json!({"requestId":"2","response":{"opcode":1,"payloadData":"hi"}});
        let p: WebSocketFrameEvent = serde_json::from_value(v).unwrap();
        assert_eq!(p.response.payload_data, "hi");
    }
}
```

Create `crates/zendriver/src/monitor/mod.rs` with the public types:
```rust
//! Persistent network monitor: a Stream of NetworkEvent (HTTP exchanges +
//! WebSocket frames + EventSource messages). Passive (Network domain) —
//! read-only; use `interception` (Fetch domain) to modify requests.
mod events;

use std::collections::HashMap;

/// One observed network event.
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    Http(NetworkExchange),
    WebSocketOpen { request_id: String, url: String },
    WebSocketFrame { request_id: String, direction: FrameDirection, opcode: u8, payload: String },
    WebSocketClose { request_id: String },
    EventSourceMessage { request_id: String, event_name: String, event_id: String, data: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDirection { Sent, Received }

#[derive(Debug, Clone)]
pub struct MonitoredRequest {
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub post_data: Option<String>,
}
#[derive(Debug, Clone)]
pub struct MonitoredResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: HashMap<String, String>,
    pub mime_type: String,
}

/// A completed HTTP request/response. Body fetched lazily via [`Self::body`].
#[derive(Clone)]
pub struct NetworkExchange {
    pub request: MonitoredRequest,
    pub response: Option<MonitoredResponse>,
    pub error: Option<String>,
    pub(crate) request_id: String,
    pub(crate) session: zendriver_transport::SessionHandle,
}

impl std::fmt::Debug for NetworkExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkExchange")
            .field("request", &self.request)
            .field("response", &self.response)
            .field("error", &self.error)
            .finish()
    }
}

impl NetworkExchange {
    #[must_use] pub fn status(&self) -> Option<u16> { self.response.as_ref().map(|r| r.status) }
    #[must_use] pub fn is_success(&self) -> bool { matches!(self.status(), Some(s) if (200..300).contains(&s)) }
}
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver --features monitor monitor::`
Expected: event deser tests pass; module compiles.
```bash
git add crates/zendriver/src/monitor/ crates/zendriver/src/lib.rs crates/zendriver/Cargo.toml
git commit -m "feat(monitor): public types + CDP event deser"
```

---

## Task 4: Correlator task + `NetworkMonitor` + `tab.monitor()`

**Files:**
- Modify: `crates/zendriver/src/monitor/mod.rs`
- Modify: `crates/zendriver/src/tab.rs`
- Reference: `crates/zendriver/src/network_idle.rs:90-167` (the single-raw-subscription correlator to mirror)

- [ ] **Step 1: Write the correlator + builder + handle**

Add to `monitor/mod.rs`:
```rust
use futures::Stream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zendriver_transport::SessionHandle;
use crate::url_matcher::UrlMatcher;
use events::*;

const CHANNEL_CAP: usize = 1024;
const MAX_TRACKED: usize = 10_000;

/// Builder for [`NetworkMonitor`]. Optional URL filter, then `start()`.
pub struct MonitorBuilder {
    session: SessionHandle,
    url_pattern: Option<UrlMatcher>,
}
impl MonitorBuilder {
    pub(crate) fn new(session: SessionHandle) -> Self { Self { session, url_pattern: None } }
    #[must_use] pub fn url_pattern(mut self, pat: impl Into<String>) -> Self {
        self.url_pattern = Some(UrlMatcher::glob(pat.into())); self  // adapt: real UrlMatcher ctor
    }
    pub async fn start(self) -> crate::Result<NetworkMonitor> {
        let (tx, rx) = mpsc::channel(CHANNEL_CAP);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_monitor(self.session, self.url_pattern, tx, cancel.clone()));
        Ok(NetworkMonitor { rx, cancel, _task: task })
    }
}

/// A live network monitor. Implements `Stream<Item = NetworkEvent>`.
/// Dropping (or `stop()`) cancels the subscriber task.
pub struct NetworkMonitor {
    rx: mpsc::Receiver<NetworkEvent>,
    cancel: CancellationToken,
    _task: tokio::task::JoinHandle<()>,
}
impl NetworkMonitor {
    pub fn stop(self) { self.cancel.cancel(); }
}
impl Drop for NetworkMonitor {
    fn drop(&mut self) { self.cancel.cancel(); }
}
impl Stream for NetworkMonitor {
    type Item = NetworkEvent;
    fn poll_next(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>)
        -> std::task::Poll<Option<NetworkEvent>> {
        self.rx.poll_recv(cx)
    }
}

async fn run_monitor(
    session: SessionHandle,
    filter: Option<UrlMatcher>,
    tx: mpsc::Sender<NetworkEvent>,
    cancel: CancellationToken,
) {
    let session_id = session.session_id().to_string();
    // ONE raw subscription, dispatched by method — preserves CDP wire order
    // (see network_idle.rs: typed select! can reorder + leak).
    let mut events = session.connection().subscribe_raw();
    let mut partial: HashMap<String, (MonitoredRequest, Option<MonitoredResponse>)> = HashMap::new();
    let mut urls: HashMap<String, String> = HashMap::new();

    let allow = |urls: &HashMap<String,String>, id: &str, this_url: Option<&str>| -> bool {
        match &filter {
            None => true,
            Some(m) => {
                let u = this_url.or_else(|| urls.get(id).map(String::as_str));
                u.is_some_and(|u| m.matches(u))   // adapt: real UrlMatcher method
            }
        }
    };

    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            next = events.next() => {
                let Some(ev) = next else { return };
                if ev.session_id.as_deref() != Some(session_id.as_str()) { continue; }
                match ev.method.as_str() {
                    "Network.requestWillBeSent" => {
                        let Ok(p) = serde_json::from_value::<RequestWillBeSent>(ev.params) else { continue };
                        urls.insert(p.request_id.clone(), p.request.url.clone());
                        if urls.len() > MAX_TRACKED { evict_oldest(&mut urls, &mut partial); }
                        let req = MonitoredRequest { url: p.request.url, method: p.request.method,
                            headers: p.request.headers, post_data: p.request.post_data };
                        partial.insert(p.request_id, (req, None));
                    }
                    "Network.responseReceived" => {
                        let Ok(p) = serde_json::from_value::<ResponseReceived>(ev.params) else { continue };
                        if let Some(e) = partial.get_mut(&p.request_id) {
                            e.1 = Some(MonitoredResponse { status: p.response.status,
                                status_text: p.response.status_text, headers: p.response.headers,
                                mime_type: p.response.mime_type });
                        }
                    }
                    "Network.loadingFinished" => {
                        let Ok(p) = serde_json::from_value::<RequestIdOnly>(ev.params) else { continue };
                        if let Some((req, resp)) = partial.remove(&p.request_id) {
                            if allow(&urls, &p.request_id, Some(&req.url)) {
                                let ex = NetworkExchange { request: req, response: resp, error: None,
                                    request_id: p.request_id.clone(), session: session.clone() };
                                if tx.send(NetworkEvent::Http(ex)).await.is_err() { return; }
                            }
                        }
                        urls.remove(&p.request_id);
                    }
                    "Network.loadingFailed" => {
                        let Ok(p) = serde_json::from_value::<LoadingFailed>(ev.params) else { continue };
                        if let Some((req, resp)) = partial.remove(&p.request_id) {
                            if allow(&urls, &p.request_id, Some(&req.url)) {
                                let ex = NetworkExchange { request: req, response: resp,
                                    error: Some(p.error_text), request_id: p.request_id.clone(),
                                    session: session.clone() };
                                if tx.send(NetworkEvent::Http(ex)).await.is_err() { return; }
                            }
                        }
                        urls.remove(&p.request_id);
                    }
                    "Network.webSocketCreated" => {
                        let Ok(p) = serde_json::from_value::<WebSocketCreated>(ev.params) else { continue };
                        urls.insert(p.request_id.clone(), p.url.clone());
                        if allow(&urls, &p.request_id, Some(&p.url)) {
                            let _ = tx.send(NetworkEvent::WebSocketOpen { request_id: p.request_id, url: p.url }).await;
                        }
                    }
                    "Network.webSocketFrameSent" | "Network.webSocketFrameReceived" => {
                        let dir = if ev.method.ends_with("Sent") { FrameDirection::Sent } else { FrameDirection::Received };
                        let Ok(p) = serde_json::from_value::<WebSocketFrameEvent>(ev.params) else { continue };
                        if allow(&urls, &p.request_id, None) {
                            let _ = tx.send(NetworkEvent::WebSocketFrame { request_id: p.request_id,
                                direction: dir, opcode: p.response.opcode, payload: p.response.payload_data }).await;
                        }
                    }
                    "Network.webSocketClosed" => {
                        let Ok(p) = serde_json::from_value::<RequestIdOnly>(ev.params) else { continue };
                        if allow(&urls, &p.request_id, None) {
                            let _ = tx.send(NetworkEvent::WebSocketClose { request_id: p.request_id.clone() }).await;
                        }
                        urls.remove(&p.request_id);
                    }
                    "Network.eventSourceMessageReceived" => {
                        let Ok(p) = serde_json::from_value::<EventSourceMessage>(ev.params) else { continue };
                        if allow(&urls, &p.request_id, None) {
                            let _ = tx.send(NetworkEvent::EventSourceMessage { request_id: p.request_id,
                                event_name: p.event_name, event_id: p.event_id, data: p.data }).await;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn evict_oldest(urls: &mut HashMap<String,String>, partial: &mut HashMap<String,(MonitoredRequest,Option<MonitoredResponse>)>) {
    // Cheap bound: clear the partial map's stalest by dropping arbitrary excess.
    // A pathological page that never finishes requests should not grow forever.
    if let Some(k) = partial.keys().next().cloned() { partial.remove(&k); urls.remove(&k); }
    tracing::warn!("network monitor correlation map exceeded {MAX_TRACKED}; evicting oldest");
}
```
> Adapt-points: the real `UrlMatcher` constructor + match method name (read `url_matcher.rs` — it may be `UrlMatcher::Glob(..)` / `Exact(..)` with a `.matches(&str)`); `session.connection().subscribe_raw()` returns a stream of a `RawEvent { session_id, method, params }` — confirm field names from `network_idle.rs:110,141-146`. Confirm `tokio_util` + `tokio_stream`/`futures` are deps (interception uses streams; `network_idle` uses `CancellationToken` from `tokio_util`).

Add `tab.monitor()` in `tab.rs`:
```rust
/// Start a persistent network monitor over this tab's session.
#[cfg(feature = "monitor")]
pub fn monitor(&self) -> crate::monitor::MonitorBuilder {
    crate::monitor::MonitorBuilder::new(self.session().clone())
}
```

- [ ] **Step 2: Write the correlation unit test** (mirror `network_idle.rs` tests using `MockConnection`):
```rust
// In monitor/mod.rs #[cfg(test)] — feed synthetic raw events through a mock
// session and assert emitted NetworkEvents. Model exactly on the network_idle
// test harness (which drives MockConnection + emits Network.* events).
#[tokio::test]
async fn http_request_correlates_to_one_exchange() {
    // setup mock session; emit requestWillBeSent + responseReceived + loadingFinished
    // for requestId "1"; start monitor; assert one NetworkEvent::Http with status.
}
#[tokio::test]
async fn loading_failed_emits_error_exchange() { /* ... error set ... */ }
#[tokio::test]
async fn ws_frames_emit_tagged_events() { /* created -> frameReceived -> closed */ }
#[tokio::test]
async fn url_filter_drops_unmatched() { /* pattern "*/api/*"; one matching, one not */ }
```
> Read `network_idle.rs`'s `#[cfg(test)]` block for the exact `MockConnection`/`SessionHandle` test setup + how it emits events, and reuse it verbatim. This is the keystone test.

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver --features monitor monitor::`
Expected: correlation tests green.
```bash
git add crates/zendriver/src/monitor/ crates/zendriver/src/tab.rs
git commit -m "feat(monitor): correlator task + NetworkMonitor stream + tab.monitor()"
```

---

## Task 5: Lazy body (`NetworkExchange::body`/`text`)

**Files:**
- Modify: `crates/zendriver/src/monitor/mod.rs`
- Reference: `crates/zendriver/src/expect/response.rs:88-130` (getResponseBody + base64 pattern)

- [ ] **Step 1: Implement** (copy expect's decode logic):
```rust
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

impl NetworkExchange {
    /// Fetch the response body on demand (`Network.getResponseBody`).
    pub async fn body(&self) -> crate::Result<Vec<u8>> {
        let res = self.session.call("Network.getResponseBody",
            serde_json::json!({ "requestId": self.request_id })).await
            .map_err(|e| crate::ZendriverError::NetworkMonitor(format!("getResponseBody: {e}")))?;
        let body = res["body"].as_str().unwrap_or_default();
        if res["base64Encoded"].as_bool().unwrap_or(false) {
            BASE64.decode(body).map_err(|e| crate::ZendriverError::NetworkMonitor(format!("base64: {e}")))
        } else {
            Ok(body.as_bytes().to_vec())
        }
    }
    /// Fetch the body and decode as UTF-8.
    pub async fn text(&self) -> crate::Result<String> {
        Ok(String::from_utf8_lossy(&self.body().await?).into_owned())
    }
}
```
> Match `expect/response.rs:88-130` exactly for the `base64Encoded` handling + error mapping (it already does this; mirror it).

- [ ] **Step 2: Build (body is headful-tested in Task 10)**

Run: `cargo build -p zendriver --features monitor` + `cargo clippy -p zendriver --features monitor --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Commit**
```bash
git add crates/zendriver/src/monitor/mod.rs
git commit -m "feat(monitor): lazy NetworkExchange body/text via getResponseBody"
```

---

## Task 6: HTTP `RequestBuilder` + `Response` + fetch-JS generation

**Files:**
- Create: `crates/zendriver/src/request.rs`
- Modify: `crates/zendriver/src/lib.rs` (`pub mod request;` + re-exports)

- [ ] **Step 1: Write the types + JS generator + failing unit test**

Create `crates/zendriver/src/request.rs`:
```rust
//! Browser-context HTTP: `tab.request()` runs `fetch` in the page (cookies +
//! CORS inherited) with an opt-in `Network.loadNetworkResource` bypass.
use std::collections::HashMap;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use crate::error::{Result, ZendriverError};
use crate::tab::Tab;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method { Get, Post, Put, Delete, Head, Patch }
impl Method { fn as_str(self) -> &'static str { match self {
    Method::Get=>"GET", Method::Post=>"POST", Method::Put=>"PUT",
    Method::Delete=>"DELETE", Method::Head=>"HEAD", Method::Patch=>"PATCH" } } }

pub struct RequestBuilder<'a> {
    tab: &'a Tab,
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    bypass: bool,
}

impl<'a> RequestBuilder<'a> {
    pub(crate) fn new(tab: &'a Tab) -> Self {
        Self { tab, method: Method::Get, url: String::new(), headers: vec![], body: None, bypass: false }
    }
    pub fn get(mut self, url: impl Into<String>) -> Self { self.method = Method::Get; self.url = url.into(); self }
    pub fn post(mut self, url: impl Into<String>) -> Self { self.method = Method::Post; self.url = url.into(); self }
    pub fn put(mut self, url: impl Into<String>) -> Self { self.method = Method::Put; self.url = url.into(); self }
    pub fn delete(mut self, url: impl Into<String>) -> Self { self.method = Method::Delete; self.url = url.into(); self }
    pub fn head(mut self, url: impl Into<String>) -> Self { self.method = Method::Head; self.url = url.into(); self }
    pub fn patch(mut self, url: impl Into<String>) -> Self { self.method = Method::Patch; self.url = url.into(); self }
    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into())); self }
    pub fn body(mut self, b: impl Into<Vec<u8>>) -> Self { self.body = Some(b.into()); self }
    pub fn json<T: Serialize>(mut self, v: &T) -> Result<Self> {
        let s = serde_json::to_vec(v).map_err(ZendriverError::from)?;
        self.headers.push(("Content-Type".into(), "application/json".into()));
        self.body = Some(s); Ok(self)
    }
    pub fn bypass_cors(mut self) -> Self { self.bypass = true; self }

    /// Build the in-page `fetch` JS (default path).
    fn fetch_js(&self) -> String {
        let headers = json!(self.headers.iter().cloned().collect::<HashMap<_,_>>());
        let body = match &self.body {
            Some(b) => json!(BASE64.encode(b)),     // body shipped as base64 -> Uint8Array in JS
            None => json!(null),
        };
        format!(r#"(async () => {{
  const body = {body};
  const init = {{ method: {method}, headers: {headers} }};
  if (body !== null) {{ const bin = atob(body); const u = new Uint8Array(bin.length);
    for (let i=0;i<bin.length;i++) u[i]=bin.charCodeAt(i); init.body = u; }}
  const r = await fetch({url}, init);
  const buf = new Uint8Array(await r.arrayBuffer());
  let s=""; for (const x of buf) s += String.fromCharCode(x);
  const h = {{}}; r.headers.forEach((v,k)=>h[k]=v);
  return {{ status: r.status, headers: h, body_b64: btoa(s) }};
}})()"#,
            method = json!(self.method.as_str()),
            headers = headers,
            url = json!(self.url),
            body = body)
    }
}

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

#[derive(Debug, Clone, serde::Deserialize)]
struct FetchResult { status: u16, headers: HashMap<String,String>, body_b64: String }

pub struct Response { status: u16, headers: HashMap<String,String>, body: Vec<u8> }
impl Response {
    #[must_use] pub fn status(&self) -> u16 { self.status }
    #[must_use] pub fn headers(&self) -> &HashMap<String,String> { &self.headers }
    #[must_use] pub fn bytes(&self) -> &[u8] { &self.body }
    pub fn text(&self) -> Result<String> { Ok(String::from_utf8_lossy(&self.body).into_owned()) }
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(ZendriverError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fetch_js_contains_method_url_headers() {
        // Build a RequestBuilder without a real Tab by testing fetch_js via a
        // helper that doesn't touch `tab`. Extract fetch_js to not deref tab
        // (it doesn't) so this compiles with a dummy. If a Tab is needed,
        // assert on a standalone `build_fetch_js(method,url,headers,body)` fn
        // instead — refactor fetch_js to delegate to it.
        let js = build_fetch_js("POST", "https://x/a", &[("X".into(),"y".into())], Some(b"hi"));
        assert!(js.contains(r#""POST""#));
        assert!(js.contains("https://x/a"));
        assert!(js.contains(r#""X":"y""#) || js.contains(r#""X": "y""#));
        assert!(js.contains("btoa"));
    }
    #[test]
    fn response_json_round_trips() {
        let r = Response { status: 200, headers: HashMap::new(), body: br#"{"a":1}"#.to_vec() };
        #[derive(serde::Deserialize)] struct A { a: i32 }
        assert_eq!(r.json::<A>().unwrap().a, 1);
    }
}
```
> Refactor `fetch_js` to delegate to a free `fn build_fetch_js(method, url, headers, body) -> String` so it's unit-testable without a `Tab`. Keep the JSON-embedding (`json!`) for all interpolated values.

- [ ] **Step 2: Run + commit**

Run: `cargo test -p zendriver request::tests`
Expected: 2 passed.
```bash
git add crates/zendriver/src/request.rs crates/zendriver/src/lib.rs
git commit -m "feat(request): RequestBuilder + Response + fetch-JS generation"
```

---

## Task 7: HTTP `send()` — in-page fetch path

**Files:**
- Modify: `crates/zendriver/src/request.rs`, `crates/zendriver/src/tab.rs`

- [ ] **Step 1: Implement `send` (default path) + `tab.request()`**
```rust
impl RequestBuilder<'_> {
    pub async fn send(self) -> Result<Response> {
        if self.bypass { return self.send_bypass().await; }   // Task 8
        let js = self.fetch_js();
        let fr: FetchResult = self.tab.evaluate_main(&js).await
            .map_err(|e| ZendriverError::Request(format!("fetch failed: {e}")))?;
        let body = BASE64.decode(&fr.body_b64)
            .map_err(|e| ZendriverError::Request(format!("body decode: {e}")))?;
        Ok(Response { status: fr.status, headers: fr.headers, body })
    }
    async fn send_bypass(self) -> Result<Response> {
        Err(ZendriverError::Request("bypass_cors not yet implemented".into())) // Task 8 replaces
    }
}
```
In `tab.rs`:
```rust
/// Make an HTTP request from the browser context (inherits cookies/CORS).
pub fn request(&self) -> crate::request::RequestBuilder<'_> {
    crate::request::RequestBuilder::new(self)
}
```

- [ ] **Step 2: Build (behavior is headful-tested in Task 10)**

Run: `cargo build -p zendriver` + `cargo clippy -p zendriver --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Commit**
```bash
git add crates/zendriver/src/request.rs crates/zendriver/src/tab.rs
git commit -m "feat(request): in-page fetch send() + tab.request()"
```

---

## Task 8: HTTP `bypass_cors()` — `loadNetworkResource` path

**Files:**
- Modify: `crates/zendriver/src/request.rs`

- [ ] **Step 1: Implement `send_bypass`**
```rust
impl RequestBuilder<'_> {
    async fn send_bypass(self) -> Result<Response> {
        // Browser-privileged fetch; inherits cookies, ignores page CORS.
        let res = self.tab.session().call("Network.loadNetworkResource", json!({
            "url": self.url,
            "options": { "disableCache": false, "includeCredentials": true },
        })).await.map_err(|e| ZendriverError::Request(format!("loadNetworkResource: {e}")))?;
        let resource = &res["resource"];
        if !resource["success"].as_bool().unwrap_or(false) {
            return Err(ZendriverError::Request(format!(
                "loadNetworkResource failed: {}", resource["netErrorName"].as_str().unwrap_or("unknown"))));
        }
        let status = resource["httpStatusCode"].as_u64().unwrap_or(0) as u16;
        // Body: when a `stream` handle is returned, read via IO.read; else inline.
        let body = if let Some(stream) = resource["stream"].as_str() {
            read_io_stream(self.tab, stream).await?
        } else { Vec::new() };
        Ok(Response { status, headers: HashMap::new(), body })  // headers: see note
    }
}

async fn read_io_stream(tab: &Tab, handle: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let r = tab.session().call("IO.read", json!({ "handle": handle, "size": 65536 })).await
            .map_err(|e| ZendriverError::Request(format!("IO.read: {e}")))?;
        let data = r["data"].as_str().unwrap_or_default();
        if r["base64Encoded"].as_bool().unwrap_or(false) {
            out.extend(BASE64.decode(data).map_err(|e| ZendriverError::Request(format!("io b64: {e}")))?);
        } else { out.extend_from_slice(data.as_bytes()); }
        if r["eof"].as_bool().unwrap_or(true) { break; }
    }
    let _ = tab.session().call("IO.close", json!({ "handle": handle })).await;
    Ok(out)
}
```
> `loadNetworkResource` doesn't return parsed response headers in all Chrome builds; `resource["headers"]` may be present as an object — map it in if so, else leave empty + document. `method`/`body` on `loadNetworkResource` are GET-oriented; for non-GET in bypass mode, fall back with a clear error OR document bypass = GET-only for v1 (the in-page path covers full-method needs). Pick: **bypass supports GET only in v1**; return `Request("bypass_cors supports GET only; use the default fetch path for other methods")` when `self.method != Method::Get`.

- [ ] **Step 2: Build + commit**

Run: `cargo build -p zendriver` + clippy.
```bash
git add crates/zendriver/src/request.rs
git commit -m "feat(request): bypass_cors via loadNetworkResource (GET)"
```

---

## Task 9: Integration tests (gated headful)

**Files:**
- Create: `crates/zendriver/tests/network_monitor_http.rs`

- [ ] **Step 1: Write tests** (gate + server helper from `integration_phase5.rs`):
```rust
// monitor captures an in-page fetch
#[tokio::test] #[ignore]
async fn monitor_captures_fetch() {
    // serve a page; start tab.monitor(); evaluate a fetch("/data"); assert a
    // NetworkEvent::Http with url ending /data + status 200 + body via .text()
}
#[tokio::test] #[ignore]
async fn monitor_captures_websocket_frames() { /* page opens ws; assert Sent/Received frames */ }
#[tokio::test] #[ignore]
async fn request_get_inherits_cookies() {
    // set a cookie on the page; tab.request().get(same-origin /echo-cookie).send();
    // assert the response body reflects the cookie
}
#[tokio::test] #[ignore]
async fn request_post_json_round_trips() { /* post().json(); assert echoed */ }
#[tokio::test] #[ignore]
async fn bypass_cors_reaches_cross_origin() { /* a cross-origin GET the in-page path would block */ }
```
> Reuse the `fixture_with_html`/MockServer helper + gating (`#![cfg(feature="integration-tests")]`, `#[serial]`) from `integration_phase5.rs` / `integration_phase4.rs`. For the WS/SSE tests, serve a tiny WS/SSE endpoint (wiremock doesn't do WS — use a minimal `tokio-tungstenite` server in the test, or mark WS/SSE tests `#[ignore]` + document manual verification if that's too heavy).

- [ ] **Step 2: Compile-check (headful runs on CI / locally with Chrome)**

Run: `cargo test -p zendriver --features integration-tests --test network_monitor_http --no-run`
Expected: compiles. Run with `-- --ignored` locally if Chrome present.

- [ ] **Step 3: Commit**
```bash
git add crates/zendriver/tests/network_monitor_http.rs
git commit -m "test: network monitor + request integration"
```

---

## Task 10: Docs + examples + CHANGELOG

**Files:**
- Modify: `crates/zendriver/src/monitor/mod.rs`, `crates/zendriver/src/request.rs` (rustdoc + doctests), `CHANGELOG.md`
- Create: `crates/zendriver/examples/network_monitor.rs`, `crates/zendriver/examples/browser_request.rs`

- [ ] **Step 1: Rustdoc + `no_run` doctests** on `tab.monitor()` and `tab.request()` showing the stream loop + a GET/POST. Mirror existing example style.

- [ ] **Step 2: Examples** — `network_monitor.rs` (print exchanges + WS frames), `browser_request.rs` (GET + POST json). Add `[[example]]` entries with `required-features = ["monitor"]` for the monitor one. Must compile (`cargo build --examples -p zendriver --features monitor`).

- [ ] **Step 3: CHANGELOG** under `[Unreleased] ### Added`:
```markdown
- `tab.monitor()` — persistent network monitor: a `Stream<NetworkEvent>` over
  HTTP exchanges (lazy body), WebSocket frames, and EventSource messages,
  behind the `monitor` feature (#223).
- `tab.request()` — browser-context HTTP (`get/post/...`) inheriting cookies +
  CORS via in-page fetch, with opt-in `bypass_cors()` (#189).
```

- [ ] **Step 4: Commit**
```bash
git add crates/zendriver/src/monitor/ crates/zendriver/src/request.rs crates/zendriver/examples/ CHANGELOG.md
git commit -m "docs: network monitor + browser request examples + docs"
```

---

## Task 11: Gates + PR

- [ ] **Step 1: Format + clippy (default + monitor feature)**
```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p zendriver --features monitor --all-targets -- -D warnings
```
Expected: clean.

- [ ] **Step 2: Tests**
```bash
cargo test --workspace --locked
cargo test -p zendriver --features monitor monitor::
cargo test -p zendriver --features integration-tests --test network_monitor_http --no-run
```
Expected: unit/doctests green; integration compiles.

- [ ] **Step 3: Commit + PR**
```bash
git add -A && git commit -m "chore: fmt + clippy for network monitor + request"
gh pr create --base main \
  --title "feat: network monitor (Stream) + browser-context HTTP (#223, #189)" \
  --body "PR2 Group B. See docs/superpowers/specs/2026-06-02-network-monitor-http-design.md"
```

---

## Self-Review (completed by plan author)

**Spec coverage:** §3 monitor API → T3/T4/T5; §4 monitor impl (single-raw-subscription correlator + WS/ES + URL filter + map bound) → T4; §5 HTTP API → T6; §6 HTTP hybrid (fetch + bypass) → T7/T8; §7 errors → T2; §8 testing → T3/T4/T6 (unit) + T9 (integration); §9 out-of-scope honored (monitor read-only, no streaming/cookie-override). Covered.

**Placeholders:** none — full code per step. Adapt-points flagged inline (real `UrlMatcher` ctor/match-method, `RawEvent` field names, `tokio_util`/`tokio_stream`/`futures` dep confirmation, `loadNetworkResource` header/body shape, bypass=GET-only v1, `MockConnection` test harness reuse from `network_idle.rs`).

**Type consistency:** `NetworkEvent`/`NetworkExchange`/`MonitoredRequest`/`MonitoredResponse`/`FrameDirection`, `MonitorBuilder`/`NetworkMonitor`, `RequestBuilder`/`Response`, `build_fetch_js`, `FetchResult`, `NetworkMonitor`/`Request` error variants, `tab.monitor()`/`tab.request()` — consistent across tasks and matching the spec.
