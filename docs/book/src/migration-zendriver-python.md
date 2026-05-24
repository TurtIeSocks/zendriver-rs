# Migration from zendriver (Python)

zendriver-rs deliberately mirrors the Python `zendriver` package's surface
shape — locator-style queries, fluent builders, and a thin wrapper over
CDP. Most scripts port across with mechanical translation: keep the
control flow, swap the `await zd.start()` for `Browser::builder().launch()`,
and let the Rust compiler tell you about the type-level differences
(`Result` vs exceptions, `&str` vs `str`). The biggest shift is
ergonomic, not architectural: every async call is `.await?`, handles
are cheap `Arc`-clones, and features that are always-on in Python live
behind Cargo features here so binary size scales with what you use.

## Crosswalk table

The Python side is shown with the conventional `import zendriver as zd`
alias.

| Operation | Python `zendriver` | zendriver-rs |
|-----------|--------------------|--------------|
| Launch browser | `browser = await zd.start()` | `let browser = Browser::builder().launch().await?;` |
| Launch headless | `await zd.start(headless=True)` | `Browser::builder().headless(true).launch().await?` |
| No-sandbox | `await zd.start(sandbox=False)` | `Browser::builder().arg("--no-sandbox").launch().await?` |
| Persistent profile | `await zd.start(user_data_dir="path")` | `Browser::builder().user_data_dir("path").launch().await?` |
| Navigate (first tab) | `tab = await browser.get(url)` | `let tab = browser.main_tab(); tab.goto(url).await?;` |
| Open new tab | `tab = await browser.get(url, new_tab=True)` | `let tab = browser.new_tab_at(url).await?;` |
| Find by text | `await tab.find("Submit", best_match=True)` | `tab.find().text("Submit").one().await?` |
| Find by CSS (one) | `await tab.select("button.go")` | `tab.find().css("button.go").one().await?` |
| Find by CSS (many) | `await tab.select_all("li")` | `tab.find_all().css("li").many().await?` |
| Find by XPath | `await tab.xpath("//button")` | `tab.find().xpath("//button").one().await?` |
| Click | `await element.click()` | `element.click().await?` |
| Type text | `await element.send_keys("hi")` | `element.type_text("hi").await?` |
| Read text | `element.text` | `element.inner_text().await?` |
| Read attribute | `element.attrs["href"]` | `element.attr("href").await?` |
| Run JS | `await tab.evaluate("document.title")` | `tab.evaluate_main::<String>("document.title").await?` |
| List tabs | `browser.tabs` | `browser.tabs().await` |
| Wait for response | `await tab.expect_request(url)` | `tab.expect_request(url).await?` (feature `expect`) |
| Block requests | `tab.add_handler(zd.cdp.fetch.RequestPaused, h)` | `tab.intercept().block("...")?.start()` (feature `interception`) |
| Solve Cloudflare | `await tab.verify_cf()` | `tab.cloudflare().wait_for_clearance(d).await?` (feature `cloudflare`) |
| Get cookies | `await browser.cookies.get_all()` | `browser.cookies().all().await?` |
| Set cookie | `await browser.cookies.set_all([...])` | `browser.cookies().set_many(vec![...]).await?` |
| Screenshot | `await tab.save_screenshot(path)` | `let png = tab.screenshot().await?; std::fs::write(path, png)?;` |
| Close | `await browser.stop()` | `browser.close().await?` |

## Behavioral differences worth knowing

### Errors are `Result`, not exceptions

