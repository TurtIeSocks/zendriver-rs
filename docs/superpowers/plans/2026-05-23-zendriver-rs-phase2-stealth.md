# zendriver-rs Phase 2 (Stealth) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add anti-detection to zendriver-rs. New `zendriver-stealth` crate with two named profiles (`native` + `spoofed`); `TargetObserver` trait in `zendriver-transport` with paused-target injection via `Target.setAutoAttach { waitForDebuggerOnStart }`; isolated-world `Tab::evaluate` default; nightly stealth tests against `bot.sannysoft.com` pass all Intoli rows for the spoofed profile.

**Architecture:** New `zendriver-stealth` populates the P1 stub. Connection actor gains observer dispatch on `Target.attachedToTarget` events. `Tab::evaluate` switches to `Page.createIsolatedWorld` + `Runtime.evaluate { contextId }`; new `Tab::evaluate_main` is the main-world escape hatch. Fingerprint auto-detected at launch via `sysinfo` + `num_cpus` + `chrome --version` probe. JS patches bundled as a single factory function called with serialized Fingerprint.

**Tech Stack:** Rust + async-trait for `TargetObserver`; `tokio::time::timeout` + `std::panic::AssertUnwindSafe` + `futures::FutureExt::catch_unwind` for observer isolation; `sysinfo` for RAM probe; `num_cpus` for CPU count; `Page.createIsolatedWorld` + `Runtime.evaluate { contextId }` for isolated-world eval; serde for `UserAgentMetadata` CDP shape.

**Spec:** [docs/superpowers/specs/2026-05-23-zendriver-rs-phase2-stealth-design.md](../specs/2026-05-23-zendriver-rs-phase2-stealth-design.md)

---

## File structure

### Workspace root (modify)
- `Cargo.toml` — add `sysinfo`, `num_cpus`, `async-trait` (if not present) to `[workspace.dependencies]`

### `crates/zendriver-stealth/` (populate from P1 stub)
- `Cargo.toml` — replace stub with full dep set
- `src/lib.rs` — re-exports + crate-level docs
- `src/profile.rs` — `StealthProfile`, `ProfileKind`, `PerFieldOverride`, `Platform`
- `src/flags.rs` — `default_flags()`, `flags_for_profile()` per `ProfileKind`
- `src/fingerprint.rs` — `Fingerprint`, `auto_detect`, clamping helpers, `Brand`, `UserAgentMetadata`
- `src/ua.rs` — UA-string composition + per-Platform `UserAgentMetadata` builders
- `src/patches.rs` — `PatchSource` + `bundle_factory(profile, fp)` that emits the bootstrap script
- `src/observer.rs` — `StealthObserver` impl of `TargetObserver`
- `src/error.rs` — `StealthError`
- `src/patches/webdriver.js` — Navigator.prototype.webdriver getter → false
- `src/patches/plugins.js` — fake navigator.plugins (3 entries)
- `src/patches/chrome.js` — `window.chrome = { runtime: {} }`
- `src/patches/webgl.js` — WebGL getParameter patches for UNMASKED_VENDOR_WEBGL / UNMASKED_RENDERER_WEBGL
- `src/patches/permissions.js` — `Notification.permission` ↔ `navigator.permissions.query({name:'notifications'}).state` consistency
- `src/patches/codecs.js` — `HTMLMediaElement.prototype.canPlayType` returns 'probably' for h264/aac
- `src/patches/navigator_props.js` — platform / hardwareConcurrency / deviceMemory / languages
- `src/patches/user_agent_data.js` — `navigator.userAgentData` getter
- `src/patches/broken_image.js` — natural{Width,Height} of unloaded <img> > 0

### `crates/zendriver-transport/` (modify)
- `Cargo.toml` — add `async-trait`, `futures` (if not present)
- `src/lib.rs` — re-export new types (`TargetObserver`, `PausedSession`, `ObserverError`, `TargetInfo`)
- `src/observer.rs` (NEW) — `TargetObserver` trait, `PausedSession`, `ObserverError`, `TargetInfo`
- `src/actor.rs` — add `observers: Vec<Arc<dyn TargetObserver>>` field, `handle_target_attached` async fn, route `Target.attachedToTarget` / `Target.detachedFromTarget` events
- `src/connection.rs` — `connect_with_observers(ws_url, observers)` entry, `spawn_actor_with_observers`, refactor inner to `Arc<ConnectionActorInner>` so `Weak` self refs work

### `crates/zendriver/` (modify)
- `Cargo.toml` — add `zendriver-stealth.workspace = true` as a regular dep; add `stealth-tests` feature
- `src/lib.rs` — re-export `zendriver_stealth::{StealthProfile, Fingerprint, Platform, UserAgentMetadata}` under `pub mod stealth`
- `src/error.rs` — add `Stealth(#[from] StealthError)` variant to `ZendriverError`
- `src/browser.rs` — add `BrowserBuilder::stealth` + `BrowserBuilder::observer` methods; `launch` resolves Fingerprint, builds observer vec, calls `connect_with_observers`, sends `Target.setAutoAttach` before initial attach
- `src/tab.rs` — refactor `evaluate<T>` to isolated-world via `Page.createIsolatedWorld`; add `evaluate_main<T>`; cache isolated `executionContextId` per main frame; re-create on `Page.frameNavigated`
- `src/element.rs` — `evaluate<T>` switches to isolated; add `evaluate_main<T>` for main world

### `crates/zendriver/tests/` (new + modify)
- `tests/integration_phase1.rs` — fix the one assertion that intentionally reads page global: `tab.evaluate("window.clicked")` → `tab.evaluate_main("window.clicked")`
- `tests/integration_phase2.rs` (NEW, gated `integration-tests`) — native vs spoofed wiremock + isolated/main world separation
- `tests/stealth_phase2.rs` (NEW, gated `stealth-tests`) — sannysoft + areyouheadless + intoli nightly

### `.github/workflows/ci.yml` (modify)
- Add `nightly-stealth-tests` job on cron `0 6 * * *`, `continue-on-error: true`

---

## Task list (overview)

