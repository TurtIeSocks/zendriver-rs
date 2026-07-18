# Network monitor & HTTP

Two complementary network features sit on the CDP `Network` domain:

- **[Network monitor](./network-monitor.md)** — [`tab.monitor()`] (feature
  `monitor`) is a long-lived `Stream<NetworkEvent>` of completed HTTP
  exchanges, WebSocket frames, and EventSource messages. Passive: it
  *observes*, never modifies.
- **[Browser-context HTTP](./network-http.md)** — [`tab.request()`] (always
  available) makes an HTTP request from the browser context, inheriting the
  page's cookies and CORS, with an opt-in privileged bypass.

For one-shot "await the next response and assert on it" use the
[`expect`](./expect.md) surface instead; for *modifying* or *blocking*
requests use [`Interception`](./interception.md) (the active `Fetch`
domain). The monitor is the persistent, read-only generalization of
`expect_response`.

[`tab.monitor()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.monitor
[`tab.request()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.request
