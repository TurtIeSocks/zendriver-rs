# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [Unreleased]

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