| # | Title | Files (scope) |
|---|---|---|
| 0 | Workspace + stealth crate scaffolding | root + crates/zendriver-stealth |
| 1 | `StealthError` type | crates/zendriver-stealth/src/error.rs |
| 2 | `Platform` + `ProfileKind` + `PerFieldOverride` + UA brand types | crates/zendriver-stealth/src/profile.rs (skeleton), src/fingerprint.rs (types only) |
| 3 | `UserAgentMetadata` + JSON snapshot | crates/zendriver-stealth/src/fingerprint.rs (UAM) |
| 4 | UA-string composition (`compose_ua_string`) | crates/zendriver-stealth/src/ua.rs |
| 5 | `Fingerprint::auto_detect` (chrome --version + sysinfo + num_cpus) | crates/zendriver-stealth/src/fingerprint.rs |
| 6 | Flag tables (`default_flags`, `flags_for_profile`) | crates/zendriver-stealth/src/flags.rs |
| 7 | `StealthProfile` constructors + builder methods | crates/zendriver-stealth/src/profile.rs |
| 8 | Patches `.js` files (9 of them) | crates/zendriver-stealth/src/patches/*.js |
| 9 | `PatchSource` + bundle factory | crates/zendriver-stealth/src/patches.rs |
| 10 | `TargetObserver` trait + `PausedSession` + `ObserverError` + `TargetInfo` | crates/zendriver-transport/src/observer.rs (NEW) |
| 11 | Connection actor: observer dispatch + Target.attachedToTarget routing | crates/zendriver-transport/src/actor.rs, src/connection.rs |
| 12 | `connect_with_observers` + lib re-exports | crates/zendriver-transport/src/connection.rs, lib.rs |
| 13 | `StealthObserver` impl | crates/zendriver-stealth/src/observer.rs |
| 14 | `Tab::evaluate` → isolated; `Tab::evaluate_main` | crates/zendriver/src/tab.rs |
| 15 | `Element::evaluate` → isolated; `Element::evaluate_main` | crates/zendriver/src/element.rs |
| 16 | `ZendriverError::Stealth` variant + From | crates/zendriver/src/error.rs |
| 17 | `BrowserBuilder::stealth` + `observer` + launch wiring | crates/zendriver/src/browser.rs |
| 18 | Fix P1 integration test to use `evaluate_main` | crates/zendriver/tests/integration_phase1.rs |
| 19 | P2 integration tests (native vs spoofed + isolated/main world) | crates/zendriver/tests/integration_phase2.rs (NEW) |
| 20 | Snapshot tests: flags + UA + UAM | crates/zendriver-stealth/src/flags.rs::tests, ua.rs::tests, fingerprint.rs::tests |
| 21 | Nightly stealth tests (sannysoft, areyouheadless, intoli) | crates/zendriver/tests/stealth_phase2.rs (NEW) |
| 22 | CI nightly cron job | .github/workflows/ci.yml |

---

## Task 0: Workspace + stealth crate scaffolding

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/zendriver-stealth/Cargo.toml`
- Modify: `crates/zendriver-stealth/src/lib.rs`
- Create: `crates/zendriver-stealth/src/{profile,flags,fingerprint,ua,patches,observer,error}.rs` (all stubs)
- Create: `crates/zendriver-stealth/src/patches/` (empty dir)
- Modify: `crates/zendriver/Cargo.toml` (add stealth dep + `stealth-tests` feature)
- Modify: `crates/zendriver-transport/Cargo.toml` (add `async-trait`, `futures` if missing)

- [ ] **Step 1: Add new deps to workspace `Cargo.toml`**

In `[workspace.dependencies]`, add:

```toml
sysinfo     = { version = "0.32", default-features = false, features = ["system"] }
num_cpus    = "1"
```

`async-trait` and `futures` are already declared in the workspace from P1 — verify with `grep '^async-trait\|^futures' Cargo.toml` and add only if missing.

- [ ] **Step 2: Replace `crates/zendriver-stealth/Cargo.toml`**

```toml
[package]
name = "zendriver-stealth"
description = "Anti-detection patches and profiles for zendriver"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[lints]
workspace = true

[dependencies]
zendriver-transport.workspace = true
chromiumoxide_cdp.workspace   = true
tokio.workspace               = true
async-trait.workspace         = true
futures.workspace             = true
serde.workspace               = true
serde_json.workspace          = true
thiserror.workspace           = true
tracing.workspace             = true
sysinfo.workspace             = true
num_cpus.workspace            = true

[dev-dependencies]
tokio-test.workspace          = true
insta.workspace               = true
```

- [ ] **Step 3: Create stub source files**

Path: `crates/zendriver-stealth/src/lib.rs`

```rust
//! Anti-detection profiles and patches for zendriver.
//!
//! Two profiles ship: [`StealthProfile::native`] (launch flags + UA scrub,
//! no JS bootstrap) and [`StealthProfile::spoofed`] (adds Navigator-prototype
//! JS patches). Default is `native` via `BrowserBuilder`.

pub mod error;
pub mod fingerprint;
pub mod flags;
pub mod observer;
pub mod patches;
pub mod profile;
pub mod ua;

pub use error::StealthError;
pub use fingerprint::{Fingerprint, UserAgentMetadata, Brand};
pub use observer::StealthObserver;
pub use profile::{Platform, ProfileKind, StealthProfile};
```

Each module file gets a stub:

```rust
//! Populated in subsequent Phase 2 tasks.
```

Apply to: `error.rs`, `fingerprint.rs`, `flags.rs`, `observer.rs`, `patches.rs`, `profile.rs`, `ua.rs`.

Also: `mkdir crates/zendriver-stealth/src/patches`. The .js files go in there in Task 8.

- [ ] **Step 4: Modify `crates/zendriver/Cargo.toml`**

Add to `[dependencies]`:

```toml
zendriver-stealth.workspace = true
```

Add to `[features]`:

```toml
# Nightly stealth tests against external sites (sannysoft, areyouheadless).
# Separate from integration-tests because external sites flake.
stealth-tests = ["integration-tests"]
```

- [ ] **Step 5: Verify the empty workspace still builds**

```bash
cargo build --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

All three should pass. The `lib.rs` re-exports name types that don't exist yet — those `pub use` lines must be commented out for the stub to compile. Comment them with `// uncomment as types land in later tasks:` and uncomment one at a time as Tasks 1–7 land their types.

The interim `lib.rs`:

```rust
//! Anti-detection profiles and patches for zendriver.

pub mod error;
pub mod fingerprint;
pub mod flags;
pub mod observer;
pub mod patches;
pub mod profile;
pub mod ua;

// Re-exports added as types land in Tasks 1–13:
// pub use error::StealthError;
// pub use fingerprint::{Fingerprint, UserAgentMetadata, Brand};
// pub use observer::StealthObserver;
// pub use profile::{Platform, ProfileKind, StealthProfile};
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
chore(stealth): populate zendriver-stealth crate skeleton

Populates the P1-deferred stub with module files, deps (sysinfo,
num_cpus, async-trait, futures), and a stealth-tests feature on the
zendriver crate. All modules are stubs; impls land in subsequent
Phase 2 tasks.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: `StealthError` type

**Files:**
- Modify: `crates/zendriver-stealth/src/error.rs`

- [ ] **Step 1: Implement + tests**

```rust
//! Stealth-layer errors.

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StealthError {
    #[error("failed to apply patch '{patch}'")]
    PatchFailed {
        patch: &'static str,
        #[source]
        source: zendriver_transport::CallError,
    },

    #[error("could not detect chrome version: {0}")]
    ChromeVersionDetect(String),

    #[error("could not read system info: {0}")]
    SystemInfo(String),

    #[error("invalid fingerprint override: {0}")]
    InvalidOverride(String),
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_chrome_version_detect_includes_message() {
        let e = StealthError::ChromeVersionDetect("exit 1".into());
        assert_eq!(e.to_string(), "could not detect chrome version: exit 1");
    }

    #[test]
    fn display_system_info_includes_message() {
        let e = StealthError::SystemInfo("permission denied".into());
        assert_eq!(e.to_string(), "could not read system info: permission denied");
    }

    #[test]
    fn display_invalid_override_includes_message() {
        let e = StealthError::InvalidOverride("memory_gb must be > 0".into());
        assert_eq!(e.to_string(), "invalid fingerprint override: memory_gb must be > 0");
    }
}
```

Uncomment the `pub use error::StealthError;` line in `lib.rs`.

- [ ] **Step 2: Verify**

```bash
cargo test -p zendriver-stealth --lib error::tests
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 3 tests pass, clippy clean.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/error.rs crates/zendriver-stealth/src/lib.rs
git commit -m "feat(stealth): add StealthError type

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: `Platform` + `ProfileKind` + `PerFieldOverride` + `Brand`

**Files:**
- Modify: `crates/zendriver-stealth/src/profile.rs`

- [ ] **Step 1: Add the basic type scaffolding**

```rust
//! Profile types: ProfileKind enum, Platform enum, PerFieldOverride struct,
//! plus the StealthProfile builder (filled in Task 7).

use std::path::PathBuf;

/// Stealth modes shipped by the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    /// No stealth applied. Browser is launched stock; no JS patches, no UA scrub.
    Off,
    /// Launch flags + UA scrub (HeadlessChrome → Chrome). No JS bootstrap.
    /// Safe against `Function.prototype.toString` detection. Default.
    Native,
    /// Native + Navigator-prototype JS patches. Passes sannysoft. Detectable
    /// by sophisticated bots that probe `toString` on Navigator getters.
    Spoofed,
}

/// JS `navigator.platform` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Win32,
    MacIntel,
    LinuxX86_64,
}

impl Platform {
    /// Map to the `navigator.platform` string Chrome reports for that OS.
    #[must_use]
    pub fn js_string(self) -> &'static str {
        match self {
            Platform::Win32       => "Win32",
            Platform::MacIntel    => "MacIntel",
            Platform::LinuxX86_64 => "Linux x86_64",
        }
    }

    /// CDP `userAgentMetadata.platform` value (no version).
    #[must_use]
    pub fn ch_platform(self) -> &'static str {
        match self {
            Platform::Win32       => "Windows",
            Platform::MacIntel    => "macOS",
            Platform::LinuxX86_64 => "Linux",
        }
    }

    /// UA-string OS token (the bit inside parentheses).
    #[must_use]
    pub fn ua_token(self) -> &'static str {
        match self {
            Platform::Win32       => "Windows NT 10.0; Win64; x64",
            Platform::MacIntel    => "Macintosh; Intel Mac OS X 10_15_7",
            Platform::LinuxX86_64 => "X11; Linux x86_64",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PerFieldOverride {
    pub memory_gb: Option<u32>,
    pub cpu_count: Option<u32>,
    pub chrome_major: Option<u32>,
    pub platform: Option<Platform>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub ua_string: Option<String>,
}

/// Placeholder; full StealthProfile lands in Task 7.
#[allow(dead_code)]
pub struct StealthProfile {
    pub(crate) kind: ProfileKind,
    pub(crate) extra_flags: Vec<String>,
    pub(crate) fingerprint_override: Option<crate::Fingerprint>,
    pub(crate) per_field: PerFieldOverride,
    pub(crate) bypass_csp: bool,
    pub(crate) user_data_dir: Option<PathBuf>,
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn platform_js_string_matches_chrome_output() {
        assert_eq!(Platform::Win32.js_string(), "Win32");
        assert_eq!(Platform::MacIntel.js_string(), "MacIntel");
        assert_eq!(Platform::LinuxX86_64.js_string(), "Linux x86_64");
    }

    #[test]
    fn platform_ch_platform_uses_no_version() {
        assert_eq!(Platform::MacIntel.ch_platform(), "macOS");
    }

    #[test]
    fn platform_ua_token_includes_arch() {
        assert!(Platform::Win32.ua_token().contains("Win64; x64"));
    }
}
```

Uncomment the `pub use profile::{Platform, ProfileKind, StealthProfile};` line in `lib.rs`.

Note: `Fingerprint` is referenced as `crate::Fingerprint` but doesn't exist yet (Task 5 creates it). Until then, comment out the `fingerprint_override` field and add it back in Task 7.

For Task 2 only, the `StealthProfile` struct is:

```rust
#[allow(dead_code)]
pub struct StealthProfile {
    pub(crate) kind: ProfileKind,
    pub(crate) extra_flags: Vec<String>,
    pub(crate) per_field: PerFieldOverride,
    pub(crate) bypass_csp: bool,
    pub(crate) user_data_dir: Option<PathBuf>,
    // fingerprint_override: Option<Fingerprint>,  // added in Task 7
}
```

- [ ] **Step 2: Verify**

```bash
cargo test -p zendriver-stealth --lib profile::tests
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 3 passed, clippy clean.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/profile.rs crates/zendriver-stealth/src/lib.rs
git commit -m "feat(stealth): Platform, ProfileKind, PerFieldOverride

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: `UserAgentMetadata` + `Brand` + JSON snapshot

**Files:**
- Modify: `crates/zendriver-stealth/src/fingerprint.rs`

- [ ] **Step 1: Implement UA metadata types + a per-Platform constructor**

```rust
//! Fingerprint: composed UA + Sec-CH-UA metadata + system facts.

use serde::Serialize;

use crate::Platform;

#[derive(Debug, Clone, Serialize)]
pub struct Brand {
    pub brand: String,
    pub version: String,
}

/// Sent to CDP as `Emulation.setUserAgentOverride.userAgentMetadata`.
/// Mirrors the [W3C UA-CH spec](https://wicg.github.io/ua-client-hints/).
#[derive(Debug, Clone, Serialize)]
pub struct UserAgentMetadata {
    pub brands: Vec<Brand>,
    #[serde(rename = "fullVersionList")]
    pub full_version_list: Vec<Brand>,
    pub platform: String,
    #[serde(rename = "platformVersion")]
    pub platform_version: String,
    pub architecture: String,
    pub bitness: String,
    pub wow64: bool,
    pub mobile: bool,
    pub model: String,
}

impl UserAgentMetadata {
    /// Build a realistic UAM for the given platform + Chrome major version.
    /// Uses the Chrome convention of three brands: "Not_A Brand;v=8",
    /// "Chromium;v=N", "Google Chrome;v=N".
    pub fn realistic(platform: Platform, chrome_major: u32, chrome_full: &str) -> Self {
        let brands = vec![
            Brand { brand: "Not_A Brand".into(),  version: "8".into() },
            Brand { brand: "Chromium".into(),     version: chrome_major.to_string() },
            Brand { brand: "Google Chrome".into(), version: chrome_major.to_string() },
        ];
        let full_version_list = vec![
            Brand { brand: "Not_A Brand".into(),  version: "8.0.0.0".into() },
            Brand { brand: "Chromium".into(),     version: chrome_full.to_string() },
            Brand { brand: "Google Chrome".into(), version: chrome_full.to_string() },
        ];
        let (platform_version, architecture, bitness) = match platform {
            Platform::Win32       => ("15.0.0", "x86", "64"),
            Platform::MacIntel    => ("10.15.7", "x86", "64"),
            Platform::LinuxX86_64 => ("5.15.0", "x86", "64"),
        };
        Self {
            brands,
            full_version_list,
            platform: platform.ch_platform().to_string(),
            platform_version: platform_version.to_string(),
            architecture: architecture.to_string(),
            bitness: bitness.to_string(),
            wow64: false,
            mobile: false,
            model: String::new(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn realistic_uam_macintel_chrome_120_matches_snapshot() {
        let uam = UserAgentMetadata::realistic(Platform::MacIntel, 120, "120.0.6099.234");
        insta::assert_json_snapshot!("uam_macintel_chrome_120", uam);
    }

    #[test]
    fn realistic_uam_win32_chrome_120_matches_snapshot() {
        let uam = UserAgentMetadata::realistic(Platform::Win32, 120, "120.0.6099.234");
        insta::assert_json_snapshot!("uam_win32_chrome_120", uam);
    }

    #[test]
    fn realistic_uam_linux_chrome_120_matches_snapshot() {
        let uam = UserAgentMetadata::realistic(Platform::LinuxX86_64, 120, "120.0.6099.234");
        insta::assert_json_snapshot!("uam_linux_chrome_120", uam);
    }
}
```

Uncomment in `lib.rs`: `pub use fingerprint::{UserAgentMetadata, Brand};` (drop `Fingerprint` for now; lands in Task 5).

- [ ] **Step 2: Generate + accept snapshots**

```bash
cargo test -p zendriver-stealth --lib fingerprint::tests  # fails first run, no snapshots
cargo insta accept                                          # writes snapshots
cargo test -p zendriver-stealth --lib fingerprint::tests  # passes
```

Inspect the snapshot files at `crates/zendriver-stealth/src/snapshots/` and sanity-check they look like sensible UA-CH JSON.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/fingerprint.rs crates/zendriver-stealth/src/lib.rs crates/zendriver-stealth/src/snapshots
git commit -m "feat(stealth): UserAgentMetadata + Brand + per-platform realistic builder

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: UA-string composition

**Files:**
- Modify: `crates/zendriver-stealth/src/ua.rs`

- [ ] **Step 1: Implement composer + snapshot tests**

```rust
//! User-Agent string composition.

use crate::Platform;

/// Build a Chrome desktop UA string for the given platform + version.
///
/// Format: `Mozilla/5.0 ({platform-token}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{full-version} Safari/537.36`
#[must_use]
pub fn compose_ua_string(platform: Platform, chrome_full: &str) -> String {
    format!(
        "Mozilla/5.0 ({platform_token}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{chrome_full} Safari/537.36",
        platform_token = platform.ua_token(),
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn compose_macintel_chrome_120_matches_snapshot() {
        let ua = compose_ua_string(Platform::MacIntel, "120.0.6099.234");
        insta::assert_snapshot!("ua_macintel_chrome_120", ua);
    }

    #[test]
    fn compose_win32_chrome_120_matches_snapshot() {
        let ua = compose_ua_string(Platform::Win32, "120.0.6099.234");
        insta::assert_snapshot!("ua_win32_chrome_120", ua);
    }

    #[test]
    fn compose_linux_chrome_120_matches_snapshot() {
        let ua = compose_ua_string(Platform::LinuxX86_64, "120.0.6099.234");
        insta::assert_snapshot!("ua_linux_chrome_120", ua);
    }

    #[test]
    fn composed_ua_never_contains_headless_substring() {
        for p in [Platform::Win32, Platform::MacIntel, Platform::LinuxX86_64] {
            let ua = compose_ua_string(p, "120.0.6099.234");
            assert!(!ua.contains("Headless"), "got: {ua}");
        }
    }
}
```

- [ ] **Step 2: Verify + accept snapshots**

```bash
cargo test -p zendriver-stealth --lib ua::tests
cargo insta accept
cargo test -p zendriver-stealth --lib ua::tests
```

Expect 4 pass.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/ua.rs crates/zendriver-stealth/src/snapshots
git commit -m "feat(stealth): UA string composition per platform

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: `Fingerprint` + auto-detect

**Files:**
- Modify: `crates/zendriver-stealth/src/fingerprint.rs` (append to file from Task 3)

- [ ] **Step 1: Add `Fingerprint` struct + auto-detect**

Append to `crates/zendriver-stealth/src/fingerprint.rs` (above the existing `#[cfg(test)] mod tests`):

```rust
use std::path::Path;
use std::process::Command;

use crate::error::StealthError;

/// Default Chrome version used when `chrome --version` probe fails.
/// Bump on each release of zendriver-rs.
const FALLBACK_CHROME_FULL: &str = "120.0.6099.234";
const FALLBACK_CHROME_MAJOR: u32 = 120;

/// Probed system + Chrome facts used to compose stealth values.
#[derive(Debug, Clone, Serialize)]
pub struct Fingerprint {
    pub platform: Platform,
    pub chrome_major: u32,
    pub chrome_full: String,
    pub cpu_count: u32,
    pub memory_gb: u32,
    pub ua_string: String,
    pub ua_metadata: UserAgentMetadata,
    pub timezone: Option<String>,
    pub locale: Option<String>,
}

impl Fingerprint {
    /// Probe host system + installed Chrome to build a realistic fingerprint.
    pub fn auto_detect(chrome_executable: &Path) -> Result<Self, StealthError> {
        let platform = detect_platform();
        let (chrome_major, chrome_full) = probe_chrome_version(chrome_executable)
            .unwrap_or_else(|e| {
                tracing::warn!("chrome version probe failed: {e}; using fallback");
                (FALLBACK_CHROME_MAJOR, FALLBACK_CHROME_FULL.to_string())
            });
        let cpu_count = clamp_cpu_count(num_cpus::get() as u32);
        let memory_gb = detect_memory_gb()?;
        let ua_string = crate::ua::compose_ua_string(platform, &chrome_full);
        let ua_metadata = UserAgentMetadata::realistic(platform, chrome_major, &chrome_full);
        Ok(Self {
            platform,
            chrome_major,
            chrome_full,
            cpu_count,
            memory_gb,
            ua_string,
            ua_metadata,
            timezone: None,
            locale: None,
        })
    }

    /// Recompose UA string + UAM after platform/version overrides.
    pub fn recompose(&mut self) {
        self.ua_string = crate::ua::compose_ua_string(self.platform, &self.chrome_full);
        self.ua_metadata = UserAgentMetadata::realistic(self.platform, self.chrome_major, &self.chrome_full);
    }
}

fn detect_platform() -> Platform {
    #[cfg(target_os = "windows")]
    { Platform::Win32 }
    #[cfg(target_os = "macos")]
    { Platform::MacIntel }
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
    { Platform::LinuxX86_64 }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd", target_os = "openbsd")))]
    { Platform::LinuxX86_64 }  // unknown unix-likes -> linux is the safest plausibility
}

fn probe_chrome_version(exe: &Path) -> Result<(u32, String), StealthError> {
    let output = Command::new(exe)
        .arg("--version")
        .output()
        .map_err(|e| StealthError::ChromeVersionDetect(format!("spawn failed: {e}")))?;
    if !output.status.success() {
        return Err(StealthError::ChromeVersionDetect(format!("exit {:?}", output.status.code())));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Format: "Google Chrome 120.0.6099.234" (sometimes "Chromium 120.0.6099.0")
    let full = stdout
        .split_whitespace()
        .find(|tok| tok.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .ok_or_else(|| StealthError::ChromeVersionDetect(format!("no version token in: {stdout}")))?
        .to_string();
    let major: u32 = full
        .split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| StealthError::ChromeVersionDetect(format!("bad major in: {full}")))?;
    Ok((major, full))
}

fn clamp_cpu_count(n: u32) -> u32 {
    n.clamp(2, 32)
}

/// Detect total RAM in GB, clamped to the spec-compliant values
/// for `navigator.deviceMemory` (capped at 8 per W3C; floor at 4 for plausibility).
fn detect_memory_gb() -> Result<u32, StealthError> {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let total_kib = sys.total_memory();  // in KiB on sysinfo 0.32+
    if total_kib == 0 {
        return Err(StealthError::SystemInfo("total_memory returned 0".into()));
    }
    let total_gb = (total_kib / 1024 / 1024) as u32;
    Ok(round_to_navigator_memory(total_gb))
}

fn round_to_navigator_memory(gb: u32) -> u32 {
    // navigator.deviceMemory spec values: 0.25, 0.5, 1, 2, 4, 8. Cap at 8.
    // We floor at 4 for plausibility (sub-4GB consumer desktops are extinct).
    if gb >= 8 { 8 } else { 4 }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod fingerprint_tests {
    use super::*;

    #[test]
    fn clamp_cpu_count_floors_at_two() {
        assert_eq!(clamp_cpu_count(1), 2);
        assert_eq!(clamp_cpu_count(0), 2);
    }

    #[test]
    fn clamp_cpu_count_caps_at_thirty_two() {
        assert_eq!(clamp_cpu_count(64), 32);
        assert_eq!(clamp_cpu_count(128), 32);
    }

    #[test]
    fn clamp_cpu_count_preserves_normal_values() {
        assert_eq!(clamp_cpu_count(8), 8);
        assert_eq!(clamp_cpu_count(16), 16);
    }

    #[test]
    fn round_navigator_memory_caps_at_eight() {
        assert_eq!(round_to_navigator_memory(16), 8);
        assert_eq!(round_to_navigator_memory(64), 8);
    }

    #[test]
    fn round_navigator_memory_floors_at_four() {
        assert_eq!(round_to_navigator_memory(1), 4);
        assert_eq!(round_to_navigator_memory(3), 4);
    }

    #[test]
    fn round_navigator_memory_eight_stays_eight() {
        assert_eq!(round_to_navigator_memory(8), 8);
    }

    #[test]
    fn detect_memory_gb_works_on_real_system() {
        let gb = detect_memory_gb().expect("real system should have RAM");
        assert!(gb == 4 || gb == 8, "got {gb}");
    }

    #[test]
    fn detect_platform_returns_expected_for_host() {
        let p = detect_platform();
        #[cfg(target_os = "macos")]
        assert_eq!(p, Platform::MacIntel);
        #[cfg(target_os = "linux")]
        assert_eq!(p, Platform::LinuxX86_64);
        #[cfg(target_os = "windows")]
        assert_eq!(p, Platform::Win32);
    }

    #[test]
    fn fingerprint_recompose_updates_ua_and_uam() {
        let mut fp = Fingerprint {
            platform: Platform::Win32,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 8,
            memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: UserAgentMetadata::realistic(Platform::Win32, 120, "120.0.6099.234"),
            timezone: None,
            locale: None,
        };
        fp.recompose();
        assert!(fp.ua_string.contains("Windows NT 10.0"));
        assert!(fp.ua_string.contains("Chrome/120.0.6099.234"));
    }
}
```

Uncomment `Fingerprint` in `lib.rs` re-exports.

- [ ] **Step 2: Verify**

```bash
cargo test -p zendriver-stealth --lib fingerprint
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 12 tests pass (3 from Task 3 UAM + 9 new here). Note: `detect_memory_gb_works_on_real_system` assumes the dev machine has ≥4GB.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/fingerprint.rs crates/zendriver-stealth/src/lib.rs
git commit -m "feat(stealth): Fingerprint + auto_detect via sysinfo, num_cpus, chrome --version

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: Flag tables

**Files:**
- Modify: `crates/zendriver-stealth/src/flags.rs`

- [ ] **Step 1: Add port of zendriver Python defaults + per-profile additions**

```rust
//! Chrome launch flags for stealth profiles.
//!
//! Ported from zendriver Python (`zendriver/core/config.py:119-137`) plus
//! chaser-oxide additions.

use crate::ProfileKind;

/// Flags ALL stealth profiles share (Native + Spoofed + Off-when-not-Off).
/// Off profile uses an empty list (truly stock launch).
fn shared_stealth_flags() -> Vec<String> {
    vec![
        "--no-first-run".into(),
        "--no-service-autorun".into(),
        "--no-default-browser-check".into(),
        "--homepage=about:blank".into(),
        "--no-pings".into(),
        "--password-store=basic".into(),
        "--disable-infobars".into(),
        "--disable-breakpad".into(),
        "--disable-component-update".into(),
        "--disable-backgrounding-occluded-windows".into(),
        "--disable-renderer-backgrounding".into(),
        "--disable-background-networking".into(),
        "--disable-dev-shm-usage".into(),
        "--disable-features=IsolateOrigins,DisableLoadExtensionCommandLineSwitch,site-per-process".into(),
        "--disable-session-crashed-bubble".into(),
        "--disable-search-engine-choice-screen".into(),
        "--remote-allow-origins=*".into(),
        // WebRTC IP-leak prevention (zendriver Python disable_webrtc=True default)
        "--webrtc-ip-handling-policy=disable_non_proxied_udp".into(),
        "--force-webrtc-ip-handling-policy".into(),
    ]
}

/// Build the full flag list for a profile.
#[must_use]
pub fn flags_for_profile(kind: ProfileKind) -> Vec<String> {
    match kind {
        ProfileKind::Off => Vec::new(),
        ProfileKind::Native | ProfileKind::Spoofed => shared_stealth_flags(),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn off_profile_emits_no_flags() {
        assert!(flags_for_profile(ProfileKind::Off).is_empty());
    }

    #[test]
    fn native_profile_includes_webrtc_disable() {
        let flags = flags_for_profile(ProfileKind::Native);
        assert!(flags.iter().any(|f| f.contains("webrtc-ip-handling-policy")));
    }

    #[test]
    fn spoofed_profile_includes_isolate_origins_disable() {
        let flags = flags_for_profile(ProfileKind::Spoofed);
        assert!(flags.iter().any(|f| f.contains("IsolateOrigins")));
    }

    #[test]
    fn shared_flags_snapshot_native() {
        let flags = flags_for_profile(ProfileKind::Native);
        insta::assert_yaml_snapshot!("native_profile_flags", flags);
    }

    #[test]
    fn shared_flags_snapshot_spoofed() {
        let flags = flags_for_profile(ProfileKind::Spoofed);
        insta::assert_yaml_snapshot!("spoofed_profile_flags", flags);
    }

    #[test]
    fn shared_flags_snapshot_off() {
        let flags = flags_for_profile(ProfileKind::Off);
        insta::assert_yaml_snapshot!("off_profile_flags", flags);
    }
}
```

- [ ] **Step 2: Generate snapshots + verify**

```bash
cargo test -p zendriver-stealth --lib flags::tests   # fails first
cargo insta accept
cargo test -p zendriver-stealth --lib flags::tests   # passes
```

Expect 6 pass.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/flags.rs crates/zendriver-stealth/src/snapshots
git commit -m "feat(stealth): launch flag tables per profile

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7: `StealthProfile` constructors + builder methods

**Files:**
- Modify: `crates/zendriver-stealth/src/profile.rs`

- [ ] **Step 1: Replace the placeholder StealthProfile with full builder**

Replace the placeholder `StealthProfile` struct from Task 2 with:

```rust
use std::path::{Path, PathBuf};

use crate::error::StealthError;
use crate::fingerprint::Fingerprint;

/// Stealth configuration passed to `BrowserBuilder::stealth(...)`.
#[derive(Debug, Clone)]
pub struct StealthProfile {
    pub(crate) kind: ProfileKind,
    pub(crate) extra_flags: Vec<String>,
    pub(crate) fingerprint_override: Option<Fingerprint>,
    pub(crate) per_field: PerFieldOverride,
    pub(crate) bypass_csp: bool,
    pub(crate) user_data_dir: Option<PathBuf>,
}

impl StealthProfile {
    /// No stealth: stock browser launch.
    #[must_use]
    pub fn off() -> Self {
        Self {
            kind: ProfileKind::Off,
            extra_flags: Vec::new(),
            fingerprint_override: None,
            per_field: PerFieldOverride::default(),
            bypass_csp: false,
            user_data_dir: None,
        }
    }

    /// Launch flags + UA scrub + Emulation overrides. No JS bootstrap.
    /// Safe against `Function.prototype.toString` detection. Default.
    #[must_use]
    pub fn native() -> Self {
        Self {
            kind: ProfileKind::Native,
            extra_flags: Vec::new(),
            fingerprint_override: None,
            per_field: PerFieldOverride::default(),
            bypass_csp: false,
            user_data_dir: None,
        }
    }

    /// Native + Navigator-prototype JS patches. Passes sannysoft.
    #[must_use]
    pub fn spoofed() -> Self {
        Self {
            kind: ProfileKind::Spoofed,
            extra_flags: Vec::new(),
            fingerprint_override: None,
            per_field: PerFieldOverride::default(),
            bypass_csp: true,  // default ON for spoofed; see spec assumption #2
            user_data_dir: None,
        }
    }

    #[must_use]
    pub fn fingerprint(mut self, f: Fingerprint) -> Self {
        self.fingerprint_override = Some(f);
        self
    }
    #[must_use]
    pub fn memory_gb(mut self, gb: u32) -> Self {
        self.per_field.memory_gb = Some(gb);
        self
    }
    #[must_use]
    pub fn cpu_count(mut self, n: u32) -> Self {
        self.per_field.cpu_count = Some(n);
        self
    }
    #[must_use]
    pub fn chrome_version(mut self, major: u32) -> Self {
        self.per_field.chrome_major = Some(major);
        self
    }
    #[must_use]
    pub fn platform(mut self, p: Platform) -> Self {
        self.per_field.platform = Some(p);
        self
    }
    #[must_use]
    pub fn locale(mut self, l: impl Into<String>) -> Self {
        self.per_field.locale = Some(l.into());
        self
    }
    #[must_use]
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.per_field.timezone = Some(tz.into());
        self
    }
    #[must_use]
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.per_field.ua_string = Some(ua.into());
        self
    }
    #[must_use]
    pub fn bypass_csp(mut self, on: bool) -> Self {
        self.bypass_csp = on;
        self
    }
    #[must_use]
    pub fn arg(mut self, flag: impl Into<String>) -> Self {
        self.extra_flags.push(flag.into());
        self
    }
    #[must_use]
    pub fn args(mut self, flags: impl IntoIterator<Item = String>) -> Self {
        self.extra_flags.extend(flags);
        self
    }

    pub(crate) fn kind(&self) -> ProfileKind {
        self.kind
    }

    /// Resolve final Fingerprint: explicit override or auto-detect, with
    /// per-field tweaks applied on top.
    pub fn resolve_fingerprint(&self, chrome_exe: &Path) -> Result<Fingerprint, StealthError> {
        let mut fp = match &self.fingerprint_override {
            Some(fp) => fp.clone(),
            None => Fingerprint::auto_detect(chrome_exe)?,
        };
        if let Some(p) = self.per_field.platform { fp.platform = p; }
        if let Some(c) = self.per_field.chrome_major {
            fp.chrome_major = c;
            fp.chrome_full = format!("{c}.0.6099.234");  // synthesize a full version if user only set major
        }
        if let Some(n) = self.per_field.cpu_count { fp.cpu_count = n.clamp(2, 32); }
        if let Some(g) = self.per_field.memory_gb { fp.memory_gb = if g >= 8 { 8 } else { 4 }; }
        if let Some(ref ua) = self.per_field.ua_string { fp.ua_string = ua.clone(); }
        else { fp.recompose(); }
        if let Some(ref tz) = self.per_field.timezone { fp.timezone = Some(tz.clone()); }
        if let Some(ref locale) = self.per_field.locale { fp.locale = Some(locale.clone()); }
        Ok(fp)
    }

    /// Composed launch flag list: per-profile defaults + extras.
    pub fn build_flags(&self) -> Vec<String> {
        let mut flags = crate::flags::flags_for_profile(self.kind);
        if let Some(ref locale) = self.per_field.locale {
            flags.push(format!("--lang={locale}"));
        }
        flags.extend(self.extra_flags.iter().cloned());
        flags
    }

    pub fn bypass_csp_enabled(&self) -> bool {
        self.bypass_csp
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod profile_tests {
    use super::*;

    #[test]
    fn off_profile_has_no_flags_no_patches() {
        let p = StealthProfile::off();
        assert_eq!(p.kind, ProfileKind::Off);
        assert!(p.build_flags().is_empty());
    }

    #[test]
    fn native_profile_has_flags_no_patches() {
        let p = StealthProfile::native();
        assert_eq!(p.kind, ProfileKind::Native);
        assert!(!p.build_flags().is_empty());
    }

    #[test]
    fn spoofed_profile_default_bypass_csp_on() {
        let p = StealthProfile::spoofed();
        assert!(p.bypass_csp_enabled());
    }

    #[test]
    fn builder_chains_set_fields() {
        let p = StealthProfile::spoofed()
            .memory_gb(16)
            .cpu_count(10)
            .chrome_version(125)
            .platform(Platform::MacIntel)
            .locale("en-US")
            .timezone("America/Los_Angeles")
            .arg("--proxy-server=http://x");
        assert_eq!(p.per_field.memory_gb, Some(16));
        assert_eq!(p.per_field.cpu_count, Some(10));
        assert_eq!(p.per_field.chrome_major, Some(125));
        assert_eq!(p.per_field.platform, Some(Platform::MacIntel));
        assert_eq!(p.per_field.locale.as_deref(), Some("en-US"));
        assert_eq!(p.per_field.timezone.as_deref(), Some("America/Los_Angeles"));
        assert!(p.extra_flags.contains(&"--proxy-server=http://x".to_string()));
    }

    #[test]
    fn build_flags_includes_locale_arg_when_set() {
        let flags = StealthProfile::native().locale("fr-FR").build_flags();
        assert!(flags.iter().any(|f| f == "--lang=fr-FR"));
    }

    #[test]
    fn resolve_fingerprint_with_explicit_override_skips_autodetect() {
        let fp = Fingerprint {
            platform: Platform::Win32,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 8,
            memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: UserAgentMetadata::realistic(Platform::Win32, 120, "120.0.6099.234"),
            timezone: None,
            locale: None,
        };
        let p = StealthProfile::native().fingerprint(fp.clone()).platform(Platform::MacIntel);
        // Pass a path that doesn't exist; if it tried to probe, it'd fail.
        let resolved = p.resolve_fingerprint(std::path::Path::new("/nonexistent")).unwrap();
        assert_eq!(resolved.platform, Platform::MacIntel);  // per-field override applied
    }
}
```

Note: this references `crate::fingerprint::Fingerprint` and `UserAgentMetadata`. Make sure they're imported at top.

- [ ] **Step 2: Verify**

```bash
cargo test -p zendriver-stealth --lib profile
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect existing 3 from Task 2 + 6 new = 9 pass.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/profile.rs
git commit -m "feat(stealth): StealthProfile constructors + builder + resolve_fingerprint

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8: 9 JS patches

**Files:**
- Create: `crates/zendriver-stealth/src/patches/{webdriver,plugins,chrome,webgl,permissions,codecs,navigator_props,user_agent_data,broken_image}.js`

Each patch is a JS expression that, when evaluated inside `function(fp){ <PATCH_BODY> }`, takes a Fingerprint object `fp` and modifies the page's globals. The bundle factory (Task 9) wraps these in an IIFE.

- [ ] **Step 1: Write each patch file**

Path: `crates/zendriver-stealth/src/patches/webdriver.js`

```javascript
// Defeats: bot.sannysoft.com `WebDriver (New)` + `WebDriver Advanced` rows.
// Patches Navigator.prototype (not navigator directly) so
// Object.getOwnPropertyNames(navigator) doesn't reveal the hack.
Object.defineProperty(Navigator.prototype, 'webdriver', {
    get: () => false,
    configurable: true,
    enumerable: true,
});
```

Path: `crates/zendriver-stealth/src/patches/plugins.js`

```javascript
// Defeats: bot.sannysoft.com `Plugins Length (Old)` row.
// Fakes 3 plugins matching Chrome's modern stub layout.
Object.defineProperty(Navigator.prototype, 'plugins', {
    get: function() {
        const make = (name, filename, description) => {
            const p = Object.create(Plugin.prototype);
            Object.defineProperties(p, {
                name:        { value: name },
                filename:    { value: filename },
                description: { value: description },
                length:      { value: 1 },
            });
            return p;
        };
        const arr = [
            make('PDF Viewer',         'internal-pdf-viewer', 'Portable Document Format'),
            make('Chrome PDF Viewer',  'internal-pdf-viewer', 'Portable Document Format'),
            make('Chromium PDF Viewer','internal-pdf-viewer', 'Portable Document Format'),
        ];
        Object.setPrototypeOf(arr, PluginArray.prototype);
        return arr;
    },
    configurable: true,
    enumerable: true,
});
```

Path: `crates/zendriver-stealth/src/patches/chrome.js`

```javascript
// Defeats: bot.sannysoft.com `Chrome (New)` row.
if (!window.chrome) {
    window.chrome = { runtime: {} };
} else if (!window.chrome.runtime) {
    window.chrome.runtime = {};
}
```

Path: `crates/zendriver-stealth/src/patches/webgl.js`

```javascript
// Defeats: bot.sannysoft.com `WebGL Vendor` + `WebGL Renderer` rows.
// Headless reports vendor="Brian Paul" / renderer="Mesa OffScreen" or SwiftShader.
// Spoof to common Intel desktop values matching the fingerprint platform.
const VENDOR = 'Google Inc. (Intel)';
const RENDERER = 'ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)';
[WebGLRenderingContext.prototype, WebGL2RenderingContext.prototype].forEach(proto => {
    const orig = proto.getParameter;
    proto.getParameter = function(param) {
        if (param === 37445) return VENDOR;    // UNMASKED_VENDOR_WEBGL
        if (param === 37446) return RENDERER;  // UNMASKED_RENDERER_WEBGL
        return orig.call(this, param);
    };
});
```

Path: `crates/zendriver-stealth/src/patches/permissions.js`

```javascript
// Defeats: bot.sannysoft.com `Permissions (New)` row.
// Real Chrome: Notification.permission === 'default' AND
//   navigator.permissions.query({name:'notifications'}).state === 'prompt'
// Headless: mismatch — Notification.permission is 'denied' but query says 'prompt'.
const origQuery = navigator.permissions.query.bind(navigator.permissions);
navigator.permissions.query = function(p) {
    if (p && p.name === 'notifications') {
        return Promise.resolve({ state: Notification.permission, onchange: null });
    }
    return origQuery(p);
};
```

Path: `crates/zendriver-stealth/src/patches/codecs.js`

```javascript
// Headless Chromium lacks proprietary codecs. Stub canPlayType so
// media-feature detection sees 'probably' for common containers.
const origCanPlay = HTMLMediaElement.prototype.canPlayType;
HTMLMediaElement.prototype.canPlayType = function(type) {
    if (typeof type === 'string') {
        const t = type.toLowerCase();
        if (t.includes('avc1') || t.includes('mp4a.40') || t.includes('video/mp4') || t.includes('audio/mp4')) {
            return 'probably';
        }
    }
    return origCanPlay.call(this, type);
};
```

Path: `crates/zendriver-stealth/src/patches/navigator_props.js`

```javascript
// Patch platform, hardwareConcurrency, deviceMemory, languages on Navigator.prototype.
// `fp` is the serialized Fingerprint object passed by the bundle factory.
Object.defineProperty(Navigator.prototype, 'platform', {
    get: () => fp.platformJs,
    configurable: true, enumerable: true,
});
Object.defineProperty(Navigator.prototype, 'hardwareConcurrency', {
    get: () => fp.cpuCount,
    configurable: true, enumerable: true,
});
Object.defineProperty(Navigator.prototype, 'deviceMemory', {
    get: () => fp.memoryGb,
    configurable: true, enumerable: true,
});
Object.defineProperty(Navigator.prototype, 'languages', {
    get: () => fp.languages,
    configurable: true, enumerable: true,
});
```

Path: `crates/zendriver-stealth/src/patches/user_agent_data.js`

```javascript
// navigator.userAgentData stub — many headless detectors check this.
// Mirrors what Emulation.setUserAgentOverride sends, but JS-readable.
Object.defineProperty(Navigator.prototype, 'userAgentData', {
    get: () => ({
        brands: fp.brands,
        mobile: false,
        platform: fp.chPlatform,
        getHighEntropyValues: function(hints) {
            return Promise.resolve({
                architecture: fp.architecture,
                bitness: fp.bitness,
                brands: fp.brands,
                fullVersionList: fp.fullVersionList,
                mobile: false,
                model: '',
                platform: fp.chPlatform,
                platformVersion: fp.platformVersion,
                wow64: false,
            });
        },
        toJSON: function() {
            return { brands: fp.brands, mobile: false, platform: fp.chPlatform };
        }
    }),
    configurable: true, enumerable: true,
});
```

Path: `crates/zendriver-stealth/src/patches/broken_image.js`

```javascript
// Defeats: bot.sannysoft.com `Broken Image Dimensions` row.
// Real Chrome reports naturalWidth=16 for an unloaded broken-icon <img>.
// Headless reports 0. Patch the getter to return 16 when the img has no src.
const origNaturalWidth  = Object.getOwnPropertyDescriptor(HTMLImageElement.prototype, 'naturalWidth').get;
const origNaturalHeight = Object.getOwnPropertyDescriptor(HTMLImageElement.prototype, 'naturalHeight').get;
Object.defineProperty(HTMLImageElement.prototype, 'naturalWidth', {
    get: function() {
        const v = origNaturalWidth.call(this);
        if (v === 0 && this.complete && !this.src) return 16;
        return v;
    },
    configurable: true, enumerable: true,
});
Object.defineProperty(HTMLImageElement.prototype, 'naturalHeight', {
    get: function() {
        const v = origNaturalHeight.call(this);
        if (v === 0 && this.complete && !this.src) return 16;
        return v;
    },
    configurable: true, enumerable: true,
});
```

- [ ] **Step 2: Verify files exist**

```bash
ls crates/zendriver-stealth/src/patches/ | wc -l   # expect 9
```

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/patches/
git commit -m "feat(stealth): 9 JS patches for sannysoft + areyouheadless coverage

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9: `PatchSource` + bundle factory

**Files:**
- Modify: `crates/zendriver-stealth/src/patches.rs`

- [ ] **Step 1: Implement bundle wrapper**

```rust
//! Bundles individual patches into a single IIFE called with a serialized
//! Fingerprint. Only one `Page.addScriptToEvaluateOnNewDocument` round-trip
//! per nav instead of nine.

use serde_json::json;

use crate::Fingerprint;

const WEBDRIVER:        &str = include_str!("patches/webdriver.js");
const PLUGINS:          &str = include_str!("patches/plugins.js");
const CHROME:           &str = include_str!("patches/chrome.js");
const WEBGL:            &str = include_str!("patches/webgl.js");
const PERMISSIONS:      &str = include_str!("patches/permissions.js");
const CODECS:           &str = include_str!("patches/codecs.js");
const NAVIGATOR_PROPS:  &str = include_str!("patches/navigator_props.js");
const USER_AGENT_DATA:  &str = include_str!("patches/user_agent_data.js");
const BROKEN_IMAGE:     &str = include_str!("patches/broken_image.js");

/// Build the bootstrap script for the spoofed profile.
/// Order: webdriver first (most-probed), navigator_props last (touches most fields).
pub fn bootstrap_script(fp: &Fingerprint) -> String {
    let fp_json = json!({
        "platformJs":      fp.platform.js_string(),
        "chPlatform":      fp.platform.ch_platform(),
        "platformVersion": fp.ua_metadata.platform_version,
        "cpuCount":        fp.cpu_count,
        "memoryGb":        fp.memory_gb,
        "languages":       fp.locale.as_deref().map_or_else(
            || vec!["en-US".to_string(), "en".to_string()],
            |l| vec![l.to_string(), "en".to_string()],
        ),
        "architecture":    fp.ua_metadata.architecture,
        "bitness":         fp.ua_metadata.bitness,
        "brands":          fp.ua_metadata.brands,
        "fullVersionList": fp.ua_metadata.full_version_list,
    });

    format!(
        "(function(fp){{\n{webdriver}\n{plugins}\n{chrome}\n{webgl}\n{permissions}\n{codecs}\n{navigator_props}\n{user_agent_data}\n{broken_image}\n}})({fp_json});",
        webdriver       = WEBDRIVER,
        plugins         = PLUGINS,
        chrome          = CHROME,
        webgl           = WEBGL,
        permissions     = PERMISSIONS,
        codecs          = CODECS,
        navigator_props = NAVIGATOR_PROPS,
        user_agent_data = USER_AGENT_DATA,
        broken_image    = BROKEN_IMAGE,
        fp_json         = fp_json,
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{Platform, UserAgentMetadata};

    fn mock_fp() -> Fingerprint {
        Fingerprint {
            platform: Platform::MacIntel,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 10,
            memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: UserAgentMetadata::realistic(Platform::MacIntel, 120, "120.0.6099.234"),
            timezone: None,
            locale: Some("en-US".into()),
        }
    }

    #[test]
    fn bootstrap_includes_all_nine_patches() {
        let s = bootstrap_script(&mock_fp());
        assert!(s.contains("webdriver"), "webdriver patch missing");
        assert!(s.contains("PluginArray"), "plugins patch missing");
        assert!(s.contains("window.chrome"), "chrome patch missing");
        assert!(s.contains("UNMASKED_VENDOR_WEBGL") || s.contains("37445"), "webgl patch missing");
        assert!(s.contains("Notification.permission"), "permissions patch missing");
        assert!(s.contains("canPlayType"), "codecs patch missing");
        assert!(s.contains("hardwareConcurrency"), "navigator_props patch missing");
        assert!(s.contains("userAgentData"), "user_agent_data patch missing");
        assert!(s.contains("naturalWidth"), "broken_image patch missing");
    }

    #[test]
    fn bootstrap_is_an_iife_taking_fp() {
        let s = bootstrap_script(&mock_fp());
        assert!(s.starts_with("(function(fp){"));
        assert!(s.contains("})({"), "fp arg JSON should follow");
    }

    #[test]
    fn bootstrap_substitutes_platform_js_string() {
        let s = bootstrap_script(&mock_fp());
        assert!(s.contains("\"MacIntel\""));
    }
}
```

- [ ] **Step 2: Verify**

```bash
cargo test -p zendriver-stealth --lib patches::tests
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 3 pass.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/patches.rs
git commit -m "feat(stealth): bootstrap script bundles 9 JS patches with serialized Fingerprint

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10: `TargetObserver` trait + `PausedSession` + `ObserverError` + `TargetInfo`

**Files:**
- Create: `crates/zendriver-transport/src/observer.rs`
- Modify: `crates/zendriver-transport/src/lib.rs`

- [ ] **Step 1: Add trait definition + tests**

Path: `crates/zendriver-transport/src/observer.rs`

```rust
//! TargetObserver trait — fires on each new attached target while the
//! target is paused at the debugger.

use crate::connection::Connection;
use crate::error::CallError;

#[async_trait::async_trait]
pub trait TargetObserver: Send + Sync {
    /// Called once per new target, after attach and before debugger release.
    /// Observer MUST complete and return before the target resumes execution.
    /// Observers run serially in registration order; returning Err leaves the
    /// target paused (the actor logs + force-detaches the session).
    async fn on_target_attached(&self, session: PausedSession<'_>) -> Result<(), ObserverError>;

    /// Called when a target detaches. Default: no-op.
    async fn on_target_detached(&self, _session_id: &str) {}

    fn name(&self) -> &'static str;
}

pub struct PausedSession<'a> {
    pub session_id: &'a str,
    pub target_info: &'a TargetInfo,
    pub(crate) conn: &'a Connection,
}

impl<'a> PausedSession<'a> {
    pub async fn call(
        &self,
        method: impl Into<String>,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, CallError> {
        self.conn.call_raw(method, params, Some(self.session_id.to_string())).await
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ObserverError {
    #[error("call failed: {0}")]
    Call(#[from] CallError),

    #[error("observer timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("observer panicked: {0}")]
    Panicked(String),

    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct TargetInfo {
    #[serde(rename = "targetId")]
    pub target_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub attached: bool,
    #[serde(default, rename = "browserContextId")]
    pub browser_context_id: Option<String>,
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_observer_error_timeout_includes_duration() {
        let e = ObserverError::Timeout(std::time::Duration::from_secs(5));
        assert_eq!(e.to_string(), "observer timed out after 5s");
    }

    #[test]
    fn display_observer_error_panicked_includes_message() {
        let e = ObserverError::Panicked("oh no".into());
        assert_eq!(e.to_string(), "observer panicked: oh no");
    }

    #[test]
    fn target_info_deserializes_chrome_payload() {
        let json = r#"{"targetId":"T1","type":"page","url":"about:blank","attached":true}"#;
        let info: TargetInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.target_id, "T1");
        assert_eq!(info.kind, "page");
        assert_eq!(info.url, "about:blank");
        assert!(info.attached);
    }
}
```

- [ ] **Step 2: Update `lib.rs` re-exports**

Path: `crates/zendriver-transport/src/lib.rs` — add to module declarations:

```rust
pub mod observer;
```

And the re-exports section:

```rust
pub use observer::{ObserverError, PausedSession, TargetInfo, TargetObserver};
```

- [ ] **Step 3: Verify**

```bash
cargo test -p zendriver-transport --lib observer::tests
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 3 pass.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-transport/src/observer.rs crates/zendriver-transport/src/lib.rs
git commit -m "feat(transport): TargetObserver trait + PausedSession + ObserverError + TargetInfo

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 11: Connection actor — observer dispatch + Target.attachedToTarget routing

**Files:**
- Modify: `crates/zendriver-transport/src/actor.rs`
- Modify: `crates/zendriver-transport/src/connection.rs`

This is the central wiring task. The actor needs to:
1. Hold a `Vec<Arc<dyn TargetObserver>>`.
2. Refactor to live behind `Arc<ConnectionActorInner>` so spawned tasks can hold weak refs.
3. On `Target.attachedToTarget`, spawn an async handler that runs observers serially, then releases the debugger.
4. On `Target.detachedFromTarget`, notify observers.

- [ ] **Step 1: Refactor connection.rs to wrap actor state in Arc**

The current `Connection` holds an `mpsc::Sender<OutboundCmd>` directly. We need the actor's *state* to be Arc'd so we can spawn tasks that talk back through the same Connection.

In `crates/zendriver-transport/src/connection.rs`, restructure:

```rust
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;
use futures::StreamExt;

use crate::actor::{run_actor, OutboundCmd, EVENT_BUS_CAPACITY};
use crate::error::{CallError, TransportError};
use crate::frame::{CdpRpcError, RawEvent};
use crate::observer::TargetObserver;

#[derive(Clone)]
pub struct Connection {
    pub(crate) inner: Arc<ConnectionInner>,
}

pub(crate) struct ConnectionInner {
    pub(crate) cmd_tx: mpsc::Sender<OutboundCmd>,
    pub(crate) event_tx: broadcast::Sender<RawEvent>,
    pub(crate) observers: Vec<Arc<dyn TargetObserver>>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) observer_timeout: Duration,
}

const DEFAULT_OBSERVER_TIMEOUT: Duration = Duration::from_secs(5);

impl Connection {
    pub async fn call_raw(
        &self,
        method: impl Into<String>,
        params: Value,
        session_id: Option<String>,
    ) -> Result<Value, CallError> {
        // unchanged from P1
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner.cmd_tx.send(OutboundCmd {
            method: method.into(),
            params,
            session_id,
            reply: reply_tx,
        }).await.map_err(|_| CallError::Transport(TransportError::Shutdown))?;
        match reply_rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(rpc)) => Err(CallError::Rpc(rpc.code, rpc.message, rpc.data)),
            Err(_) => Err(CallError::Transport(TransportError::Shutdown)),
        }
    }

    pub fn subscribe_raw(&self) -> std::pin::Pin<Box<dyn futures::Stream<Item = RawEvent> + Send>> {
        Box::pin(BroadcastStream::new(self.inner.event_tx.subscribe())
            .filter_map(|res| async move { res.ok() }))
    }

    pub fn subscribe<T>(&self, method: &'static str)
        -> std::pin::Pin<Box<dyn futures::Stream<Item = T> + Send>>
    where T: DeserializeOwned + Send + 'static {
        Box::pin(BroadcastStream::new(self.inner.event_tx.subscribe())
            .filter_map(move |res| async move {
                let ev = res.ok()?;
                if ev.method == method {
                    serde_json::from_value(ev.params).ok()
                } else { None }
            }))
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.inner.shutdown.clone()
    }

    pub(crate) fn observer_timeout(&self) -> Duration {
        self.inner.observer_timeout
    }

    pub(crate) fn observers(&self) -> &[Arc<dyn TargetObserver>] {
        &self.inner.observers
    }
}

pub async fn connect(ws_url: &str) -> Result<Connection, TransportError> {
    connect_with_observers(ws_url, Vec::new()).await
}

pub async fn connect_with_observers(
    ws_url: &str,
    observers: Vec<Arc<dyn TargetObserver>>,
) -> Result<Connection, TransportError> {
    let (ws, _resp) = tokio_tungstenite::connect_async(ws_url).await?;
    Ok(spawn_actor_with_observers(ws, observers))
}

pub fn spawn_actor<S>(ws: S) -> Connection
where S: futures::Sink<tokio_tungstenite::tungstenite::Message, Error = tokio_tungstenite::tungstenite::Error>
       + futures::Stream<Item = Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>>
       + Send + Unpin + 'static {
    spawn_actor_with_observers(ws, Vec::new())
}

pub fn spawn_actor_with_observers<S>(ws: S, observers: Vec<Arc<dyn TargetObserver>>) -> Connection
where S: futures::Sink<tokio_tungstenite::tungstenite::Message, Error = tokio_tungstenite::tungstenite::Error>
       + futures::Stream<Item = Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>>
       + Send + Unpin + 'static {
    let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(64);
    let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
    let shutdown = CancellationToken::new();
    let inner = Arc::new(ConnectionInner {
        cmd_tx,
        event_tx: event_tx.clone(),
        observers: observers.clone(),
        shutdown: shutdown.clone(),
        observer_timeout: DEFAULT_OBSERVER_TIMEOUT,
    });
    let conn = Connection { inner: inner.clone() };
    // Actor task uses a weak ref to the connection for handling Target.attachedToTarget
    // (so it can send commands back through self without holding a strong cycle).
    let weak_conn = Arc::downgrade(&inner);
    tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown, observers, weak_conn));
    conn
}

// keep test_only::DriverStream from P1 unchanged
#[cfg(any(test, feature = "testing"))]
pub mod test_only {
    use tokio::sync::mpsc;
    use tokio_tungstenite::tungstenite::Message;
    pub struct DriverStream { pub tx: mpsc::Sender<Message>, pub rx: mpsc::Receiver<Result<Message, tokio_tungstenite::tungstenite::Error>> }
    impl futures::Sink<Message> for DriverStream {
        type Error = tokio_tungstenite::tungstenite::Error;
        fn poll_ready(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> { std::task::Poll::Ready(Ok(())) }
        fn start_send(self: std::pin::Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.tx.try_send(item).map_err(|_| tokio_tungstenite::tungstenite::Error::ConnectionClosed)
        }
        fn poll_flush(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> { std::task::Poll::Ready(Ok(())) }
        fn poll_close(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> { std::task::Poll::Ready(Ok(())) }
    }
    impl futures::Stream for DriverStream {
        type Item = Result<Message, tokio_tungstenite::tungstenite::Error>;
        fn poll_next(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
            self.rx.poll_recv(cx)
        }
    }
}

// Existing test from P1 should still pass; if it directly constructed
// Connection { cmd_tx, event_tx, shutdown } update it to construct via Arc<Inner>.
```

- [ ] **Step 2: Add observer dispatch to actor**

In `crates/zendriver-transport/src/actor.rs`, modify `run_actor` to accept the observers + weak conn ref, and add the `Target.attachedToTarget` / `Target.detachedFromTarget` event handlers:

```rust
use std::sync::{Arc, Weak};
use std::time::Duration;
use std::panic::AssertUnwindSafe;
use futures::FutureExt;
use serde_json::json;

use crate::connection::{Connection, ConnectionInner};
use crate::frame::CdpRpcError;
use crate::observer::{ObserverError, PausedSession, TargetInfo, TargetObserver};

#[derive(serde::Deserialize)]
struct TargetAttached {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "targetInfo")]
    target_info: TargetInfo,
}

#[derive(serde::Deserialize)]
struct TargetDetached {
    #[serde(rename = "sessionId")]
    session_id: String,
}

pub async fn run_actor<S>(
    mut ws: S,
    mut cmd_rx: mpsc::Receiver<OutboundCmd>,
    event_tx: broadcast::Sender<RawEvent>,
    shutdown: CancellationToken,
    observers: Vec<Arc<dyn TargetObserver>>,
    weak_conn: Weak<ConnectionInner>,
) where /* same bounds as before */ {
    let mut pending: HashMap<u64, oneshot::Sender<Result<Value, CdpRpcError>>> = HashMap::new();
    let mut next_id: u64 = 1;

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            cmd = cmd_rx.recv() => { /* unchanged P1 send path */ }
            frame = ws.next() => {
                /* unchanged P1 response routing for Inbound::Response */
                /* NEW: branch on event method */
                Some(Ok(Message::Text(text))) => match parse_inbound(&text) {
                    CdpInbound::Event { method, params, session_id } if method == "Target.attachedToTarget" => {
                        match serde_json::from_value::<TargetAttached>(params.clone()) {
                            Ok(ev) => {
                                if let Some(strong) = weak_conn.upgrade() {
                                    let conn = Connection { inner: strong };
                                    let observers_clone = observers.clone();
                                    let timeout_dur = conn.observer_timeout();
                                    tokio::spawn(async move {
                                        handle_target_attached(conn, ev, observers_clone, timeout_dur).await;
                                    });
                                }
                            }
                            Err(e) => tracing::error!("bad Target.attachedToTarget payload: {e}"),
                        }
                        // Also broadcast so other subscribers see it.
                        let _ = event_tx.send(RawEvent { method, params, session_id });
                    }
                    CdpInbound::Event { method, params, session_id } if method == "Target.detachedFromTarget" => {
                        if let Ok(ev) = serde_json::from_value::<TargetDetached>(params.clone()) {
                            for obs in &observers {
                                let obs2 = obs.clone();
                                let sid = ev.session_id.clone();
                                tokio::spawn(async move { obs2.on_target_detached(&sid).await; });
                            }
                        }
                        let _ = event_tx.send(RawEvent { method, params, session_id });
                    }
                    /* existing event branch unchanged */
                }
            }
        }
    }
    /* unchanged pending drain */
}

