# zendriver-rs — Phase 2: Stealth

**Date:** 2026-05-23
**Status:** Approved (delegate-mode brainstorming complete, ready for implementation plan)
**Phase:** 2 of 6 (stealth — see [Roadmap](#roadmap))
**Depends on:** Phase 1 (foundation) — landed `12bf170` on `main`

## Summary

Add anti-detection capabilities to zendriver-rs. Ship `zendriver-stealth` crate exposing two named `StealthProfile` modes (`native` and `spoofed`). Wire stealth into `BrowserBuilder` via the existing builder pattern, with `native` as the default. Adopt chaser-oxide's protocol-level innovations: `Page.createIsolatedWorld` + isolated-context evaluation (never call `Runtime.enable`), `Emulation.setUserAgentOverride` with full `UserAgentMetadata` (Sec-CH-UA-* sync), `Emulation.setDeviceMetricsOverride` for screen-size consistency, `Emulation.setFocusEmulationEnabled` for visibility. Auto-detect real system fingerprint (OS, CPU count, RAM, installed Chrome version) at launch with per-field override. New `TargetObserver` trait in `zendriver-transport` enables paused-target patch injection via `Target.setAutoAttach { waitForDebuggerOnStart: true }` for popup/new-tab support that future phases can lean on.

Phase 2 exit criterion: nightly stealth test against `bot.sannysoft.com` passes all Intoli rows when launched with `StealthProfile::spoofed()`. `areyouheadless` and `intoli.com/chrome-headless-test` likewise pass.

## Goals

- New crate `zendriver-stealth` (currently a P1 stub).
- `StealthProfile` with `off()` / `native()` / `spoofed()` constructors + per-field builder overrides (memory, cpu, chrome_version, platform, locale, timezone, custom flags).
- `Fingerprint` auto-detection (sysinfo + num_cpus + `chrome --version` probe) with per-field override and a clamping policy for spec-compliant Navigator values.
- `TargetObserver` trait in `zendriver-transport` + Connection-actor changes to dispatch observers on `Target.attachedToTarget` events with proper failure-mode isolation (timeout, panic, Err).
- `Target.setAutoAttach { autoAttach: true, waitForDebuggerOnStart: true, flatten: true }` wired into the `Browser::launch` flow so the spoofed profile's bootstrap script installs before any page JS runs on EVERY target (initial + future popups).
- New isolated-world evaluation path: `Tab::evaluate<T>` switches to use `Page.createIsolatedWorld` + `Runtime.evaluate { contextId }`; `Tab::evaluate_main<T>` becomes the escape hatch for main-world access. Same change for `Element::evaluate` / `Element::evaluate_main`.
- 9 JS patches as `include_str!`-loaded `.js` files under `crates/zendriver-stealth/src/patches/`. Patches written as factory functions taking a serialized Fingerprint.
- `StealthError` type integrated into the `ZendriverError` hierarchy via `#[from]`.
- Nightly CI job exercising `stealth-tests` feature against real Chrome + real internet (`bot.sannysoft.com`, `areyouheadless`).

## Non-goals

Explicitly **out of scope** for Phase 2:
- Cloudflare Turnstile / challenge bypass (P5).
- `bot.incolumitas.com` behavioral score / TCP-fingerprint / TLS-JA3 evasion (no JS-only lib can pass these; requires proxy + TLS stack control out of scope).
- Realistic mouse Bezier paths + per-key typing timing for input events (P3 — Element interaction).
- Per-tab profile overrides (P4 multi-tab work; for now stealth is browser-wide).
- Browser fingerprint randomization across launches (current: one fingerprint per launch, deterministic from system probes).
- Sec-CH-UA header injection for non-Chrome browsers (Chrome only; Brave/Edge/Vivaldi support is post-1.0).

## Architecture

### Crate layout

```
crates/zendriver-stealth/
├── Cargo.toml
└── src/
    ├── lib.rs                  # StealthProfile + public re-exports
    ├── profile.rs              # StealthProfile struct + Off/Native/Spoofed + builder methods
    ├── flags.rs                # Chrome launch flags
    ├── fingerprint.rs          # auto-detect + Fingerprint struct + clamping
    ├── ua.rs                   # UA string composition + UserAgentMetadata
    ├── patches.rs              # bundle Vec<PatchSource> per profile + factory wrapping
    ├── observer.rs             # StealthObserver impl of TargetObserver
    ├── error.rs                # StealthError
    └── patches/                # bundled JS sources
        ├── webdriver.js
        ├── plugins.js
        ├── chrome.js
        ├── webgl.js
        ├── permissions.js
        ├── codecs.js
        ├── navigator_props.js
        ├── user_agent_data.js
        └── broken_image.js
```

### Dependency graph delta

```
zendriver
  ├─ zendriver-transport       (P1, gains TargetObserver trait + observer wiring in P2)
  ├─ zendriver-stealth         (NEW in P2)
  └─ chromiumoxide_cdp
```

### New external dependencies

| Crate | Version | Purpose |
|---|---|---|
| `sysinfo` | `0.32` | Real RAM detection in fingerprint auto-detect |
| `num_cpus` | `1` | Real CPU count |
| `async-trait` | `0.1` | `TargetObserver` async trait |

`async-trait` was already declared in the P1 workspace `Cargo.toml` (no version bump). `sysinfo` is the only meaningful new dep — well-maintained, ~30k LOC, used by tools like `bottom` and `zellij`.

## Components

### `StealthProfile`

```rust
// crates/zendriver-stealth/src/profile.rs

#[derive(Debug, Clone)]
pub struct StealthProfile {
    pub(crate) kind: ProfileKind,
    pub(crate) extra_flags: Vec<String>,
    pub(crate) fingerprint_override: Option<Fingerprint>,
    pub(crate) per_field_override: PerFieldOverride,
    pub(crate) bypass_csp: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProfileKind {
    Off,
    Native,
    Spoofed,
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

impl StealthProfile {
    pub fn off() -> Self;
    pub fn native() -> Self;
    pub fn spoofed() -> Self;

    pub fn fingerprint(mut self, f: Fingerprint) -> Self;
    pub fn memory_gb(mut self, gb: u32) -> Self;
    pub fn cpu_count(mut self, n: u32) -> Self;
    pub fn chrome_version(mut self, major: u32) -> Self;
    pub fn platform(mut self, p: Platform) -> Self;
    pub fn locale(mut self, l: impl Into<String>) -> Self;
    pub fn timezone(mut self, tz: impl Into<String>) -> Self;
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self;
    pub fn bypass_csp(mut self, on: bool) -> Self;
    pub fn arg(mut self, flag: impl Into<String>) -> Self;
    pub fn args(mut self, flags: impl IntoIterator<Item = String>) -> Self;

    /// Resolve the final Fingerprint by combining the auto-detected baseline
    /// (or the explicit override) with per-field tweaks. Called once at launch.
    pub(crate) fn resolve_fingerprint(&self, chrome_exe: &Path) -> Result<Fingerprint, StealthError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Win32,
    MacIntel,
    LinuxX86_64,
}
```

### `Fingerprint`

```rust
// crates/zendriver-stealth/src/fingerprint.rs

#[derive(Debug, Clone, serde::Serialize)]
pub struct Fingerprint {
    pub platform: Platform,
    pub chrome_major: u32,
    pub chrome_full: String,
    pub cpu_count: u32,        // clamped [2, 32]
    pub memory_gb: u32,         // clamped to navigator-spec {4, 8}
    pub ua_string: String,
    pub ua_metadata: UserAgentMetadata,
    pub timezone: Option<String>,
    pub locale: Option<String>,
}

impl Fingerprint {
    /// Detect from the host system. Spawns `chrome --version` synchronously.
    pub fn auto_detect(chrome_executable: &Path) -> Result<Self, StealthError>;

    /// Compose UA string from current fields. Re-run after any field change.
    pub(crate) fn compose_ua_string(&mut self);
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UserAgentMetadata {
    pub brands: Vec<Brand>,
    pub full_version_list: Vec<Brand>,
    pub platform: String,
    pub platform_version: String,
    pub architecture: String,
    pub bitness: String,
    pub wow64: bool,
    pub mobile: bool,
    pub model: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Brand {
    pub brand: String,
    pub version: String,
}
```

**Clamping policy**:
- `cpu_count`: floor 2 (single-core devices are extinct), ceil 32 (Sec-CH-UA hint plausibility).
- `memory_gb`: rounded to nearest spec-compliant power-of-2, capped at 8 (per W3C `navigator.deviceMemory` spec).
- `chrome_major`: floor 100 (we don't claim ancient Chrome).

**Chrome version probe** runs `chrome --version` with a 2-second timeout. On failure, fall back to a hardcoded `(120, "120.0.6099.234")` and emit a `tracing::warn`. The hardcoded version gets bumped each release.

### `TargetObserver` trait (in `zendriver-transport`)

```rust
// crates/zendriver-transport/src/observer.rs

#[async_trait::async_trait]
pub trait TargetObserver: Send + Sync {
    async fn on_target_attached(&self, session: PausedSession<'_>) -> Result<(), ObserverError>;
    async fn on_target_detached(&self, _session_id: &str) {}
    fn name(&self) -> &'static str;
}

pub struct PausedSession<'a> {
    pub session_id: &'a str,
    pub target_info: &'a TargetInfo,
    conn: &'a Connection,
}

impl<'a> PausedSession<'a> {
    pub async fn call(&self, method: impl Into<String>, params: serde_json::Value)
        -> Result<serde_json::Value, CallError>;
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
    pub target_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub title: Option<String>,
    pub attached: bool,
    pub browser_context_id: Option<String>,
}
```

### `StealthObserver` impl (in `zendriver-stealth`)

```rust
// crates/zendriver-stealth/src/observer.rs

pub struct StealthObserver {
    profile: StealthProfile,
    fingerprint: Fingerprint,
    bootstrap_source: String,  // wrapped factory: function(fp){ ...all patches... }(<fp-json>)
}

impl StealthObserver {
    pub fn new(profile: StealthProfile, fingerprint: Fingerprint) -> Self;
}

#[async_trait::async_trait]
impl TargetObserver for StealthObserver {
    fn name(&self) -> &'static str { "stealth" }

    async fn on_target_attached(&self, session: PausedSession<'_>) -> Result<(), ObserverError> {
        // Only patch real page targets, not iframes/workers (workers have no DOM to patch).
        // For workers, just release without patching (still need to release the debugger pause).
        if session.target_info.kind != "page" {
            return Ok(());
        }

        // Page domain must be enabled before addScriptToEvaluateOnNewDocument works.
        session.call("Page.enable", serde_json::json!({})).await?;

        if self.profile.kind != ProfileKind::Off {
            // UA override applies at network level (Sec-CH-UA-* headers) AND JS level (navigator.userAgent).
            session.call("Emulation.setUserAgentOverride", serde_json::json!({
                "userAgent": &self.fingerprint.ua_string,
                "acceptLanguage": self.fingerprint.locale.as_deref().unwrap_or("en-US,en;q=0.9"),
                "platform": platform_string(&self.fingerprint.platform),
                "userAgentMetadata": &self.fingerprint.ua_metadata,
            })).await?;
            // Screen-size fix: headless reports innerWidth correctly but screen.width is wrong without this.
            session.call("Emulation.setDeviceMetricsOverride", serde_json::json!({
                "width": 1920,
                "height": 1080,
                "deviceScaleFactor": 1.0,
                "mobile": false,
                "screenWidth": 1920,
                "screenHeight": 1080,
            })).await?;
            // visibilityState='visible' + document.hasFocus() return true even in headless.
            session.call("Emulation.setFocusEmulationEnabled", serde_json::json!({
                "enabled": true,
            })).await?;

            if let Some(ref tz) = self.fingerprint.timezone {
                session.call("Emulation.setTimezoneOverride", serde_json::json!({
                    "timezoneId": tz,
                })).await?;
            }
            if let Some(ref locale) = self.fingerprint.locale {
                session.call("Emulation.setLocaleOverride", serde_json::json!({
                    "locale": locale,
                })).await?;
            }
        }

        // Only Spoofed profile installs JS bootstrap.
        if self.profile.kind == ProfileKind::Spoofed {
            if self.profile.bypass_csp {
                session.call("Page.setBypassCSP", serde_json::json!({ "enabled": true })).await?;
            }
            session.call("Page.addScriptToEvaluateOnNewDocument", serde_json::json!({
                "source": &self.bootstrap_source,
                "worldName": "zendriver-stealth",
                "includeCommandLineAPI": false,
                "runImmediately": true,
            })).await?;
        }

        Ok(())
    }
}
```

### Connection actor changes

New fields on `ConnectionActor`:
- `observers: Vec<Arc<dyn TargetObserver>>`
- `observer_timeout: Duration` (default `Duration::from_secs(5)`)

New actor responsibility: handle `Target.attachedToTarget` events by spawning an async task (the actor loop continues running so observer commands can round-trip):

```rust
async fn handle_target_attached(actor: Arc<ConnectionActorInner>, ev: TargetAttached) {
    let session_id = ev.session_id.clone();
    let conn = Connection::from_actor(actor.clone());

    for obs in &actor.observers {
        let paused = PausedSession {
            session_id: &session_id,
            target_info: &ev.target_info,
            conn: &conn,
        };
        match tokio::time::timeout(
            actor.observer_timeout,
            AssertUnwindSafe(obs.on_target_attached(paused)).catch_unwind(),
        ).await {
            Ok(Ok(Ok(()))) => continue,
            Ok(Ok(Err(e))) => {
                tracing::error!(observer = obs.name(), %session_id, error = %e, "observer failed; detaching");
                let _ = conn.call_raw(
                    "Target.detachFromTarget",
                    json!({ "sessionId": &session_id }),
                    None,
                ).await;
                return;
            }
            Ok(Err(panic)) => {
                let msg = panic_payload(&panic);
                tracing::error!(observer = obs.name(), %session_id, panic = %msg, "observer panicked; detaching");
                let _ = conn.call_raw(
                    "Target.detachFromTarget",
                    json!({ "sessionId": &session_id }),
                    None,
                ).await;
                return;
            }
            Err(_) => {
                tracing::warn!(observer = obs.name(), %session_id, "observer timed out; releasing");
                break;
            }
        }
    }

    let _ = conn.call_raw(
        "Runtime.runIfWaitingForDebugger",
        json!({}),
        Some(session_id.clone()),
    ).await;
}
```

`spawn_actor` signature gains an `observers: Vec<Arc<dyn TargetObserver>>` parameter. `connect_with_observers(ws_url, observers)` becomes the new public entry; the existing `connect(ws_url)` becomes a convenience that passes `vec![]`.

### Tab + Element evaluate refactor

Per locked Approach B (per [memory: api-churn-acceptable-pre-release](../../../../../.claude/projects/-Users-rin-GitHub-zendriver-rs/memory/api-churn-acceptable-pre-release.md), API churn through P6 is fine):

```rust
impl Tab {
    /// Evaluate JS in an isolated world for this tab (sandbox per-frame, no
    /// access to page globals). Default for stealth-safe execution.
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T>;

    /// Evaluate JS in the main world (page globals accessible). Use when you
    /// need to read or modify `window.*` from the page.
    pub async fn evaluate_main<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T>;
}

impl Element {
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T>;
    pub async fn evaluate_main<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T>;
}
```

**Isolated-world implementation**:
- Per `Tab` (cached): call `Page.createIsolatedWorld { frameId: <main>, worldName: "zendriver-eval" }` once per main-frame attachment.
- Store the resulting `executionContextId` on the Tab inner. Re-create on `Page.frameNavigated` event (each navigation replaces the world).
- `Tab::evaluate` sends `Runtime.evaluate { expression, contextId: <isolated-id>, returnByValue: true, awaitPromise: true }`.

**Main-world implementation**: `Tab::evaluate_main` sends `Runtime.evaluate` with no `contextId` (Chrome picks the default world).

**P1 integration test fix**: rewrite `tab.evaluate("window.clicked")` as `tab.evaluate_main("window.clicked")` — the test intentionally reads a page global.

### `BrowserBuilder` additions

```rust
impl BrowserBuilder {
    pub fn stealth(mut self, profile: StealthProfile) -> Self {
        self.stealth = Some(profile);
        self
    }

    pub fn observer(mut self, obs: Arc<dyn TargetObserver>) -> Self {
        self.extra_observers.push(obs);
        self
    }
}
```

Default: `stealth = Some(StealthProfile::native())`. Explicit opt-out: `.stealth(StealthProfile::off())`.

## Data flow

### Launch sequence (P2 additions in bold)

1. Resolve Chrome executable. (P1)
2. Build user_data_dir. (P1)
3. **Resolve `Fingerprint` from profile (auto-detect or explicit). Returns early `StealthError` if Chrome version probe fails AND no override given (latter shouldn't happen due to hardcoded fallback).**
4. **Compose final launch flags: P1 defaults + `profile.extra_flags` + fingerprint-derived flags (`--lang={locale}` if set).**
5. Spawn Chrome via tokio; parse WS URL from stderr. (P1)
6. **Construct observer list: `vec![Arc::new(StealthObserver::new(profile, fingerprint)), ...extra_observers]`. (Empty if `StealthProfile::off()` and no extra observers.)**
7. **`connect_with_observers(ws_url, observers)` — spawns the actor with observers baked in.**
8. **Send `Target.setAutoAttach { autoAttach: true, waitForDebuggerOnStart: true, flatten: true }` at browser level (no session_id) — applies to all current and future targets.**
9. Call `Target.getTargets` and find the initial page target. (P1)
10. Call `Target.attachToTarget { targetId, flatten: true }` — this generates a `Target.attachedToTarget` event which the actor handles, running observers on the paused initial session, then releasing.
11. Wait briefly (≤200ms via event subscription) for observer flow to complete on the initial session — needed so the returned Tab is in a fully-patched state before user code runs.
12. Wrap session in `Tab`; return `Browser`.

### Per-target lifecycle (any target after launch)

```
Target.targetCreated   (Chrome spawns; auto-attach fires)
   ↓
Target.attachedToTarget arrives via WS
   ↓
ConnectionActor spawns handle_target_attached(...)
   ↓
For each observer:                     ← serial, in registration order
   tokio::time::timeout(5s,
       AssertUnwindSafe(observer.on_target_attached(paused)).catch_unwind())
   ↓
Release: Runtime.runIfWaitingForDebugger { sessionId }
   ↓
Target now runs page JS with all patches installed
```

## Error handling

### New error types

```rust
// zendriver-transport: extend existing
#[derive(Debug, thiserror::Error)]
pub enum ObserverError { /* see Components */ }

// zendriver-stealth: new
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StealthError {
    #[error("failed to apply patch '{patch}'")]
    PatchFailed { patch: &'static str, #[source] source: CallError },
    #[error("could not detect chrome version: {0}")]
    ChromeVersionDetect(String),
    #[error("could not read system info: {0}")]
    SystemInfo(String),
    #[error("invalid fingerprint override: {0}")]
    InvalidOverride(String),
}
```

### `ZendriverError` extension

```rust
// crates/zendriver/src/error.rs — add variant:
#[non_exhaustive]
pub enum ZendriverError {
    // ... existing P1 variants ...
    #[error("stealth: {0}")]
    Stealth(#[from] StealthError),
}
```

Since `#[non_exhaustive]` was set in P1, this addition is non-breaking.

`StealthObserver::on_target_attached` returns `Result<(), ObserverError>`. To bridge: `StealthError` cases are wrapped as `ObserverError::Other(stealth_err.to_string())` — `ObserverError` lives in transport crate so cannot directly depend on `StealthError`.

## Testing

### Tier 1 — Unit (MockConnection, no Chrome)

Targets:
- **StealthObserver dispatched-command sequence**: register observer with mocked Connection, emit `Target.attachedToTarget`, assert exact CDP method sequence (`Page.enable`, `Emulation.setUserAgentOverride`, `Emulation.setDeviceMetricsOverride`, `Emulation.setFocusEmulationEnabled`, `Page.addScriptToEvaluateOnNewDocument` for Spoofed, `Runtime.runIfWaitingForDebugger`).
- **Profile flag composition**: `StealthProfile::native()` flag set, `StealthProfile::spoofed()` flag set, `StealthProfile::off()` flag set — snapshot tests.
- **Fingerprint clamping**: 64 GB system → `memory_gb == 8`; 1 CPU system → `cpu_count == 2`; 3 GB system → `memory_gb == 4`.
- **Chrome version probe**: stub Command that emits `"Google Chrome 120.0.6099.234"` → `chrome_major == 120`, `chrome_full == "120.0.6099.234"`.
- **Chrome version probe failure**: stub Command returns exit 1 → fingerprint falls back to hardcoded major + emits a warning.
- **UA string composition**: known Fingerprint → known UA string (snapshot).
- **`UserAgentMetadata` JSON shape**: serializes to the exact structure CDP expects (snapshot).
- **TargetObserver actor wiring**: stub observer records calls, mock emits `Target.attachedToTarget`, assert observer fires with correct `session_id`.
- **Observer Err return**: stub returns `Err(ObserverError::Other(...))`, assert `Target.detachFromTarget` is sent.
- **Observer panic**: stub panics, assert `Target.detachFromTarget` is sent and actor keeps running (subsequent commands still work).
- **Observer timeout**: stub sleeps > 5s, assert `Runtime.runIfWaitingForDebugger` is sent after timeout (degraded path) and remaining observers skipped.
- **Multiple observers ordering**: 3 stubs, assert called in registration order, no parallelism.
- **`Tab::evaluate` isolated path**: mock emits result for `Page.createIsolatedWorld` (returns `contextId`), then `Runtime.evaluate { contextId }` returns value; assert correct types.
- **`Tab::evaluate_main` no contextId**: mock receives `Runtime.evaluate` with no `contextId` field present.

### Tier 2 — Integration (real Chrome, gated by `integration-tests` feature)

- **`phase2_native_profile_against_wiremock`**: launch with `StealthProfile::native()`, wiremock fixture records `navigator.userAgent`, assert no "HeadlessChrome" substring. Skip `navigator.webdriver` assertion (native doesn't patch it).
- **`phase2_spoofed_profile_against_wiremock`**: launch with `StealthProfile::spoofed()`, fixture records `navigator.webdriver`, assert `false`.
- **`phase2_isolated_world_evaluate`**: launch + nav to wiremock page that defines `window.evil = "should not be visible"`. Call `tab.evaluate::<Option<String>>("window.evil")` — assert `None` (isolated world doesn't see page globals).
- **`phase2_main_world_evaluate`**: same setup, call `tab.evaluate_main::<String>("window.evil")` — assert `"should not be visible"`.

### Tier 3 — Snapshot tests

- `native_profile_flags.snap`
- `spoofed_profile_flags.snap`
- `off_profile_flags.snap`
- `composed_ua_macintel_chrome_120.snap`
- `composed_ua_win32_chrome_120.snap`
- `composed_ua_linux_chrome_120.snap`
- `ua_metadata_macintel_chrome_120.snap` (JSON shape sent to CDP)

### Tier 4 — Stealth tests (NIGHTLY only, real Chrome, real internet)

Gated behind a new `stealth-tests` feature (separate from `integration-tests`).

- **`spoofed_passes_sannysoft_intoli_block`**: navigate to `https://bot.sannysoft.com`, wait for async tests, scrape pass/fail status of each row, assert zero failures in the Intoli rows (`User Agent`, `WebDriver`, `WebDriver Advanced`, `Chrome`, `Permissions`, `Plugins Length`, `Languages`, `WebGL Vendor`, `WebGL Renderer`, `Broken Image Dimensions`).
- **`spoofed_passes_areyouheadless`**: navigate to `https://arh.antoinevastel.com/bots/areyouheadless`, assert result text contains `"not Chrome headless"`.
- **`spoofed_passes_intoli_basic_test`**: navigate to `https://intoli.com/blog/not-possible-to-block-chrome-headless/chrome-headless-test.html`, scrape its 6-row test table, assert all green.
- **`native_fails_sannysoft_navigator_webdriver_but_passes_user_agent`**: opposite-direction assertion — proves the `native` profile honors its "no JS patches" contract while still scrubbing the headless UA.

CI gets a new workflow job:

```yaml
nightly-stealth-tests:
  schedule:
    - cron: '0 6 * * *'
  runs-on: ubuntu-latest
  continue-on-error: true   # external sites flake; don't block
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - uses: Swatinem/rust-cache@v2
    - name: Install Chromium
      run: sudo apt-get update && sudo apt-get install -y chromium-browser
    - run: cargo test --workspace --features stealth-tests --test stealth_phase2 -- --test-threads=1
```

### Determinism rules

| Source of nondeterminism | Mitigation |
|---|---|
| `Fingerprint::auto_detect` depends on host system | Tests use explicit `Fingerprint::new(...)` constructors instead of auto-detect |
| `chrome --version` output across machines | Mock the Command via the existing `MockConnection`-style helper or skip auto-detect in unit tests |
| sannysoft / areyouheadless flake | nightly-only, `continue-on-error: true`, no PR-blocking gate |
| Observer ordering races | Observers run serially, single-threaded; no test flake |
| `Page.createIsolatedWorld` race with navigation | Subscribe to `Page.frameNavigated` and re-create the isolated world; integration test exercises this path |

## Assumptions (delegate mode — judgement calls made on user's behalf)

These are the calls I made without explicit user input. Review and push back on any:

1. **Page targets only patched.** `StealthObserver` skips iframe / worker targets — they get released (no debugger pause kept) but no patches applied. Rationale: workers have no DOM, iframes inherit via parent's contextId in flat mode. If a future detector probes iframe `navigator` directly, revisit.
2. **`Spoofed` profile turns `bypass_csp` ON by default.** Some strict-CSP sites would otherwise block our injected patches. Trade-off: 1 small detection vector (a site probing whether CSP got modified) for guaranteeing patches install. User can `.bypass_csp(false)` to opt out.
3. **`memory_gb` clamped to {4, 8}.** W3C spec for `navigator.deviceMemory` allows {0.25, 0.5, 1, 2, 4, 8} — I dropped values below 4 because (a) sub-4GB consumer devices are rare on desktop, (b) sub-4 reads as "this is a low-end mobile" and increases bot-probability score on detectors that weight this.
4. **`cpu_count` clamped [2, 32].** Lower bound 2 because single-core desktops are extinct; upper bound 32 because >32 reads as server-class and decreases plausibility for a consumer browsing.
5. **Chrome version hardcoded fallback = `120.0.6099.234`.** Used when `chrome --version` probe fails. Will rot; doc the policy "bump on each release of zendriver-rs".
6. **Default `screenWidth/screenHeight = 1920x1080`** in `Emulation.setDeviceMetricsOverride`. This is the most common desktop resolution. Future P3+ could randomize per-launch, but P2 ships a single value.
7. **`observer_timeout = 5s`** is a fixed default, not user-tunable in P2. If we see real-world cases of slow stealth setup (say, on overloaded CI), we'll expose a `BrowserBuilder::observer_timeout(Duration)` knob later.
8. **`Tab::evaluate` switch is breaking change to P1 contract** — per stored memory "API churn OK pre-release", this is intentional and the P1 integration test gets rewritten. No deprecation cycle, no shim.
9. **JS patches written as a single bootstrap factory** (`function(fp){...all patches concatenated...}(<fp-json>)`) rather than separate `Page.addScriptToEvaluateOnNewDocument` calls. One CDP round-trip per nav instead of nine. Tradeoff: harder to disable individual patches.
10. **`StealthProfile::off()` is a real ProfileKind**, not just absence-of-stealth. Wires `vec![]` observers but still goes through the same `BrowserBuilder::stealth(...)` API for consistency. The "completely raw browser" use case has a named API.
11. **No randomization across launches** within a single `StealthProfile`. Calling `Browser::builder().stealth(StealthProfile::spoofed()).launch()` twice produces identical Fingerprints (modulo system state changes). Per-launch randomization is post-P2 work.
12. **`stealth-tests` feature is separate from `integration-tests`**, with a separate CI cron schedule. Rationale: stealth-tests hit real internet and flake; we don't want them to block PR merges or appear in standard `cargo test` runs.

## Roadmap

| Phase | Status | Goal |
|---|---|---|
| 1 | DONE | Foundation: transport + minimal Tab/Element |
| **2 (this spec)** | IN PROGRESS | Stealth: launch flags + JS patches + TargetObserver + chaser-oxide additions |
| 3 | planned | Element API completeness (xpath/text/role, hover/focus/scroll, type_text, attrs) |
| 4 | planned | Tab/Browser completeness (cookies, storage, screenshots, multi-tab, iframes) |
| 5 | planned | Optional gated features (interception, cloudflare, expect, fetcher) |
| 6 | planned | Polish + crates.io publish |

Rough P2 sizing: 2-3 weeks solo.

## Brainstorm cross-ref

This spec captures the brainstorm session of 2026-05-23 (delegate mode for sections 3-6). Decisions locked during brainstorming:

- **Profile shape:** two named profiles (`native` default, `spoofed` opt-in, `off` escape-hatch).
- **Patch injection timing:** paused-target flow via `Target.setAutoAttach { waitForDebuggerOnStart }`. The reserved P1 hook (which P1 didn't actually wire) lands in P2.
- **Fingerprint:** auto-detect at launch (sysinfo + num_cpus + `chrome --version`) with per-field override.
- **API wiring:** `BrowserBuilder::stealth(StealthProfile)`, default native.
- **Eval semantics:** isolated-world by default (`Tab::evaluate`, `Element::evaluate`); main-world is the explicit opt-in (`Tab::evaluate_main`, `Element::evaluate_main`). Breaking change to P1 contract, accepted because no users yet.
- **Stealth scope:** zendriver Python's full surface (launch flags + UA scrub) + chaser-oxide's protocol-level additions (UserAgentMetadata, setDeviceMetricsOverride, setFocusEmulationEnabled, isolated-world eval, Navigator.prototype patches). Bezier mouse paths + typing realism deferred to P3.
