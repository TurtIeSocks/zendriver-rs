# Interception

The `interception` Cargo feature wraps Chrome's [`Fetch`] CDP domain in a
fluent rule-based API plus a lower-level `Stream` of paused requests. It
lets you block, redirect, synthesize, or rewrite any subresource a page
asks for — useful for ad blocking, response mocking, header injection,
and offline-replay tests.

[`Fetch`]: https://chromedevtools.github.io/devtools-protocol/tot/Fetch/

Enable it in `Cargo.toml`:

```toml
[dependencies]
zendriver = { version = "0.1", features = ["interception"] }
```

The entry point is [`Tab::intercept`], which returns an
[`InterceptBuilder`] bound to that tab's session. The builder has two
terminal methods: [`start`] activates a background actor with declarative
rules, and [`subscribe`] returns a `Stream<Item = PausedRequest>` for the
manual escape-hatch path.

[`Tab::intercept`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.intercept
[`InterceptBuilder`]: https://docs.rs/zendriver/latest/zendriver/struct.InterceptBuilder.html
[`start`]: https://docs.rs/zendriver/latest/zendriver/struct.InterceptBuilder.html#method.start
[`subscribe`]: https://docs.rs/zendriver/latest/zendriver/struct.InterceptBuilder.html#method.subscribe

## Rule-based API

The four rule methods chain on the builder and dispatch the matching
terminal action automatically when the URL pattern fires. Patterns use
CDP wildcard syntax (`*` matches any characters; `?` matches any single
character).

| Method | Action | Use case |
|--------|--------|----------|
| [`block`] | `Fetch.failRequest` with `BlockedByClient` | Ad / tracker blocking |
| [`redirect`] | `Fetch.continueRequest` with new URL | Move an endpoint without page changes |
| [`respond`] | `Fetch.fulfillRequest` with synthetic body | Mock an API for tests |
| [`modify_request`] | `Fetch.continueRequest` with overrides | Inject headers, change method/body |

[`block`]: https://docs.rs/zendriver/latest/zendriver/struct.InterceptBuilder.html#method.block
[`redirect`]: https://docs.rs/zendriver/latest/zendriver/struct.InterceptBuilder.html#method.redirect
[`respond`]: https://docs.rs/zendriver/latest/zendriver/struct.InterceptBuilder.html#method.respond
[`modify_request`]: https://docs.rs/zendriver/latest/zendriver/struct.InterceptBuilder.html#method.modify_request

### Blocking ads

```rust,no_run
{{#include ../../../crates/zendriver/examples/intercept_block_ads.rs}}
```

[`start`] returns an [`InterceptHandle`]. The handle owns the background
actor — when you drop it, the actor receives a cancel signal, dispatches
`Fetch.disable`, and stops processing events. **Bind the handle to a
variable; letting it drop immediately silently disables interception.**
Use `let _intercept = ...` (note the leading underscore — Rust would warn
on a plain `_`, which is a different binding semantics that drops at end
of statement, not end of scope).

### Modifying headers

CDP semantics for `headers` on `Fetch.continueRequest` is **replacement**,
not merge — every header you want sent must appear in the returned map.
The `modify_request` closure receives a [`RequestInfo`] with the
original headers; copy them forward before stamping your additions on
top:

```rust,no_run
{{#include ../../../crates/zendriver/examples/intercept_modify_headers.rs}}
```

The closure runs synchronously per-event on the actor task; it should
not block. Spawn off the runtime if you need to call out to an async
service before deciding the override.

### Redirect and synthesize

```rust,ignore
let _intercept = tab.intercept()
    .redirect("*/old-api/*", "https://example.com/new-api/")?
    .respond(
        "*/api/health*",
        200,
        vec![("content-type".into(), "application/json".into())],
        b"{\"ok\":true}".to_vec(),
    )?
    .start();
```

Both `redirect` and `respond` reach an internal `Fetch.continueRequest`
or `Fetch.fulfillRequest` — the actor decides per event, so a single
`InterceptHandle` can host any mix of the four rule types.

## Stream API

When rules are too restrictive — e.g. you need to inspect the upstream
response body before deciding what to do, or you want to forward events
to your own pipeline — call [`subscribe`] instead of `start`:

```rust,ignore
use futures::StreamExt;
use zendriver::{AbortReason, RequestStage};

let mut stream = Box::pin(
    tab.intercept()
        .at_response()  // pause AFTER headers come back
        .subscribe()
);

while let Some(paused) = stream.next().await {
    let body = paused.body().await?;
    if body.windows(7).any(|w| w == b"BLOCKED") {
        paused.abort(AbortReason::BlockedByClient).await?;
    } else {
        paused.continue_().await?;
    }
}
```

Each [`PausedRequest`] is consumed by exactly one of
[`continue_`] / [`abort`] / [`respond`] / [`modify_and_continue`]. The
[`body`] method is `&self` and reads the upstream response body
non-destructively — only useful at the `Response` stage; at the `Request`
stage Chrome has no body yet.

[`PausedRequest`]: https://docs.rs/zendriver/latest/zendriver/struct.PausedRequest.html
[`continue_`]: https://docs.rs/zendriver/latest/zendriver/struct.PausedRequest.html#method.continue_
[`abort`]: https://docs.rs/zendriver/latest/zendriver/struct.PausedRequest.html#method.abort
[`respond`]: https://docs.rs/zendriver/latest/zendriver/struct.PausedRequest.html#method.respond
[`modify_and_continue`]: https://docs.rs/zendriver/latest/zendriver/struct.PausedRequest.html#method.modify_and_continue
[`body`]: https://docs.rs/zendriver/latest/zendriver/struct.PausedRequest.html#method.body

**Forgetting to release a `PausedRequest` deadlocks the page.** Chrome
holds the connection open until exactly one of the four terminal methods
arrives. If your stream consumer panics mid-flow, the actor's `Drop`
will dispatch `Fetch.disable` — every still-paused request fails with
`net::ERR_BLOCKED_BY_CLIENT`, which is unpleasant but not silent.

## Tracker / fingerprinter blocklist

The `tracker-blocking` feature builds a ready-made host-blocking rule on top
of interception, so you can block third-party trackers and fingerprinters
without writing patterns. Enable it on the builder and it installs on the main
tab and every new tab automatically.

```toml
[dependencies]
zendriver = { version = "0.1", features = ["tracker-blocking"] }
```

```rust,no_run
use zendriver::Browser;

let browser = Browser::builder()
    .block_trackers(true)                        // curated bundled list
    .tracker_blocklist_add(["ads.example.com"])  // + your own hosts
    .launch().await?;
```

| Method | Effect |
|--------|--------|
| [`block_trackers(true)`] | Enable blocking with the curated bundled list (`include_str!`-embedded — only adds binary size when the feature is on) |
| [`tracker_blocklist_add`] | Add extra hosts (repeatable; implicitly enables blocking) |
| [`tracker_blocklist_file`] | Add hosts from a local file (one host per line; `0.0.0.0 host` hosts-file lines tolerated) |
| [`tracker_blocklist_url`] | Add hosts fetched from a URL at launch |

Matching is host-based: a blocked host also blocks its subdomains. The bundle
ships only our own clean list — point `tracker_blocklist_url` at a third-party
list only if you accept that list's license.

[`block_trackers(true)`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.block_trackers
[`tracker_blocklist_add`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.tracker_blocklist_add
[`tracker_blocklist_file`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.tracker_blocklist_file
[`tracker_blocklist_url`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.tracker_blocklist_url

## Pattern + stage filters

By default the builder pauses on every request at the `Request` stage.
Restrict it with builder modifiers:

```rust,ignore
use zendriver::ResourceType;

let _h = tab.intercept()
    .pattern("*/static/*")   // URL filter for next-emitted RequestPattern
    .resource(ResourceType::Image)  // only images
    .at_response()           // pause after response headers, not before request
    .block("*/static/*")?
    .start();
```

Each call to `pattern()` opens a new `Fetch.RequestPattern`; chained
`resource` / `at_request` / `at_response` modifiers mutate that most-recent
pattern. Without a `pattern()`, the builder synthesizes one matching all
URLs at the rule-declared stage.

## Gotchas

- **`Fetch.enable` serializes the network.** Chrome routes every matched
  request through the JSON-RPC channel — round-trip per request adds
  latency. On heavy pages, expect 10-30% throughput loss even with a
  no-op `continue_` handler. Scope patterns tightly; prefer
  resource-type filters over `*` patterns.
- **Each tab carries one actor.** Calling `tab.intercept().start()` twice
  on the same tab without dropping the first handle leaves the second
  rule set inert — the first actor still has `Fetch.enable` and the new
  one can't re-enable on the same target.
- **CSP-restricted pages.** `Fetch.fulfillRequest` synthesizes responses
  that may violate a page's `Content-Security-Policy` (especially for
  scripts). Combine with `StealthProfile::spoofed()` which sets
  `bypass_csp = true` if you're synthesizing scripts.
- **HTTPS responses can't be re-signed.** `respond` and `modify_request`
  work for the wire payload Chrome sees, not for upstream-TLS-signed
  artifacts.

## See also

- [`InterceptBuilder` rustdoc] for every modifier method.
- [`Expect()`](./expect.md) for the orthogonal "wait for this network
  event" surface — `expect_request` observes without holding traffic;
  `intercept` actively rewrites it.

[`InterceptBuilder` rustdoc]: https://docs.rs/zendriver/latest/zendriver/struct.InterceptBuilder.html
