# Changelog

All notable changes to this project documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SEMVER.md].

## [Unreleased]

### Added

- `zendriver-mcp` — Model Context Protocol server crate exposing
  zendriver-rs through 49 MCP tools over stdio + streamable HTTP. See
  the [MCP chapter](https://turtiesocks.github.io/zendriver-rs/mcp.html).

## [0.1.0] - 2026-05-23

First public release. Built across 6 phases of internal development.

### Added
- CDP transport actor + minimal Browser/Tab/Element surface (Phase 1)
- Anti-detection (StealthProfile::native + spoofed; chaser-oxide-derived
  protocol patches; nightly stealth tests) (Phase 2)
- Full Element + FindBuilder surface (xpath/text/role selectors, hover/
  focus/scroll, type_text with realistic Bezier mouse + per-key typing,
  file upload, attributes, traversal, true isolated-world evaluate,
  actionability checks, auto-refresh on stale handles, screenshots)
  (Phase 3)
- Multi-tab management + Frame as first-class type (OOPIF supported)
  + CookieJar (CRUD + JSON persistence) + per-Tab Storage + nav history
  + wait_for_idle + traversal-origin refresh chain + per-Tab Input
  (Phase 4)
- Optional gated features: zendriver-interception (Fetch.* CDP wrapper
  with rule-based + Stream API), in-tree expect() module (Playwright-
  style pre-register + await), zendriver-cloudflare (Turnstile bypass),
  zendriver-fetcher (custom Chrome download via Chrome for Testing JSON
  API) (Phase 5)
- Trait extraction (Queryable + Evaluable across Tab/Frame/Element);
  rustdoc + doctest coverage; mdBook docs site; CHANGELOG + SEMVER;
  topological publish script (Phase 6)

### Internal phases (pre-release)
Phases 1-5 were internal development with API churn allowed.
No upgrade path needed; v0.1.0 is the first published release.
