# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

## [0.5.0] - 2026-07-16

### Added

- Wire Persona.geolocation to Emulation.setGeolocationOverride


## [0.4.6] - 2026-07-15

### Fixed

- Stop exec'ing `chrome --version` on Windows, where it never exits


## [0.4.5] - 2026-07-10

### Fixed

- Pass a plain locale list to setUserAgentOverride.acceptLanguage


## [0.4.4] - 2026-07-10

### Added

- Coherent chrome/webgl surface + screen geometry + pointer entropy

### Fixed

- Hide navigator.webdriver in the native profile
- Install prototype overrides via defineProperty


## [0.4.3] - 2026-06-13


## [0.4.2] - 2026-06-03


## [0.4.1] - 2026-06-03

### Added

- Native-function toString masking (full + cross-realm) ([#49](https://github.com/TurtIeSocks/zendriver-rs/pull/49))


## [0.4.0] - 2026-06-03


## [0.3.1] - 2026-06-03

### Added

- Accept_encoding_for(major) coherence rule
- Warn when a pinned Chrome major leaks Accept-Encoding


## [0.3.0] - 2026-06-02

### Added

- Add Surface::Webgpu (Value kind) + Persona.webgpu plumbing
- Renderer->GPUAdapter coherence map for WebGPU
- WebGPU coherence patch (navigator.gpu adapter from WebGL renderer)
- Validated WebGPU arch tokens from Dawn gpu_info.json (nvidia/amd/intel/apple model->uarch map)

### Fixed

- WebGPU patches GPUAdapter.prototype getter (no own-property/toString tell)
- WebGPU adapter validated against real Chrome (Apple metal-3, mask device/description)


## [0.2.1] - 2026-06-02


## [0.2.0] - 2026-06-02

### Added

- Seed type (random/from_system/from_u64)
- Per-surface persona spec types
- Persona struct + serde
- Persona::overlay field-wise merge
- Persona JSON ingestion (try_from_json + FromStr)
- PersonaBuilder
- Persona::system() host probe (cached)
- Persona patch-templating accessors
- Surface/Strategy + per-kind resolution
- Canvas farble patch
- Audio farble patch
- Webgl vendor/renderer substitution
- Font metrics + enumeration patch
- ClientRects sub-pixel patch
- Webrtc ip-leak guard
- Hardware surface patch
- Lib.rs top-level re-exports + clippy fix
- Bootstrap_script(persona, identity) + surface patch wiring
- Persona::from_browser live-probe via JsProbe trait
- Browser builder .persona/.persona_overlay/.surface

### Fixed

- Canvas restore + audio guard + getClientRects consistency


## [0.1.4] - 2026-06-02


## [0.1.3] - 2026-05-26


## [0.1.2] - 2026-05-25

### Fixed

- Repair stealth + cloudflare nightly tests against drift


## [0.1.1] - 2026-05-25

### Changed

- Split workspace.package.version into per-crate versions ([#5](https://github.com/TurtIeSocks/zendriver-rs/pull/5))

