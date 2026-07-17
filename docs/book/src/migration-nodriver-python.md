# Migration from nodriver (Python)

If you're coming from `nodriver` (the original Python CDP wrapper that
zendriver-py was forked from), you'll find zendriver-rs's shape
familiar: same locator-style queries, same per-tab handle, same
isolated-world JS evaluation as a sandbox layer. The Rust port closes a
few rough edges nodriver carried — explicit `Frame` types instead of
flatten-mode juggling, a dedicated Cloudflare driver instead of the
inline `verify_cf` helper, and named API surface for the things
nodriver did through Python's dunder methods. The translation is
mostly mechanical: swap `await` for `.await?`, learn the four query
terminals, opt into Cargo features for the optional surface.

## Crosswalk table

The Python side uses the conventional `import nodriver as nd` alias.

| Operation | Python `nodriver` | zendriver-rs |
|-----------|-------------------|--------------|
| Launch browser | `browser = await nd.start()` | `let browser = Browser::builder().launch().await?;` |
| Launch headless | `await nd.start(headless=True)` | `Browser::builder().headless(true).launch().await?` |
| No-sandbox | `await nd.start(sandbox=False)` | `Browser::builder().arg("--no-sandbox").launch().await?` |
| Navigate (first tab) | `tab = await browser.get(url)` | `let tab = browser.main_tab(); tab.goto(url).await?;` |
| Open new tab | `tab = await browser.get(url, new_tab=True)` | `let tab = browser.new_tab_at(url).await?;` |
| Find by text | `await tab.find("Submit")` | `tab.find().text("Submit").one().await?` |
| Find by CSS (one) | `await tab.select("button.go")` | `tab.find().css("button.go").one().await?` |
| Find by CSS (many) | `await tab.select_all("li")` | `tab.find_all().css("li").many().await?` |
| Nth element | `(await tab.select_all("li"))[2]` | `tab.find().css("li").nth(2).one().await?` |
| Click | `await element.click()` | `element.click().await?` |
| Type text | `await element.send_keys("hi")` | `element.type_text("hi").await?` |
| Read text | `element.text` | `element.inner_text().await?` |
| Read attribute | `element.attrs["href"]` | `element.attr("href").await?` |
| Eval JS | `await tab.evaluate("document.title")` | `tab.evaluate_main::<String>("document.title").await?` |
| Eval JS, await promise | `tab.evaluate("p()", await_promise=True)` | `tab.evaluate_main::<T>("await p()").await?` |
| Iterate tabs | `browser.tabs` | `browser.tabs().await` |
| Cookies | `await browser.cookies.get_all()` | `browser.cookies().all().await?` |
| Screenshot | `await tab.save_screenshot(path)` | `let png = tab.screenshot().await?; std::fs::write(path, png)?;` |
| Solve Cloudflare | `await tab.verify_cf()` | `tab.cloudflare().wait_for_clearance(d).await?` (feature `cloudflare`) |
| Close | `await browser.stop()` | `browser.close().await?` |

## Behavioral differences worth knowing

### Iframes get a first-class `Frame` type

nodriver inherited Chromium's "flatten mode" for nested frames — every
node from a same-origin iframe appeared in the parent document's tree,
and you switched into out-of-process iframes (OOPIFs) by attaching to
the iframe's CDP target manually. zendriver-rs makes [`Frame`] a
first-class type with its own [`SessionHandle`], `find` / `find_all` /
`evaluate` / `evaluate_main`, and the same auto-refresh semantics as
top-level elements:

```rust,ignore
let main = tab.main_frame().await?;
let h1 = main.find().css("h1").one().await?;

// OOPIFs work the same — no manual attach.
if let Some(yt) = tab.frame_by_url("youtube.com").await? {
    yt.evaluate::<()>("document.querySelector('video').play()").await?;
}
```

You can also start from the `Tab` and re-target the query at a `Frame`
via [`FindBuilder::in_frame`]. See [Frames](./frames.md).

[`Frame`]: https://docs.rs/zendriver/latest/zendriver/struct.Frame.html
[`SessionHandle`]: https://docs.rs/zendriver-transport/latest/zendriver_transport/struct.SessionHandle.html
[`FindBuilder::in_frame`]: https://docs.rs/zendriver/latest/zendriver/struct.FindBuilder.html#method.in_frame

### Cloudflare bypass is a dedicated crate

nodriver ships a `tab.verify_cf()` helper that walks the shadow DOM to
find Turnstile's iframe and dispatches a click at the checkbox's
expected offset. zendriver-rs lifts that flow into the
[`zendriver-cloudflare`] crate (Cargo feature `cloudflare`), exposed
via [`Tab::cloudflare`] → [`CloudflareBypass::wait_for_clearance`]:

