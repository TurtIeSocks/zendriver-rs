---
name: bypass
description: Use when a page is blocked by an anti-bot wall — Cloudflare Turnstile, Imperva/Incapsula, or DataDome — to detect which wall it is and solve it so the underlying content loads.
---

# Bypassing anti-bot walls with zendriver

Invoked from the `scraping` skill when an interstitial replaces real content. Identify the
wall, call the matching solver, verify, then resume extraction.

## Detect which wall

Look at the current DOM / title / response:

| Signal | Wall | Solver |
|--------|------|--------|
| "Just a moment…", `cf-chl`, `__cf` cookies, Turnstile iframe, Cloudflare ray ID | Cloudflare Turnstile | `browser_solve_turnstile` |
| "Incapsula", `_Incapsula_Resource`, `visid_incap_*` cookies, Imperva notice | Imperva / Incapsula | `browser_solve_imperva` |
| `datadome` cookie, "DataDome", `geo.captcha-delivery.com` | DataDome | `browser_solve_datadome` |

If you can't tell, re-`browser_html { trim: true }` and inspect; don't guess-fire a solver.

## Solve flow

1. Confirm the wall (table above).
2. Call the matching `browser_solve_*` tool.
3. Re-check: `browser_html { trim: true }` (or a `browser_find` for known content) — did the
   real page load?
4. If solved → return to the `scraping` skill and extract.

## When a solve doesn't stick

These walls are an active arms race; solves are **best-effort**, not guaranteed:

- Retry once (transient challenges happen).
- Try a fresh navigation (`browser_reload` or re-`browser_goto`).
- If a wall keeps returning, **surface it to the user** — say plainly that the site is
  serving a challenge the automated solver can't clear, and let them decide (manual solve,
  different approach, or stop). Do not loop indefinitely, and do not pretend a page loaded
  when it didn't.

## Responsible use

Solving a wall is for reaching content you're **authorized** to access. Don't use these tools
for mass-targeting, credential-stuffing, or defeating access controls you have no right to
bypass.
