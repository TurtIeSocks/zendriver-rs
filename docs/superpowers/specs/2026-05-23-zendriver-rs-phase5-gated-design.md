# zendriver-rs — Phase 5: Gated Optional Features

**Date:** 2026-05-23
**Status:** Approved (delegate-mode brainstorming complete, ready for implementation plan)
**Phase:** 5 of 6 — see [Roadmap](#roadmap)
**Depends on:** Phases 1-4, all on `main`

## Summary

Populate the four phase-deferred stub crates from P1: `zendriver-interception` (Fetch.* CDP wrapper with rule-based + stream-escape-hatch API), `zendriver-cloudflare` (Turnstile/challenge click bypass ported from `cloudflare.py`), `zendriver-fetcher` (custom Chrome-binary downloader for CI environments without pre-installed Chrome), plus an `expect()` module in core `zendriver` (Playwright-style pre-register + await for waiting on network requests/responses/dialogs/downloads). All shipped behind cargo feature flags so users pay only for what they import.

Phase 5 exit criterion: each sub-crate + module ships with at least one example exercising its API + one integration test (gated under that sub-crate's feature flag). Cloudflare gets a separate nightly stealth test against the Turnstile demo page.

## Goals

- **`zendriver-interception`** populated. Public API: `tab.intercept()` returns `InterceptBuilder`. Declarative rules: `block(pattern)`, `redirect(from, to)`, `respond(pattern, status, headers, body)`, `modify_request(pattern, |req| RequestOverrides)`. Stream escape hatch: `subscribe(pattern, stage) -> impl Stream<Item = PausedRequest>` where the user calls `paused.continue_()/abort()/respond()/modify_and_continue()` on each. Background actor consumes `Fetch.requestPaused` events, matches rules, dispatches via `Fetch.continueRequest`/`failRequest`/`fulfillRequest`.
- **`expect()` module** in `zendriver` core. Methods on Tab: `expect_request(pattern)`, `expect_response(pattern)`, `expect_dialog()`, `expect_download()`. Returns `Expectation<T>` handle that subscribes to the matching CDP event immediately (avoiding race) + resolves via `.await` to the matched event payload. Pattern arg accepts `&str` (substring) or `regex::Regex`.
- **`zendriver-cloudflare`** populated. `tab.cloudflare()` returns a `CloudflareBypass` builder. `wait_for_clearance(timeout)` walks the page's shadow DOM for `challenges.cloudflare.com`, locates the Turnstile checkbox iframe, computes click coordinates at the canonical 15% offset from top-left, synthesizes the click via the existing realistic-mouse path (P3 InputController). Loops until the challenge `cf-turnstile-response` input has a value OR disappears, OR until timeout fires. Ported behavior-for-behavior from Python's `cloudflare.py`.
- **`zendriver-fetcher`** populated. Custom impl from scratch (not wrapping chromiumoxide_fetcher). Public API: `Fetcher::new()` returns a `Fetcher` configured with a cache dir + version + platform; `Fetcher::ensure_chrome()` downloads-if-missing and returns the executable path. Uses Chrome for Testing JSON API (`https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json`) for the download URL resolution, then HTTP GET via `reqwest` + zip/dmg extraction. Atomic cache write (download to temp, rename on completion). Optional integrity check via SHA-256 from the JSON manifest.
- **All four sub-areas behind cargo feature flags**. `zendriver` crate gains 4 new optional dep entries + 4 corresponding features (`interception`, `expect`, `cloudflare`, `fetcher`). User `cargo add zendriver --features "stealth interception"` to opt in.
- **Error handling**: each sub-crate exports its own `*Error` enum that flows into `ZendriverError` via new `#[from]` variants. All `#[non_exhaustive]`.
- **Tests**: per-crate unit tests with `MockConnection`. Per-crate integration tests against real Chrome (interception, cloudflare). Fetcher gets unit tests for URL resolution + integration test gated behind `fetcher-network-tests` that actually downloads Chrome (long-running, opt-in).

## Non-goals

- HTTP/3 / QUIC interception (Chrome can but plumbing is heavier).
- Custom DNS resolution.
- Service Worker / WebRTC interception (separate CDP domains beyond Fetch).
- Cloudflare Turnstile *invisible-mode* — only the interactive variant. Python upstream same limit.
- bot.incolumitas-style behavioral classifier evasion (P2 stealth tier; out-of-scope here).
- Drop-in Selenium-WebDriver API surface — explicit non-goal since P1.
- Pre-1.0 SemVer guarantees — per memory `api-churn-acceptable-pre-release`, API can churn through P6.

## Architecture

### Crate + feature flag layout

```
zendriver crate Cargo.toml:
[features]
default                = ["stealth"]
stealth                = ["dep:zendriver-stealth"]
interception           = ["dep:zendriver-interception"]
expect                 = []                                 # in-tree module
cloudflare             = ["interception", "dep:zendriver-cloudflare"]
fetcher                = ["dep:zendriver-fetcher"]
testing                = ["zendriver-transport/testing"]
integration-tests      = ["dep:wiremock", "dep:serial_test", "interception", "expect", "cloudflare"]
stealth-tests          = ["integration-tests"]
fetcher-network-tests  = ["fetcher"]                        # opt-in; downloads real Chrome
```

`cloudflare` implies `interception` (the CF bypass uses Fetch.* to detect challenge load completion). `expect` is in-tree (lives in `zendriver/src/expect/`) — no separate crate because tightly coupled to Tab's session.

### File layout

```
crates/zendriver-interception/src/
├── lib.rs                  # InterceptBuilder + InterceptionError re-exports
├── builder.rs              # InterceptBuilder + rule registration (block/redirect/respond/modify_request/subscribe)
├── actor.rs                # Background task: subscribes Fetch.requestPaused, matches rules, dispatches Fetch.* responses
├── rule.rs                 # Rule enum + matching logic + URL pattern type
├── paused.rs               # PausedRequest handle (returned via subscribe stream) with .continue_/abort/respond/modify methods
├── error.rs                # InterceptionError
└── url_pattern.rs          # Glob-style pattern matcher (* and ?) + regex fallback

crates/zendriver/src/expect/
├── mod.rs                  # Expectation<T> + Tab::expect_request/expect_response/expect_dialog/expect_download
├── request.rs              # request matching impl
├── response.rs             # response matching + body fetching impl
├── dialog.rs               # Page.javascriptDialogOpened subscription
└── download.rs             # Page.downloadWillBegin + Page.downloadProgress subscriptions

crates/zendriver-cloudflare/src/
├── lib.rs                  # CloudflareBypass + CloudflareError re-exports
├── bypass.rs               # CloudflareBypass struct + wait_for_clearance impl
├── detection.rs            # Shadow-DOM walk for challenges.cloudflare.com host
├── click.rs                # Bbox computation + click dispatch via InputController
└── error.rs                # CloudflareError

crates/zendriver-fetcher/src/
├── lib.rs                  # Fetcher + FetcherError re-exports
├── fetcher.rs              # Fetcher struct + ensure_chrome
├── platform.rs             # Platform enum + per-OS download URL mapping
├── version.rs              # Version resolution: latest / stable / explicit / channel
├── manifest.rs             # Chrome for Testing JSON API client + struct types
├── download.rs             # HTTP GET via reqwest + progress reporting + integrity check
├── extract.rs              # zip/dmg/tar.bz2 extraction per platform
├── cache.rs                # Atomic cache write + cache layout (~/.cache/zendriver/chrome/<version>/)
└── error.rs                # FetcherError
```

### Dependency graph delta

```
zendriver
  ├─ zendriver-transport
  ├─ zendriver-stealth       (default feature)
  ├─ zendriver-interception  (optional; feature: interception)
  ├─ zendriver-cloudflare    (optional; feature: cloudflare → enables interception)
  └─ zendriver-fetcher       (optional; feature: fetcher)
```

### External dependencies added

| Crate | Feature | Purpose | Used by |
|---|---|---|---|
| `reqwest` (rustls-tls-only, gzip+deflate, blocking off) | `fetcher` | HTTP download | zendriver-fetcher |
| `zip` (deflate, time off) | `fetcher` | Extract Linux/Windows Chrome zips | zendriver-fetcher |
| `sha2` | `fetcher` | SHA-256 integrity check (optional) | zendriver-fetcher |
| `glob` (or hand-roll) | `interception` | URL pattern matching | zendriver-interception |
| `dirs` | `fetcher` | OS cache dir lookup | zendriver-fetcher |

`reqwest` is the biggest add — ~1.2k LOC of transitive deps. Gated behind `fetcher` feature so users who don't enable it pay nothing.

## Components — zendriver-interception

### Public API

```rust
// zendriver crate (re-exports under feature flag):
#[cfg(feature = "interception")]
pub use zendriver_interception::{InterceptBuilder, PausedRequest, RequestStage, ResourceType, InterceptionError};

// Tab method (gated):
#[cfg(feature = "interception")]
impl Tab {
    pub fn intercept(&self) -> InterceptBuilder<'_>;
}
```

### InterceptBuilder

```rust
pub struct InterceptBuilder<'tab> {
    tab: &'tab Tab,
    patterns: Vec<RequestPattern>,
    rules: Vec<Rule>,
}

impl<'tab> InterceptBuilder<'tab> {
    /// Add an interception pattern: URL glob + stage + resource type.
    pub fn pattern(mut self, pattern: impl Into<String>) -> Self;
    pub fn at_request(self) -> Self;           // RequestStage::Request
    pub fn at_response(self) -> Self;          // RequestStage::Response
    pub fn resource(self, kind: ResourceType) -> Self;

    /// Declarative rules. Each rule has its own pattern.
    pub fn block(mut self, pattern: impl Into<String>) -> Self;
    pub fn redirect(mut self, from: impl Into<String>, to: impl Into<String>) -> Self;
    pub fn respond(mut self, pattern: impl Into<String>, status: u16, headers: Vec<(String, String)>, body: Vec<u8>) -> Self;
    pub fn modify_request(mut self, pattern: impl Into<String>, modify: impl Fn(&RequestInfo) -> RequestOverrides + Send + Sync + 'static) -> Self;

    /// Start interception with the registered rules + patterns.
    /// Returns an InterceptHandle; drop or call .stop() to disable.
    pub async fn start(self) -> Result<InterceptHandle, InterceptionError>;

    /// Stream escape hatch: returns a stream of PausedRequest for the user
    /// to drive manually. Does NOT register declarative rules — for full
    /// control. Pattern + stage required.
    pub fn subscribe(self) -> impl Stream<Item = PausedRequest> + Send;
}

pub struct InterceptHandle {
    cancel: CancellationToken,
    // Drop cancels the background task + sends Fetch.disable
}

impl InterceptHandle {
    pub async fn stop(self) -> Result<(), InterceptionError>;
}

pub struct PausedRequest {
    pub request_id: String,
    pub request: RequestInfo,
    pub response: Option<ResponseInfo>,  // populated for at_response stage
    tab: Tab,
}

pub struct RequestInfo {
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub post_data: Option<Vec<u8>>,
    pub resource_type: ResourceType,
}

pub struct ResponseInfo {
    pub status: u16,
    pub status_text: String,
    pub headers: HashMap<String, String>,
}

impl PausedRequest {
    pub async fn continue_(self) -> Result<(), InterceptionError>;
    pub async fn abort(self, reason: AbortReason) -> Result<(), InterceptionError>;
    pub async fn respond(self, status: u16, headers: Vec<(String, String)>, body: Vec<u8>) -> Result<(), InterceptionError>;
    pub async fn modify_and_continue(self, overrides: RequestOverrides) -> Result<(), InterceptionError>;
    pub async fn body(&self) -> Result<Vec<u8>, InterceptionError>;  // Fetch.getResponseBody
}

pub struct RequestOverrides {
    pub url: Option<String>,
    pub method: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub post_data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
pub enum AbortReason { Failed, Aborted, TimedOut, AccessDenied, ConnectionClosed, ConnectionReset, ConnectionRefused, ConnectionAborted, ConnectionFailed, NameNotResolved, InternetDisconnected, AddressUnreachable, BlockedByClient, BlockedByResponse }

#[derive(Debug, Clone, Copy)]
pub enum RequestStage { Request, Response }

#[derive(Debug, Clone, Copy)]
pub enum ResourceType { Document, Stylesheet, Image, Media, Font, Script, TextTrack, XHR, Fetch, EventSource, WebSocket, Manifest, SignedExchange, Ping, CSPViolationReport, Preflight, Other }
```

### Actor implementation

`InterceptBuilder::start()` spawns a background task that:
1. Calls `Fetch.enable { patterns: <combined from rules + explicit patterns>, handleAuthRequests: false }`.
2. Subscribes to `Fetch.requestPaused` events on the tab's session.
3. For each event: walks the rules in registration order; first match wins.
   - `Block` → `Fetch.failRequest { requestId, errorReason: "BlockedByClient" }`.
   - `Redirect` → `Fetch.continueRequest { requestId, url: target }`.
   - `Respond` → `Fetch.fulfillRequest { requestId, responseCode, responseHeaders, body: base64 }`.
   - `Modify` → call user closure with RequestInfo, get RequestOverrides, dispatch `Fetch.continueRequest { requestId, url?, method?, headers?, postData? }`.
   - No match → `Fetch.continueRequest { requestId }` (let it through).
4. On cancellation: `Fetch.disable` then exit.

`subscribe()` skips rule processing entirely; pushes raw `PausedRequest` to an mpsc channel exposed as a Stream. User code on the stream side calls explicit `.continue_/.abort/.respond` to dispose of each.

### URL pattern matching

Use Chrome's CDP pattern syntax (`*` = wildcard, `?` = single char) per Fetch domain spec. `url_pattern.rs` translates to Rust regex internally for the rule-side match (Chrome itself filters at the wire level via the `patterns` array on `Fetch.enable`, but the rule-side needs to differentiate WHICH rule matched).

## Components — expect()

```rust
#[cfg(feature = "expect")]
pub use crate::expect::{Expectation, RequestExpectation, ResponseExpectation, DialogExpectation, DownloadExpectation};

#[cfg(feature = "expect")]
impl Tab {
    pub fn expect_request(&self, pattern: impl Into<UrlMatcher>) -> RequestExpectation;
    pub fn expect_response(&self, pattern: impl Into<UrlMatcher>) -> ResponseExpectation;
    pub fn expect_dialog(&self) -> DialogExpectation;
    pub fn expect_download(&self) -> DownloadExpectation;
}

pub enum UrlMatcher { Substring(String), Regex(regex::Regex) }
impl From<&str> for UrlMatcher { /* Substring */ }
impl From<String> for UrlMatcher { /* Substring */ }
impl From<regex::Regex> for UrlMatcher { /* Regex */ }

pub struct RequestExpectation {
    rx: oneshot::Receiver<MatchedRequest>,
    // Optional timeout (default 30s)
    timeout: Duration,
}

impl RequestExpectation {
    pub fn timeout(mut self, dur: Duration) -> Self;
    pub async fn matched(self) -> Result<MatchedRequest>;   // = await impl Future
}

impl std::future::Future for RequestExpectation { type Output = Result<MatchedRequest>; /* ... */ }

pub struct MatchedRequest {
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub post_data: Option<Vec<u8>>,
    pub request_id: String,
}
```

Similar shapes for `ResponseExpectation` (gives `MatchedResponse` with status + headers + body via `Network.getResponseBody`), `DialogExpectation` (gives `MatchedDialog` with `accept(prompt_text)` / `dismiss()` methods), `DownloadExpectation` (gives `MatchedDownload` with `path()` + `save_to(path)`).

Each `expect_*` method:
1. Spawns a subscriber task on the tab's session for the relevant CDP event.
2. Task filters incoming events by pattern.
3. On first match, sends through the oneshot channel + cancels itself.
4. Returns the Expectation handle with the rx half.
5. User triggers action (e.g. `button.click().await?`).
6. User awaits the Expectation; future resolves with the matched event.

If the trigger doesn't happen / event never fires within `timeout`, returns `ZendriverError::Timeout(timeout)`.

## Components — zendriver-cloudflare

### Public API

```rust
// zendriver crate (re-exports under feature flag):
#[cfg(feature = "cloudflare")]
pub use zendriver_cloudflare::{CloudflareBypass, CloudflareError};

#[cfg(feature = "cloudflare")]
impl Tab {
    pub fn cloudflare(&self) -> CloudflareBypass<'_>;
}
```

### CloudflareBypass

```rust
pub struct CloudflareBypass<'tab> {
    tab: &'tab Tab,
    poll_interval: Duration,
}

impl<'tab> CloudflareBypass<'tab> {
    pub fn poll_interval(mut self, dur: Duration) -> Self;        // default 500ms

    /// Walks the page DOM looking for Cloudflare Turnstile challenge UI.
    /// If found, clicks the checkbox at the canonical 15% offset, then
    /// polls for `cf-turnstile-response` input value (or challenge UI
    /// disappearing). Returns Ok once cleared OR Err on timeout.
    pub async fn wait_for_clearance(self, timeout: Duration) -> Result<ClearanceOutcome, CloudflareError>;

    /// Detects whether the current page is showing a Turnstile challenge.
    /// Cheaper than wait_for_clearance — single shadow-DOM walk.
    pub async fn is_challenge_present(&self) -> Result<bool, CloudflareError>;
}

pub enum ClearanceOutcome {
    /// `cf-turnstile-response` input got a non-empty value.
    TokenAcquired(String),
    /// Challenge UI disappeared without a token (e.g. JS clearance cookie set).
    ChallengeGone,
}
```

### Bypass implementation

Port from `cloudflare.py:126-269`:
1. `evaluate_main` a JS function that recursively walks the document looking for shadow roots hosting `challenges.cloudflare.com` iframes. Returns the iframe's bounding box.
2. Compute click point: bbox.left + bbox.width * 0.15, bbox.top + bbox.height * 0.50 (per Python's offset).
3. Dispatch click via `Tab::cdp()` → `Input.dispatchMouseEvent` (use raw, not realistic — Cloudflare's bot detection wants a real click on the visible checkbox, the realistic Bezier path is overkill).
4. Poll every `poll_interval`: `evaluate_main` checks for `[name="cf-turnstile-response"]` input value OR challenge container removal.
5. Return on success or timeout.

## Components — zendriver-fetcher

### Public API

```rust
// zendriver crate (re-exports under feature flag):
#[cfg(feature = "fetcher")]
pub use zendriver_fetcher::{Fetcher, FetcherError, Channel, FetcherProgress};
```

### Fetcher

```rust
pub struct Fetcher {
    cache_dir: PathBuf,
    version: VersionSpec,
    platform: Platform,
    progress_cb: Option<Box<dyn Fn(FetcherProgress) + Send + Sync>>,
}

#[derive(Debug, Clone)]
pub enum VersionSpec {
    Latest,
    Stable,
    Channel(Channel),                // Stable | Beta | Dev | Canary
    Explicit(String),                // e.g. "120.0.6099.234"
}

#[derive(Debug, Clone, Copy)]
pub enum Channel { Stable, Beta, Dev, Canary }

#[derive(Debug, Clone, Copy)]
pub enum Platform { LinuxX64, MacX64, MacArm64, Win32, Win64 }

#[derive(Debug, Clone)]
pub struct FetcherProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub phase: FetcherPhase,
}

#[derive(Debug, Clone, Copy)]
pub enum FetcherPhase { Resolving, Downloading, Extracting, Verifying, Done }

impl Fetcher {
    pub fn new() -> Self;
    pub fn cache_dir(mut self, dir: impl Into<PathBuf>) -> Self;        // default: dirs::cache_dir().join("zendriver/chrome")
    pub fn version(mut self, spec: VersionSpec) -> Self;                // default: Latest
    pub fn platform(mut self, p: Platform) -> Self;                     // default: auto-detect
    pub fn on_progress(mut self, cb: impl Fn(FetcherProgress) + Send + Sync + 'static) -> Self;

    /// Returns the path to the Chrome executable for the configured
    /// version/platform. Downloads + extracts to cache_dir if missing.
    /// Idempotent: re-uses cached binary if already present.
    pub async fn ensure_chrome(self) -> Result<PathBuf, FetcherError>;

    /// Just resolve the download URL without downloading.
    pub async fn resolve_url(&self) -> Result<String, FetcherError>;

    /// Clear the cache.
    pub async fn clear_cache(&self) -> Result<(), FetcherError>;
}
```

### Implementation flow

1. **Resolve version** → query Chrome for Testing JSON API at `https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json`. Parse JSON. For `Latest`/`Stable`/`Channel`, walk versions list + pick. For `Explicit`, look up directly.
2. **Resolve platform URL** → from the resolved version's `downloads.chrome[]` array, pick the entry matching the configured platform.
3. **Check cache** → `cache_dir/<version>/chrome` exists and is executable? → return early.
4. **Download** → HTTP GET via `reqwest`. Stream to a temp file in cache_dir (e.g. `cache_dir/<version>.tmp.zip`) with progress callback.
5. **Verify** → optional SHA-256 against manifest's hash.
6. **Extract** → zip on Linux/Windows, dmg on macOS (use `hdiutil attach` + copy from mounted volume for macOS; tar.bz2 was old format, skip).
7. **Atomic rename** → `cache_dir/<version>.tmp/` → `cache_dir/<version>/`.
8. **Mark executable** → `chmod +x` on Unix.
9. **Return path** to the binary.

### Browser builder convenience

```rust
#[cfg(feature = "fetcher")]
impl BrowserBuilder {
    /// Convenience: ensure Chrome is downloaded + cached, then use its path
    /// as the executable. Equivalent to:
    ///     let path = Fetcher::new().ensure_chrome().await?;
    ///     self.executable(path)
    pub async fn ensure_chrome(self) -> Result<Self, ZendriverError>;
}
```

## Error handling

Each sub-crate exports its own error enum:

```rust
// zendriver-interception
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InterceptionError {
    #[error("call failed: {0}")]
    Call(#[from] zendriver_transport::CallError),
    #[error("invalid url pattern: {0}")]
    InvalidPattern(String),
    #[error("interception already started")]
    AlreadyStarted,
    #[error("interception not started")]
    NotStarted,
}

// zendriver-cloudflare
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CloudflareError {
    #[error("no Turnstile challenge detected")]
    NoChallenge,
    #[error("clearance timed out")]
    ClearanceTimeout,
    #[error("tab error: {0}")]
    Tab(#[source] Box<crate::ZendriverError>),
}

// zendriver-fetcher
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FetcherError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest parse error: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error("version not found: {0}")]
    VersionNotFound(String),
    #[error("platform not supported")]
    UnsupportedPlatform,
    #[error("integrity check failed: expected {expected}, got {actual}")]
    IntegrityFailed { expected: String, actual: String },
    #[error("extraction failed: {0}")]
    Extraction(String),
}
```

`ZendriverError` gains `#[from]` variants for each:

```rust
#[non_exhaustive]
pub enum ZendriverError {
    // existing variants ...
    #[cfg(feature = "interception")]
    #[error("interception: {0}")]
    Interception(#[from] zendriver_interception::InterceptionError),

    #[cfg(feature = "cloudflare")]
    #[error("cloudflare: {0}")]
    Cloudflare(#[from] zendriver_cloudflare::CloudflareError),

    #[cfg(feature = "fetcher")]
    #[error("fetcher: {0}")]
    Fetcher(#[from] zendriver_fetcher::FetcherError),
}
```

## Testing

### Tier 1 — Unit (mocked CDP, no Chrome)
- **Interception:** rule matching by URL pattern; block/redirect/respond payload construction; subscribe stream emits PausedRequest on event; modify_and_continue dispatches correct overrides.
- **expect:** RequestExpectation.matched() returns event matching pattern; timeout fires correctly; multiple expectations don't interfere.
- **Cloudflare:** detection JS walks shadow DOM correctly (test via mock that intercepts Runtime.evaluate); click coordinate computation matches Python's 15% offset.
- **Fetcher:** version resolution from a stubbed JSON manifest; URL resolution per platform; cache hit short-circuits download; SHA-256 verification fails on tampered data.

### Tier 2 — Integration (real Chrome + wiremock, gated `integration-tests`)
- **Interception:** block rule prevents request from reaching wiremock; respond rule serves fake JSON; redirect rule shifts URL.
- **expect:** `expect_response("*/api/*")` + click → resolves with the matched response body.
- **Cloudflare:** No reliable test fixture (live Cloudflare sites). Test scaffolding present; actual run is part of nightly stealth.
- **Fetcher:** Stub Chrome for Testing JSON API endpoint locally (wiremock) + serve a small dummy "chrome.zip" containing a sentinel file; assert Fetcher downloads + extracts + caches.

### Tier 3 — Snapshot
- URL pattern → regex conversion table
- Cloudflare detection JS source (insta snapshot of the bundled JS string)
- Fetcher progress event sequence for a known download

### Tier 4 — Nightly (external sites, gated `stealth-tests` + new `cloudflare-tests`)
- Cloudflare bypass against `https://nopecha.com/demo/cloudflare` Turnstile demo page. Expects ClearanceOutcome::TokenAcquired within 30s.

### Tier 5 — Network-heavy fetcher tests (gated `fetcher-network-tests`, manual-run only)
- Fetcher actually downloads + extracts Chrome Stable. Asserts the resulting executable runs `--version` successfully. Long-running (~30-60s per OS); not in PR CI.

## Assumptions (delegate mode — judgement calls)

1. **`cloudflare` feature implies `interception`** — declared in Cargo.toml feature dep. Reason: CF bypass may need to detect challenge-page load completion via Fetch events in some flows.
2. **`expect()` lives in zendriver core**, not its own crate. Tightly coupled to Tab + Network/Page domains; no benefit from crate boundary.
3. **URL pattern syntax mirrors CDP's** (`*` wildcard, `?` single-char). Documented; users coming from CDP docs don't need to learn a new dialect. Regex is an opt-in via the explicit `regex::Regex` constructor on UrlMatcher.
4. **`PausedRequest` is consumed by-value on .continue_/abort/respond/modify_and_continue** — once you act on it, it's gone. Prevents double-dispatch bugs.
5. **`InterceptHandle` stops interception on Drop** — RAII. User can also call `.stop().await` for explicit + awaited cleanup.
6. **Rules match in registration order, first match wins.** No catch-all semantics; user controls priority by registration order. If no rule matches, request continues unmodified.
7. **`expect_request` returns the FIRST matching request after registration** — additional matches are dropped. For multi-match use cases, use the interception stream.
8. **`expect_*` registers subscription BEFORE returning** — solves the trigger-runs-before-await race. Subscription is alive from the moment the method returns.
9. **`expect_dialog` auto-dismisses if not handled** by closure within Page.javascriptDialogOpened handling — defensive, but user may want to handle. Document the default: explicit user must call `.accept()/.dismiss()`. We do NOT auto-handle.
10. **Cloudflare bypass uses raw mouse dispatch** (not realistic Bezier from P3). Reason: Turnstile checks the click is on the visible checkbox + within a sub-second realistic interval; the Bezier overhead adds zero stealth benefit for this specific surface and risks moving outside the checkbox bbox.
11. **Fetcher uses Chrome for Testing JSON** (`googlechromelabs.github.io/chrome-for-testing/...`). Not "Chromium snapshots" (older system; no signed releases). Chrome for Testing is the modern recommended path.
12. **Fetcher cache layout** = `cache_dir/<version>/chrome` (Linux), `cache_dir/<version>/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing` (macOS), `cache_dir/<version>/chrome.exe` (Win). Exact paths from Chrome for Testing zip layout.
13. **Fetcher skips integrity check by default** (no SHA in CFT manifest for all platforms historically) — optional opt-in via `Fetcher::verify_integrity(true)`. Default off.
14. **Fetcher download retry policy** = 3 attempts with exponential backoff (500ms / 2s / 8s). On final failure, surface as `FetcherError::Http`.
15. **No proxy support on fetcher** in P5 — uses default reqwest config. Users behind corporate proxies set `HTTPS_PROXY` env var; reqwest respects it automatically.
16. **No tar.bz2 extraction** — Chrome for Testing ships zip everywhere now. Old Chromium snapshots used tar.bz2 on Linux; if support needed, add later.
17. **`BrowserBuilder::ensure_chrome()` convenience** is async (does network). Two-step is fine for users who want explicit control.
18. **Subscriber task lifetime** — interception + expect tasks tied to `CancellationToken` derived from the Tab's `network_cancel` (P4 InFlightTracker pattern). Tab Drop cancels everything.
19. **Default expect timeout = 30s.** Same as P4 wait_for_idle. Override per-call via `.timeout(dur)` builder method.
20. **No streaming/chunked response handling in interception** — `respond()` takes a full body Vec<u8>. Streaming would complicate the API significantly; revisit only if a real user demands it.

## Roadmap

| Phase | Status | Goal |
|---|---|---|
| 1 | DONE | Foundation |
| 2 | DONE | Stealth |
| 3 | DONE | Element + input + actionability |
| 4 | DONE | Multi-tab + cookies + storage + frames |
| **5 (this spec)** | IN PROGRESS | Optional gated features: interception, expect, cloudflare, fetcher |
| 6 | planned | Polish + crates.io publish |

Sizing: 4-5 weeks solo (largest single phase due to 4 sub-areas).

## Brainstorm cross-ref

Decisions locked during brainstorming:
- **Scope:** single P5 covering all 4 sub-areas.
- **Interception API:** hybrid rule-based + stream escape hatch.
- **expect() API:** Playwright-style pre-register + await.
- **Fetcher:** custom impl from scratch (not wrapping chromiumoxide_fetcher).
- **Feature gating:** each sub-area behind its own cargo feature; `cloudflare` implies `interception`; `expect` lives in core.
