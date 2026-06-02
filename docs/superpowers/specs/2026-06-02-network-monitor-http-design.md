# Network Monitor + Browser-Context HTTP — Design (PR2 / Group B)

- **Date:** 2026-06-02
- **Status:** Approved (brainstorming), pending implementation plan
- **Upstream drivers:** zendriver #223 (permanent network monitor), #189 (browser-context HTTP request API)
- **Scope:** Group B of the PR2 batch. Two independent network features in one spec. May ship as one PR or split B1 (monitor) / B2 (HTTP) at build time — they share no code. Groups A (find/DOM), C (robustness), D (datadome) are separate cycles.

---

## 1. Context

The port's network surface today ([investigated]):
- `SessionHandle::subscribe::<T>(method) -> Stream<T>` (filters by session + CDP method, deserializes params) is the event primitive. Subscribe-before-enable is the established pattern.
- `Network.enable` is **always on** per tab (`network_idle.rs` `InFlightTracker`), so all `Network.*` events are available without extra enable/disable churn.
- `expect_request`/`expect_response` are **one-shot** subscriptions to `Network.requestWillBeSent`/`responseReceived`; `MatchedResponse::body()` calls `Network.getResponseBody`. The monitor is the **persistent** generalization.
- `zendriver-interception` is the **Fetch**-domain interceptor (active: pause/modify/abort). The monitor is **Network**-domain (passive: observe). These stay distinct (see Out of Scope).
- `Tab::evaluate_main::<T>()` runs JS in the page main world (cookies/session visible) and deserializes the result — the basis for in-page HTTP.
- `Tab::cookies()` → `CookieJar` (browser-scoped cookie CRUD).

Two gaps:
1. **#223** — a long-lived monitor streaming request/response (+ WebSocket frames + EventSource messages) as they happen, beyond one-shot `expect_*`.
2. **#189** — `tab.request().get(url)` making HTTP calls that inherit the browser's cookies/session, respecting CORS.

---

## 2. Decisions locked in brainstorming

- **Monitor delivery: `Stream`** (not callback) — the project's established event idiom; composable; matches `SessionHandle::subscribe` + interception.
- **Monitor event model: completed exchanges, lazy body** — HTTP emitted on `loadingFinished`/`loadingFailed`, correlated by `requestId`, body fetched on demand.
- **Monitor scope: HTTP + WebSocket + EventSource**, delivered as **one unified `Stream<NetworkEvent>`** (tagged enum), not separate per-kind streams.
- **HTTP implementation: hybrid** — in-page `fetch` (default, inherits cookies + respects CORS) + opt-in `bypass_cors()` via `Network.loadNetworkResource` (browser-privileged, no CORS, no page context needed).
- **HTTP API: pydoll-style builder** (`get/post/…/header/body/json/bypass_cors/send`) → `Response` holding bytes with `status`/`headers`/`text`/`json`/`bytes`.

---

## 3. Network monitor — public API

```rust
let mut mon = tab.monitor()
    .url_pattern("*/api/*")     // optional; reuses expect's UrlMatcher
    .start().await?;            // NetworkMonitor: impl Stream<Item = NetworkEvent>

while let Some(ev) = mon.next().await {
    match ev {
        NetworkEvent::Http(ex) => {
            // ex.request.{url,method,headers}, ex.status(), ex.is_success()
            if ex.is_success() { let body = ex.text().await?; }   // lazy: Network.getResponseBody
        }
        NetworkEvent::WebSocketOpen { url, .. } => {}
        NetworkEvent::WebSocketFrame { direction, opcode, payload, .. } => {}  // payload inline
        NetworkEvent::WebSocketClose { .. } => {}
        NetworkEvent::EventSourceMessage { event_name, data, .. } => {}        // inline
    }
}
// drop(mon) or mon.stop() cancels the subscriber task.
```

