# Frames

A page is a tree of frames: one main frame (the top-level document) plus
zero or more child frames (typically `<iframe>` elements). zendriver-rs
exposes every frame as a first-class [`Frame`] handle with its own query
and JS-evaluation surface, so you can drive iframe content with the same
ergonomics as the top-level page.

[`Frame`]: https://docs.rs/zendriver/latest/zendriver/struct.Frame.html

## The frame tree

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.main_tab();
let main = tab.main_frame().await?;     // top-level document
let all  = tab.frames().await?;         // main + every attached child
println!("{} frames total", all.len());
for f in &all {
    println!("  - id={} main={} url={:?}",
             f.id(), f.is_main(), f.url().await);
}
# Ok(()) }
```

[`Tab::main_frame()`] returns the top-level frame (lazily — the first
call dispatches `Page.getFrameTree` and caches the result).
[`Tab::frames()`] returns a snapshot of the main frame plus every
attached child the tab currently tracks.

The registry observes `Page.frameAttached` / `Page.frameDetached`
events, so the snapshot is current as of the last event drained from the
session. Just-attached frames (within the same event loop tick as the
parent's `load` event) may not appear until the event lands — poll
briefly if you depend on a specific child being present immediately.

[`Tab::main_frame()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.main_frame
[`Tab::frames()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.frames

## Looking up specific frames

Two convenience lookups for common cases:

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.main_tab();
// By URL substring — useful for "the YouTube embed somewhere on this page".
if let Some(yt) = tab.frame_by_url("youtube.com").await? {
    yt.evaluate::<()>("document.querySelector('video').play()").await?;
}

// By name attribute — useful for legacy frame layouts that name their iframes.
if let Some(content) = tab.frame_by_name("content").await? {
    let el = content.find().css("h1").one().await?;
    println!("{}", el.inner_text().await?);
}
# Ok(()) }
```

[`Tab::frame_by_url()`] does a substring match against each child
frame's URL (useful when you don't know the exact path / query string).
[`Tab::frame_by_name()`] reads the `name` attribute set by the parent
`<iframe name="...">`.

Both return `Option<Frame>`. Iterate `tab.frames().await?` yourself for
anything more elaborate.

[`Tab::frame_by_url()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.frame_by_url
[`Tab::frame_by_name()`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.frame_by_name

## Frame-scoped queries

A [`Frame`] has its own `find` / `find_all` / `evaluate` /
`evaluate_main` — all scoped to that frame's document and execution
context:

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.main_tab();
let main = tab.main_frame().await?;

let h1 = main.find().css("h1").one().await?;
println!("main frame h1 = {}", h1.inner_text().await?);

// Per-frame JS evaluation. Isolated world by default — same as
// Tab::evaluate.
let title: String = main.evaluate("document.title").await?;
println!("main frame title = {title}");
# Ok(()) }
```

[`Frame::find()`] and [`Frame::evaluate()`] dispatch CDP calls bound to
the frame's `contextId`, so the query runs against that frame's DOM,
not the parent's. This is the lever for driving iframe content without
hunting for cross-frame DOM access workarounds.

[`Frame::find()`]: https://docs.rs/zendriver/latest/zendriver/struct.Frame.html#method.find
[`Frame::evaluate()`]: https://docs.rs/zendriver/latest/zendriver/struct.Frame.html#method.evaluate

## `FindBuilder::in_frame`

When you'd rather start the query from the [`Tab`] but target a specific
[`Frame`], use [`FindBuilder::in_frame()`]:

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.main_tab();
let yt = tab.frame_by_url("youtube.com").await?
    .ok_or_else(|| zendriver::ZendriverError::Other("no YT frame".into()))?;

let play = tab
    .find()
    .in_frame(&yt)            // re-target to the YT iframe
    .css(".ytp-play-button")
    .one()
    .await?;
play.click().await?;
# Ok(()) }
```

This is precedence-equivalent to `yt.find().css(...).one().await?`.
Use whichever reads better at the call site — `in_frame` shines when
you're composing helpers that take a [`Tab`] and a [`Frame`] reference
and want a single fluent chain.

[`FindBuilder::in_frame()`]: https://docs.rs/zendriver/latest/zendriver/struct.FindBuilder.html#method.in_frame

## Cross-origin iframes (OOPIFs)

A same-origin child iframe shares the parent's render process, so its
DOM is reachable via the parent's CDP session. A cross-origin iframe
(Out-Of-Process IFrame — OOPIF) gets its own render process, and Chrome
exposes it as a **separate CDP target** that auto-attaches to the same
browser connection.

zendriver's tab registrar wires OOPIF targets in automatically. From
your code's perspective:

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.main_tab();
# tab.goto("https://example.com").await?;
// Same call as for same-origin frames — OOPIFs show up in the same
// `tab.frames()` snapshot.
for f in tab.frames().await? {
    println!("frame {} url={:?}", f.id(), f.url().await);
}

// And the same Frame::find call works regardless of which process
// hosts the iframe.
if let Some(ad) = tab.frame_by_url("doubleclick.net").await? {
    let _ = ad.find().css(".close").one().await?.click().await;
}
# Ok(()) }
```

There's no special API and no `attach_to_oopif`-style call to remember.
This is one of the major ergonomic wins over the WebDriver-derived
libraries, where you usually have to manually switch frames or attach
to the OOPIF's session by hand.

## End-to-end example

This example loads a page that hosts a `srcdoc` iframe, enumerates
every frame, then runs a query inside the child to prove that
`Frame::find` resolves against the iframe's document (not the parent's):

```rust,no_run
{{#include ../../../crates/zendriver/examples/iframe_inspect.rs}}
```

Expected output:

```text
frames so far: 2
  - id=... main=true url=Ok("data:text/html,...")
  - id=... main=false url=Ok("about:srcdoc")
iframe button text = "hello from iframe"
```

The `srcdoc` keeps the iframe same-origin so the example stays
self-contained, but the API call shape is identical for cross-origin
OOPIFs.

## Frame lifecycle and auto-refresh

Frames navigate independently. When a child iframe navigates, its
existing execution context is destroyed and a new one is created. Any
[`Element`] handles you hold from the old context become stale.

[`Element`]: https://docs.rs/zendriver/latest/zendriver/struct.Element.html

zendriver's auto-refresh handles this transparently in most cases — the
next method call on a stale handle re-resolves the original query
against the new context. For repeated reads against a freshly-navigated
iframe, just re-run the query against the [`Frame`] handle — the
[`Frame`] itself stays valid across the navigation (the `frameId` is
stable; only the `contextId` rotates).