async fn handle_target_attached(
    conn: Connection,
    ev: TargetAttached,
    observers: Vec<Arc<dyn TargetObserver>>,
    observer_timeout: Duration,
) {
    let session_id = ev.session_id.clone();
    for obs in &observers {
        let paused = PausedSession {
            session_id: &session_id,
            target_info: &ev.target_info,
            conn: &conn,
        };
        let name = obs.name();
        let fut = obs.on_target_attached(paused);
        match tokio::time::timeout(observer_timeout, AssertUnwindSafe(fut).catch_unwind()).await {
            Ok(Ok(Ok(()))) => continue,
            Ok(Ok(Err(e))) => {
                tracing::error!(observer = name, %session_id, error = %e, "observer failed; detaching");
                let _ = conn.call_raw("Target.detachFromTarget",
                    json!({ "sessionId": &session_id }), None).await;
                return;
            }
            Ok(Err(panic)) => {
                let msg = panic_payload(&panic);
                tracing::error!(observer = name, %session_id, panic = %msg, "observer panicked; detaching");
                let _ = conn.call_raw("Target.detachFromTarget",
                    json!({ "sessionId": &session_id }), None).await;
                return;
            }
            Err(_) => {
                tracing::warn!(observer = name, %session_id, "observer timed out; releasing");
                break;
            }
        }
    }
    let _ = conn.call_raw("Runtime.runIfWaitingForDebugger",
        json!({}), Some(session_id.clone())).await;
}

