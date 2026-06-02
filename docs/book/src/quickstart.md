# Quickstart

This chapter walks line-by-line through the "hello world" example from
`crates/zendriver/examples/hello.rs`. After this chapter you'll know how
to launch a browser, navigate to a URL, run a query, read element text,
and shut everything down.

## The example

```rust,no_run
{{#include ../../../crates/zendriver/examples/hello.rs}}
```

You can run it from the workspace root:

```text
cargo run --example hello -p zendriver
```

It launches Chrome headless, navigates to `https://example.com`, finds
the `<h1>` element on the page, prints its inner text, then shuts the
browser down. Expected output:

```text
h1 text: Example Domain
```

## Walkthrough

### 1. Launch the browser

```rust,ignore
let browser = Browser::builder().headless(true).launch().await?;
```

[`Browser::builder()`] returns a [`BrowserBuilder`] with sensible
defaults. The chain pattern lets you customize before launch:

- `.headless(true)` — runs Chrome without a UI window.
- `.headless(false)` — runs headed; useful while developing scripts.
- `.user_data_dir(path)` — pins Chrome's profile to a directory so
  cookies and localStorage persist across runs.
- `.stealth(StealthProfile::native())` — opt into the anti-detection
  patches (covered in [Stealth](./stealth.md)).
- `.chrome_path(path)` — bypass the `$PATH` lookup when you want a
  specific Chrome binary.

`.launch()` is async because it has to spin up the Chrome subprocess,
wait for the CDP WebSocket endpoint to come up, perform the initial
handshake, and attach to the first auto-opened tab.

[`Browser::builder()`]: https://docs.rs/zendriver/latest/zendriver/struct.Browser.html#method.builder
[`BrowserBuilder`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html

### 2. Grab the main tab

```rust,ignore
let tab = browser.main_tab();
```

Chrome always opens with one tab at `about:blank`. zendriver registers
that tab eagerly at launch and exposes it via
[`Browser::main_tab()`]. The returned [`Tab`] is `Clone + Send + Sync`,
so you can stash it in a struct, clone it across spawns, and pass it
into helpers freely — every clone refers to the same underlying CDP
session.

[`Browser::main_tab()`]: https://docs.rs/zendriver/latest/zendriver/struct.Browser.html#method.main_tab
[`Tab`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html

### 3. Navigate

```rust,ignore
tab.goto("https://example.com").await?;
tab.wait_for_load().await?;
```

`goto` dispatches `Page.navigate` and returns as soon as Chrome
acknowledges the request — not when the page is done loading. Call
`wait_for_load` afterwards if you want to block until the `load` event
fires. (You can also use `wait_for_idle` for "no network requests
in-flight", or set up `expect_response` ahead of the navigation for a
targeted wait — covered in [Expect()](./expect.md).)

### 4. Query an element

```rust,ignore
let h1 = tab.find().css("h1").one().await?;
```

[`Tab::find()`] returns a [`FindBuilder`] in the *configure* phase. The
chain encodes both the selector and any modifiers:

- `.css("h1")` — CSS selector (the most common selector kind).
- `.text("Submit")` — text-content matcher (anchor-style "find by
  visible text").
- `.xpath("//div[@id='main']")` — XPath escape hatch.
- `.role(AriaRole::Button)` — ARIA-role + accessible-name match
  (Playwright-style).
- `.tag("button").attr_contains("class", "primary").containing_text("Buy")`
  — bs4-like combinable *predicate* finders (`tag`, `attr`, `attr_contains`,
  `attr_starts_with`, `attr_ends_with`, `has_attr`, `attr_regex`,
  `containing_text`, `text_equals`, `text_matches`), all AND-ed together.
  Predicate methods can't be mixed with the single-selector methods above on
  one query (doing so errors with `ConflictingSelectors`).
- `.nth(2)` — pick the 2nd match.
- `.visible_only()` — skip `display:none` / zero-bbox elements.
- `.timeout(Duration::from_secs(5))` — override the default 30s wait.

A terminal method (`one`, `first`, `many`, `many_or_empty`, `count`,
`exists`) consumes the builder and dispatches. `.one()` waits for
exactly one match — errors with `ElementNotUnique` if there are zero or
more than one — making it perfect for queries that should be deterministic.

For the common CSS case, `tab.select("h1")` / `tab.select_all("nav a")` are
Python-parity convenience aliases for `find().css(...).one()` /
`find_all().css(...).many()`. The same pair exists on `Frame` and `Element`
(`Element::select` scopes to that element's subtree).

[`Tab::find()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.find
[`FindBuilder`]: https://docs.rs/zendriver/latest/zendriver/struct.FindBuilder.html

### 5. Read element text

```rust,ignore
let text = h1.inner_text().await?;
println!("h1 text: {text}");
```

[`Element::inner_text()`] dispatches a `Runtime.callFunctionOn` against
the cached `RemoteObjectId` for this element. There is no extra DOM
query — the [`Element`] handle remembers its CDP node, so reads /
attribute lookups / clicks / typing all share that single handle.

zendriver also auto-refreshes stale handles: if the page re-rendered and
your handle's `RemoteObjectId` was discarded, the next method call
silently re-runs the original query, gets a fresh handle, and retries.
You get to write straight-line code as if elements were durable.

[`Element::inner_text()`]: https://docs.rs/zendriver/latest/zendriver/struct.Element.html#method.inner_text
[`Element`]: https://docs.rs/zendriver/latest/zendriver/struct.Element.html

### 6. Shut down

```rust,ignore
browser.close().await?;
```

[`Browser::close()`] is the graceful shutdown path: send `Browser.close`,
wait for the Chrome subprocess to exit, then drop the transport actor.
You can also rely on `Drop` — the last `Browser` clone going out of
scope will fire `SIGTERM` at the subprocess — but explicit
`browser.close().await?` is preferred so you can surface shutdown
failures in your `Result`.

[`Browser::close()`]: https://docs.rs/zendriver/latest/zendriver/struct.Browser.html#method.close

## Next steps

- [Stealth](./stealth.md) — turn on anti-detection for sites that block
  headless browsers.
- [Input](./input.md) — realistic typing, mouse clicks with Bezier-path
  cursor moves, modifier keys.
- [Multi-tab](./multi-tab.md) — `Browser::new_tab` /
  `Browser::new_tab_at`, tab iteration, activate.
- [Frames](./frames.md) — querying inside cross-origin iframes, the
  `FindBuilder::in_frame` modifier.
