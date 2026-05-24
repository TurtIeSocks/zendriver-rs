# Migration from Playwright

zendriver-rs's surface borrows heavily from Playwright (locator-based
queries, pre-register expectations, fluent builders) so most flows port
straightforwardly. This page is a crosswalk for the common operations,
plus a note on the structural differences that don't have a 1:1 mapping.

The Playwright side is shown in JavaScript/TypeScript; the Python
binding is structurally identical (snake_case methods, otherwise the
same shape).

## Crosswalk table

| Operation | Playwright | zendriver-rs |
|-----------|------------|--------------|
| Launch browser | `await chromium.launch()` | `Browser::builder().launch().await?` |
| Launch headed | `chromium.launch({ headless: false })` | `Browser::builder().headless(false).launch().await?` |
| Open a tab / page | `await context.newPage()` | `browser.new_tab().await?` |
| Reuse first tab | `(await context.pages())[0]` | `browser.main_tab()` |
| Navigate | `await page.goto(url)` | `tab.goto(url).await?` |
| Wait for load | implicit on `goto` | `tab.wait_for_load().await?` |
| Wait for network idle | `await page.waitForLoadState("networkidle")` | `tab.wait_for_idle().await?` |
| Find one element (CSS) | `page.locator("button").click()` | `tab.find().css("button").one().await?.click().await?` |
| Find by text | `page.getByText("Submit")` | `tab.find().text("Submit").one().await?` |
| Find by ARIA role | `page.getByRole("button", { name: "Go" })` | `tab.find().role(AriaRole::Button).name("Go").one().await?` |
| Find by XPath | `page.locator("xpath=//button")` | `tab.find().xpath("//button").one().await?` |
| Find all | `await page.locator("li").all()` | `tab.find().css("li").many().await?` |
| Get nth match | `page.locator("li").nth(2)` | `tab.find().css("li").nth(2).one().await?` |
| Click | `await locator.click()` | `el.click().await?` |
| Type text | `await locator.fill("hello")` | `el.set_value("hello").await?` (instant) |
| Type with key events | `await locator.pressSequentially("hi")` | `el.type_text("hi").await?` |
| Press key | `await locator.press("Enter")` | `el.press(Key::Special(SpecialKey::Enter)).await?` |
| Read text | `await locator.innerText()` | `el.inner_text().await?` |
| Read attribute | `await locator.getAttribute("href")` | `el.attr("href").await?` |
| Check visibility | `await locator.isVisible()` | `el.is_visible().await?` |
| Eval JS (page world) | `await page.evaluate(() => document.title)` | `tab.evaluate_main::<String>("document.title").await?` |
| Eval JS (isolated) | n/a (always main world) | `tab.evaluate::<String>("...").await?` |
| Wait for response | `await page.waitForResponse("**/api/*")` | `tab.expect_response("/api/").await?` |
| Wait for request | `await page.waitForRequest("**/auth")` | `tab.expect_request("/auth").await?` |
| Wait for download | `page.waitForEvent("download")` | `tab.expect_download().await?.await?` |
| Handle dialog | `page.on("dialog", d => d.accept())` | `let d = tab.expect_dialog(); ...; d.await?.accept(None).await?` |
| Intercept / block | `route.abort()` in `page.route` | `tab.intercept().block("*/ads/*")?.start()` |
| Modify request | `route.continue({headers})` | `tab.intercept().modify_request("...", \|req\| {...})?.start()` |
| Screenshot | `await page.screenshot()` | `tab.screenshot().await?` |
| Cookies (get all) | `await context.cookies()` | `browser.cookies().all().await?` |
| LocalStorage | `await page.evaluate("...")` (no helper) | `tab.local_storage().get("k").await?` |
| Close | `await browser.close()` | `browser.close().await?` |

## Structural differences

### Async runtime is Tokio, not the JS event loop

Every `await` is a Tokio await. Every method that does I/O takes
`&self` and returns a `Future`. You drive it with `tokio::main`:

```rust,ignore
#[tokio::main]
async fn main() -> zendriver::Result<()> {
    let browser = zendriver::Browser::builder().launch().await?;
    // ...
    Ok(())
}
```

