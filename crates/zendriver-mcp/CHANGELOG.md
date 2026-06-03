# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

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