```rust
pub enum NetworkEvent {
    Http(NetworkExchange),
    WebSocketOpen { request_id: String, url: String },
    WebSocketFrame { request_id: String, direction: FrameDirection, opcode: u8, payload: String },
    WebSocketClose { request_id: String },
    EventSourceMessage { request_id: String, event_name: String, event_id: String, data: String },
}

pub enum FrameDirection { Sent, Received }

pub struct NetworkExchange {
    pub request: RequestInfo,            // url, method, headers, post_data
    pub response: Option<ResponseInfo>,  // status, headers, mime_type; None if failed pre-response
    pub error: Option<String>,           // Some(errorText) on loadingFailed
    // request_id + session: private, used by body()
}

impl NetworkExchange {
    pub fn status(&self) -> Option<u16>;
    pub fn is_success(&self) -> bool;      // 2xx
    pub async fn body(&self) -> Result<Vec<u8>>;   // base64-decoded Network.getResponseBody
    pub async fn text(&self) -> Result<String>;
}
```

- `tab.monitor()` returns a `MonitorBuilder` (`.url_pattern(...)` optional) whose `.start().await?` yields a `NetworkMonitor`.
- `NetworkMonitor: Stream<Item = NetworkEvent>` + `stop(self)`; RAII drop cancels the subscriber task (like `InterceptHandle`). Per tab.
- WS/ES payloads are inline in the CDP events — no lazy fetch. Only HTTP bodies are lazy.

---

## 4. Monitor implementation

A spawned task subscribes (before relying on events) to:
- HTTP: `Network.requestWillBeSent`, `responseReceived`, `loadingFinished`, `loadingFailed`.
- WS: `Network.webSocketCreated`, `webSocketFrameSent`, `webSocketFrameReceived`, `webSocketClosed`.
- ES: `Network.eventSourceMessageReceived`.

State: `HashMap<requestId, PartialExchange>` for HTTP correlation + `HashMap<requestId, url>` for WS/ES URL tracking.

- `requestWillBeSent` → insert partial (request); also record `requestId→url` (covers ES, which rides a normal request).
- `responseReceived` → attach response.
- `loadingFinished` → emit `NetworkEvent::Http(exchange)`; remove from map.
- `loadingFailed` → emit with `error`; remove.
- `webSocketCreated {requestId, url}` → record `requestId→url`; emit `WebSocketOpen`.
- `webSocketFrameSent/Received` → emit `WebSocketFrame` (opcode + payloadData from the event).
- `webSocketClosed` → emit `WebSocketClose`; drop url.
- `eventSourceMessageReceived {requestId, eventName, eventId, data}` → emit `EventSourceMessage`.

All emitted into one bounded channel; its receiver is the `NetworkMonitor` stream.

**Filtering:** when `url_pattern` is set, an event passes only if its `requestId`'s recorded URL matches (HTTP matches its own request URL; WS matches the handshake URL; ES matches the request URL). Events for unmatched `requestId`s are dropped before the channel.

**Bounds:** cap the correlation map (e.g. 10k entries); on overflow evict oldest with a `tracing::warn` — no silent unbounded growth for pathological pages that never finish requests.

---

## 5. Browser-context HTTP — public API

```rust
let resp = tab.request()
    .post("https://example.com/api/user")     // get/post/put/delete/head/patch
    .header("X-Trace", "1")
    .json(&payload)?                           // body + Content-Type: application/json
    .bypass_cors()                             // opt-in; default = in-page fetch
    .send().await?;                            // Response

let code: u16 = resp.status();
let hdrs: &HashMap<String, String> = resp.headers();
let user: User = resp.json()?;                 // serde from body
let txt: String = resp.text()?;
let raw: &[u8] = resp.bytes();
```

```rust
pub struct RequestBuilder { /* method, url, headers: Vec<(String,String)>, body: Option<Vec<u8>>, bypass: bool, tab */ }
impl RequestBuilder {
    pub fn get(self, url: impl Into<String>) -> Self;     // + post/put/delete/head/patch
    pub fn header(self, k: impl Into<String>, v: impl Into<String>) -> Self;
    pub fn body(self, b: impl Into<Vec<u8>>) -> Self;
    pub fn json<T: Serialize>(self, v: &T) -> Result<Self>;
    pub fn bypass_cors(self) -> Self;
    pub async fn send(self) -> Result<Response>;
}
pub struct Response { /* status: u16, headers: HashMap<String,String>, body: Vec<u8> */ }
impl Response {
    pub fn status(&self) -> u16;
    pub fn headers(&self) -> &HashMap<String, String>;
    pub fn text(&self) -> Result<String>;
    pub fn json<T: DeserializeOwned>(&self) -> Result<T>;
    pub fn bytes(&self) -> &[u8];
}
```

