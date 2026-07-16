# Per-context isolation

A [`BrowserContext`](https://docs.rs/zendriver/latest/zendriver/struct.BrowserContext.html)
is a thin RAII wrapper over a Chrome
[`BrowserContextID`](https://chromedevtools.github.io/devtools-protocol/tot/Browser/#type-BrowserContextID)
— the CDP-side primitive for cookie + storage isolation within a
single Chrome process. Tabs opened in a `BrowserContext` see their
own cookie jar, IndexedDB, and (optionally) their own proxy; tabs in
the default context still share the browser-wide jar, exactly as
before. The two APIs coexist; existing code that calls
`browser.new_tab()` is unaffected.

> **When to use it.** Per-request proxy bindings, parallel sessions
> with different logins under one Chrome process, A/B fingerprint
> tests where the cookie state must not bleed across runs. If you
> need separate user-data-dir, separate GPU caches, or
> process-level isolation, launch a second [`Browser`](./multi-tab.md)
> instead.

## Quick start

```rust,no_run
use zendriver::Browser;

#[tokio::main]
async fn main() -> zendriver::Result<()> {
    let browser = Browser::builder().launch().await?;

    let ctx = browser.create_browser_context().await?;
    let tab = ctx.new_tab().await?;
    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    // ctx dropped at end of scope -> Target.disposeBrowserContext
    // is scheduled on the runtime, tearing down cookies + tabs.
    Ok(())
}
```

`Browser::create_browser_context` rejects if the underlying connection
is closed; otherwise it returns the new guard immediately — the CDP
round-trip is sub-millisecond.

## Per-context proxy

Use [`create_browser_context_with`](https://docs.rs/zendriver/latest/zendriver/struct.Browser.html#method.create_browser_context_with)
when the isolated context should route through its own upstream
proxy:

```rust,no_run
# use zendriver::Browser;
# async fn ex(browser: &Browser) -> zendriver::Result<()> {
let ctx = browser
    .create_browser_context_with(
        Some("http://proxy.example.com:8080".into()),
        // bypass list — same shape as Chrome's --proxy-bypass-list
        Some("<-loopback>".into()),
    )
    .await?;
let tab = ctx.new_tab().await?;
tab.goto("https://api.ipify.org").await?;
# Ok(()) }
```

`proxy_server` is forwarded to CDP `Target.createBrowserContext` as the
`proxyServer` field. Chrome accepts the same URL shapes as the
`--proxy-server` command-line flag (`http://`, `socks5://`,
`host:port` without scheme). `proxy_bypass_list` defaults to none.

Note: `proxyServer` alone does **not** carry proxy authentication —
Chrome will issue a 407 if the upstream requires Basic auth. For that,
use the first-class builder below instead.

## Per-context proxy authentication

[`Browser::browser_context`](https://docs.rs/zendriver/latest/zendriver/struct.Browser.html#method.browser_context)
returns a [`BrowserContextBuilder`](https://docs.rs/zendriver/latest/zendriver/struct.BrowserContextBuilder.html)
that binds a proxy *and* its credentials to the context, so every tab
it opens is transparently authenticated — no per-tab handle to hold:

```rust,no_run
# use zendriver::Browser;
# async fn ex(browser: &Browser) -> zendriver::Result<()> {
let ctx = browser
    .browser_context()
    .proxy("http://user:pass@proxy.example.com:8080")
    .proxy_bypass("<-loopback>")
    .build()
    .await?;
let tab = ctx.new_tab().await?;
tab.goto("https://api.ipify.org").await?;
# Ok(()) }
```

Userinfo embedded in `.proxy()` (`user:pass@host:port`) is auto-split
into credentials and stripped from the `proxyServer` sent to Chrome
(which would otherwise ignore it). Call `.proxy_auth(user, pass)`
(requires the `interception` feature) to supply — or override —
credentials explicitly:

```rust,no_run
# use zendriver::Browser;
# async fn ex(browser: &Browser) -> zendriver::Result<()> {
let ctx = browser
    .browser_context()
    .proxy("http://proxy.example.com:8080")
    .proxy_auth("user", "pass")
    .build()
    .await?;
# let _ = ctx; Ok(()) }
```

Under the hood, `build()` registers the credentials in the browser's
per-context registry; each tab opened in that context has a
`Fetch.authRequired` handler auto-installed, chained into the same
per-session interception actor used for tracker blocking (never two
actors on one session). Without the `interception` feature, `.proxy()`
and `.proxy_bypass()` still work (proxy routing only); embedded or
explicit credentials are logged as inactive since there is no actor to
install them into.

`create_browser_context_with` (proxy, no auth) and
`create_browser_context` (no proxy) remain available as lighter-weight
convenience constructors when authentication isn't needed.

## Tabs in a context

```rust,no_run
# use zendriver::{Browser, BrowserContext};
# async fn ex(ctx: &BrowserContext) -> zendriver::Result<()> {
// One tab on about:blank — same defaults as `Browser::new_tab`.
let blank = ctx.new_tab().await?;

// Or start at a specific URL — saves one `goto` round-trip.
let preset = ctx.new_tab_at("https://example.com").await?;
# Ok(()) }
```

Both methods thread `ctx.id()` into `Target.createTarget` so the new
target is bound to this context. Cross-context tabs cannot share
cookies; the test suite exercises this on every CI run.

## Drop semantics

`BrowserContext::drop` schedules `Target.disposeBrowserContext` on the
current Tokio runtime via `tokio::spawn` (the CDP call is async, but
`Drop` is sync). Two implications:

- **Disposal is fire-and-forget.** If the parent runtime is shutting
  down at the same instant the guard drops, the dispose may not land
  before the process exits. The Chrome side cleans up at process exit
  anyway; this only matters for long-lived browsers reused across
  many contexts.
- **Drop order matters for observability.** Drop the `Tab` handles
  first (they hold references into the context's targets), then the
  `BrowserContext`. The Rust borrow checker enforces this for you —
  the example below compiles only because `tab` goes out of scope
  before `ctx`.

```rust,no_run
# use zendriver::Browser;
# async fn ex(browser: &Browser) -> zendriver::Result<()> {
let ctx = browser.create_browser_context().await?;
{
    let tab = ctx.new_tab().await?;
    tab.goto("https://example.com").await?;
    // `tab` drops here.
}
// `ctx` drops next. dispose() spawned now.
# drop(ctx); Ok(())
# }
```

If you need to wait for disposal to complete before continuing (e.g.
before launching a second context that shares the proxy host), use
the explicit
[`BrowserContext::dispose`](https://docs.rs/zendriver/latest/zendriver/struct.BrowserContext.html#method.dispose)
method, which awaits the CDP call and returns its `Result`.

## Worked example: rotating-proxy session pool

A common pattern: pool of independent sessions, each pinned to a
different upstream proxy, recycled per request. Each iteration spawns
a fresh `BrowserContext` and disposes it via `Drop` once the request
result is captured.

```rust,no_run
use zendriver::Browser;

#[tokio::main]
async fn main() -> zendriver::Result<()> {
    let browser = Browser::builder().launch().await?;
    let proxies = [
        "http://proxy-a.example.com:8080",
        "http://proxy-b.example.com:8080",
        "http://proxy-c.example.com:8080",
    ];

    for proxy in proxies {
        let ctx = browser
            .create_browser_context_with(Some(proxy.into()), None)
            .await?;
        let tab = ctx.new_tab_at("https://httpbin.org/ip").await?;
        tab.wait_for_load().await?;
        let body = tab.find().css("pre").one().await?.inner_text().await?;
        println!("via {proxy}: {body}");
        // ctx + tab drop here — context disposed before next iter.
    }

    Ok(())
}
```

See `examples/browser_context_isolation.rs` in the source tree for a
runnable variant exercising the full round-trip — including
`BrowserContextBuilder`'s auth — against a real rotating upstream proxy
(set `ZD_PROXY`).

## Limitations

- **Single Chrome process.** Contexts share Chromium binaries, GPU
  process, GPU cache, and the user-data-dir. A compromised renderer
  in one context can in principle observe shared state. If process-
  level isolation matters, launch a second `Browser`.
- **Per-context auth requires `interception`.** `Browser::browser_context().proxy_auth(...)`
  (or embedded `.proxy("http://user:pass@host:port")` userinfo) needs the
  `interception` feature to install the per-tab `Fetch.authRequired`
  handler; without it, proxy routing still works but credentials are
  inactive (a warning is logged). See
  [Per-context proxy authentication](#per-context-proxy-authentication)
  above, or use a per-tab [interception handler](./interception.md) for
  finer control.
- **Extension scoping.** Chrome only loads `--load-extension` content
  scripts into the default context. Tabs opened in a non-default
  `BrowserContext` will not see your extensions. Workaround: stay on
  the default context when extensions are required, or inject
  equivalent scripts via `Page.addScriptToEvaluateOnNewDocument`.
