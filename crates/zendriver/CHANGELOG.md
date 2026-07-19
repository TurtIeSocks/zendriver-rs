# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [Unreleased]

## [0.3.5] - 2026-07-19

### Fixed

- Content-key the farble PRNG; fix canvas fingerprint tests


## [0.3.4] - 2026-07-19

### Added

- Add Tab::tap/Element::tap for touch dispatch (Input.dispatchTouchEvent)


## [0.3.3] - 2026-07-19

### Added

- Add Tab::expect_file_chooser for button-triggered file pickers


## [0.3.2] - 2026-07-19

### Added

- Add case-insensitive predicate matchers (attr_i/attr_contains_i/attr_starts_with_i/attr_ends_with_i/containing_text_i/text_equals_i)


## [0.3.1] - 2026-07-19

### Added

- Add Browser::version() (CDP Browser.getVersion); wire MCP chrome_version
- Mirror the browser proxy into a custom geo_endpoint resolver


## [0.3.0] - 2026-07-18

### Added

- Add loss-accounted raw event stream (opt-in)
- Add EventStreamIncomplete to distinguish lost-observation from timeout
- Add BoundedBody bounded capture with explicit truncation
- Surface delivery-loss boundaries and bounded bodies instead of silent gaps
- Opt-in coherent input profile decoupled from stealth selection

### Changed

- Rename BoundedBody.encoded_len/body_encoded_bytes to full_len/body_full_bytes **(BREAKING)**

### Fixed

- Treat responseReceived as headers-only; add opt-in strict loss policy
- Ready-barrier tab handoff, atomic seed, fail-closed on corrupt identity **(BREAKING)**
- BestEffort timeout continues the observer chain (no orphaned target)
- Report EventStreamIncomplete on transport teardown instead of Timeout
- Default input profile follows stealth selection (no silent regression)
- Evict oldest tracked request (FIFO) instead of an arbitrary one


## [0.2.30] - 2026-07-17

### Added

- Model partition_key as a structured CookiePartitionKey (CDP M119+)


## [0.2.29] - 2026-07-17

### Added

- Geo_auto uses the exact timezone from the exit-IP probe


## [0.2.28] - 2026-07-17

### Fixed

- Sort frames() by frame id for deterministic cross-frame results


## [0.2.27] - 2026-07-17

### Fixed

- Narrow text_regex matches to the innermost element (was returning ancestors)


## [0.2.26] - 2026-07-17

### Fixed

- Pin frame-scoped xpath/text/role selectors to the frame contextId


## [0.2.25] - 2026-07-17


## [0.2.24] - 2026-07-17

### Fixed

- Explicit persona fields win over geo-derived (locale + timezone)


## [0.2.23] - 2026-07-16

### Added

- Structured BrowserBuilder::proxy reusing split_proxy_url
- Add IpApiResolver (proxied exit-IP country probe)
- Geo_auto/geo_resolver builder + launch-time exit-IP resolution

### Fixed

- Authenticate the exit-IP probe through the proxy + honor base-persona locale


## [0.2.22] - 2026-07-16

### Fixed

- Honor visible_only by filtering finds through check_visible
- Honor visible_only across include_frames() fan-out paths


## [0.2.21] - 2026-07-16

### Added

- Add split_proxy_url helper for per-context proxy config
- Add BrowserContextBuilder with per-context proxy credentials
- Auto-install per-context proxy auth on each context tab
- Unregister proxy credentials on BrowserContext drop

### Fixed

- Decode + redact proxy userinfo, default socks port, guard one-actor invariant


## [0.2.20] - 2026-07-16


## [0.2.19] - 2026-07-15

### Added

- Resolve the WS endpoint from DevToolsActivePort as a fallback
- Discover per-user %LOCALAPPDATA% chrome installs on windows

### Fixed

- Bound the CDP handshake and kill the child on timeout
- Send CDP Browser.close before falling back to signals
- Sweep stray page targets instead of silently discarding them
- Kill chrome's whole process tree via a windows job object
- Gate the owning-browser test helper to unix like its callers
- Bound every CDP call, not just the launch handshake
- Stop exec'ing `chrome --version` on Windows, where it never exits
- Stop crying wolf on the initial target attach


## [0.2.18] - 2026-07-12


## [0.2.17] - 2026-07-10


## [0.2.16] - 2026-07-10

### Fixed

- Hide navigator.webdriver in the native profile


## [0.2.15] - 2026-06-13


## [0.2.14] - 2026-06-03


