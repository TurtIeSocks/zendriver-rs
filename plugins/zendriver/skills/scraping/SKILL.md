---
name: scraping
description: Use when scraping, extracting, or reading content or data from a website with zendriver — especially JS-rendered SPAs, bot-walled pages, or anything plain fetch/WebFetch can't reach. Covers the open→goto→wait→extract flow, surgical extraction, tool selection, and gotchas.
---

# Scraping with zendriver

The canonical playbook for driving the zendriver browser well. The `/zendriver:scrape`
command and the `zendriver-scraper` subagent both follow this skill — it is the single
source of truth.

## When zendriver is the right tool

Reach for zendriver when the page is **JS-rendered**, **bot-walled**, or needs
**interaction/session state**. For static HTML a plain fetch is lighter — don't spin up a
browser to read a sitemap.

## Core flow

1. `browser_status` — is a browser already open this session? Reuse it; don't re-open.
2. `browser_open` — launch once. Stealth is **on by default**; do NOT re-enable it or set a
   profile unless a site specifically needs it.
3. `browser_goto { url, wait_for: "load" }` — navigate.
4. **Extract** (see below).
5. Close with `browser_close` when the job is done (the server also kills its owned Chrome on
   exit).

## The effectiveness lever: surgical extraction, not DOM-dumping

`browser_html { trim: true }` returns the whole page — ~40–50k tokens for a real article.
**Only dump full DOM when you genuinely need the whole page.** When the goal is specific,
pull just that:

- A few known elements / text → `browser_find` (one) or `browser_find_all` (many).
- All links → `browser_get_links`.
- Structured data, tables, computed values, JSON embedded in `<script>` → `browser_evaluate`
  with a small JS snippet that returns exactly the shape you want.
- "What resources/endpoints did this page load?" → `browser_search_resources`.

Default to targeted tools; reserve `browser_html { trim: true }` for genuinely unstructured
full-page reads.

## Gotchas (hard-won)

- **`wait_for: "load"`, not `"idle"`** on ad/tracker-heavy sites — idle's ~5s cap times out
  waiting on hanging tracker requests. Use `load` and read the DOM regardless; escalate to
  `browser_wait_for_idle` only for SPAs that genuinely settle.
- **Always `trim: true`** on `browser_html` unless you need raw markup.
- **Check `browser_status`** before assuming a browser/tab is open — calling navigation on a
  closed browser errors.
- A page that loaded but shows little content is often **JS-gated** — give it a
  `browser_wait_for_load` / short `browser_wait_for_idle`, or check for a wall (next section).

## Wall detection → hand off

If the DOM shows an anti-bot interstitial instead of content — "Just a moment…", a Cloudflare
ray ID, an Imperva/Incapsula notice, or a DataDome block — **invoke the `bypass` skill**
(via the Skill tool) and follow it, then resume extraction.

## Multi-page / pagination

Loop the core flow per URL. Prefer `browser_get_links` to discover "next" targets over
guessing URL patterns. For big crawls, run inside the `zendriver-scraper` subagent (and
background it) so the main thread's context stays clean.

## Responsible use

zendriver is for **authorized access**: your own sites, content you're permitted to read,
authorized testing, and research. Respect `robots`-style intent and rate limits; throttle
between requests. Don't use it for mass-targeting or to defeat access controls you have no
right to bypass.
