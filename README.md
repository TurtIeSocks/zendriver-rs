# zendriver-rs

Async-first, undetectable browser automation via the Chrome DevTools Protocol — drive real Chrome from Rust, or hand the keys to an LLM agent over the [Model Context Protocol](https://modelcontextprotocol.io/).

[![crates.io](https://img.shields.io/crates/v/zendriver.svg)](https://crates.io/crates/zendriver)
[![docs.rs](https://docs.rs/zendriver/badge.svg)](https://docs.rs/zendriver)
[![Book](https://img.shields.io/badge/book-mdBook-blue)](https://turtiesocks.github.io/zendriver-rs/)
[![MCP](https://img.shields.io/badge/MCP-server-blue)](https://turtiesocks.github.io/zendriver-rs/mcp.html)
[![MSRV 1.85](https://img.shields.io/badge/rustc-1.85+-lightgray.svg)](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](#license)
[![CI](https://github.com/TurtIeSocks/zendriver-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/TurtIeSocks/zendriver-rs/actions/workflows/ci.yml)

A Rust port of [zendriver](https://github.com/cdpdriver/zendriver). Drives Chrome via raw CDP — no WebDriver, no JS shim — with anti-detection patches baked in by default.

📖 **[User guide & full documentation →](https://turtiesocks.github.io/zendriver-rs/)** · 🦀 **[API reference (docs.rs) →](https://docs.rs/zendriver)** · 🤖 **[MCP server for AI agents →](https://turtiesocks.github.io/zendriver-rs/mcp.html)**

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
| `imperva`      | no       | Imperva WAF / Incapsula bypass (reese84 / legacy / CAPTCHA)   | `zendriver-imperva`                   |
| `datadome`     | no       | DataDome bypass (device-check / CAPTCHA / block) + `Surface::Webgpu` coherence | `zendriver-datadome` |
| `monitor`      | no       | Passive network monitor — buffered HTTP / WebSocket / SSE events via `tab.monitor()` | (in-tree, no extra crate)             |
| `geo`          | no       | Country code → coherent `locale` + `languages` (Accept-Language) persona overlay | (via `zendriver-stealth`)             |
| `tracker-blocking` | no   | Opt-in third-party tracker / fingerprinter host blocklist (curated bundled list + BYO file / URL) | `reqwest` + `dirs`         |
| `fetcher`      | no       | Auto-download a pinned Chrome for Testing build               | `zendriver-fetcher` + `reqwest`/`zip` |

Separate binary crate (not a feature on `zendriver`):

| Crate           | Use case                                                                                                                         | Install                                                                                     |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| `zendriver-mcp` | Drive a stealth Chrome from any LLM agent — [Model Context Protocol](https://modelcontextprotocol.io/) server, 70 tools, stdio + streamable HTTP | `cargo install zendriver-mcp` ([docs](https://turtiesocks.github.io/zendriver-rs/mcp.html)) |

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
cargo add zendriver --features "interception expect cloudflare imperva datadome monitor geo tracker-blocking fetcher"
```

Adds request interception, `expect()` matchers, Cloudflare Turnstile bypass, Imperva WAF / Incapsula bypass, DataDome bypass, the passive network monitor, country→locale geo overlay, the opt-in tracker/fingerprinter blocklist, and the Chrome for Testing fetcher.

## Drive a stealth browser from your AI agent

`zendriver-mcp` is a **first-class [Model Context Protocol](https://modelcontextprotocol.io/) server** that hands the entire zendriver-rs surface to any LLM client — Claude Desktop, Claude Code, Cursor, or your own agent loop. **70 tools**, two transports (stdio + streamable HTTP), and the same stealth-by-default fingerprinting baked into the lib. Unlike generic browser MCP servers, this one bypasses Cloudflare Turnstile, ships an isolated-world JS eval that survives anti-bot detection, and lets agents persist auth state across sessions.

```bash
cargo install zendriver-mcp
```

**Claude Desktop / Claude Code config:**

```json
{
  "mcpServers": {
    "zendriver": {
      "command": "zendriver-mcp"
    }
  }
}
```

**Even easier — the [Claude Code plugin](plugins/zendriver/):**

```bash
claude plugin marketplace add TurtIeSocks/zendriver-rs
claude plugin install zendriver@zendriver-rs
# then, in a session:
/zendriver:setup     # prebuilt (no Rust), source, or link
```

No manual MCP config — the plugin bundles the server plus scraping skills, the `/zendriver:scrape` and `/zendriver:extract` commands, and a `zendriver-scraper` subagent.

**What agents get out of the box:**

- **Stealth navigation** — `browser_open`, `browser_goto`, `browser_back/forward/reload`, `browser_wait_for_idle`, plus a runtime-swappable `browser_set_stealth_profile` (auto / native / spoof_macos / spoof_linux / spoof_windows)
- **Selector-based find + actions** — one `Selector` arg works across `browser_find`, `browser_click`, `browser_type`, `browser_press`, `browser_set_value`, `browser_upload`, etc., with CSS / XPath / visible-text / ARIA-role lookups and per-frame scoping
- **Three ways to "see" the page** — `browser_html` (trimmed DOM), `browser_screenshot` (PNG / JPEG / WebP as inline image content), `browser_element_state` (visibility / geometry / attrs)
- **Stateful primitives** agents need for real work — `browser_cookies_persist` for save/load auth, full `browser_storage_*`, multi-tab management, frame traversal; `browser_monitor_*` passive network monitor (HTTP / WS / SSE); `browser_request` for HTTP from the page's own session
- **Anti-bot superpowers** (gated cargo features, on by default for the published binary):
  - `browser_solve_turnstile` — Cloudflare Turnstile bypass without a CAPTCHA-solving service
  - `browser_open` `block_trackers` / `tracker_blocklist` — opt-in third-party tracker & fingerprinter host blocking (curated bundled list + bring-your-own file / URL)
  - `browser_solve_datadome` — DataDome device-check / CAPTCHA / block bypass (with `Surface::Webgpu` coherence)
  - `browser_intercept_*` — block/redirect/respond/modify requests via CDP `Fetch.*`
  - `browser_expect_register / _await` — Playwright-style "wait for response/dialog/download" matchers, split across MCP calls so the agent can act in between
  - `browser_install_chrome` — pull a pinned Chrome-for-Testing build on demand
- **Actionable errors** — every error carries an `_meta.suggested_next` hint pointing the agent at the right recovery tool (e.g. `ElementNotFound` → "try `browser_html` to inspect")

See the [MCP chapter](https://turtiesocks.github.io/zendriver-rs/mcp.html) for the full tool reference, CLI flags, HTTP-mode operator notes, and troubleshooting guide.

## Phases

Six development phases shipped into the v0.1.0 release. The [mdBook](https://turtiesocks.github.io/zendriver-rs/) covers each surface in depth.

1. **Foundation** — CDP transport + minimal `Browser`/`Tab`/`Element`. See [introduction](https://turtiesocks.github.io/zendriver-rs/introduction.html).
2. **Stealth** — fingerprint patches + isolated worlds + stealth JS bundle. See [stealth](https://turtiesocks.github.io/zendriver-rs/stealth.html).
3. **Element API completeness** — CSS/XPath/text/role selectors, actionability, input controller, screenshots. See [quickstart](https://turtiesocks.github.io/zendriver-rs/quickstart.html).
4. **`Tab`/`Browser` completeness** — multi-tab, cookies, storage, frames, nav history, `wait_for_idle`. See [multi-tab](https://turtiesocks.github.io/zendriver-rs/multi-tab.html) + [frames](https://turtiesocks.github.io/zendriver-rs/frames.html).
5. **Optional gated features** — request interception, `expect()` matchers, Cloudflare / Imperva / DataDome bypass, network monitor, geo locale overlay, tracker blocklist, Chrome-for-Testing fetcher. See [interception](https://turtiesocks.github.io/zendriver-rs/interception.html), [expect](https://turtiesocks.github.io/zendriver-rs/expect.html), [cloudflare](https://turtiesocks.github.io/zendriver-rs/cloudflare.html), [fetcher](https://turtiesocks.github.io/zendriver-rs/fetcher.html).
6. **Polish + release** — trait extraction, rustdoc + mdBook, publish to crates.io.

## Comparison

| Feature                  | zendriver-rs            | chromiumoxide     | fantoccini      | headless_chrome | thirtyfour      |
| ------------------------ | ----------------------- | ----------------- | --------------- | --------------- | --------------- |
| API ergonomics _opinion_ | builder + auto-wait     | raw CDP types     | WebDriver verbs | sync wrappers   | WebDriver verbs |
| Stealth out-of-box       | yes (default)           | no                | no              | no              | no              |
| Multi-tab                | yes (first-class)       | yes               | yes             | yes             | yes             |
| Interception             | yes (`Fetch.*` wrapper) | yes (raw)         | no (proxy-only) | partial         | no (proxy-only) |
| Cloudflare bypass        | yes (`zendriver-cloudflare`) | no           | no              | no              | no              |
| MCP server for AI agents | yes (`zendriver-mcp`, 70 tools) | no        | no              | no              | no              |
| License                  | MIT OR Apache-2.0       | MIT OR Apache-2.0 | Apache-2.0      | MIT             | MIT             |
| Async runtime            | tokio                   | tokio / async-std | tokio           | sync            | tokio           |

Subjective rows marked `*opinion`. All claims accurate as of the 0.1.0 release; check upstream changelogs before relying on them.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Issues and PRs welcome.

## License

Dual-licensed under MIT ([LICENSE-MIT](LICENSE-MIT)) and Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)) at your option.
