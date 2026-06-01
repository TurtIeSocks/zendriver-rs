# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [Unreleased]

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

