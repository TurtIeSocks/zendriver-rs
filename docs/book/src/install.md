# Install

zendriver-rs is published on crates.io as `zendriver`. The base crate
gives you everything you need for navigation, queries, input, multi-tab,
frames, and stealth. Optional Cargo features turn on network
interception, the `expect()` surface, Cloudflare bypass, and the Chrome
for Testing downloader.

## Basic install

For the standard always-on surface (Browser + Tab + Element + Frame +
StealthProfile + queries + input + cookies + storage + screenshots):

```toml
[dependencies]
zendriver = "0.1"
tokio = { version = "1", features = ["full"] }
```

This pulls in `zendriver`, `zendriver-transport`, and `zendriver-stealth`
transitively. No system dependencies beyond Chrome (or Chromium / Edge —
anything that speaks CDP).

## Minimum install

zendriver requires a Tokio runtime. The smallest viable setup:

```toml
[dependencies]
zendriver = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

You give up the convenience features of `tokio = "full"` but pay less in
compile time. The `macros` feature is required for `#[tokio::main]` and
`#[tokio::test]`; `rt-multi-thread` is required by zendriver's internal
spawn calls.

## Feature matrix

| Feature        | Pulls in                          | Use case                                                                  | Extra deps                                  |
|----------------|-----------------------------------|---------------------------------------------------------------------------|---------------------------------------------|
| (default)      | zendriver-transport, -stealth     | Navigation, queries, input, cookies, storage, screenshots, multi-tab     | none                                        |
| `interception` | zendriver-interception            | Block/modify/serve requests via the Fetch CDP domain                     | none                                        |
| `expect`       | (in-tree module)                  | Playwright-style `expect_request` / `expect_response` / `expect_dialog`  | none                                        |
| `monitor`      | (in-tree module)                  | `tab.monitor()` — persistent `Stream<NetworkEvent>` (HTTP / WS / SSE)     | none                                        |
| `cloudflare`   | zendriver-cloudflare, interception| Auto-solve Cloudflare Turnstile challenges                               | none                                        |
| `fetcher`      | zendriver-fetcher                 | Download Chrome for Testing on-demand via the official JSON API         | `reqwest`, `zip`, `sha2`, `dirs`            |

Enable features additively. For example, an automation script that needs
to block ads and bypass Cloudflare:

```toml
[dependencies]
zendriver = { version = "0.1", features = ["interception", "cloudflare"] }
tokio = { version = "1", features = ["full"] }
```

A scraper that needs all of it:

```toml
[dependencies]
zendriver = { version = "0.1", features = [
    "interception",
    "expect",
    "cloudflare",
    "fetcher",
] }
tokio = { version = "1", features = ["full"] }
```

## Re-exports

zendriver re-exports the types you'll typically need from the sub-crates,
so a single `use zendriver::*` will reach `Browser`, `Tab`, `Element`,
`Frame`, `StealthProfile`, `Platform`, `Key`, `KeyModifiers`,
`SpecialKey`, `ClickOptions`, `CookieJar`, and the `Queryable` /
`Evaluable` traits. The sub-crate paths (e.g. `zendriver::stealth::*`,
`zendriver::interception::*`) stay available for the rare cases where you
need a type the prelude doesn't surface.

## MSRV

zendriver targets **Rust 1.75 minimum**. The MSRV bumps follow SemVer —
a Rust version bump counts as a minor change in the 0.x series and a
major change post-1.0. See [SEMVER.md] in the repository for the full
policy.

[SEMVER.md]: https://github.com/TurtIeSocks/zendriver-rs/blob/main/SEMVER.md

## Platform support

"Tested in CI" below means a CI job actually runs the test suite on that
platform — not that a binary merely compiles for it.

| Platform         | Supported | Notes                                                                                                                                        |
|------------------|-----------|----------------------------------------------------------------------------------------------------------------------------------------------|
| Linux (x86_64)   | yes       | Fully tested in CI on `ubuntu-latest`: fmt, clippy, unit, doc and real-Chrome integration tests. Recommended for headless scraping.           |
| Linux (aarch64)  | yes       | Release binary is cross-built in CI; no tests run on this target.                                                                             |
| macOS (x86_64)   | yes       | Release binary is built in CI; no tests run on this target. Tested locally by maintainers.                                                    |
| macOS (Apple Si) | yes       | Release binary is built in CI; no tests run on this target. Tested locally by maintainers.                                                    |
| Windows          | yes       | Real-Chrome integration tests run in CI on `windows-latest`; fmt/clippy/unit/doc jobs are Linux-only. Path semantics differ slightly.         |

Chrome (or Chromium / Edge / any Chromium-derived browser) must be on
`$PATH`, or you must pass an explicit `chrome_path` to the builder, or
you must enable the `fetcher` feature and let zendriver download Chrome
for Testing at startup.

## Verifying the install

A 10-line smoke test you can drop into `src/main.rs`:

```rust,no_run
#[tokio::main]
async fn main() -> zendriver::Result<()> {
    let browser = zendriver::Browser::builder()
        .headless(true)
        .launch()
        .await?;
    let tab = browser.main_tab();
    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;
    let h1 = tab.find().css("h1").one().await?;
    println!("{}", h1.inner_text().await?);
    browser.close().await?;
    Ok(())
}
```

If this prints `Example Domain`, the install is working. See
[Quickstart](./quickstart.md) for a walkthrough of what each line is
doing.