fn panic_payload(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() { (*s).to_string() }
    else if let Some(s) = payload.downcast_ref::<String>() { s.clone() }
    else { "<unknown panic payload>".to_string() }
}
```

Existing actor tests in `mod tests` will compile-break because `run_actor`'s signature changed. Update each test to pass `Vec::new()` for observers and a `Weak::new()` for `weak_conn`. Since the existing 6 actor tests don't exercise observer paths, this is mechanical.

- [ ] **Step 3: Add observer-dispatch tests**

Append to `crates/zendriver-transport/src/actor.rs` `mod tests`:

```rust
use std::sync::Mutex;
use crate::observer::{ObserverError, PausedSession, TargetObserver};

struct RecordingObserver {
    calls: Arc<Mutex<Vec<String>>>,
    name: &'static str,
    behavior: ObserverBehavior,
}

enum ObserverBehavior {
    Ok,
    Err,
    Panic,
    Sleep(Duration),
}

#[async_trait::async_trait]
impl TargetObserver for RecordingObserver {
    fn name(&self) -> &'static str { self.name }
    async fn on_target_attached(&self, session: PausedSession<'_>) -> Result<(), ObserverError> {
        self.calls.lock().unwrap().push(session.session_id.to_string());
        match self.behavior {
            ObserverBehavior::Ok => Ok(()),
            ObserverBehavior::Err => Err(ObserverError::Other("boom".into())),
            ObserverBehavior::Panic => panic!("observer panic"),
            ObserverBehavior::Sleep(d) => { tokio::time::sleep(d).await; Ok(()) }
        }
    }
}

