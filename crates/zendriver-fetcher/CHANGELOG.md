# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [0.2.1] - 2026-08-07


## [0.2.0] - 2026-08-07

### Added

- Resolve ungoogled-chromium and Chromium snapshots
- Add the zendriver-fetch CLI behind a non-default cli feature

### Changed

- Build staging paths without a lossy display() round-trip
- Derive the CfT binary path from cft_top_dir
- Compute the ungoogled zip top-level directory once

### Fixed

- Ignore ungoogled assets whose name is not a plain filename
- Extract validated symlinks, or macOS never gets a binary
- Gate resolve_target to unix so Windows builds compile
- Reject drive-relative ungoogled asset names
- Send GITHUB_TOKEN only to https://api.github.com
- Fail manifest fetches on HTTP status
- Name the channel the flat manifest cannot resolve
- Resolve symlink targets against the link's real parent
- Validate the build id an index hands back
- Resolve symlink target components against the filesystem
- Refuse zip entries carrying a parent-directory component
- Give each fetch its own staging directory
- Prefer the ungoogled tag that has this platform's asset
- Verify symlink containment on the finished tree


## [0.1.15] - 2026-07-29


## [0.1.14] - 2026-07-23


## [0.1.13] - 2026-07-20

### Added

- Opt-in stream_bodies via Network.streamResourceContent


## [0.1.12] - 2026-07-19

### Added

- Add Tab::tap/Element::tap for touch dispatch (Input.dispatchTouchEvent)


## [0.1.11] - 2026-07-18

### Added

- Opt-in coherent input profile decoupled from stealth selection


## [0.1.10] - 2026-07-17


## [0.1.9] - 2026-07-17

### Fixed

- Wire Beta/Dev/Canary channels


## [0.1.8] - 2026-07-17


## [0.1.7] - 2026-07-16


## [0.1.6] - 2026-07-16


## [0.1.5] - 2026-06-13


## [0.1.4] - 2026-06-03


## [0.1.3] - 2026-06-02


## [0.1.2] - 2026-05-26


## [0.1.1] - 2026-05-25

### Changed

- Split workspace.package.version into per-crate versions ([#5](https://github.com/TurtIeSocks/zendriver-rs/pull/5))

