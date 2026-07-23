# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [0.1.16] - 2026-07-23


## [0.1.15] - 2026-07-20

### Added

- Opt-in stream_bodies via Network.streamResourceContent


## [0.1.14] - 2026-07-19

### Added

- Add Tab::tap/Element::tap for touch dispatch (Input.dispatchTouchEvent)


## [0.1.13] - 2026-07-18

### Added

- Opt-in coherent input profile decoupled from stealth selection


## [0.1.12] - 2026-07-17


## [0.1.11] - 2026-07-17

### Fixed

- Use public-suffix list for cookie_domain (co.uk etc.)


## [0.1.10] - 2026-07-17


## [0.1.9] - 2026-07-16


## [0.1.8] - 2026-07-16


## [0.1.7] - 2026-07-15


## [0.1.6] - 2026-07-12


## [0.1.5] - 2026-07-10


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

