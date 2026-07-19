# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [0.13.1] - 2026-07-19

### Added

- Add cache-TTL + force_refresh to pool/generative download caches


## [0.13.0] - 2026-07-19

### Added

- Add Tab::tap/Element::tap for touch dispatch (Input.dispatchTouchEvent)


## [0.12.0] - 2026-07-19

### Added

- Add Tab::expect_file_chooser for button-triggered file pickers


## [0.11.0] - 2026-07-19

### Added

- Expose case-insensitive predicate matching on browser_find/browser_find_all


## [0.10.0] - 2026-07-19

### Added

- Expose extended cookie fields (partition_key, priority, ...)
- Expose method/post_data overrides in intercept modify_request
- Add Browser::version() (CDP Browser.getVersion); wire MCP chrome_version
- Mirror the browser proxy into a custom geo_endpoint resolver


## [0.9.0] - 2026-07-19

### Fixed

- Drain tab-scoped expectations/rules on browser_tab_close


## [0.8.0] - 2026-07-18

### Added

- Add loss-accounted raw event stream (opt-in)
- Add BoundedBody bounded capture with explicit truncation
- Surface delivery-loss boundaries and bounded bodies instead of silent gaps
- Opt-in coherent input profile decoupled from stealth selection
- Add opt-in native-isolation/real-WebGL profile (default unchanged)

### Changed

- Rename BoundedBody.encoded_len/body_encoded_bytes to full_len/body_full_bytes **(BREAKING)**

### Fixed

- Treat responseReceived as headers-only; add opt-in strict loss policy
- Ready-barrier tab handoff, atomic seed, fail-closed on corrupt identity **(BREAKING)**
- Browser_open input profile follows stealth by default
- Default capture_body_max_bytes to 0 (unbounded)


## [0.7.7] - 2026-07-17

### Added

- Model partition_key as a structured CookiePartitionKey (CDP M119+)


## [0.7.6] - 2026-07-17


## [0.7.5] - 2026-07-17

### Fixed

- Wire Beta/Dev/Canary channels


## [0.7.4] - 2026-07-17


## [0.7.3] - 2026-07-17


## [0.7.2] - 2026-07-17


## [0.7.1] - 2026-07-17


## [0.7.0] - 2026-07-16

### Fixed

- Authenticate the exit-IP probe through the proxy + honor base-persona locale


## [0.6.10] - 2026-07-16


## [0.6.9] - 2026-07-16


## [0.6.8] - 2026-07-16


## [0.6.7] - 2026-07-15

### Fixed

- Bound every CDP call, not just the launch handshake


## [0.6.6] - 2026-07-12


## [0.6.5] - 2026-07-10


## [0.6.4] - 2026-07-10


## [0.6.3] - 2026-06-13


## [0.6.2] - 2026-06-13


## [0.6.1] - 2026-06-03

### Fixed

- Point element-not-found hints at browser_html


## [0.6.0] - 2026-06-03

### Added

- Opt-in tracker/fingerprinter blocklist ([#51](https://github.com/TurtIeSocks/zendriver-rs/pull/51))


## [0.5.1] - 2026-06-03


## [0.5.0] - 2026-06-03

### Added

- Opt-in stuck-request eviction for wait_for_idle (max_inflight_age)


## [0.4.0] - 2026-06-03

### Added

- Geo_country stealth override + schema snapshots

### Fixed

- Make geo_country field unconditional for feature-stable schema


## [0.3.4] - 2026-06-03


## [0.3.3] - 2026-06-03


## [0.3.2] - 2026-06-03


## [0.3.1] - 2026-06-02

### Added

- Browser_solve_datadome tool + ledger + schema snapshot

### Changed

- TimedOut is an Outcome, not an Error (unify result model)
- TimedOut is an Outcome, not an Error (unify result model)


## [0.3.0] - 2026-06-02

### Added

- AttrOp/AttrPredicate types + predicate-mode Selector
- Thread AttrOp/AttrPredicate predicates through resolve bridge
- Browser_open preferences + persona
- Browser_request tool
- Browser_fingerprint_generate (fingerprints feature)
- Monitor feature + SessionState monitor handles
- Browser_monitor_start/read/stop tools

### Changed

- Collapse find.rs selector-bridge duplication

### Fixed

- Stop monitors on browser_close + RAII MonitorState Drop


## [0.2.4] - 2026-06-02


## [0.2.3] - 2026-06-02


## [0.2.2] - 2026-06-02


## [0.2.1] - 2026-06-02


## [0.2.0] - 2026-06-02

### Added

- Browser_solve_imperva + imperva feature (Task A1)
- Scroll, window, pdf/mhtml, coordinate-mouse tools (Tasks A2-A5)
- Drive matched expectations — dialog accept/dismiss, response body, download save (Task A6)
- Tier 2 tools — chords, download, runtime-UA, fine stealth, modify_response, frame nav/load waits (Phase B)
- Tier 3 tools + docs — links, resource search, element extras, set-text/clear modes, TLS bypass (Phase C + D1)


## [0.1.5] - 2026-06-02

### Added

- Full nodriver / zendriver-py parity — phases P-A…P-E ([#17](https://github.com/TurtIeSocks/zendriver-rs/pull/17))


## [0.1.4] - 2026-06-01


## [0.1.3] - 2026-05-26


## [0.1.2] - 2026-05-25


## [0.1.1] - 2026-05-25

### Changed

- Split workspace.package.version into per-crate versions ([#5](https://github.com/TurtIeSocks/zendriver-rs/pull/5))