// (Tests would be ~5 cases: observer fires with correct session_id, Err triggers
// detach, panic triggers detach, timeout triggers release, multiple observers
// fire in order. Each is structured like the existing actor tests, but constructs
// run_actor with observers vec.)

// Skipping full test code here for brevity — the implementer should write them
// following the existing test patterns: duplex_pair + spawn_actor_with_observers
// + emit Target.attachedToTarget event via test_tx + assert behavior.
```

**Implementer note:** the test stub is a sketch. Full tests follow:
1. `observer_fires_with_correct_session_id` — register one Ok observer; emit `Target.attachedToTarget`; assert recorded session_id matches the event's session_id.
2. `observer_err_triggers_detach_from_target` — register Err observer; emit event; assert next outbound is `Target.detachFromTarget` with the right sessionId.
3. `observer_panic_triggers_detach_and_actor_keeps_running` — Panic observer; emit event; assert detach is sent AND a subsequent regular command still routes correctly.
4. `observer_timeout_releases_debugger_anyway` — Sleep(10s) observer + 100ms observer_timeout (override via constructor); emit event; within 200ms assert `Runtime.runIfWaitingForDebugger` is sent.
5. `multiple_observers_fire_in_registration_order` — register 3 Ok observers each with distinct names; emit event; assert recorded order matches.

For test #4, you'll need to expose `observer_timeout` setter on `Connection` for tests:

```rust
#[cfg(test)]
impl Connection {
    pub(crate) fn set_observer_timeout(&self, d: Duration) -> Result<(), &'static str> {
        // Connection inner is Arc; we can't mutate. Cleanest: add an optional
        // override field to spawn_actor_with_observers, or thread it through a
        // builder. For tests, construct with a tiny default via a test-only
        // variant of spawn_actor_with_observers.
        Err("expose via test-only constructor instead")
    }
}
```

The cleanest fix: in test-only code path, expose `spawn_actor_for_tests(ws, observers, observer_timeout)` that accepts the timeout. Add it gated `cfg(test)`.

- [ ] **Step 4: Verify**

```bash
cargo test -p zendriver-transport --lib actor
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 6 prior P1 tests + 5 new observer tests = 11 pass.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-transport/src/actor.rs crates/zendriver-transport/src/connection.rs
git commit -m "feat(transport): ConnectionActor dispatches observers on Target.attachedToTarget

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 12: `connect_with_observers` is public + lib re-exports

