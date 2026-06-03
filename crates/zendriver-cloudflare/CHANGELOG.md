# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [Unreleased]

## [0.2.1] - 2026-06-03


## [0.2.0] - 2026-06-02

### Changed

- TimedOut is an Outcome, not an Error (unify result model)


## [0.1.4] - 2026-06-02


## [0.1.3] - 2026-05-26

### Added

- Stalled-poll tracing::warn!


## [0.1.2] - 2026-05-25

### Fixed

- Repair stealth + cloudflare nightly tests against drift


### Changed

- `CloudflareBypass::wait_for_clearance` now drives a unified poll loop
  rather than an upfront iframe-detect + click + poll. Each tick fetches
  the `cf-turnstile-response` token, the challenge iframe bbox (shadow-DOM
  aware), and a challenge-marker flag in a single CDP round-trip. The
  click flow still fires when the interactive iframe mounts; the
  **invisible Turnstile** path — where Cloudflare populates the token
  with no iframe ever mounting — now resolves to `TokenAcquired` instead
  of `Err(NoChallenge)`. `NoChallenge` is now reserved for the case
  where the entire timeout window elapsed without any challenge marker on
  the page. Public surface unchanged.
- Dropped stale `zendriver-interception` Cargo dep (declared but never
  imported in `src/`).
- `CloudflareBypass::wait_for_clearance` emits one `tracing::warn!`
  after ~2.5s of stalled clearance, suggesting
  `BrowserBuilder::stealth`.
- `lib.rs` module docs gain a *Stealth recommended* call-out at the
  top for posture parity with `zendriver-imperva`.

## [0.1.1] - 2026-05-25

### Changed

- Split workspace.package.version into per-crate versions ([#5](https://github.com/TurtIeSocks/zendriver-rs/pull/5))
