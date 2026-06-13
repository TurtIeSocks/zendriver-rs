# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [0.1.4] - 2026-06-13


## [0.1.3] - 2026-06-03


## [0.1.2] - 2026-06-03


## [0.1.1] - 2026-06-03


## [0.1.0] - 2026-06-02

### Added

- Scaffold zendriver-datadome crate
- DataDomeError (faults-only)
- Surface detection (detect.js + DataDomeSurface)
- Captcha challenge extraction + cookie application
- ClearanceOutcome + DataDomeBypass driver + poll loop
- Opt-in Fetch interception fast-path

### Fixed

- Deadline-race the pre-loop probe; warn on empty challenge url/ua
- Deadline-bound the in-loop probe; warn on rejected setCookie; cover cookie_domain + build_challenge