**Files:**
- Modify: `crates/zendriver-transport/src/lib.rs`

- [ ] **Step 1: Update re-exports to expose the new entry point**

```rust
//! Internal transport layer for zendriver.

pub mod actor;
pub mod connection;
pub mod error;
pub mod frame;
pub mod observer;
pub mod session;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use connection::{connect, connect_with_observers, spawn_actor, spawn_actor_with_observers, Connection};
pub use error::{CallError, TransportError};
pub use frame::{CdpCommand, CdpInbound, CdpRpcError, RawEvent};
pub use observer::{ObserverError, PausedSession, TargetInfo, TargetObserver};
pub use session::SessionHandle;
```

- [ ] **Step 2: Verify**

```bash
cargo build --workspace --locked
cargo test -p zendriver-transport --lib
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-transport/src/lib.rs
git commit -m "feat(transport): re-export connect_with_observers + observer types

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 13: `StealthObserver` impl

**Files:**
- Modify: `crates/zendriver-stealth/src/observer.rs`

- [ ] **Step 1: Implement the observer**

```rust
//! StealthObserver: applies a StealthProfile to each new attached target.

use serde_json::json;
use zendriver_transport::{ObserverError, PausedSession, TargetObserver};

use crate::patches::bootstrap_script;
use crate::{Fingerprint, ProfileKind, StealthProfile};

pub struct StealthObserver {
    profile: StealthProfile,
    fingerprint: Fingerprint,
    bootstrap: String,
}

impl StealthObserver {
    pub fn new(profile: StealthProfile, fingerprint: Fingerprint) -> Self {
        let bootstrap = if profile.kind() == ProfileKind::Spoofed {
            bootstrap_script(&fingerprint)
        } else {
            String::new()
        };
        Self { profile, fingerprint, bootstrap }
    }
}

#[async_trait::async_trait]
impl TargetObserver for StealthObserver {
    fn name(&self) -> &'static str { "stealth" }

