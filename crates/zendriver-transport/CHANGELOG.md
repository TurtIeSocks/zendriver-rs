# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [Unreleased]

## [0.2.3] - 2026-07-23

### Added

- Hoist the observer surface into the facade; flag transport internal


## [0.2.2] - 2026-07-20

### Added

- Opt-in stream_bodies via Network.streamResourceContent


## [0.2.1] - 2026-07-19

### Added

- Add Tab::tap/Element::tap for touch dispatch (Input.dispatchTouchEvent)


## [0.2.0] - 2026-07-18

### Added

- Add loss-accounted raw event stream (opt-in)
- Observer timeout fails closed by default (BestEffort opt-out) **(BREAKING)**
- Opt-in coherent input profile decoupled from stealth selection

### Changed

- Log Required-observer timeout at error!, matching its fail-closed peers

### Fixed

- Treat responseReceived as headers-only; add opt-in strict loss policy
- BestEffort timeout continues the observer chain (no orphaned target)


## [0.1.11] - 2026-07-17


## [0.1.10] - 2026-07-17


## [0.1.9] - 2026-07-16


## [0.1.8] - 2026-07-16


## [0.1.7] - 2026-07-15

### Fixed

- Bound every CDP call, not just the launch handshake


## [0.1.6] - 2026-06-13


## [0.1.5] - 2026-06-03


## [0.1.4] - 2026-06-02


## [0.1.3] - 2026-06-02

### Added

- Full nodriver / zendriver-py parity — phases P-A…P-E ([#17](https://github.com/TurtIeSocks/zendriver-rs/pull/17))


### Added

- `Connection::reconnect(ws)` / `Connection::redial(ws_url)` — restart the actor on a fresh socket while reusing the same `Connection` handle and broadcast event bus (raw event subscribers re-attach automatically), re-spawning with the original observer chain. `redial` re-applies the WebSocket size config. Pre-existing CDP `sessionId`s are stale after a reconnect.
- Unexpected ws death (Chrome-sent Close frame, read error, stream end) now drains in-flight calls with a distinct `DISCONNECTED_CODE`, mapped by `Connection::call_raw` to `TransportError::Disconnected` — separate from the `SHUTDOWN_DRAIN_CODE`/`TransportError::Shutdown` used for a caller-requested `shutdown()`.
- `MockConnection::disconnect()` test helper to simulate an unexpected socket drop.

## [0.1.2] - 2026-05-26


## [0.1.1] - 2026-05-25

### Changed

- Split workspace.package.version into per-crate versions ([#5](https://github.com/TurtIeSocks/zendriver-rs/pull/5))

