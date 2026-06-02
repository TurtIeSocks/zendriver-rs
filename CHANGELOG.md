# Changelog

All notable changes to this project documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SEMVER.md].

## [Unreleased]

### Added

- `tab.monitor()` — persistent network monitor: a `Stream<NetworkEvent>` over
  HTTP exchanges (lazy body), WebSocket frames, and EventSource messages,
  behind the `monitor` feature (#223).
- `tab.request()` — browser-context HTTP (`get`/`post`/…) inheriting cookies +
  CORS via in-page fetch, with opt-in `bypass_cors()` (#189).
- **Parity with nodriver / zendriver-py (phases P-A…P-D).** A large additive
  surface closing the feature gap with upstream while preserving every rs-only
  strength (3-tier stealth, single-socket flat transport, actionability,
  Imperva, MCP). Design + plans in
  `docs/superpowers/{specs,plans}/2026-06-01-parity-phase-*.md`.
  - **Input / query (P-A):** full keyboard parity — non-ASCII/emoji/CJK via the
    `char` event path, Shift synthesis for uppercase/symbols, real
    modifier-wrapper key events for chords (`press_with`), and
    `Element::type_keys(KeySequence)` for mixed input. Cross-frame
    `Tab::find().include_frames()` + nodriver-style `.best_match()`
    (closest-text-length). React-controlled-input fix in `Element::clear` /
    `set_value` (native prototype setter) + new `Element::clear_by_deleting`.
  - **Tab / page (P-B):** `content()`, PDF export (`pdf_builder` / `print_to_pdf`),
    MHTML (`snapshot_mhtml` / `save_snapshot`), `scroll_down` / `scroll_up` /
    `scroll_with`, runtime `set_user_agent`, coordinate `mouse_move` /
    `mouse_click` / `mouse_click_with` / `flash_point`, `reload_with`,
    window-state (`window_bounds` / `set_window_bounds` / `set_window_size` /
    `maximize` / `minimize` / `fullscreen`), `bring_to_front`,
    `bypass_insecure_connection_warning`, `inspector_url`, `Browser::new_window`.
  - **Launch / config (P-C):** `Browser::connect` / `BrowserBuilder::connect` to
    an already-running Chrome (ws:// or http:// endpoint; never kills the
    attached process), `expert` mode + opt-in `force_open_shadow_roots`,
    extension loading (`add_extension` / `extensions`, `.crx` auto-unzip),
    `lang` / `user_agent` / `sandbox` / `channel` (Brave/Edge) builders, and
    `grant_permissions` / `grant_all_permissions` / `reset_permissions`.
  - **Network / cookies (P-D):** `Fetch.continueResponse` via
    `PausedRequest::continue_response` + `Rule::ModifyResponse`; CHIPS + priority
    cookie fields (`partition_key` / `priority` / `same_party` /
    `source_scheme` / `source_port`); runtime `Tab`/`Browser::set_download_path`;
    filtered cookie persistence (`save_to_file_matching` /
    `load_from_file_matching`); reconnection v1 — typed
    `ZendriverError::Disconnected` + manual `Browser::reconnect`.
  - **Convenience sweep (P-E) — literal method-for-method parity:**
    `Tab::js_dumps`, `get_all_urls` / `get_all_linked_sources`,
    `wait_for_ready_state`, `download_file`, `mouse_drag`,
    `search_frame_resources`; `Element::set_text`, `flash` /
    `highlight_overlay`, `mouse_drag`, and `bounding_box_page` (absolute
    page coordinates). With this, zendriver-rs covers the full
    nodriver / zendriver-py user-facing surface (remaining upstream items
    are deliberate non-goals — see below).
- `zendriver-mcp` — Model Context Protocol server crate exposing
  zendriver-rs through 49 MCP tools over stdio + streamable HTTP. See
  the [MCP chapter](https://turtiesocks.github.io/zendriver-rs/mcp.html).
- `zendriver-interception` — `test-support` cargo feature exposing a
  hidden `InterceptHandle::for_tests()` constructor for downstream
  crates' unit tests. Not intended for production use; default off.
- `zendriver-imperva` crate: passive bypass for Imperva WAF / Incapsula
  (reese84, legacy Incapsula, CAPTCHA surfaces). Opt-in Fetch-domain
  fast-path and opt-in CAPTCHA solver callback. `Tab::imperva()`
  convenience method gated by the `imperva` cargo feature.

### Changed

- **Parity (P-A…P-D) behavior/API changes:** `zendriver::Channel` now names the
  browser-brand enum (Chrome/Chromium/Brave/Edge/Auto); the fetcher
  release-channel enum is re-exported as `FetcherChannel`. `Element::press_with`
  now emits real modifier keyDown/keyUp wrapper events (was modifier-bits only).
  `Element::clear` / `set_value` now assign through the native prototype value
  setter (defeats React `_valueTracker`) and `clear` also fires `change`. The
  WebSocket message/frame cap was raised to 256 MiB (was tungstenite's default).
  Unexpected socket loss now surfaces `ZendriverError::Disconnected` (distinct
  from a clean shutdown).
- `zendriver-cloudflare::CloudflareBypass::wait_for_clearance` now drives a
  unified poll loop instead of an upfront iframe-detect + click + poll.
  Each tick fetches token, iframe bbox (shadow-DOM aware), and a
  challenge-marker flag in a single CDP round-trip. The click flow still
  fires when the interactive iframe mounts, but the **invisible
  Turnstile** path — where Cloudflare's loader populates
  `cf-turnstile-response` directly with no iframe — now resolves to
  `TokenAcquired` instead of `Err(NoChallenge)`. `NoChallenge` is now
  reserved for the case where the entire timeout window elapsed without
  any challenge marker on the page. Public API unchanged; behavior
  strictly more permissive.
- Nightly Cloudflare integration test (`cloudflare_phase5.rs`) switched
  from `nopecha.com/demo/cloudflare` to a wiremock-served local HTML page
  with Cloudflare's invisible test sitekey
  `1x00000000000000000000AA`. The nopecha demo now serves a non-iframe
  JS-Challenge interstitial that no headless Chrome — stealth or
  otherwise — can clear via the click flow, so the previous test no
  longer exercised meaningful code paths. The local page still loads
  `challenges.cloudflare.com/turnstile/v0/api.js`, so the end-to-end
  token-detection / poll-loop is still validated against real
  Cloudflare script traffic.
- Nightly stealth integration test (`stealth_phase2.rs`) updated its
  sannysoft pass-cell detection to accept sannysoft's 2026 olive-green
  shade (`rgb(200, 216, 109)`) in addition to the legacy pure-green
  variants.
- Per-crate version automation via [release-plz](https://release-plz.dev).
  Each crate now versions independently based on conventional commits;
  the old manual `publish.yml` is replaced by `release-plz-pr.yml`
  (opens "chore: release" PR with version bumps + per-crate changelogs)
  and `release-plz-release.yml` (publishes on PR merge). Per-crate
  `CHANGELOG.md` files (next to each crate's `Cargo.toml`) are the
  authoritative changelog source going forward; this top-level file
  remains for human-curated release narrative. Design and migration
  plan in `docs/superpowers/{specs,plans}/2026-05-25-publish-version-automation*.md`.
- `zendriver-cloudflare`: dropped stale `zendriver-interception` Cargo
  dep (never imported). Added Stealth-recommended call-out in lib docs.
- Both `cloudflare` and `imperva` bypass loops emit one `tracing::warn!`
  after ~2.5s of stalled clearance, nudging callers toward
  `BrowserBuilder::stealth`.

### Known issues

- `zendriver-mcp::browser_tab_close` does NOT reap `browser_expect_*`
  expectations or `browser_intercept_*` rules that were registered
  against the closing tab. The per-handle registries
  (`SessionState::expectations` / `SessionState::rules`) are flat
  HashMaps with no `tab_id` field, so a per-tab close can't filter
  them. Workarounds: explicitly call `browser_expect_cancel` /
  `browser_intercept_remove_rule` before closing the tab, or let
  `browser_close` tear them down at session end (which DOES drain
  both registries as of this release). Fix tracked for a follow-up.

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
