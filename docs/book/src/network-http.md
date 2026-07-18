# Browser-context HTTP

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
[`tab.request()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.request
