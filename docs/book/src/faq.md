# FAQ

Common questions about zendriver-rs. Each entry links into the relevant
chapter for the long-form answer.

## How do I run headed (with a visible window)?

Pass `.headless(false)` to the builder:

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
let browser = zendriver::Browser::builder()
    .headless(false)
    .launch()
    .await?;
# Ok(()) }
```

Useful while debugging — you can watch what the script does. Switch
back to `headless(true)` for production or CI. There is no "slow-mo" or
"keep open" flag; if you need the window to stick around after the
script exits, comment out `browser.close().await?` and `Ctrl+C` the
process.

## Why am I getting `NotActionable`?

[`ZendriverError::NotActionable`] fires when an element didn't pass the
actionability checks within the gate timeout. The checks are: visible,
enabled, stable (not animating), and hit-tested (no overlay blocking
clicks). The error message includes which check failed.

Common causes:

- **Visibility** — element has `display: none`, `visibility: hidden`,
  or zero bounding box. Use `tab.find().css("...").visible_only()` to
  skip these during the query.
- **Hit-test failure** — a modal overlay sits above the element. Close
  the overlay first, or pass `ClickOptions { force: true, ..default() }`
  to bypass the check.
- **Animation** — the element is still moving. Wait for
  `tab.wait_for_idle().await?` before clicking; the gate retries a few
  frames automatically but won't wait through a 2-second CSS transition.

If you genuinely want to click an invisible element (e.g. testing
keyboard nav), use `el.click_fast()` instead of `el.click()` — the
`_fast` variant skips the realism gate.

[`ZendriverError::NotActionable`]: https://docs.rs/zendriver/latest/zendriver/enum.ZendriverError.html#variant.NotActionable

## Does this work on Apple Silicon / M1+?

Yes. Chrome ships native arm64 binaries; zendriver-rs picks them up via
the standard PATH discovery. The
[Fetcher](./fetcher.md) also has a `Platform::MacArm64` variant and
downloads the matching CFT zip on Apple Silicon hosts.

## Does this work on Linux ARM64 / aarch64?

The library itself builds cleanly. The Fetcher does **not** download
Chrome on `linux-aarch64` because Chrome for Testing doesn't ship a
linux-arm64 build. Install Chrome through your distro's package
manager, then let the standard PATH discovery find it.

## Can I use a custom Chrome binary?

Yes — `.executable(path)` on the builder:

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
let browser = zendriver::Browser::builder()
    .executable("/opt/chrome/126/chrome")
    .launch()
    .await?;
# Ok(()) }
```

Useful for pinning a specific Chrome version, running Chromium / Edge,
or running a custom-built debug Chrome. The binary needs to support
`--remote-debugging-port=0` and emit the standard
`DevTools listening on ws://...` line — every recent stable Chrome /
Chromium / Edge does.

## Why is my `evaluate()` not seeing `window.foo`?