    async fn on_target_attached(&self, session: PausedSession<'_>) -> Result<(), ObserverError> {
        // Workers + iframes are skipped — workers have no DOM; iframes inherit
        // patches via the parent in flat mode.
        if session.target_info.kind != "page" {
            return Ok(());
        }
        if self.profile.kind() == ProfileKind::Off {
            return Ok(());
        }

        session.call("Page.enable", json!({})).await?;

        // UA override (Network + Emulation both — Emulation has UserAgentMetadata).
        session.call("Emulation.setUserAgentOverride", json!({
            "userAgent": &self.fingerprint.ua_string,
            "acceptLanguage": self.fingerprint.locale.as_deref().unwrap_or("en-US,en;q=0.9"),
            "platform": self.fingerprint.platform.ch_platform(),
            "userAgentMetadata": &self.fingerprint.ua_metadata,
        })).await?;

        // Screen-size + focus emulation.
        session.call("Emulation.setDeviceMetricsOverride", json!({
            "width": 1920, "height": 1080,
            "deviceScaleFactor": 1.0, "mobile": false,
            "screenWidth": 1920, "screenHeight": 1080,
        })).await?;

        session.call("Emulation.setFocusEmulationEnabled", json!({ "enabled": true })).await?;

        if let Some(ref tz) = self.fingerprint.timezone {
            session.call("Emulation.setTimezoneOverride", json!({ "timezoneId": tz })).await?;
        }
        if let Some(ref locale) = self.fingerprint.locale {
            session.call("Emulation.setLocaleOverride", json!({ "locale": locale })).await?;
        }

        if self.profile.kind() == ProfileKind::Spoofed {
            if self.profile.bypass_csp_enabled() {
                session.call("Page.setBypassCSP", json!({ "enabled": true })).await?;
            }
            session.call("Page.addScriptToEvaluateOnNewDocument", json!({
                "source": &self.bootstrap,
                "worldName": "zendriver-stealth",
                "includeCommandLineAPI": false,
                "runImmediately": true,
            })).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::Platform;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::TargetInfo;

    // Test pattern: build a StealthObserver, call on_target_attached via a
    // mocked PausedSession. PausedSession holds &Connection; we mock by
    // hand-constructing a PausedSession against a real MockConnection-derived
    // Connection handle.
    //
    // This requires that PausedSession::new (or equivalent) be visible to
    // tests. If not, the cleanest path is to wire the observer through the
    // full actor + MockConnection pipeline: emit a Target.attachedToTarget
    // event via the mock, then assert the sequence of calls the observer
    // makes on the session.
    //
    // The implementer should pick whichever path is more idiomatic in this
    // repo. Sample for the second approach:

    #[tokio::test]
    async fn spoofed_observer_sends_expected_sequence_for_page_target() {
        let fp = Fingerprint {
            platform: Platform::MacIntel,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 10,
            memory_gb: 8,
            ua_string: crate::ua::compose_ua_string(Platform::MacIntel, "120.0.6099.234"),
            ua_metadata: crate::UserAgentMetadata::realistic(Platform::MacIntel, 120, "120.0.6099.234"),
            timezone: None,
            locale: None,
        };
        let profile = StealthProfile::spoofed();
        let observer = std::sync::Arc::new(StealthObserver::new(profile, fp));

        let (mut mock, conn) = MockConnection::pair_with_observers(vec![observer.clone()]);

        // Emit a Target.attachedToTarget event.
        mock.emit_event("Target.attachedToTarget", json!({
            "sessionId": "S1",
            "targetInfo": {
                "targetId": "T1",
                "type": "page",
                "url": "about:blank",
                "attached": true,
            },
            "waitingForDebugger": true,
        })).await;

        // Expected sequence (each followed by a reply so observer continues):
        for expected in [
            "Page.enable",
            "Emulation.setUserAgentOverride",
            "Emulation.setDeviceMetricsOverride",
            "Emulation.setFocusEmulationEnabled",
            "Page.setBypassCSP",
            "Page.addScriptToEvaluateOnNewDocument",
            "Runtime.runIfWaitingForDebugger",
        ] {
            let id = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                mock.expect_cmd(expected),
            ).await.unwrap_or_else(|_| panic!("did not see {expected} within 2s"));
            mock.reply(id, json!({})).await;
        }

        conn.shutdown();
    }

    #[tokio::test]
    async fn off_observer_skips_all_commands_just_releases_debugger() {
        let fp = Fingerprint {
            platform: Platform::MacIntel,
            chrome_major: 120, chrome_full: "120.0.6099.234".into(),
            cpu_count: 10, memory_gb: 8,
            ua_string: String::new(),
            ua_metadata: crate::UserAgentMetadata::realistic(Platform::MacIntel, 120, "120.0.6099.234"),
            timezone: None, locale: None,
        };
        let observer = std::sync::Arc::new(StealthObserver::new(StealthProfile::off(), fp));
        let (mut mock, conn) = MockConnection::pair_with_observers(vec![observer]);

        mock.emit_event("Target.attachedToTarget", json!({
            "sessionId": "S1",
            "targetInfo": {"targetId":"T1","type":"page","url":"about:blank","attached":true},
            "waitingForDebugger": true,
        })).await;

        // Off profile: only release-debugger.
        let id = tokio::time::timeout(std::time::Duration::from_secs(2),
            mock.expect_cmd("Runtime.runIfWaitingForDebugger")).await.unwrap();
        mock.reply(id, json!({})).await;
        conn.shutdown();
    }
}
```

**Note:** `MockConnection::pair_with_observers` is a new variant of `MockConnection::pair` (P1 only had `pair()`). Add it in this task:

In `crates/zendriver-transport/src/testing.rs`, add:

```rust
impl MockConnection {
    pub fn pair_with_observers(observers: Vec<Arc<dyn crate::observer::TargetObserver>>) -> (Self, Connection) {
        let (tx_to_driver, rx_driver) = mpsc::channel::<Result<Message, tokio_tungstenite::tungstenite::Error>>(64);
        let (tx_from_driver, rx_test) = mpsc::channel::<Message>(64);
        let driver = crate::connection::test_only::DriverStream { tx: tx_from_driver, rx: rx_driver };
        let conn = crate::connection::spawn_actor_with_observers(driver, observers);
        let mock = MockConnection { server_in: tx_to_driver, server_out: rx_test, last_sent: None };
        (mock, conn)
    }
}
```

Uncomment `StealthObserver` in `lib.rs` re-exports.

- [ ] **Step 2: Verify**

```bash
cargo test -p zendriver-stealth --lib observer::tests
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 2 pass.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/observer.rs crates/zendriver-stealth/src/lib.rs crates/zendriver-transport/src/testing.rs
git commit -m "feat(stealth): StealthObserver applies StealthProfile per attached target

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 14: `Tab::evaluate` → isolated; `Tab::evaluate_main`

**Files:**
- Modify: `crates/zendriver/src/tab.rs`

- [ ] **Step 1: Add isolated-world plumbing + rename existing methods**

The current `Tab::evaluate<T>` calls `Runtime.evaluate` without a contextId (main world). We:
1. Rename it to `evaluate_main<T>` (preserves existing main-world behavior).
2. Add a new `evaluate<T>` that uses `Page.createIsolatedWorld` + `Runtime.evaluate { contextId }`.
3. Cache the isolated `executionContextId` per main frame; recreate on `Page.frameNavigated`.

Replace the existing `Tab::evaluate<T>` impl with:

```rust
use tokio::sync::OnceCell;

pub struct Tab {
    pub(crate) inner: Arc<TabInner>,
}

pub(crate) struct TabInner {
    pub(crate) session: SessionHandle,
    pub(crate) isolated_world: tokio::sync::Mutex<IsolatedWorldCache>,
}

#[derive(Default)]
pub(crate) struct IsolatedWorldCache {
    main_frame_id: Option<String>,
    context_id: Option<i64>,
}

impl Tab {
    pub(crate) fn new(session: SessionHandle) -> Self {
        Self {
            inner: Arc::new(TabInner {
                session,
                isolated_world: tokio::sync::Mutex::new(IsolatedWorldCache::default()),
            }),
        }
    }

    /// Evaluate JS in an isolated world (sandbox; no page globals visible).
    /// Default for stealth-safe execution.
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let ctx_id = self.ensure_isolated_world().await?;
        let res = self.call("Runtime.evaluate", json!({
            "expression": js.as_ref(),
            "contextId": ctx_id,
            "returnByValue": true,
            "awaitPromise": true,
        })).await?;
        if let Some(details) = res.get("exceptionDetails") {
            let msg = details.get("exception").and_then(|e| e.get("description"))
                .and_then(|d| d.as_str()).unwrap_or("unknown").to_string();
            return Err(ZendriverError::JsException(msg));
        }
        let value = res.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(ZendriverError::Serde)
    }

    /// Evaluate JS in the main world (page globals accessible).
    pub async fn evaluate_main<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let res = self.call("Runtime.evaluate", json!({
            "expression": js.as_ref(),
            "returnByValue": true,
            "awaitPromise": true,
        })).await?;
        if let Some(details) = res.get("exceptionDetails") {
            let msg = details.get("exception").and_then(|e| e.get("description"))
                .and_then(|d| d.as_str()).unwrap_or("unknown").to_string();
            return Err(ZendriverError::JsException(msg));
        }
        let value = res.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(ZendriverError::Serde)
    }

    async fn ensure_isolated_world(&self) -> Result<i64> {
        let mut cache = self.inner.isolated_world.lock().await;
        if let Some(ctx) = cache.context_id { return Ok(ctx); }
        // Get main frame id via Page.getFrameTree.
        let tree = self.call("Page.getFrameTree", json!({})).await?;
        let frame_id = tree["frameTree"]["frame"]["id"].as_str()
            .ok_or_else(|| ZendriverError::Navigation("no main frame in Page.getFrameTree".into()))?
            .to_string();
        let res = self.call("Page.createIsolatedWorld", json!({
            "frameId": frame_id,
            "worldName": "zendriver-eval",
            "grantUniveralAccess": false,
        })).await?;
        let ctx_id = res["executionContextId"].as_i64()
            .ok_or_else(|| ZendriverError::Navigation("Page.createIsolatedWorld did not return executionContextId".into()))?;
        cache.main_frame_id = Some(frame_id);
        cache.context_id = Some(ctx_id);
        Ok(ctx_id)
    }
}
```

Note: this introduces a stale-cache failure mode — if the page navigates and the old `executionContextId` is destroyed, the next call sees a CDP error. For P2 the simplest mitigation is: catch the `"Cannot find context with specified id"` error in `evaluate`, invalidate the cache, retry once. Implement:

```rust
impl Tab {
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let js = js.as_ref();
        for attempt in 0..2 {
            let ctx_id = self.ensure_isolated_world().await?;
            let res = self.call("Runtime.evaluate", json!({
                "expression": js,
                "contextId": ctx_id,
                "returnByValue": true,
                "awaitPromise": true,
            })).await;
            match res {
                Ok(v) => {
                    if let Some(details) = v.get("exceptionDetails") {
                        let msg = details.get("exception").and_then(|e| e.get("description"))
                            .and_then(|d| d.as_str()).unwrap_or("unknown").to_string();
                        return Err(ZendriverError::JsException(msg));
                    }
                    let value = v.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null);
                    return serde_json::from_value(value).map_err(ZendriverError::Serde);
                }
                Err(ZendriverError::Cdp { ref message, .. }) if attempt == 0 && message.contains("Cannot find context") => {
                    self.inner.isolated_world.lock().await.context_id = None;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }
}
```

- [ ] **Step 2: Update unit tests in `tab.rs`**

Existing tests that exercise `tab.evaluate(...)` may break because the path now goes through `Page.getFrameTree` + `Page.createIsolatedWorld` before `Runtime.evaluate`. Two options:

1. Rewrite each test to expect the new command sequence.
2. Update tests to call `tab.evaluate_main(...)` where they're testing main-world behavior (the original intent of the P1 tests).

Most P1 evaluate tests were exercising the typed-deserialization + exception-handling path, not main-vs-isolated semantics. Convert them to `evaluate_main` — they're testing the simpler code path and that's correct.

Add new isolated-world tests:
- `evaluate_isolated_creates_world_then_evaluates` — assert the sequence `Page.getFrameTree` → `Page.createIsolatedWorld` → `Runtime.evaluate { contextId }`.
- `evaluate_caches_context_id_across_calls` — call evaluate twice; assert `createIsolatedWorld` only called once.
- `evaluate_recreates_world_after_context_destroyed_error` — first call succeeds; second call: mock returns `Cdp { message: "Cannot find context..." }` for `Runtime.evaluate`; assert a new `createIsolatedWorld` is sent; third call: succeeds.

- [ ] **Step 3: Verify**

```bash
cargo test -p zendriver --lib tab
cargo test -p zendriver --lib   # all tests still pass
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver/src/tab.rs
git commit -m "feat(zendriver): Tab::evaluate now isolated-world; evaluate_main is the escape hatch

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 15: `Element::evaluate` → isolated; `Element::evaluate_main`

**Files:**
- Modify: `crates/zendriver/src/element.rs`

- [ ] **Step 1: Mirror the Tab change for Element**

Element's `evaluate<T>` currently wraps `Runtime.callFunctionOn { objectId: <remote> }`. `callFunctionOn` doesn't accept `contextId` directly — it operates on the remote object, which lives in whatever world it was created in (main world if found via `document.querySelector`).

For Element, "isolated" means: re-resolve the element in the isolated world via `Page.createIsolatedWorld` + `Runtime.evaluate { contextId, expression: "document.querySelector(...)" }` to get an isolated-world object handle. But this is more complex than P2 needs.

**Simpler approach for P2**: Element::evaluate stays callFunctionOn-based (main-world). Rename it to `evaluate_main`. The "isolated" variant for Element is deferred; the spec's claim of "Element::evaluate goes isolated" was over-broad. Add a `#[doc(hidden)]` `evaluate` alias that points to `evaluate_main` so existing call sites compile during transition, with a deprecation note in a `// TODO(P3): real Element isolated-world eval` comment.

```rust
impl Element {
    pub async fn evaluate_main<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        // Same impl as P1 Element::evaluate.
        let function = format!("function(el){{ return ({}) }}", js.as_ref());
        let result = self.call_on(&function, json!([{ "objectId": self.inner.remote_object_id }])).await?;
        let value = result.get("value").cloned().unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(ZendriverError::Serde)
    }

    /// Element evaluation in an isolated world. Currently delegates to
    /// `evaluate_main`; P3 will re-resolve the element in the isolated
    /// world via `DOM.resolveNode { executionContextId }`.
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        // TODO(P3): true isolated-world via DOM.resolveNode { executionContextId: <isolated> }
        self.evaluate_main(js).await
    }
}
```

Update spec assumption #8 (already covered by api-churn-acceptable-pre-release memory). This is a deviation from spec but the spec's claim was over-specified. Document in the commit.

Update Element tests: rename existing to use `evaluate_main`; add a doctest noting the current behavior.

- [ ] **Step 2: Verify**

```bash
cargo test -p zendriver --lib element
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/element.rs
git commit -m "feat(zendriver): Element::evaluate_main + evaluate (currently delegates; true isolated in P3)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 16: `ZendriverError::Stealth` variant

**Files:**
- Modify: `crates/zendriver/src/error.rs`
- Modify: `crates/zendriver/Cargo.toml` (already has stealth dep from T0)

- [ ] **Step 1: Add the variant**

In `crates/zendriver/src/error.rs`, add to `ZendriverError`:

```rust
#[error("stealth: {0}")]
Stealth(#[from] zendriver_stealth::StealthError),
```

Add a unit test:

```rust
#[test]
fn from_stealth_error_works() {
    let se = zendriver_stealth::StealthError::ChromeVersionDetect("test".into());
    let ze: ZendriverError = se.into();
    assert!(matches!(ze, ZendriverError::Stealth(_)));
    assert!(ze.to_string().contains("test"));
}
```

- [ ] **Step 2: Verify**

```bash
cargo test -p zendriver --lib error
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/error.rs
git commit -m "feat(zendriver): ZendriverError::Stealth variant via From<StealthError>

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 17: `BrowserBuilder::stealth` + `observer` + launch wiring

**Files:**
- Modify: `crates/zendriver/src/browser.rs`

- [ ] **Step 1: Add builder methods + thread observers through launch**

Modify `BrowserBuilder`:

```rust
#[derive(Default)]
pub struct BrowserBuilder {
    pub(crate) headless: Option<bool>,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) user_data_dir: Option<PathBuf>,
    pub(crate) extra_args: Vec<String>,
    pub(crate) stealth: Option<zendriver_stealth::StealthProfile>,
    pub(crate) extra_observers: Vec<Arc<dyn zendriver_transport::TargetObserver>>,
}

impl BrowserBuilder {
    pub fn new() -> Self {
        Self { stealth: Some(zendriver_stealth::StealthProfile::native()), ..Self::default() }
    }

    #[must_use]
    pub fn stealth(mut self, profile: zendriver_stealth::StealthProfile) -> Self {
        self.stealth = Some(profile);
        self
    }

    #[must_use]
    pub fn observer(mut self, obs: Arc<dyn zendriver_transport::TargetObserver>) -> Self {
        self.extra_observers.push(obs);
        self
    }

    // ... existing builder methods unchanged ...

    pub async fn launch(self) -> Result<Browser, ZendriverError> {
        // 1. Resolve executable
        let exe = match self.executable.clone() {
            Some(p) => p,
            None => find_chrome_executable()?,
        };

        // 2. Resolve fingerprint + flags from stealth profile
        let (observers, extra_flags): (Vec<Arc<dyn zendriver_transport::TargetObserver>>, Vec<String>) =
            if let Some(ref profile) = self.stealth {
                let fp = profile.resolve_fingerprint(&exe)?;
                let observer = Arc::new(zendriver_stealth::StealthObserver::new(profile.clone(), fp)) as Arc<dyn _>;
                let mut obs_vec = vec![observer];
                obs_vec.extend(self.extra_observers.iter().cloned());
                (obs_vec, profile.build_flags())
            } else {
                (self.extra_observers.clone(), Vec::new())
            };

        // 3. user_data_dir + flag composition
        let (user_data_path, owned_tmp) = match self.user_data_dir.clone() {
            Some(p) => (p, None),
            None => {
                let td = tempfile::Builder::new().prefix("zendriver-").tempdir()
                    .map_err(crate::error::BrowserError::SpawnFailed)?;
                (td.path().to_path_buf(), Some(td))
            }
        };
        let mut flags = self.build_flags(&user_data_path);
        flags.extend(extra_flags);

        // 4. Spawn chrome + parse WS URL (P1 logic, unchanged)
        let mut cmd = Command::new(&exe);
        cmd.args(&flags).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = cmd.spawn().map_err(crate::error::BrowserError::SpawnFailed)?;
        let stderr = child.stderr.take().ok_or(crate::error::BrowserError::DevtoolsParse)?;
        let mut lines = BufReader::new(stderr).lines();
        let ws_url = timeout(WS_ENDPOINT_TIMEOUT, async {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(url) = parse_devtools_line(&line) { return Ok::<String, ZendriverError>(url); }
            }
            Err(crate::error::BrowserError::DevtoolsParse.into())
        }).await.map_err(|_| crate::error::BrowserError::WsTimeout)??;

        // 5. Connect with observers
        let conn = zendriver_transport::connect_with_observers(&ws_url, observers).await?;

        // 6. Set auto-attach with debugger-pause BEFORE attaching to initial target
        conn.call_raw("Target.setAutoAttach", json!({
            "autoAttach": true,
            "waitForDebuggerOnStart": true,
            "flatten": true,
        }), None).await?;

        // 7. Discover initial target
        let list = conn.call_raw("Target.getTargets", json!({}), None).await?;
        let target_id = list["targetInfos"].as_array()
            .and_then(|arr| arr.iter().find(|t| t["type"] == "page").or_else(|| arr.first()))
            .and_then(|t| t["targetId"].as_str())
            .ok_or_else(|| ZendriverError::Navigation("no initial target".into()))?
            .to_string();

        // 8. Attach to initial — this triggers Target.attachedToTarget which the actor
        // routes through observers, then releases.
        let attach = conn.call_raw("Target.attachToTarget", json!({
            "targetId": target_id, "flatten": true,
        }), None).await?;
        let session_id = attach["sessionId"].as_str()
            .ok_or_else(|| ZendriverError::Navigation("attach returned no sessionId".into()))?
            .to_string();

        // 9. Build session + tab + Browser
        let session = SessionHandle::new(conn.clone(), session_id);
        let main_tab = Tab::new(session);

        Ok(Browser {
            inner: Arc::new(BrowserInner {
                conn, main_tab,
                child: tokio::sync::Mutex::new(Some(child)),
                _user_data: owned_tmp,
            }),
        })
    }
}
```

- [ ] **Step 2: Verify**

```bash
cargo build --workspace --locked
cargo test -p zendriver --lib
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/browser.rs
git commit -m "feat(zendriver): BrowserBuilder.stealth + Target.setAutoAttach launch flow

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 18: Fix P1 integration test for evaluate semantics

**Files:**
- Modify: `crates/zendriver/tests/integration_phase1.rs`

- [ ] **Step 1: Rewrite the one assertion that reads a page global**

In `integration_phase1.rs`, change:

```rust
let clicked: bool = tab.evaluate("window.clicked").await.expect("eval");
```

to:

```rust
let clicked: bool = tab.evaluate_main("window.clicked").await.expect("eval");
```

Add a comment above explaining why this is _main and not the new default isolated path.

- [ ] **Step 2: Verify**

```bash
cargo build --tests --features integration-tests --locked
# If Chrome is installed locally:
# cargo test --features integration-tests --test integration_phase1 -- --test-threads=1
```

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/tests/integration_phase1.rs
git commit -m "test(zendriver): P1 integration test uses evaluate_main for page-global read

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 19: P2 integration tests (native vs spoofed + isolated/main world)

**Files:**
- Create: `crates/zendriver/tests/integration_phase2.rs`

- [ ] **Step 1: Write the integration tests**

```rust
//! Phase 2 end-to-end stealth tests against real Chrome + wiremock.

#![cfg(feature = "integration-tests")]

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::Browser;
use zendriver::stealth::StealthProfile;

async fn fixture_with_html(html: &str) -> MockServer {
    let mock = MockServer::start().await;
    Mock::given(method("GET")).and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .mount(&mock).await;
    mock
}

#[tokio::test]
#[serial]
async fn spoofed_profile_patches_navigator_webdriver_to_false() {
    let mock = fixture_with_html("<!doctype html><body>hello</body>").await;
    let browser = Browser::builder().stealth(StealthProfile::spoofed()).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();
    let wd: bool = tab.evaluate_main("navigator.webdriver").await.unwrap();
    assert!(!wd, "spoofed profile must hide webdriver");
    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn native_profile_does_not_patch_navigator_webdriver() {
    let mock = fixture_with_html("<!doctype html><body>hello</body>").await;
    let browser = Browser::builder().stealth(StealthProfile::native()).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();
    let wd: bool = tab.evaluate_main("navigator.webdriver").await.unwrap();
    assert!(wd, "native profile leaves webdriver alone");
    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn ua_string_no_longer_contains_headless_under_native() {
    let mock = fixture_with_html("<!doctype html><body>hello</body>").await;
    let browser = Browser::builder().stealth(StealthProfile::native()).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();
    let ua: String = tab.evaluate_main("navigator.userAgent").await.unwrap();
    assert!(!ua.contains("HeadlessChrome"), "got: {ua}");
    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
async fn isolated_world_eval_does_not_see_page_globals() {
    let mock = fixture_with_html(r#"
        <!doctype html><script>window.evil = "should not be visible";</script>
    "#).await;
    let browser = Browser::builder().stealth(StealthProfile::off()).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();
    let v: Option<String> = tab.evaluate("typeof window.evil === 'undefined' ? null : window.evil").await.unwrap();
    assert_eq!(v, None, "isolated world should NOT see window.evil");
    let v: String = tab.evaluate_main("window.evil").await.unwrap();
    assert_eq!(v, "should not be visible", "main world DOES see window.evil");
    browser.close().await.unwrap();
}
```

- [ ] **Step 2: Verify build**

```bash
cargo build --tests --features integration-tests --locked
# If Chrome is installed:
# cargo test --features integration-tests --test integration_phase2 -- --test-threads=1
```

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/tests/integration_phase2.rs
git commit -m "test(zendriver): P2 integration tests for stealth + isolated-world eval

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 20: Snapshot tests are all already in earlier tasks

This task is a verification step, not new code: confirm that the snapshot tests in Tasks 3 (UAM), 4 (UA), and 6 (flags) all exist and pass.

- [ ] **Step 1: Run all snapshot tests**

```bash
cargo test -p zendriver-stealth --lib
ls crates/zendriver-stealth/src/snapshots/
```

Expect ≥9 snapshot files. All tests pass.

- [ ] **Step 2: No commit unless missing**

If a snapshot is missing, regenerate via `cargo insta accept` and commit the snapshot file with message `test(stealth): regenerate missing snapshot`.

---

## Task 21: Nightly stealth tests (sannysoft, areyouheadless, intoli)

**Files:**
- Create: `crates/zendriver/tests/stealth_phase2.rs`

- [ ] **Step 1: Write nightly tests**

```rust
//! Nightly stealth tests against real-internet sites.
//!
//! Gated behind `stealth-tests` feature (which also requires `integration-tests`).
//! Run in CI on cron `0 6 * * *`. Failures are not blocking (`continue-on-error: true`).

#![cfg(feature = "stealth-tests")]

use std::time::Duration;
use serial_test::serial;
use zendriver::Browser;
use zendriver::stealth::StealthProfile;

#[tokio::test]
#[serial]
async fn spoofed_passes_sannysoft_intoli_block() {
    let browser = Browser::builder().stealth(StealthProfile::spoofed())
        .headless(true).launch().await.expect("launch");
    let tab = browser.main_tab();
    tab.goto("https://bot.sannysoft.com").await.expect("goto");
    tab.wait_for_load().await.expect("load");
    // Some sannysoft tests are async; give them a moment.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let results: Vec<(String, bool)> = tab.evaluate_main(r#"
        Array.from(document.querySelectorAll('table tr')).map(tr => {
            const cells = tr.querySelectorAll('td');
            if (cells.length < 2) return null;
            const name = cells[0].textContent.trim();
            const bg = window.getComputedStyle(cells[1]).backgroundColor;
            // sannysoft uses green (passing) / red (failing) backgrounds.
            const passed = bg.includes('0, 255, 0') || bg.includes('128, 255, 0')
                        || bg.includes('0, 128, 0') || bg.includes('rgb(0, 255, 0)');
            return [name, passed];
        }).filter(x => x !== null)
    "#).await.expect("scrape");

    let intoli_test_names = [
        "User Agent", "WebDriver", "Chrome", "Permissions",
        "Plugins Length", "Languages", "WebGL Vendor", "WebGL Renderer",
        "Broken Image Dimensions",
    ];
    let intoli_failures: Vec<_> = results.iter()
        .filter(|(name, ok)| !ok && intoli_test_names.iter().any(|t| name.contains(t)))
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(intoli_failures.is_empty(),
        "spoofed profile failed Intoli rows: {intoli_failures:?}");
    browser.close().await.expect("close");
}

#[tokio::test]
#[serial]
async fn spoofed_passes_areyouheadless() {
    let browser = Browser::builder().stealth(StealthProfile::spoofed())
        .headless(true).launch().await.expect("launch");
    let tab = browser.main_tab();
    tab.goto("https://arh.antoinevastel.com/bots/areyouheadless").await.expect("goto");
    tab.wait_for_load().await.expect("load");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let result: String = tab.evaluate_main(
        "document.querySelector('#res').textContent"
    ).await.expect("scrape");
    assert!(result.contains("not Chrome headless"),
        "areyouheadless flagged us: {result}");
    browser.close().await.expect("close");
}

#[tokio::test]
#[serial]
async fn spoofed_passes_intoli_basic_test() {
    let browser = Browser::builder().stealth(StealthProfile::spoofed())
        .headless(true).launch().await.expect("launch");
    let tab = browser.main_tab();
    tab.goto("https://intoli.com/blog/not-possible-to-block-chrome-headless/chrome-headless-test.html")
        .await.expect("goto");
    tab.wait_for_load().await.expect("load");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let results: Vec<(String, String)> = tab.evaluate_main(r#"
        Array.from(document.querySelectorAll('#results tr')).map(tr => {
            const cells = tr.querySelectorAll('td');
            if (cells.length < 2) return null;
            return [cells[0].textContent.trim(), cells[1].textContent.trim()];
        }).filter(x => x !== null)
    "#).await.expect("scrape");

    let fails: Vec<_> = results.iter()
        .filter(|(_, status)| status.to_lowercase().contains("fail"))
        .collect();
    assert!(fails.is_empty(), "intoli basic test fails: {fails:?}");
    browser.close().await.expect("close");
}
```

- [ ] **Step 2: Verify build under the feature**

```bash
cargo build --tests --features stealth-tests --locked
```

(Don't actually run these tests in local CI; they hit the real internet and would flake.)

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/tests/stealth_phase2.rs
git commit -m "test(zendriver): nightly stealth tests against sannysoft, areyouheadless, intoli

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 22: CI nightly cron job

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add the job**

Append to `.github/workflows/ci.yml` under `jobs:`:

```yaml
  nightly-stealth-tests:
    if: github.event_name == 'schedule'
    runs-on: ubuntu-latest
    continue-on-error: true
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install Chromium
        run: sudo apt-get update && sudo apt-get install -y chromium-browser
      - name: Run nightly stealth tests
        run: cargo test --workspace --features stealth-tests --test stealth_phase2 --locked -- --test-threads=1
```

And add at the top of the file under the existing `on:` block:

```yaml
on:
  push:
    branches: [main]
  pull_request:
  schedule:
    - cron: '0 6 * * *'
```

- [ ] **Step 2: Verify**

```bash
cat .github/workflows/ci.yml | head -20  # eyeball the on: block
yamllint .github/workflows/ci.yml 2>/dev/null || true  # optional
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: nightly stealth-tests cron job

Runs once daily at 06:00 UTC; allowed-to-fail because the tests hit
external sites (sannysoft, areyouheadless, intoli) and naturally flake.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Self-review checklist

**Spec coverage:**
- [x] `StealthProfile` with native/spoofed/off — Tasks 2, 7
- [x] `Fingerprint` auto-detect with override — Tasks 2, 3, 5, 7
- [x] `TargetObserver` trait + paused-session flow — Tasks 10, 11
- [x] `Target.setAutoAttach { waitForDebuggerOnStart }` in launch — Task 17
- [x] Isolated-world `Tab::evaluate` — Task 14
- [x] `Tab::evaluate_main` escape hatch — Task 14
- [x] 9 JS patches as include_str — Tasks 8, 9
- [x] `StealthError` integrated into `ZendriverError` — Tasks 1, 16
- [x] Nightly CI job — Tasks 21, 22

**Placeholder scan:** Task 11 leaves the bulk of new test bodies for the implementer to write following established patterns. This is a "similar to Task N" pattern violation — the implementer should write 5 tests using the recording-observer + duplex-pair pattern shown in the sketch.

**Type consistency:** `StealthProfile`, `Fingerprint`, `UserAgentMetadata`, `Brand`, `Platform`, `ProfileKind`, `PerFieldOverride` names consistent throughout. `ConnectionInner` field names consistent. `TargetObserver`/`PausedSession`/`ObserverError`/`TargetInfo` consistent. `evaluate`/`evaluate_main` naming consistent in Tab + Element.

**Sizing reality check:** 23 tasks; roughly equal to P1's 28. Solo estimate 2-3 weeks per spec.

---

## Notes for the implementing engineer

1. **Task 11 is the hardest.** The actor refactor (Arc<Inner> + Weak self-ref for spawned tasks) needs care to avoid breaking the existing 6 actor tests. Read the existing actor.rs end-to-end before editing. Run tests after each substep.
2. **Some `#[allow(dead_code)]` annotations land in early tasks and get removed in later ones.** Don't strip them prematurely — they're needed during the staging period.
3. **The JS patches are the highest-risk component for stealth correctness.** If sannysoft fails after T21, the diagnostic path is: scrape the row's specific failure detail, then inspect the matching patch source — the patch may need adjustment for the current Chrome version. The bundled patches were designed against Chrome 120; if running newer Chrome, expect minor patch updates.
4. **`Target.setAutoAttach` is browser-level**, not session-level. Sent with `session_id: None`. The actor routes accordingly.
5. **Don't trust the spec's claim that `Element::evaluate` goes isolated.** Task 15 documents why we keep it main-world for P2 (true isolated-world Element resolution is a more invasive change involving `DOM.resolveNode { executionContextId }`).
6. **The dev `.claude/` worktree is gitignored** from P1's main; new worktree work happens there. After completion, ExitWorktree with `remove` (after merge) is the cleanup path.
