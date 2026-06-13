# Claude Code plugin

The `zendriver` Claude Code plugin bundles the [MCP server](./mcp.md) with skills, commands,
and a subagent so Claude can drive zendriver effectively out of the box.

## Install

```bash
claude plugin marketplace add TurtIeSocks/zendriver-rs
claude plugin install zendriver@zendriver-rs
/zendriver:setup
```

`/zendriver:setup` provisions the `zendriver-mcp` binary — choose:

- **prebuilt** — download the matching binary from the latest `zendriver-mcp-v*` GitHub
  release (checksum-verified). No Rust toolchain.
- **source** — `cargo install zendriver-mcp` (compile the public source yourself).
- **link** — reuse a `zendriver-mcp` already on your PATH.

Restart the session afterward. Chrome is fetched automatically on first use.

## What's included

| Kind | Items |
|------|-------|
| MCP server | the full zendriver browser tool surface |
| Skills | `scraping` (canonical playbook), `bypass` (anti-bot walls), `advanced` (interception/monitor/capture/sessions) |
| Commands | `/zendriver:setup`, `/zendriver:scrape <url> [goal]`, `/zendriver:extract <url> <schema>` |
| Subagent | `zendriver-scraper` |

## How it fits together

The `scraping` skill is the single source of truth for "how to scrape well"; the commands and
the subagent both follow it. The subagent runs jobs in its own context so big extractions
don't fill the main thread.

## Responsible use

Authorized access only — your own sites, permitted content, authorized testing, research.
Respect rate limits and site terms.
