# zendriver-mcp

MCP server exposing [zendriver-rs](https://crates.io/crates/zendriver) — a stealth-by-default browser automation library — to any Model Context Protocol client.

See the [mdBook chapter](https://turtiesocks.github.io/zendriver-rs/mcp.html) for the full tool reference and Claude Desktop config snippet.

## As a Claude Code plugin

Claude Code users can skip manual MCP wiring and install the bundled plugin:

```bash
claude plugin marketplace add TurtIeSocks/zendriver-rs
claude plugin install zendriver@zendriver-rs
# then, in a session:
/zendriver:setup     # prebuilt (no Rust), source, or link
```

It ships this server plus scraping skills, the `/zendriver:scrape` and
`/zendriver:extract` commands, and a `zendriver-scraper` subagent. See
[`plugins/zendriver/`](../../plugins/zendriver/) and the
[plugin chapter](https://turtiesocks.github.io/zendriver-rs/plugin.html).