## [0.2.13] - 2026-06-03

### Added

- Opt-in tracker/fingerprinter blocklist ([#51](https://github.com/TurtIeSocks/zendriver-rs/pull/51))


## [0.2.12] - 2026-06-03

### Added

- Native-function toString masking (full + cross-realm) ([#49](https://github.com/TurtIeSocks/zendriver-rs/pull/49))


## [0.2.11] - 2026-06-03

### Added

- Opt-in stuck-request eviction for wait_for_idle (max_inflight_age)

### Fixed

- Start initial tab on about:blank to fix flaky wait_for_idle


## [0.2.10] - 2026-06-03

### Added

- BrowserBuilder::geo_locale (geo feature)


## [0.2.9] - 2026-06-03

### Added

- Warn when a pinned Chrome major leaks Accept-Encoding


## [0.2.8] - 2026-06-03


## [0.2.7] - 2026-06-02

### Added

- Wire Tab::datadome() + re-exports + error mapping behind the datadome feature

### Changed

- TimedOut is an Outcome, not an Error (unify result model)
- TimedOut is an Outcome, not an Error (unify result model)


## [0.2.6] - 2026-06-02


## [0.2.5] - 2026-06-02


## [0.2.4] - 2026-06-02

### Added

- PredicateSet types + CSS compilation
- Predicate JS post-filter compilation
- ConflictingSelectors variant
- Predicate methods on FindBuilder
- Predicate methods on FindAllBuilder
- Predicate resolution + mixing guard at terminal
- Select/select_all CSS aliases on Tab/Frame/Element

### Changed

- Simplify resolver match, drop dead helper, doc attr-name safety


## [0.2.3] - 2026-06-02

### Added

- Persona::from_browser live-probe via JsProbe trait
- Browser builder .persona/.persona_overlay/.surface
- Persist persona seed alongside user_data_dir


## [0.2.2] - 2026-06-02


## [0.2.1] - 2026-06-02

### Added

- Tier 2 tools — chords, download, runtime-UA, fine stealth, modify_response, frame nav/load waits (Phase B)


## [0.2.0] - 2026-06-02

### Added

- Full nodriver / zendriver-py parity — phases P-A…P-E ([#17](https://github.com/TurtIeSocks/zendriver-rs/pull/17))


### Added

- `Browser::reconnect()` — re-establish a dropped connection to the same still-running Chrome process (re-dials the surviving `/devtools/browser/<id>` endpoint on the existing `Connection`, re-arms `Target.setAutoAttach{flatten:true}` so stealth re-injects, refreshes the tab registry). Scoped v1: existing `Tab`/`Frame`/`Element` handles are **invalidated** — re-acquire via `tabs()` (not `main_tab()`, which still returns the stale handle). Per-feature domain re-arm (`Network.enable`, `Fetch` rules, etc.) and transparent handle-preserving reconnect are deferred.

### Changed

- An unexpected WebSocket drop (Chrome died / socket severed) now surfaces in-flight CDP calls as the new distinct `ZendriverError::Disconnected` variant, instead of the opaque shutdown error used for a clean `close()`. Long-running callers can now tell "connection lost" apart from "I closed it" and recover via `Browser::reconnect()`.

## [0.1.4] - 2026-06-01

### Added

- Add BrowserContext skeleton
- BrowserInner::dispose_browser_context
- Drop for BrowserContext schedules dispose
- Browser::create_browser_context[_with]
- BrowserContext::new_tab[_at] threads browserContextId


### Added

- `Browser::create_browser_context` and `Browser::create_browser_context_with(proxy_server, proxy_bypass_list)` — high-level wrappers around CDP `Target.createBrowserContext` for per-context proxy and storage isolation.
- `BrowserContext` RAII guard with `new_tab` / `new_tab_at` (threads `browserContextId` into `Target.createTarget`) and a `Drop` impl that schedules `Target.disposeBrowserContext` via the underlying connection.
- `examples/browser_context_isolation` demonstrating per-context proxy bindings against a rotating upstream.


## [0.1.3] - 2026-05-26

### Added

- Scaffold zendriver-imperva crate
- Wire imperva feature into parent crate


## [0.1.2] - 2026-05-25

### Fixed

- Repair stealth + cloudflare nightly tests against drift


## [0.1.1] - 2026-05-25

### Changed

- Split workspace.package.version into per-crate versions ([#5](https://github.com/TurtIeSocks/zendriver-rs/pull/5))

