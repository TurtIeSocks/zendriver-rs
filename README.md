# zendriver-rs

A Rust port of [zendriver](https://github.com/cdpdriver/zendriver) — an undetectable, async-first browser automation library using the Chrome DevTools Protocol directly.

**Status:** Pre-alpha. Phase 1 of a six-phase port is in progress; not yet published.

## Phases

1. **Foundation** (in progress): transport + minimal `Browser`/`Tab`/`Element`.
2. Stealth (planned).
3. Element API completeness (planned).
4. `Tab`/`Browser` completeness, cookies, screenshots, multi-tab, iframes (planned).
5. Optional gated features: interception, Cloudflare bypass, `expect()`, fetcher (planned).
6. Polish + 0.1 release (planned).

See `docs/superpowers/specs/` for the per-phase design documents.

## License

Dual-licensed under MIT (`LICENSE-MIT`) and Apache-2.0 (`LICENSE-APACHE`) at your option.
