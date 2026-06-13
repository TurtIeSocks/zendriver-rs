# zendriver — Claude Code plugin

Stealth web scraping & extraction for Claude, powered by
[zendriver-rs](https://github.com/TurtIeSocks/zendriver-rs). Bundles the `zendriver-mcp`
Chrome-DevTools-Protocol browser server plus skills, commands, and a subagent that teach
Claude how to drive it well.

## Install

```bash
claude plugin marketplace add TurtIeSocks/zendriver-rs
claude plugin install zendriver@zendriver-rs
```

Then, in a session, provision the browser binary once:

```
/zendriver:setup
```

Pick **prebuilt** (fast, no Rust), **source** (compile it yourself), or **link** (reuse a
`zendriver-mcp` already on your PATH), then restart the session. Chrome is downloaded
automatically on first use — no system Chrome required.

## What you get

- **MCP server** — 70 stealth browser tools (`browser_goto`, `browser_html`, `browser_find`,
  `browser_solve_turnstile`, …).
- **Skills** — `scraping` (the playbook, auto-triggers), `bypass` (anti-bot walls), `advanced`
  (interception, monitoring, capture, sessions).
- **Commands** — `/zendriver:setup`, `/zendriver:scrape <url> [goal]`,
  `/zendriver:extract <url> <schema>`.
- **Subagent** — `zendriver-scraper` runs scrape/extract jobs in its own context.

## Responsible use

For authorized access only — your own sites, permitted content, authorized testing, research.
Respect rate limits and site terms.