A non-2xx status is **not** an error — `Response` carries the status. Only transport / JS / network failures error.

---

## 6. HTTP implementation (hybrid)

**Default — in-page `fetch` via `evaluate_main`:** generate JS that runs
```js
const r = await fetch(url, { method, headers, body });
const buf = new Uint8Array(await r.arrayBuffer());
let bin = ""; for (const x of buf) bin += String.fromCharCode(x);  // binary string
return { status: r.status, headers: Object.fromEntries(r.headers), body_b64: btoa(bin) };
```
Body returned as base64 so binary survives the JSON round-trip; decoded on the Rust side into `Response.body`. Inherits cookies/auth and respects CORS exactly like the page. **Requires a loaded document** — on `about:blank`, cross-origin calls hit `null`-origin CORS; docs instruct navigating to the origin first for same-origin calls.

**Opt-in — `bypass_cors()` via `Network.loadNetworkResource`:** browser-privileged fetch through Chrome's network stack; inherits cookies, ignores page CORS, works without a page context. Read the body from the returned resource (stream handle via `IO.read` loop, or inline). Feature-detect; if unavailable on the running Chrome, return a clear `Request` error.

Both paths produce the same `Response`.

---

## 7. Error handling

New `ZendriverError` variants:
- `NetworkMonitor(String)` — subscription/correlation failures.
- `Request(String)` — in-page `fetch` rejection (network error / CORS block, carrying the JS message), or `loadNetworkResource` failure / unavailability.

Edge cases:
- Monitor `body()` when Chrome has evicted the body → clear error, no panic.
- HTTP `send()` non-2xx → success path (status on `Response`); only thrown fetch / failed CDP load → `Request` error.

---

## 8. Testing

- **Unit (no browser):**
  - Monitor correlation state machine: feed synthetic `requestWillBeSent`→`responseReceived`→`loadingFinished` → one `Http` exchange; `loadingFailed` → error exchange; WS `created`→`frameSent`/`frameReceived`→`closed` → tagged events; `eventSourceMessageReceived` → `EventSourceMessage`; URL-filter passes only matched `requestId`s; map cap eviction.
  - HTTP request-JS generation (method/headers/body/base64 wrapper) + `Response` accessors (status/json/text/bytes) + base64 decode.
- **Integration (gated headful, mirror `integration_phase5.rs`):**
  - Monitor captures an in-page `fetch` (URL/status/body); a page WebSocket's sent/received frames; an SSE endpoint's message.
  - `request().get()` inherits a cookie set on the page; `post().json()` round-trips; `bypass_cors()` reaches a cross-origin URL the in-page path would block.

---

## 9. Out of scope

- **Request modification from the monitor** — that is `zendriver-interception`'s job (Fetch domain, active). The monitor is read-only (Network domain, passive).
- **Streaming response bodies** — `Network.getResponseBody` is whole-body, post-completion; streaming needs the Fetch interception path. (HTTP `request()` response streaming likewise deferred.)
- **Per-request cookie override** — `CookieJar` + auto-inherited browser cookies cover the real need; `fetch` forbids setting the `Cookie` header anyway.
- **Groups A / C / D** — separate sub-PRs.

---

## 10. Open questions for the plan stage

- Confirm the exact `SessionHandle::subscribe` typed-vs-raw choice for multiplexing many CDP methods into one task (likely several typed `subscribe` streams merged via `futures::stream::select`, or the raw `subscribe_raw` + method match used by `network_idle.rs`).
- Confirm `Network.loadNetworkResource` body-read shape on the pinned Chrome (inline `stream` handle vs `base64Encoded` data) for the bypass path.
- Channel bound size + eviction threshold for the correlation map.
- Whether to gate the monitor/HTTP behind a cargo feature (like `expect`/`interception`) or ship in-tree (lean toward a `monitor` feature for the subscriber task; HTTP is light enough to be in-tree).
