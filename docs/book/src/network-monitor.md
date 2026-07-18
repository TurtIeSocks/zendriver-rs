# Network monitor

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
subscriber. The correlation map is bounded (10k in-flight requests).

### Delivery-loss boundaries

`Http` exchanges are assembled by correlating `requestWillBeSent` →
`responseReceived` → `loadingFinished` / `loadingFailed` by `requestId`. The
correlator rides the transport's loss-accounted event stream, so instead of
ever silently stitching a possibly-bogus "complete" exchange across a gap,
silently evicting a stuck correlation entry, or silently skipping a payload
it couldn't decode, every one of those cases surfaces as an explicit
[`NetworkEvent::DeliveryBoundary`]`(`[`NetworkDeliveryBoundary`]`)` on the
same stream:

| Variant | Emitted when | The correlator also… |
|---------|--------------|------------------------|
| `Lagged { missed, generation }` | this subscription fell behind the connection's accounted event bus | clears all in-flight correlation state — a gap means any partial exchange spanning it can't be trusted |
| `Reconnected { previous, generation }` | the transport re-established a fresh WebSocket | clears all in-flight correlation state — nothing from the old socket will ever complete |
| `Disconnected { generation }` | the transport's WebSocket died unexpectedly | clears state, then **ends the monitor task** — see below |
| `CorrelationEvicted { url }` | the in-flight correlation map exceeded its 10k bound | evicts one entry (previously silent beyond a `tracing` warning) |
| `DecodeFailed` | a CDP payload didn't match the shape expected for its method | skips that one event only — no raw payload is ever included |
| `Unknown` | a future transport-level variant this correlator doesn't yet recognize | conservatively clears state, but keeps running |

`DeliveryBoundary` events bypass any [`MonitorBuilder::url_pattern`] filter —
they describe the monitor's own health, not a specific exchange. A consumer
that ignores this variant still sees every fully-observed exchange exactly as
before; it just loses the ability to tell "nothing happened" apart from
"something was lost and I was never told."

```rust,ignore
match event {
    zendriver::NetworkEvent::DeliveryBoundary(b) => {
        eprintln!("network monitor gap: {b:?}");
        // Decide whether to resync, alert, or (for `Disconnected`) restart
        // the monitor.
    }
    _ => { /* handle the fully-observed variants as usual */ }
}
```

**`Disconnected` ends the monitor task — fail closed.** There is no
automatic reconnect: once a `Disconnected` boundary is emitted, the
correlator task returns and the [`NetworkMonitor`] stream ends (`None` on the
next poll). A consumer that wants to keep observing across a transport blip
must call `tab.monitor()...start()` again to spawn a fresh correlator.

### Bounded response bodies

[`NetworkExchange::body`] / [`text`][`NetworkExchange::text`] are unbounded —
they fetch and return the entire body every time, unchanged by this section.
For a size cap, wrap the fetched bytes yourself with [`BoundedBody::capture`]:

```rust,ignore
use zendriver::BoundedBody;

if let zendriver::NetworkEvent::Http(ex) = event {
    let full = ex.body().await?;
    let bounded = BoundedBody::capture(&full, 1024 * 1024); // cap at 1 MiB
    if bounded.truncated {
        println!("body truncated: kept {} of {} bytes", bounded.bytes.len(), bounded.full_len);
    }
}
```

`max_bytes == 0` means unbounded — every byte is kept and `truncated` is
always `false`. Bounding is always computed against the raw decoded byte
length, never a base64 (or other encoded) representation's length.

The MCP `browser_monitor_start` tool wires this in for you: its
`capture_body_max_bytes` input (default 1 MiB, `0` for unbounded) bounds each
captured body, and `browser_monitor_read`'s `http` events carry
`body_truncated` / `body_full_bytes` alongside `body` / `body_base64`. A
body-fetch failure (e.g. Chrome already evicted the response) sets
`body_capture_error` instead of silently degrading to an indistinguishable
empty body. See [`mcp.md`](./mcp.md).

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
[`NetworkEvent::DeliveryBoundary`]: https://docs.rs/zendriver/latest/zendriver/enum.NetworkEvent.html#variant.DeliveryBoundary
[`NetworkDeliveryBoundary`]: https://docs.rs/zendriver/latest/zendriver/enum.NetworkDeliveryBoundary.html
[`BoundedBody::capture`]: https://docs.rs/zendriver/latest/zendriver/struct.BoundedBody.html#method.capture
[`Stream`]: https://docs.rs/futures/latest/futures/stream/trait.Stream.html
[`regex::Regex`]: https://docs.rs/regex/latest/regex/struct.Regex.html
[`tab.monitor()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.monitor
