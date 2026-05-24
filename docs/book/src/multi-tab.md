# Multi-tab

Chrome opens with one tab. zendriver-rs treats every additional tab as a
first-class [`Tab`] handle — the same type as the main tab, with the
same query / input / evaluate surface. Tabs are tracked in a browser-wide
registry that you can iterate, look up, or close from any clone of the
[`Browser`].

[`Tab`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html
[`Browser`]: https://docs.rs/zendriver/latest/zendriver/struct.Browser.html

## Opening tabs

Two constructors:

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
// Open about:blank and return as soon as the registrar sees the new tab.
let blank = browser.new_tab().await?;

// Open a URL — equivalent to new_tab().await? then goto(url).await?
let live = browser.new_tab_at("https://example.com").await?;
live.wait_for_load().await?;
# Ok(()) }
```

[`Browser::new_tab()`] and [`Browser::new_tab_at()`] both go through
`Target.createTarget` at browser scope (no `sessionId`). Each returns a
fully-initialised [`Tab`]:

1. Page/DOM/Runtime/Network CDP domains enabled.
2. Stealth bootstrap re-applied via the auto-attach observer chain.
3. Isolated-world ready for `evaluate()` calls.

Internally, `new_tab*` polls the tab registry every 50 ms for up to 5 s
waiting for the new target to register — typically returns within a few
milliseconds. If the auto-attach observer crashes or is misconfigured,
you'll get [`ZendriverError::TabNotFound`] after the 5 s window.

[`Browser::new_tab()`]: https://docs.rs/zendriver/latest/zendriver/struct.Browser.html#method.new_tab
[`Browser::new_tab_at()`]: https://docs.rs/zendriver/latest/zendriver/struct.Browser.html#method.new_tab_at
[`ZendriverError::TabNotFound`]: https://docs.rs/zendriver/latest/zendriver/enum.ZendriverError.html

## Iterating tabs

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
for tab in browser.tabs().await {
    println!("tab {}: {}", tab.target_id(), tab.url().await?);
}
# Ok(()) }
```

[`Browser::tabs()`] returns a snapshot `Vec<Tab>` covering every
currently-registered tab, including:

- The main tab (the one Chrome opened with).
- Tabs you opened via `new_tab*`.
- Tabs page scripts opened via `window.open(...)` (auto-attach wires
  these into the registrar).

Order is unspecified — the registry is a `HashMap` keyed by
`sessionId`. Tabs that close concurrently disappear from the snapshot on
the next call.

[`Browser::tab_count()`] is the cheap len-read on the same registry —
prefer it over `browser.tabs().await.len()` when you only need the
count.

[`Browser::tabs()`]: https://docs.rs/zendriver/latest/zendriver/struct.Browser.html#method.tabs
[`Browser::tab_count()`]: https://docs.rs/zendriver/latest/zendriver/struct.Browser.html#method.tab_count

## Activating a tab

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.new_tab().await?;
tab.activate().await?;
# Ok(()) }
```

[`Tab::activate()`] sends `Target.activateTarget`, which is what
"clicking the tab in Chrome's tab strip" does. The activated tab
becomes the visible tab in headed mode and the receiver of keyboard
focus events.

[`Tab::activate()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.activate

### Gotcha: inactive tabs don't receive input events

Chrome serializes physical input (mouse moves, key presses) through
whichever tab currently has OS focus. CDP `Input.dispatchMouseEvent`
and `Input.dispatchKeyEvent` calls **route through the page's render
process directly**, so they *do* reach inactive tabs — clicks, typing,
and the realistic Bezier-mouse path all work without activating first.

But **events that flow back through the OS layer** — e.g. fullscreen
requests, clipboard reads, focus-trap behaviors that read
`document.hasFocus()` — observe the OS-level active tab. If you hit a
"works in active tab, breaks in background tab" issue, the cause is
almost always `document.hasFocus()` returning `false` or a feature
gated on `document.visibilityState`.

The fix is to activate the tab before the input sequence:

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.new_tab_at("https://example.com").await?;
tab.activate().await?;
let btn = tab.find().css("button#go").one().await?;
btn.click().await?;
# Ok(()) }
```

You can leave any of the *other* tabs inactive — `activate` only sets
the OS focus to a single tab; it doesn't affect anyone else.

## Closing tabs

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.new_tab().await?;
tab.close().await?;
# Ok(()) }
```

[`Tab::close()`] consumes the [`Tab`] handle and sends
`Target.closeTarget`. The registrar removes the entry from the
browser-wide registry on the resulting `Target.targetDestroyed` event.
Existing clones of the closed tab will error on their next CDP call
with [`ZendriverError::SessionClosed`].

Closing every tab does **not** close the browser. To shut down the
whole subprocess, call [`Browser::close()`] (see [Quickstart](./quickstart.md)).

[`Tab::close()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.close
[`Browser::close()`]: https://docs.rs/zendriver/latest/zendriver/struct.Browser.html#method.close

## End-to-end example

This example opens three tabs at distinct URLs, prints each tab's URL +
title, then closes the whole browser (which tears down every tab):

```rust,no_run
{{#include ../../../crates/zendriver/examples/multi_tab.rs}}
```

Run it with:

```text
cargo run --example multi_tab -p zendriver
```

Expected output (target IDs vary):

```text
tab_count = 3
  [0] target=B... url=data:text/html,... title="B"
  [1] target=C... url=data:text/html,... title="C"
  [2] target=A... url=https://example.com/ title="Example Domain"
```

## Concurrency note

[`Tab`] is `Clone + Send + Sync`. You can spawn one worker per tab and
drive them in parallel:

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
let urls = ["https://example.com", "https://example.org", "https://example.net"];
let mut handles = Vec::new();
for u in urls {
    let tab = browser.new_tab().await?;
    handles.push(tokio::spawn(async move {
        tab.goto(u).await?;
        tab.wait_for_load().await?;
        let title = tab.title().await?;
        Ok::<_, zendriver::ZendriverError>(title)
    }));
}
for h in handles {
    println!("{}", h.await.unwrap()?);
}
# Ok(()) }
```

The transport actor serializes CDP frames across the WebSocket, so the
underlying CDP traffic is sequenced — but every `await` point yields,
letting other tab workers make progress. The aggregate throughput is
limited by CDP RTT rather than your spawn count.
