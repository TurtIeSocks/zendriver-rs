# zendriver-rs

A Rust port of [zendriver](https://github.com/cdpdriver/zendriver) — an undetectable, async-first browser automation library using the Chrome DevTools Protocol directly.

**Status:** Phase 1 (foundation) under active development. Not yet published to crates.io.

## Example

```rust
use zendriver::Browser;

#[tokio::main]
async fn main() -> zendriver::Result<()> {
    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();
    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    let h1 = tab.find().css("h1").one().await?;
    println!("h1: {}", h1.inner_text().await?);

    browser.close().await?;
    Ok(())
}
```

## Phases

1. **Foundation** (in progress): transport + minimal `Browser`/`Tab`/`Element`.
2. Stealth (planned).
3. Element API completeness (planned).
4. `Tab`/`Browser` completeness, cookies, screenshots, multi-tab, iframes (planned).
5. Optional gated features: interception, Cloudflare bypass, `expect()`, fetcher (planned).
6. Polish + 0.1 release (planned).

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
