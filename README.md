# zendriver-rs

A Rust port of [zendriver](https://github.com/cdpdriver/zendriver) — an undetectable, async-first browser automation library using the Chrome DevTools Protocol directly.

**Status:** Phases 1-5 shipped. Not yet published to crates.io.

## Example

```rust
use zendriver::{Browser, Cookie, SameSite};

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

    // Evaluate in the page's main world.
    let title: String = tab.evaluate_main("document.title").await?;
    println!("title: {title}");

    // Open a second tab in parallel — each tab is its own CDP session.
    let tab2 = browser.new_tab_at("https://example.org").await?;
    tab2.wait_for_load().await?;
    println!("now driving {} tabs", browser.tab_count().await);

    // Cookies live at browser scope (shared across all tabs).
    browser
        .cookies()
        .set(Cookie {
            name: "session".into(),
            value: "abc123".into(),
            domain: "example.org".into(),
            path: "/".into(),
            expires: None,
            http_only: true,
            secure: false,
            same_site: Some(SameSite::Lax),
            url: None,
        })
        .await?;

    browser.close().await?;
    Ok(())
}
```

## Phases

1. **Foundation** **DONE**: transport + minimal `Browser`/`Tab`/`Element`.
2. **Stealth** **DONE**: fingerprint patches + isolated worlds + stealth JS bundle.
3. **Element API completeness** **DONE**: selectors (CSS/XPath/text/role), actionability, input controller, screenshots.
4. **`Tab`/`Browser` completeness** **DONE**: multi-tab + cookies + storage + frames + nav history + `wait_for_idle`.
5. **Optional gated features** **DONE**: request interception, `expect()` matchers, Cloudflare bypass, and Chrome-for-Testing fetcher — all behind opt-in feature flags (`interception`, `expect`, `cloudflare`, `fetcher`).
6. Polish + 0.1 release (planned).

### Gated feature example

```rust,ignore
// Cargo.toml: zendriver = { version = "...", features = ["interception", "expect"] }
use std::time::Duration;
use zendriver::Browser;

#[tokio::main]
async fn main() -> zendriver::Result<()> {
    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();

    // Block tracking requests before they hit the wire.
    let _block = tab
        .intercept()
        .block("*/analytics/*")?
        .start();

    // Arm an expectation, then trigger navigation — the matcher is live before goto.
    let api = tab
        .expect_response("*/api/data*")
        .timeout(Duration::from_secs(5));
    tab.goto("https://example.com").await?;
    let matched = api.await?;
    println!("api status: {}", matched.status);

    browser.close().await?;
    Ok(())
}
```

See `docs/superpowers/specs/` for per-phase design documents.

## Development

```bash
cargo test --workspace --lib                                       # unit tests, no Chrome
cargo test --workspace --doc                                       # doctests
cargo clippy --workspace --all-targets --locked -- -D warnings    # lint
cargo fmt --all --check                                            # format
cargo test --workspace --features integration-tests --test '*' -- --test-threads=1  # real Chrome (requires Chrome on $PATH)
```

## License

Dual-licensed under MIT ([LICENSE-MIT](LICENSE-MIT)) and Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)) at your option.
