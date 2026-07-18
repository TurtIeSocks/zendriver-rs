# Introduction

**zendriver-rs** is an async-first browser-automation library for Rust
with a coherent stealth identity and explicit anti-detection controls,
on by default. It drives a real Chrome instance over the [Chrome DevTools
Protocol] (CDP) directly — no WebDriver shim, no Selenium grid, no JSON
wire — and ships with anti-detection patches that pass mainstream
fingerprint checks (e.g. [sannysoft], [areyouheadless]) out of the box.
No automation stack can guarantee invisibility to a determined,
adaptively-defended site — see [Stealth](./stealth.md) for what the
patches actually cover and their limits.

It is a Rust port of the Python [zendriver] / [nodriver] projects, with
the API redesigned around Rust's type system: builder patterns where Python
uses kwargs, traits where Python uses duck-typed protocols, `Result` where
Python uses exceptions, and explicit lifetimes for query scopes that the
borrow checker tracks instead of letting them drift across `await` points.

[Chrome DevTools Protocol]: https://chromedevtools.github.io/devtools-protocol/
[zendriver]: https://github.com/cdpdriver/zendriver
[nodriver]: https://github.com/ultrafunkamsterdam/nodriver

## Use cases

- **Scraping sites that block headless browsers.** The `spoofed` stealth
  profile patches `navigator.webdriver`, the Chrome runtime object, the
  permissions API, and a half-dozen other tells. Many Cloudflare
  Turnstile, PerimeterX, and DataDome challenges pass without manual
  headers; sites with more aggressive detection layers may still need
  the dedicated bypass features (see [Cloudflare](./cloudflare.md) /
  [DataDome](./datadome.md)) or still catch a scripted session.
- **End-to-end testing of real-world web apps.** First-class multi-tab,
  cross-origin iframe (OOPIF) support, network interception, and a
  Playwright-style `expect()` pre-register surface make whole-flow tests
  expressive without the wire-protocol churn of WebDriver.
- **Browser automation pipelines under load.** Tab handles are `Send + Sync
  + Clone`, the transport is a single Tokio actor, and queries are
  zero-copy `&str` selectors — comfortable inside any `tokio::spawn`'d
  worker pool.
- **Drop-in replacement for `chromiumoxide` callers** who want stealth,
  multi-tab, and an ergonomic `find().css("...").one()` query surface
  instead of hand-rolling `Page.querySelector` calls.

## What makes it different

- **CDP-direct.** Every method maps to one or two CDP commands. There is
  no WebDriver-style adapter layer, so latency is one network round-trip
  per call — typically under 1 ms on localhost.
- **Anti-detection controls on by default.** `StealthProfile::native` is
  the suggested starting point — UA scrub plus Emulation overrides, no JS
  bootstrap, no prototype patching. `StealthProfile::spoofed` adds
  Navigator-prototype patches that pass [sannysoft] + [areyouheadless].
  Neither is a guarantee against a determined, adaptive detector — see
  [Stealth](./stealth.md#when-to-use-which) for the tradeoffs. Use
  `StealthProfile::off` when you want a vanilla browser for reproduction.
- **Async-first.** Built on Tokio. Every call returns a `Future`. No
  blocking, no `block_on`, no `tokio::task::spawn_blocking`. Browser /
  Tab / Element handles are `Clone + Send + Sync` so they cross
  `.await` boundaries and task spawns without ceremony.
- **Rust-native.** Errors are typed via `thiserror` and surfaced through
  `Result`. Selectors are checked at call time, not parsed at startup.
  Resources clean up via `Drop` (Chrome subprocess gets `SIGTERM` when the
  last `Browser` clone drops). The borrow checker tracks query scopes for
  you.

[sannysoft]: https://bot.sannysoft.com/
[areyouheadless]: https://arh.antoinevastel.com/bots/areyouheadless

## Comparison

| feature                  | zendriver-rs | chromiumoxide | thirtyfour | fantoccini |
|--------------------------|--------------|---------------|------------|------------|
| Transport                | CDP-direct   | CDP-direct    | WebDriver  | WebDriver  |
| Stealth out of the box   | yes          | no            | no         | no         |
| Builder-style queries    | yes          | partial       | no         | no         |
| Cross-origin iframes     | yes          | partial       | yes        | yes        |
| Send+Sync handles        | yes          | yes           | yes        | yes        |
| Async runtime            | Tokio        | async-std/Tokio | Tokio    | Tokio      |
| Network interception     | yes          | yes           | limited    | limited    |
| Multi-tab orchestration  | yes          | manual        | manual     | manual     |

Comparisons against Playwright + Selenium are covered in
[Migration from Playwright](./migration-playwright.md).

## How this book is organized

- **Setup chapters** — [Install](./install.md) and
  [Quickstart](./quickstart.md) cover the minimum needed to write your
  first script.
- **Core API chapters** — [Stealth](./stealth.md),
  [Multi-tab](./multi-tab.md), [Frames](./frames.md), and
  [Input](./input.md) cover the always-on Browser/Tab/Element surface.
- **Optional-feature chapters** — [Interception](./interception.md),
  [Expect()](./expect.md), [Cloudflare](./cloudflare.md), and
  [Fetcher](./fetcher.md) cover the gated Cargo features.
- **Reference chapters** — [Architecture](./architecture.md),
  [Migration from Playwright](./migration-playwright.md),
  [FAQ](./faq.md), and [Error Reference](./error-reference.md) round out
  the long tail.

The API rustdoc on [docs.rs/zendriver] is the source of truth for the
public surface. The book covers the *how* and *why*; rustdoc covers the
*what*.

[docs.rs/zendriver]: https://docs.rs/zendriver