`tab.evaluate()` runs in an **isolated world** by default — a sandbox
that shares the DOM with the page but has its own globals. Use
`tab.evaluate_main()` for page-global access:

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.main_tab();
let title: String = tab.evaluate_main("document.title").await?;
let app_state: serde_json::Value = tab.evaluate_main("JSON.stringify(window.appState)").await?;
# Ok(()) }
```

The isolated default is a stealth feature — page scripts can't detect
your eval the way they could if you wrote into the main world. See
[Architecture](./architecture.md#isolated-world-evaluation).

## How do I detect bot-detection?

There's no built-in detector. The pragmatic test:

1. Run your target site headed with `StealthProfile::off()` first, then
   `native()`, then `spoofed()`. Compare behavior — if a feature works
   off but not native, the issue is in your stealth setup, not the
   anti-bot.
2. Hit [bot.sannysoft.com](https://bot.sannysoft.com/) and
   [arh.antoinevastel.com](https://arh.antoinevastel.com/bots/areyouheadless)
   to see what generic detectors find.
3. For Cloudflare specifically, check whether the gate is the visible
   Turnstile checkbox (`cloudflare` feature can pass it) or the silent
   challenge (which requires better stealth, not bypass tooling).
4. If the site blocks you even with `spoofed()`, the issue is usually
   not headless detection but: TLS JA3 fingerprint (use a real Chrome
   build, not chromiumoxide's), datacenter IP (rotate to residential),
   or rate-limit thresholds.

## What's the difference between native and spoofed stealth?

- **`StealthProfile::native()`** — patches only what fingerprinters see
  at the protocol level: UA scrub, launch flags, Emulation overrides.
  No JS bootstrap. Cheap, undetectable via `Function.prototype.toString`
  drift. Passes most consumer sites.
- **`StealthProfile::spoofed()`** — `native()` plus Navigator-prototype
  JS patches injected via `Page.addScriptToEvaluateOnNewDocument`.
  Restores `navigator.webdriver` to undefined, fixes `navigator.plugins`
  / `chrome` runtime / WebGL vendor, etc. Required to pass `sannysoft`
  and other active detectors. Pays a small per-navigation cost (script
  runs on every new document).

Full table in [Stealth](./stealth.md).

## My Chrome subprocess didn't clean up on Ctrl+C

`Drop` on the last `Browser` clone sends `SIGTERM`; the subprocess
exits within a second on a graceful shutdown. If your process panics
without unwinding (or aborts), the subprocess may linger. Two fixes:

- **Use `browser.close().await?` explicitly** at the end of your script
  — `close` waits for the subprocess to exit and surfaces any failure
  via the `Result`. `Drop` is a fallback, not the primary path.
- **Run zendriver-rs inside `tokio::select!` with a `ctrl_c` arm** so
  panics still trigger drop:

```rust,ignore
tokio::select! {
    res = your_main(&browser) => res?,
    _ = tokio::signal::ctrl_c() => {
        browser.close().await?;
    }
}
```

## How do I share login state across runs?

Pass `.user_data_dir(path)` to the builder. Chrome stores cookies,
localStorage, IndexedDB, etc under that path; second-and-onwards launches
inherit the state.

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
let browser = zendriver::Browser::builder()
    .user_data_dir("/home/me/.zendriver-state")
    .launch()
    .await?;
# Ok(()) }
```

Caveat: Chrome locks the directory while running. Two simultaneous
launches against the same `user_data_dir` will error out. Either
coordinate access (mutex) or use separate dirs per worker.

## Can I run multiple browsers in parallel?

Yes. `Browser` clones are cheap (`Arc` underneath) and `Send + Sync`,
so you can stash them in any worker pool. Run multiple **independent**
Chrome subprocesses by calling `Browser::builder().launch()` more than
once — each call spawns a separate Chrome. RAM-bound: each Chrome
instance is ~150-300 MB headless.

For multi-tab orchestration *within* one Chrome (cheaper), see
[Multi-tab](./multi-tab.md).

## How do I capture network traffic?

Two paths:

- **Observe only** — use [`expect_request`](./expect.md) /
  `expect_response` for individual events, or stash a
  `tab.intercept().subscribe()` stream that auto-`continue_()`s and logs
  each `PausedRequest`.
- **Modify** — use [`Interception`](./interception.md)'s rule API
  (block / redirect / respond / modify_request).

There's no Playwright-style "trace viewer" output; assemble the data
you want from those streams.

## Why is the first launch slow on macOS?

The `chromedriver` framework's notarization check runs the first time
the OS sees a Chrome binary. Subsequent launches reuse the cached
result and start in &lt;500 ms. On a fresh CFT download via the
[Fetcher](./fetcher.md) this is more visible because the binary is new
to the OS.

## What's the MSRV?

Rust 1.75. We don't aim to track stable bleeding-edge; MSRV bumps
follow the same SemVer policy as API changes
(see [`SEMVER.md`](https://github.com/cdpdriver/zendriver-rs/blob/main/SEMVER.md)).

## I'm getting `ZendriverError::Cdp` with code -32000. What now?

Code `-32000` ("Cannot find context") usually means the page navigated
out from under your call. zendriver-rs maps this specifically to
[`ZendriverError::Navigation`] rather than the raw Cdp variant — if
you're seeing the raw `Cdp` form, you're on a CDP method we haven't
special-cased. Wait for `wait_for_load()` / `wait_for_idle()` before
the call, or use [`expect_response`](./expect.md) to pin the wait to
the specific event you care about.

[`ZendriverError::Navigation`]: https://docs.rs/zendriver/latest/zendriver/enum.ZendriverError.html#variant.Navigation

## Where do I find the full list of errors?

[Error Reference](./error-reference.md) — every public variant of
`ZendriverError` plus the sub-crate errors that flow into it.
