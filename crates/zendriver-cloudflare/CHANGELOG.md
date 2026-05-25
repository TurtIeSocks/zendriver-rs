# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [Unreleased]

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

## [0.1.1] - 2026-05-25

### Changed

- Split workspace.package.version into per-crate versions ([#5](https://github.com/TurtIeSocks/zendriver-rs/pull/5))

