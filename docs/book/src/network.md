# Network monitor & HTTP

Two complementary network features sit on the CDP `Network` domain:

- **[`tab.monitor()`]** (feature `monitor`) — a long-lived
  `Stream<NetworkEvent>` of completed HTTP exchanges, WebSocket frames, and
  EventSource messages. Passive: it *observes*, never modifies.
- **[`tab.request()`]** (always available) — make an HTTP request from the
  browser context, inheriting the page's cookies and CORS, with an opt-in
  privileged bypass.

For one-shot "await the next response and assert on it" use the
[`expect`](./expect.md) surface instead; for *modifying* or *blocking*
requests use [`Interception`](./interception.md) (the active `Fetch`
domain). The monitor is the persistent, read-only generalization of
`expect_response`.

[`tab.monitor()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.monitor
[`tab.request()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.request

## Network monitor

Enable the feature:

```toml
[dependencies]
zendriver = { version = "0.1", features = ["monitor"] }
```

[`tab.monitor()`] returns a [`MonitorBuilder`]; set an optional URL filter,
then `.start().await?` spawns the correlator task and hands back a
[`NetworkMonitor`] — a [`Stream`] of [`NetworkEvent`]:

```rust,ignore
use futures::StreamExt;

// Start the monitor BEFORE navigating so no events are missed.
let mut monitor = tab.monitor().url_pattern("/api/").start().await?;
tab.goto("https://example.com").await?;

while let Some(event) = monitor.next().await {
    match event {
        zendriver::NetworkEvent::Http(ex) => {
            println!("{} {} -> {:?}", ex.request.method, ex.request.url, ex.status());
            if ex.is_success() {
                let body = ex.text().await?; // lazy: fetched on demand
                println!("{body}");
            }
        }
        zendriver::NetworkEvent::WebSocketFrame { direction, payload, .. } => {
            println!("ws {direction:?}: {payload}");
        }
        _ => {}
    }
}
```

### Event model

[`NetworkEvent`] is a tagged enum:

| Variant | Emitted when | Payload |
|---------|--------------|---------|
| `Http(`[`NetworkExchange`]`)` | a request reaches `loadingFinished` / `loadingFailed` | request + optional response + optional error |
| `WebSocketOpen` | `Network.webSocketCreated` | `request_id`, `url` |
| `WebSocketFrame` | a frame is sent / received | `direction`, `opcode`, inline `payload` |
| `WebSocketClose` | `Network.webSocketClosed` | `request_id` |
| `EventSourceMessage` | an SSE message arrives | `event_name`, `event_id`, inline `data` |

HTTP exchanges are **completed**: the monitor correlates
`requestWillBeSent` → `responseReceived` → `loadingFinished` by `requestId`
and emits one [`NetworkEvent::Http`] per request. WebSocket and EventSource
payloads are delivered **inline** (they arrive whole in the CDP event); only
HTTP bodies are lazy.

### Lazy bodies

HTTP bodies are fetched on demand via [`NetworkExchange::body`] /
[`NetworkExchange::text`] (CDP `Network.getResponseBody`):

```rust,ignore
if let zendriver::NetworkEvent::Http(ex) = event {
    let bytes: Vec<u8> = ex.body().await?;
}
```

Chrome only retains a response body for a short window after the response
completes, so call `body()` / `text()` promptly after observing the
exchange — a later call can fail with
[`ZendriverError::NetworkMonitor`](https://docs.rs/zendriver/latest/zendriver/enum.ZendriverError.html#variant.NetworkMonitor)
if the body was already evicted.

### URL filtering

[`MonitorBuilder::url_pattern`] takes any `Into<UrlMatcher>` (a `&str` /
`String` substring, or a [`regex::Regex`]) — the same matcher type the
[`expect`](./expect.md) surface uses. For HTTP the request URL is matched;
for WebSocket / EventSource the connection URL observed at open time is
matched. Unmatched events are dropped before they reach the stream.

### Lifecycle

[`NetworkMonitor`] owns the correlator task. Dropping the monitor — or
calling [`NetworkMonitor::stop`] — cancels that task; there is no leaked
subscriber. The correlation map is bounded (10k in-flight requests); a
pathological page that opens requests it never finishes evicts old entries
with a `tracing` warning rather than growing without limit.

### Full example

```rust,no_run
{{#include ../../../crates/zendriver/examples/network_monitor.rs}}
```

[`MonitorBuilder`]: https://docs.rs/zendriver/latest/zendriver/struct.MonitorBuilder.html
[`MonitorBuilder::url_pattern`]: https://docs.rs/zendriver/latest/zendriver/struct.MonitorBuilder.html#method.url_pattern
[`NetworkMonitor`]: https://docs.rs/zendriver/latest/zendriver/struct.NetworkMonitor.html
[`NetworkMonitor::stop`]: https://docs.rs/zendriver/latest/zendriver/struct.NetworkMonitor.html#method.stop
[`NetworkEvent`]: https://docs.rs/zendriver/latest/zendriver/enum.NetworkEvent.html
[`NetworkEvent::Http`]: https://docs.rs/zendriver/latest/zendriver/enum.NetworkEvent.html#variant.Http
[`NetworkExchange`]: https://docs.rs/zendriver/latest/zendriver/struct.NetworkExchange.html
[`NetworkExchange::body`]: https://docs.rs/zendriver/latest/zendriver/struct.NetworkExchange.html#method.body
[`NetworkExchange::text`]: https://docs.rs/zendriver/latest/zendriver/struct.NetworkExchange.html#method.text
[`Stream`]: https://docs.rs/futures/latest/futures/stream/trait.Stream.html
[`regex::Regex`]: https://docs.rs/regex/latest/regex/struct.Regex.html

## Browser-context HTTP

[`tab.request()`] makes an HTTP call that inherits the browser's cookies and
session. It needs no feature flag. The builder mirrors pydoll's shape:

```rust,ignore
use serde_json::json;

// GET (inherits cookies + same-origin CORS of the current page)
let resp = tab.request().get("https://example.com/api/data").send().await?;
println!("{} {}", resp.status(), resp.text()?);

// POST a JSON body
let resp = tab
    .request()
    .post("https://example.com/api/echo")
    .header("X-Trace", "1")
    .json(&json!({ "key": "value" }))?
    .send()
    .await?;
let parsed: serde_json::Value = resp.json()?;
```

[`get`] / [`post`] / [`put`] / [`delete`] / [`head`] / [`patch`] set the
method and URL; [`header`] appends a header; [`body`] sets raw bytes;
[`json`] serializes a value and sets `Content-Type: application/json`.
[`send`] returns a [`Response`] exposing [`status`] / [`headers`] /
[`text`] / [`json`] / [`bytes`].

A non-2xx status is **not** an error — the [`Response`] carries the status.
Only a thrown `fetch` (network failure, CORS block) or a failed privileged
load surfaces a
[`ZendriverError::Request`](https://docs.rs/zendriver/latest/zendriver/enum.ZendriverError.html#variant.Request).

### Default path: in-page `fetch`

By default `send()` runs `fetch` in the page via `evaluate_main`, so the
request behaves exactly like one the page itself made — same cookies, same
CORS rules. Because of that it **needs a loaded document of the right
origin**: navigate to the origin first, then make same-origin calls. On
`about:blank` a cross-origin call is a `null`-origin request and will be
CORS-blocked.

The request URL, headers, and body are embedded into the generated JS via
`serde_json`, so arbitrary url / header / body values can't break out of the
JS string — there is no injection surface. The body round-trips as base64 so
binary payloads survive intact.

### Opt-in: `bypass_cors()`

[`bypass_cors`] routes through Chrome's privileged
`Network.loadNetworkResource` instead — it ignores page CORS and works
without a same-origin document, while still inheriting session cookies. It
is **GET-only** in this version; for other methods use the default `fetch`
path.

```rust,ignore
// Reach a cross-origin endpoint that the in-page fetch would be blocked on.
let resp = tab
    .request()
    .get("https://other-origin.example/resource")
    .bypass_cors()
    .send()
    .await?;
```

### Full example

```rust,no_run
{{#include ../../../crates/zendriver/examples/browser_request.rs}}
```

[`get`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestBuilder.html#method.get
[`post`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestBuilder.html#method.post
[`put`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestBuilder.html#method.put
[`delete`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestBuilder.html#method.delete
[`head`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestBuilder.html#method.head
[`patch`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestBuilder.html#method.patch
[`header`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestBuilder.html#method.header
[`body`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestBuilder.html#method.body
[`json`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestBuilder.html#method.json
[`bypass_cors`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestBuilder.html#method.bypass_cors
[`send`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestBuilder.html#method.send
[`Response`]: https://docs.rs/zendriver/latest/zendriver/struct.Response.html
[`status`]: https://docs.rs/zendriver/latest/zendriver/struct.Response.html#method.status
[`headers`]: https://docs.rs/zendriver/latest/zendriver/struct.Response.html#method.headers
[`text`]: https://docs.rs/zendriver/latest/zendriver/struct.Response.html#method.text
[`json`]: https://docs.rs/zendriver/latest/zendriver/struct.Response.html#method.json
[`bytes`]: https://docs.rs/zendriver/latest/zendriver/struct.Response.html#method.bytes
