# zendriver-rs

Async-first, undetectable browser automation via the Chrome DevTools Protocol.

[![crates.io](https://img.shields.io/crates/v/zendriver.svg)](https://crates.io/crates/zendriver)
[![docs.rs](https://docs.rs/zendriver/badge.svg)](https://docs.rs/zendriver)
[![MSRV 1.75](https://img.shields.io/badge/rustc-1.75+-lightgray.svg)](https://blog.rust-lang.org/2023/12/28/Rust-1.75.0.html)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](#license)
[![CI](https://github.com/cdpdriver/zendriver-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/cdpdriver/zendriver-rs/actions/workflows/ci.yml)

A Rust port of [zendriver](https://github.com/cdpdriver/zendriver). Drives Chrome via raw CDP — no WebDriver, no JS shim — with anti-detection patches baked in by default.

## Quick example

```rust,no_run
use zendriver::Browser;

#[tokio::main]
async fn main() -> zendriver::Result<()> {
    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();

    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    // Find by visible text (auto-waits up to the selector's timeout).
    let link = tab.find().text("More information...").one().await?;
    link.click().await?;
    tab.wait_for_load().await?;

    // Read back from the page's main world.
    let title: String = tab.evaluate_main("document.title").await?;
    println!("title: {title}");

    browser.close().await?;
    Ok(())
}
```

More working examples in [`crates/zendriver/examples/`](crates/zendriver/examples/).

## Feature matrix

| Feature        | Default? | Use case                                                      | Extra deps                            |
| -------------- | -------- | ------------------------------------------------------------- | ------------------------------------- |
| `stealth`      | yes      | Anti-detection: spoofed UA/platform, isolated worlds, JS shim | (built-in to `zendriver`)             |
| `interception` | no       | Block/modify requests via CDP `Fetch.*`; rule-based + streams | `zendriver-interception`              |
| `expect`       | no       | Playwright-style `expect_response()` / `expect_request()`     | (in-tree, no extra crate)             |
| `cloudflare`   | no       | Solve Cloudflare Turnstile challenges                         | `zendriver-cloudflare`                |
| `fetcher`      | no       | Auto-download a pinned Chrome for Testing build               | `zendriver-fetcher` + `reqwest`/`zip` |

## Install

Pick the use case that matches what you're building.

**Just browse:**

```bash
cargo add zendriver
```

Default stealth is on.

**Stealth scraping (explicit):**

```bash
cargo add zendriver --features stealth
```

Same as above — only spell out the feature if you want it visible in `Cargo.toml`.

**Everything:**

```bash
cargo add zendriver --features "interception expect cloudflare fetcher"
```

Adds request interception, `expect()` matchers, Cloudflare Turnstile bypass, and the Chrome for Testing fetcher.

## Phases

Six development phases shipped into the v0.1.0 release. The mdBook covers each surface in depth.

1. **Foundation** — CDP transport + minimal `Browser`/`Tab`/`Element`. See [introduction](docs/book/src/introduction.md).
2. **Stealth** — fingerprint patches + isolated worlds + stealth JS bundle. See [stealth](docs/book/src/stealth.md).
3. **Element API completeness** — CSS/XPath/text/role selectors, actionability, input controller, screenshots. See [quickstart](docs/book/src/quickstart.md).
4. **`Tab`/`Browser` completeness** — multi-tab, cookies, storage, frames, nav history, `wait_for_idle`. See [multi-tab](docs/book/src/multi-tab.md) + [frames](docs/book/src/frames.md).
5. **Optional gated features** — request interception, `expect()` matchers, Cloudflare bypass, Chrome-for-Testing fetcher. See [interception](docs/book/src/interception.md), [expect](docs/book/src/expect.md), [cloudflare](docs/book/src/cloudflare.md), [fetcher](docs/book/src/fetcher.md).
6. **Polish + release** — trait extraction, rustdoc + mdBook, publish to crates.io.

## Comparison

| Feature                  | zendriver-rs            | chromiumoxide     | fantoccini      | headless_chrome | thirtyfour      |
| ------------------------ | ----------------------- | ----------------- | --------------- | --------------- | --------------- |
| API ergonomics _opinion_ | builder + auto-wait     | raw CDP types     | WebDriver verbs | sync wrappers   | WebDriver verbs |
| Stealth out-of-box       | yes (default)           | no                | no              | no              | no              |
| Multi-tab                | yes (first-class)       | yes               | yes             | yes             | yes             |
| Interception             | yes (`Fetch.*` wrapper) | yes (raw)         | no (proxy-only) | partial         | no (proxy-only) |
| License                  | MIT OR Apache-2.0       | MIT OR Apache-2.0 | Apache-2.0      | MIT             | MIT             |
| Async runtime            | tokio                   | tokio / async-std | tokio           | sync            | tokio           |

Subjective rows marked `*opinion`. All claims accurate as of the 0.1.0 release; check upstream changelogs before relying on them.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Issues and PRs welcome.

## License

Dual-licensed under MIT ([LICENSE-MIT](LICENSE-MIT)) and Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)) at your option.