Every fallible call returns
[`Result<T, ZendriverError>`](https://docs.rs/zendriver/latest/zendriver/type.Result.html).
You propagate with `?` and pattern-match on the enum to recover. There
is no `try / except zendriver.NoSuchElementError` — `ElementNotFound`
arrives as an `Err(ZendriverError::ElementNotFound { selector })` that
you handle inline:

```rust,ignore
match tab.find().css(".banner").one().await {
    Ok(el) => el.click().await?,
    Err(ZendriverError::ElementNotFound { .. }) => {
        // soft-fail: banner wasn't on this page variant
    }
    Err(e) => return Err(e),
}
```

See the [Error Reference](./error-reference.md) for every variant.

### `Tab` and `Browser` are cheap `Arc`-clones

In Python you mostly hold one `Browser` and one `Tab` reference per
script. In Rust both types are `Clone + Send + Sync` — internally an
`Arc` over the connection plus a session id. You can clone them freely
to pass into helper functions or `tokio::spawn` blocks; every clone
points at the same underlying CDP session, so a `tab.clone().goto(...)`
in one task is visible to the original `tab` in another.

```rust,ignore
let tab = browser.main_tab();
let probe = tab.clone();
tokio::spawn(async move {
    let _ = probe.expect_response("/api/").await;
});
tab.goto("https://example.com").await?;
```

### Pre-register-then-await replaces handler callbacks

Python `zendriver` exposes raw CDP event handlers
(`tab.add_handler(zd.cdp.fetch.RequestPaused, callback)`) for both
network observation and modification. zendriver-rs splits those two
intents:

- **Observation** → [`expect_request`] / [`expect_response`] /
  [`expect_dialog`] / [`expect_download`] from the `expect` feature.
  Pre-register before triggering the action; await the returned handle
  after. The subscriber is live from the moment `expect_*` returns, so
  no race with fast responses.
- **Modification** → [`Tab::intercept`] from the `interception`
  feature. A declarative rule builder (`block` / `redirect` /
  `respond` / `modify_request`) or a `subscribe()` stream for
  callback-style control.

Direct CDP-event handlers are still available via
[`Tab::session().subscribe::<E>()`](https://docs.rs/zendriver-transport/latest/zendriver_transport/struct.SessionHandle.html)
if you need them — but most flows lift cleanly into one of the two
helpers above.

[`expect_request`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.expect_request
[`expect_response`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.expect_response
[`expect_dialog`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.expect_dialog
[`expect_download`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.expect_download
[`Tab::intercept`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.intercept

### Builder methods replace keyword arguments

Python's `start(headless=True, sandbox=False, user_data_dir=...)` becomes
`Browser::builder().headless(true).arg("--no-sandbox").user_data_dir(...).launch().await?`.
Same for query options (`tab.find("x", best_match=True, timeout=10)` →
`tab.find().text("x").timeout(Duration::from_secs(10)).one().await?`).
The compiler catches typos that would silently default in Python.

### Isolated-world JS is the default

`tab.evaluate("document.title")` in Python runs in the page's main
world. In Rust `tab.evaluate::<T>("...")` runs in an
**isolated world** (sandboxed; no access to page globals like
`document` or `window.appConfig`). Use `tab.evaluate_main::<T>("...")`
for main-world access. The isolated default is what lets stealth keep
the page from detecting your evaluator script; see
[Architecture](./architecture.md#isolated-world-evaluation).

```rust,ignore
let n: i32 = tab.evaluate("[1,2,3].length").await?;             // sandbox; no DOM
let title: String = tab.evaluate_main("document.title").await?; // page globals
```

The turbofish (`::<String>`) drives JSON deserialization via `serde`,
so you can return any `serde::de::DeserializeOwned` type — `String`,
`i32`, your own `#[derive(Deserialize)]` struct, `serde_json::Value`
for dynamic payloads, etc.

### Find terminals are explicit

Python's `tab.find()` and `tab.select()` return "the match" (raising on
zero, silent on multiple). zendriver-rs forces the choice at query time:

| Terminal | Semantic |
|----------|----------|
| `.one()` | Exactly one match — errors with `ElementNotFound` if zero, `ElementNotUnique` if more. |
| `.one_or_none()` | Returns `Option<Element>`. |
| `.many()` | All matches — errors with `ElementNotFound` if zero. |
| `.many_or_empty()` | All matches; returns `Vec::new()` if zero. |

This makes "zero matches" an explicit code path in the source rather
than a runtime surprise.

## Cargo features

Python `zendriver` is one PyPI package — every feature ships in the
default install. zendriver-rs splits optional surface behind Cargo
features so binary size and compile time scale with what you use.

| Python capability | Rust Cargo feature | What it gates |
|-------------------|--------------------|---------------|
| `tab.add_handler(zd.cdp.fetch.RequestPaused, ...)` | `interception` | `Tab::intercept()`, the `Fetch.*`-based rule builder, and the `subscribe()` stream. |
| `tab.expect_request(...)`, dialogs, downloads | `expect` | All four `expect_*` methods on `Tab`. |
| `tab.verify_cf()` | `cloudflare` | `Tab::cloudflare()` and the `CloudflareBypass` driver. Pulls in `interception`. |
| Chrome auto-download | `fetcher` | `zendriver_fetcher` re-exports; downloads Chrome for Testing on demand. |
| Stealth | always on | `zendriver-stealth` is a non-optional dep; profiles via `StealthProfile::native()` / `spoofed()` / `off()`. |

Enable in your `Cargo.toml`:

```toml
[dependencies]
zendriver = { version = "0.1", features = ["interception", "expect", "cloudflare"] }
```

If you're not sure which to enable, start with `expect` (most scripts
end up wanting `expect_response` for network assertions) and add
`interception` / `cloudflare` if you hit those needs.

## Known gaps in v0.1.0

Capabilities the Python `zendriver` ships that are **not yet** in the
Rust port:

- **Canvas / WebGL / font / audio fingerprint spoofing.** Python's
  stealth layer randomizes these per-launch via JS bootstrap injection;
  the Rust port only ships the protocol-level patches (UA scrub,
  `webdriver` removal, hardware overrides) plus the optional
  `spoofed()` profile that patches the Navigator prototype. Active
  canvas-noise injection is on the post-v0.1 roadmap.
- **browserforge integration.** Python's optional dep for
  pre-canned realistic fingerprints isn't ported. Build a
  `StealthProfile` from explicit `UserAgentMetadata` fields instead.
- **OCR helpers.** Python's bundled OCR wrappers (`tesseract` /
  `easyocr`) for text-in-image extraction aren't ported. Pair Rust's
  `tab.screenshot()` with the `tesseract-rs` or `leptess` crate.
- **Widevine / DRM playback.** Python supports loading the Widevine CDM
  for protected video. The Rust port launches a vanilla Chrome / CfT
  binary that doesn't ship the CDM. Track upstream Chromium for a
  pluggable CDM story.
- **`browser.get(url)` shorthand.** Python returns a tab from
  `browser.get`. In Rust use `browser.main_tab(); tab.goto(url).await?`
  (or `browser.new_tab_at(url).await?` for the equivalent of
  `new_tab=True`). The split is intentional — `main_tab()` is sync, so
  binding it doesn't add a turn to your code.
- **`page.find(text, best_match=True)` fuzzy matching.** Rust's
  `.text(...)` is a substring match. For "closest match" semantics,
  use `text_regex(...)` with a permissive regex.

If you hit a gap that blocks your migration, please file an issue at
<https://github.com/TurtIeSocks/zendriver-rs/issues> — pre-1.0 prioritization
is largely driven by reported migration friction.

## See also

- [Quickstart](./quickstart.md) — the minimal Rust launch / navigate /
  find / read flow, walked line by line.
- [Expect()](./expect.md) — full coverage of the pre-register-then-await
  pattern that replaces Python's CDP event handlers for observation.
- [Interception](./interception.md) — the rule builder + stream API
  that replaces the handler-based rewriting flow.
- [Architecture](./architecture.md) — the Rust-specific design choices
  (single-actor CDP transport, isolated-world default, auto-refresh on
  stale handles) that shape the public surface.