There is no implicit "current page" — every operation takes an explicit
[`Tab`] handle. Multiple tabs run in parallel by cloning the
`Tab` and spawning Tokio tasks.

[`Tab`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html

### Builder pattern instead of object-config

Playwright uses option-bag objects:

```js
await page.click("button", { force: true, timeout: 5000 });
```

zendriver-rs uses fluent builders with terminal methods:

```rust,ignore
use std::time::Duration;
use zendriver::ClickOptions;

let el = tab.find().css("button")
    .timeout(Duration::from_secs(5))
    .one()
    .await?;
el.click_with(ClickOptions { force: true, ..Default::default() }).await?;
```

Builders are checked at compile time — there's no `{ tymeout: ... }`
typo that silently uses the default.

### No global `Browser` / `BrowserContext` split

Playwright separates `Browser` (the process) from `BrowserContext`
(an isolated cookie/storage scope). zendriver-rs has only `Browser`;
every `Tab` shares the browser-scope cookie jar and is the equivalent
of one Playwright page inside the default context.

If you need multi-context isolation, launch a second `Browser`. The
overhead is one extra Chrome subprocess — heavier than a context, but
the isolation is total (separate user-data-dir, separate cookies,
separate process). Most testing flows can avoid it.

### Find returns one or many, explicitly

Playwright's `locator()` lazily evaluates to "zero or more matches"
until you call a terminal action. zendriver-rs forces the choice at
query time:

| Terminal | Semantic |
|----------|----------|
| `.one()` | Exactly one match — errors with `ElementNotUnique` otherwise. |
| `.first()` | First match — errors with `ElementNotFound` if zero. |
| `.many()` | All matches — errors with `ElementNotFound` if zero. |
| `.many_or_empty()` | All matches; returns `Vec::new()` if zero. |
| `.count()` | Just the count. |
| `.exists()` | Boolean. |

Force-picking `.first()` over `.one()` is a documented choice when the
page may legitimately have multiple matching elements. Playwright's
implicit "first-of-many" can mask bugs.

### Element handles auto-refresh on stale

Playwright re-resolves locators on every call by design. zendriver-rs
caches the CDP `RemoteObjectId` per `Element` for speed; if the page
re-renders and invalidates the handle, the next method call replays the
original query, gets a fresh handle, and retries silently. Handles
returned from raw `evaluate` calls (without an underlying selector)
error with `NotRefreshable` instead.

This means `let el = tab.find().css("...").one().await?; el.click().await?;`
is just as safe across navigations as Playwright; you don't need to
re-find before every action.

### Isolated-world vs main-world JS

Playwright always evaluates user JS in the main world. zendriver-rs
defaults to an **isolated world** (sandbox) via `evaluate()`, which
means your JS can't see page globals — useful for stealth (the page
can't detect your eval), risky if you actually need `document.title`
etc. `evaluate_main()` is the main-world escape hatch.

```rust,ignore
let title: String = tab.evaluate_main("document.title").await?;     // page globals work
let n: i32 = tab.evaluate("[1,2,3].length").await?;                  // sandboxed; no DOM
```

### Stealth is on by default

Playwright launches with the headless-Chrome `HeadlessChrome` UA, the
`navigator.webdriver = true` tell, and no anti-detection patches.
zendriver-rs defaults to `StealthProfile::native()` — the UA scrub plus
launch-flag set passes most consumer-site detectors. For active
fingerprint detection (`sannysoft`, etc), opt into
`StealthProfile::spoofed()`. See [Stealth](./stealth.md).

## What's not ported

- **Trace viewer / video recording** — out of scope; integrate with
  `tab.screenshot()` plus your own ffmpeg pipeline if needed.
- **Test runner** (`@playwright/test`) — use `cargo test` plus the
  zendriver-rs surface; the [`expect`](./expect.md) feature covers the
  pre-register pattern that powers Playwright's `expect(locator)`
  assertions.
- **Codegen** (`playwright codegen`) — not implemented.
- **Mobile emulation** (`devices`) — set the User-Agent + viewport
  manually via `StealthProfile`.

See [Architecture](./architecture.md) for why these are out of scope
rather than not-yet-built.
