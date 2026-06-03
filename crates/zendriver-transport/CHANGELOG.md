# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [Unreleased]

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