```rust,ignore
use std::time::Duration;
use zendriver::CloudflareError;

match tab.cloudflare()
    .wait_for_clearance(Duration::from_secs(30))
    .await
{
    Ok(_) => { /* cleared (token acquired or challenge gone) */ }
    Err(CloudflareError::NoChallenge) => { /* already clear */ }
    Err(e) => return Err(e.into()),
}
```

The driver uses the same shadow-DOM walk approach as nodriver, runs
the canonical 15%-from-left / 50%-from-top click at the iframe offset,
and polls the `cf-turnstile-response` input for a non-empty value.
Pair with `StealthProfile::spoofed()` for the best bypass rate. See
[Cloudflare](./cloudflare.md).

[`zendriver-cloudflare`]: https://docs.rs/zendriver-cloudflare/
[`Tab::cloudflare`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.cloudflare
[`CloudflareBypass::wait_for_clearance`]: https://docs.rs/zendriver/latest/zendriver/struct.CloudflareBypass.html#method.wait_for_clearance

### No magic methods — explicit `.await` and `.nth()`

nodriver leans on Python dunders to make the API feel imperative:

- `await tab` — `__await__` waits for the page to be ready.
- `tab[2]` — `__getitem__` returns the 3rd element of the last query.
- `for el in elements:` — implicit element iteration after a `find_all`.

Rust has no equivalent to these — every operation is a named method
call. The translations:

| Python idiom | Rust replacement |
|--------------|------------------|
| `await tab` | `tab.wait_for_load().await?` |
| `result = await tab.find_all("li"); result[2]` | `tab.find().css("li").nth(2).one().await?` |
| `for el in await tab.select_all("li"):` | `for el in tab.find_all().css("li").many().await? { ... }` |
| `tab[2]` (last result indexing) | not supported — capture the `Vec<Element>` to a `let` and index it |

The verbosity is a one-time tax for code that's easier to grep, easier
to refactor, and lets `rust-analyzer` see every callsite.

### `evaluate` returns deserialized JSON, not a CDP RemoteObject

nodriver's `tab.evaluate(js, await_promise=False)` returns
Chromium-specific `cdp.runtime.RemoteObject` wrappers — you fish out
`.value` or `.description`, type-check what you got, and handle the
"object reference" case manually for non-serializable returns.
zendriver-rs returns a typed Rust value via `serde`:

```rust,ignore
// Primitives.
let n: i32 = tab.evaluate_main("[1,2,3].length").await?;

// Strings.
let title: String = tab.evaluate_main("document.title").await?;

// Dynamic JSON.
let json: serde_json::Value = tab.evaluate_main("({a: 1, b: [2,3]})").await?;

// Strongly typed (define your own struct).
#[derive(serde::Deserialize)]
struct Meta { name: String, count: i32 }

let m: Meta = tab.evaluate_main("({name: 'x', count: 5})").await?;
```

For promise return values, await the promise *inside* the JS string:

```rust,ignore
let result: serde_json::Value = tab
    .evaluate_main("await fetch('/api/me').then(r => r.json())")
    .await?;
```

Non-serializable returns (DOM nodes, functions) error with
`ZendriverError::JsException` — for DOM access prefer `tab.find()`,
which returns an [`Element`] handle that exposes `inner_text`,
`attr`, `click`, etc.

[`Element`]: https://docs.rs/zendriver/latest/zendriver/struct.Element.html

### Errors are `Result`, not exceptions

Every fallible call returns
[`Result<T, ZendriverError>`](https://docs.rs/zendriver/latest/zendriver/type.Result.html).
nodriver raises Python exceptions (`NoSuchElementError`, `TimeoutError`,
plus a few wrappers around chromiumoxide errors). The Rust port flattens
them all into one
[`ZendriverError`](https://docs.rs/zendriver/latest/zendriver/enum.ZendriverError.html)
enum with `#[from]` conversions for the sub-crate errors. See the
[Error Reference](./error-reference.md) for every variant.

### Tab / Browser are cheap `Arc`-clones

`Tab` and `Browser` are `Clone + Send + Sync` — they're thin
`Arc`-wrappers over the underlying CDP session. Clone freely to pass
into helpers or `tokio::spawn` blocks. Every clone references the same
session, so an action on one clone is visible to all.

```rust,ignore
let tab = browser.main_tab();
let probe = tab.clone();
let handle = tokio::spawn(async move {
    probe.expect_response("/api/data").await
});
tab.goto("https://example.com").await?;
let _matched = handle.await??;
```

### Isolated-world is the default eval target

`tab.evaluate()` runs in an **isolated world** (sandboxed; no access
to page globals like `document` or `window.appConfig`). The escape
hatch is `tab.evaluate_main()` which runs in the page's default
context — the equivalent of nodriver's `tab.evaluate(...)`. The
isolated default keeps the page from detecting your evaluator via
`Function.prototype.toString` drift. See
[Architecture](./architecture.md#isolated-world-evaluation).

```rust,ignore
let n: i32 = tab.evaluate("[1,2,3].length").await?;             // sandbox
let title: String = tab.evaluate_main("document.title").await?; // page globals
```

## Cargo features

nodriver is one PyPI package with everything in the box. zendriver-rs
splits optional capabilities behind Cargo features so you pay only for
what you use.

| nodriver capability | Rust Cargo feature | What it gates |
|---------------------|--------------------|---------------|
| `tab.add_handler(nd.cdp.fetch.RequestPaused, ...)` rewriting | `interception` | `Tab::intercept()` plus the rule builder (`block` / `redirect` / `respond` / `modify_request`) and the `subscribe()` stream. |
| `await tab.expect_request(...)` (where supported) | `expect` | The `expect_request` / `expect_response` / `expect_dialog` / `expect_download` methods on `Tab`. |
| `await tab.verify_cf()` | `cloudflare` | `Tab::cloudflare()` plus the `CloudflareBypass` driver. Pulls in `interception`. |
| Chrome auto-download (separate `nodriver` extras) | `fetcher` | `zendriver_fetcher` for downloading Chrome for Testing binaries on demand. |
| Stealth | always on | Profiles via `StealthProfile::native()` (default recommendation), `spoofed()`, or `off()`. |

Enable in your `Cargo.toml`:

```toml
[dependencies]
zendriver = { version = "0.1", features = ["interception", "expect", "cloudflare"] }
```

If you're not sure where to start, enable `expect` (the
pre-register-then-await pattern saves you from event-handler race
conditions) and add the rest as you hit them.

## Known gaps in v0.1.0

Canvas / WebGL / audio / font fingerprint spoofing and a browserforge
equivalent have shipped since this section was first written — see the
[Fingerprint spoofing](./fingerprint.md) chapter, including
[pool + generative sources](./fingerprint.md#pool--generative-sources-zendriver-fingerprints)
for the `zendriver-fingerprints` crate (real-device persona dataset /
Bayesian-network sampler) that plays the same role as nodriver's pairing
with the `browserforge` library.

Things nodriver supports that zendriver-rs **doesn't yet**:

- **OCR helpers.** nodriver bundles `easyocr` / `tesseract` wrappers
  for text-in-image extraction. Not ported; pair `tab.screenshot()`
  with the `tesseract-rs` or `leptess` crate.
- **Widevine / DRM playback.** nodriver supports loading the Widevine
  CDM for protected video; zendriver-rs launches a vanilla Chrome / CfT
  binary that doesn't ship the CDM.
- **`__await__` on `tab` and `flow_to_finish`.** nodriver overloads
  `await tab` for "wait for the page to be ready". Rust call sites are
  always explicit: `tab.wait_for_load().await?` or `tab.wait_for_idle().await?`.
- **`__getitem__` on element collections.** No `tab[2]` shortcut —
  call `tab.find().css(...).nth(2).one().await?` or capture a
  `Vec<Element>` from `.many()` and index it via `[2]`.
- **`Element.children` walks.** nodriver exposes parent / child / sibling
  traversal on the element handle. zendriver-rs has limited traversal
  (see [`Element::children`](https://docs.rs/zendriver/latest/zendriver/struct.Element.html#method.children)
  and friends in the docs); deeper DOM walks may need a JS `evaluate`
  call returning the structured shape you need.
- **`tab.send_dom_event`** — direct DOM event synthesis isn't a
  first-class helper. Use `tab.evaluate_main` with the corresponding JS
  (`el.dispatchEvent(new Event('change'))`).

If you hit a gap that blocks your migration, please file an issue at
<https://github.com/TurtIeSocks/zendriver-rs/issues> — pre-1.0
prioritization is largely driven by reported migration friction.

## See also

- [Migration from zendriver (Python)](./migration-zendriver-python.md)
  — the zendriver Python package is a downstream fork of nodriver, so
  most differences from nodriver also apply to it.
- [Quickstart](./quickstart.md) — the minimal Rust launch / navigate /
  find / read flow, walked line by line.
- [Frames](./frames.md) — covers `Frame` semantics and the OOPIF
  auto-attach behavior in detail.
- [Cloudflare](./cloudflare.md) — full `CloudflareBypass` documentation
  including the four internal stages, limitations, and stealth pairing.
- [Architecture](./architecture.md) — the design choices behind the
  isolated-world default, auto-refresh on stale handles, and the
  single-actor CDP transport.
