# zendriver-rs Phase 5 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use `- [ ]` checkbox syntax.

**Goal:** Populate the 4 P1-deferred stub crates (`zendriver-interception`, `zendriver-cloudflare`, `zendriver-fetcher`) + add in-tree `expect()` module. All behind cargo feature flags.

**Architecture:** 4 sub-areas. Each gated behind its own cargo feature (`interception`, `expect`, `cloudflare`, `fetcher`). `cloudflare` implies `interception`. Each sub-crate has its own `*Error` enum flowing into `ZendriverError` via `#[cfg(feature = "...")]` `#[from]` variants. New external deps (reqwest, zip, sha2, glob, dirs) all gated behind feature flags so users who don't opt in pay nothing.

**Tech Stack:** Same as P4 + new gated deps: `reqwest` (rustls-tls), `zip`, `sha2`, `dirs`, `glob` (or hand-rolled URL pattern matcher).

**Spec:** [docs/superpowers/specs/2026-05-23-zendriver-rs-phase5-gated-design.md](../specs/2026-05-23-zendriver-rs-phase5-gated-design.md)

---

## File structure

See spec for full layout. Quick summary:

- `crates/zendriver-interception/src/` populated: lib, builder, actor, rule, paused, error, url_pattern
- `crates/zendriver/src/expect/` NEW (in-tree): mod, request, response, dialog, download
- `crates/zendriver-cloudflare/src/` populated: lib, bypass, detection, click, error
- `crates/zendriver-fetcher/src/` populated: lib, fetcher, platform, version, manifest, download, extract, cache, error

## Task list

| # | Title | Files |
|---|---|---|
| 0 | Cargo feature plumbing + ZendriverError gated variants | workspace Cargo.toml, zendriver Cargo.toml, zendriver-* Cargo.toml stubs, error.rs |
| 1 | zendriver-interception scaffolding | crates/zendriver-interception/src/{lib,error,url_pattern}.rs |
| 2 | URL pattern matcher + tests | crates/zendriver-interception/src/url_pattern.rs |
| 3 | RequestStage/ResourceType/AbortReason enums + RequestInfo/ResponseInfo types | crates/zendriver-interception/src/lib.rs (or types.rs) |
| 4 | PausedRequest + continue_/abort/respond/modify_and_continue/body | crates/zendriver-interception/src/paused.rs |
| 5 | Rule enum + InterceptBuilder + rule matching | crates/zendriver-interception/src/{rule.rs, builder.rs} |
| 6 | Interception actor + InterceptHandle | crates/zendriver-interception/src/actor.rs |
| 7 | InterceptBuilder::start + subscribe + Tab::intercept | crates/zendriver-interception/src/builder.rs, crates/zendriver/src/tab.rs |
| 8 | expect/ scaffolding + UrlMatcher + Expectation trait | crates/zendriver/src/expect/mod.rs |
| 9 | RequestExpectation + Tab::expect_request | crates/zendriver/src/expect/request.rs |
| 10 | ResponseExpectation + body fetching + Tab::expect_response | crates/zendriver/src/expect/response.rs |
| 11 | DialogExpectation + Tab::expect_dialog | crates/zendriver/src/expect/dialog.rs |
| 12 | DownloadExpectation + Tab::expect_download | crates/zendriver/src/expect/download.rs |
| 13 | zendriver-cloudflare scaffolding + CloudflareError + ClearanceOutcome | crates/zendriver-cloudflare/src/{lib,error}.rs |
| 14 | Shadow-DOM detection JS + is_challenge_present | crates/zendriver-cloudflare/src/detection.rs |
| 15 | wait_for_clearance + click dispatch + Tab::cloudflare | crates/zendriver-cloudflare/src/{bypass,click}.rs, crates/zendriver/src/tab.rs |
| 16 | zendriver-fetcher scaffolding + FetcherError + Platform/Channel/VersionSpec | crates/zendriver-fetcher/src/{lib,error,platform,version}.rs |
| 17 | Chrome for Testing manifest fetcher | crates/zendriver-fetcher/src/manifest.rs |
| 18 | Version + platform → download URL resolution | crates/zendriver-fetcher/src/{version,platform}.rs |
| 19 | HTTP download via reqwest + progress reporting | crates/zendriver-fetcher/src/download.rs |
| 20 | Zip extraction (Linux/Windows) + macOS handling | crates/zendriver-fetcher/src/extract.rs |
| 21 | Atomic cache layout + ensure_chrome end-to-end | crates/zendriver-fetcher/src/{cache,fetcher}.rs |
| 22 | BrowserBuilder::ensure_chrome convenience | crates/zendriver/src/browser.rs |
| 23 | P5 integration tests (per-feature) | crates/zendriver/tests/integration_phase5.rs |
| 24 | Port 5 P5-flavored examples | crates/zendriver/examples/*.rs |
| 25 | Nightly cloudflare-tests gated CI job | .github/workflows/ci.yml |
| 26 | README + snapshot regen | various |

---

## Task 0: Cargo feature plumbing + ZendriverError variants

**Files:**
- Workspace `Cargo.toml` — add new deps to `[workspace.dependencies]`
- `crates/zendriver/Cargo.toml` — add feature gates + optional deps for the 4 sub-crates
- `crates/zendriver-interception/Cargo.toml` — replace P1 stub with real package def
- `crates/zendriver-cloudflare/Cargo.toml` — same
- `crates/zendriver-fetcher/Cargo.toml` — same
- `crates/zendriver/src/error.rs` — add gated variants

- [ ] **Step 1: Add new workspace deps**

In root `Cargo.toml` `[workspace.dependencies]`:

```toml
reqwest  = { version = "0.12", default-features = false, features = ["rustls-tls", "gzip", "deflate", "stream"] }
zip      = { version = "2", default-features = false, features = ["deflate"] }
sha2     = "0.10"
dirs     = "5"
glob     = "0.3"
```

- [ ] **Step 2: Update zendriver Cargo.toml features**

Add to `crates/zendriver/Cargo.toml` `[features]`:

```toml
interception           = ["dep:zendriver-interception"]
expect                 = []
cloudflare             = ["interception", "dep:zendriver-cloudflare"]
fetcher                = ["dep:zendriver-fetcher"]
fetcher-network-tests  = ["fetcher"]
```

In `[dependencies]`, make the sub-crates optional:

```toml
zendriver-interception = { workspace = true, optional = true }
zendriver-cloudflare   = { workspace = true, optional = true }
zendriver-fetcher      = { workspace = true, optional = true }
```

Update existing `integration-tests` feature to include the new ones:

```toml
integration-tests = ["dep:wiremock", "dep:serial_test", "interception", "expect", "cloudflare"]
```

- [ ] **Step 3: Populate sub-crate Cargo.tomls**

`crates/zendriver-interception/Cargo.toml`:

```toml
[package]
name = "zendriver-interception"
description = "Network interception (Fetch.* CDP domain) for zendriver"
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
tokio.workspace               = true
tokio-util.workspace          = true
tokio-stream.workspace        = true
async-trait.workspace         = true
futures.workspace             = true
serde.workspace               = true
serde_json.workspace          = true
thiserror.workspace           = true
tracing.workspace             = true
base64                        = "0.22"
glob.workspace                = true

[dev-dependencies]
tokio-test.workspace          = true
zendriver-transport           = { workspace = true, features = ["testing"] }
```

`crates/zendriver-cloudflare/Cargo.toml`:

```toml
[package]
name = "zendriver-cloudflare"
description = "Cloudflare Turnstile bypass for zendriver"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[lints]
workspace = true

[dependencies]
zendriver-transport.workspace    = true
zendriver-interception.workspace = true
tokio.workspace                  = true
serde.workspace                  = true
serde_json.workspace             = true
thiserror.workspace              = true
tracing.workspace                = true

[dev-dependencies]
tokio-test.workspace = true
zendriver-transport  = { workspace = true, features = ["testing"] }
```

`crates/zendriver-fetcher/Cargo.toml`:

```toml
[package]
name = "zendriver-fetcher"
description = "Chrome binary downloader for zendriver"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[lints]
workspace = true

[dependencies]
tokio.workspace      = true
reqwest.workspace    = true
zip.workspace        = true
sha2.workspace       = true
dirs.workspace       = true
serde.workspace      = true
serde_json.workspace = true
thiserror.workspace  = true
tracing.workspace    = true
futures.workspace    = true

[dev-dependencies]
tokio-test.workspace = true
wiremock.workspace   = true
tempfile.workspace   = true
```

- [ ] **Step 4: Add gated ZendriverError variants**

In `crates/zendriver/src/error.rs`, add to `ZendriverError`:

```rust
    #[cfg(feature = "interception")]
    #[error("interception: {0}")]
    Interception(#[from] zendriver_interception::InterceptionError),

    #[cfg(feature = "cloudflare")]
    #[error("cloudflare: {0}")]
    Cloudflare(#[from] zendriver_cloudflare::CloudflareError),

    #[cfg(feature = "fetcher")]
    #[error("fetcher: {0}")]
    Fetcher(#[from] zendriver_fetcher::FetcherError),
```

(These types don't exist yet; the `cfg(feature)` gate means they only compile when the feature is enabled, which won't happen until each sub-crate ships its error type.)

- [ ] **Step 5: Verify nothing breaks under default features**

```bash
cargo build --workspace --locked
cargo test --workspace --lib --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
```

All gates green under default features (which excludes the new feature flags). The new gated variants stay invisible until the sub-crates land their error types.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
chore(zendriver): P5 feature flag plumbing + sub-crate Cargo.tomls

Adds workspace deps (reqwest, zip, sha2, dirs, glob), feature flags on
zendriver crate (interception, expect, cloudflare → interception,
fetcher, fetcher-network-tests), populates the 3 sub-crate Cargo.tomls
with full dep sets, and adds gated ZendriverError variants for each
sub-area. Sub-crate types don't exist yet — variants are
#[cfg(feature)]-gated to stay invisible until each crate ships its
error.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: zendriver-interception scaffolding

**Files:**
- `crates/zendriver-interception/src/lib.rs`
- `crates/zendriver-interception/src/error.rs`

- [ ] **Step 1: lib.rs module declarations + re-exports**

Replace existing stub `crates/zendriver-interception/src/lib.rs`:

```rust
//! Network interception via the Fetch CDP domain.
//!
//! Two entry points:
//! - Rule-based: register declarative block/redirect/respond rules via
//!   `InterceptBuilder` and start a background actor.
//! - Stream: subscribe to paused requests and drive them manually.

pub mod actor;
pub mod builder;
pub mod error;
pub mod paused;
pub mod rule;
pub mod types;
pub mod url_pattern;

pub use builder::{InterceptBuilder, InterceptHandle};
pub use error::InterceptionError;
pub use paused::PausedRequest;
pub use types::{AbortReason, RequestInfo, RequestOverrides, RequestStage, ResourceType, ResponseInfo};
```

Other module files are stubs (`//! Populated in subsequent Phase 5 tasks.`) for now.

- [ ] **Step 2: InterceptionError**

`crates/zendriver-interception/src/error.rs`:

```rust
//! Interception-layer errors.

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

    #[error("subscription channel closed")]
    SubscriptionClosed,
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_invalid_pattern() {
        let e = InterceptionError::InvalidPattern("**bad".into());
        assert_eq!(e.to_string(), "invalid url pattern: **bad");
    }
}
```

- [ ] **Step 3: Verify**

```bash
cargo test -p zendriver-interception --lib error
cargo build --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-interception/
git commit -m "feat(interception): scaffolding + InterceptionError

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

# Tasks 2-26 — Compact form

**Note to implementer:** Tasks 2-26 reference the spec for code details. Patterns from P1-P4 carry forward (TDD with MockConnection, file-scoped allows for test modules, `cargo fmt --all` between tasks, etc). Spec at `docs/superpowers/specs/2026-05-23-zendriver-rs-phase5-gated-design.md` is the source of truth.

For each task: implement per spec, add 1-3 tests covering happy path + 1 error path where applicable, verify build + clippy clean (with the relevant feature flag enabled), commit with the listed message.

## Task 2: URL pattern matcher

**Files:** `crates/zendriver-interception/src/url_pattern.rs`
**Spec:** Section "URL pattern matching"

**Implement:** `UrlPattern` struct that compiles a CDP-style pattern (`*`/`?` wildcards) into a `regex::Regex`. Method `matches(url) -> bool`. `new(pattern)` returns `Result<Self, InterceptionError::InvalidPattern>`. Internally converts `*` → `.*`, `?` → `.`, escapes other regex metachars.

**Tests (3):** wildcard `*` matches all; `*.example.com` matches subdomain; `?` matches single char; invalid pattern (unmatched bracket etc) errors.

**Commit:** `feat(interception): URL pattern matcher with CDP wildcard syntax`

## Task 3: Type definitions (enums + info structs)

**Files:** `crates/zendriver-interception/src/types.rs` (NEW)
**Spec:** Section "Public API" → "InterceptBuilder" subsection

**Implement:**
- `RequestStage { Request, Response }` enum
- `ResourceType { Document, Stylesheet, Image, Media, Font, Script, TextTrack, XHR, Fetch, EventSource, WebSocket, Manifest, SignedExchange, Ping, CSPViolationReport, Preflight, Other }` enum with `as_cdp_str(&self) -> &'static str` returning the CDP string ("Document", "Stylesheet", etc).
- `AbortReason { Failed, Aborted, ...14 variants per spec }` with `as_cdp_str` similarly.
- `RequestInfo { url, method, headers, post_data, resource_type }` struct (Debug + Clone).
- `ResponseInfo { status, status_text, headers }` struct.
- `RequestOverrides { url: Option<String>, method: Option<String>, headers: Option<HashMap>, post_data: Option<Vec<u8>> }` struct.

**Tests:** 1 enum-to-CDP-string snapshot.

**Commit:** `feat(interception): RequestStage + ResourceType + AbortReason + Info types`

## Task 4: PausedRequest

**Files:** `crates/zendriver-interception/src/paused.rs`
**Spec:** Section "Public API" → PausedRequest

**Implement:** `pub struct PausedRequest { pub request_id, pub request, pub response, tab: Tab }`. Methods:
- `continue_(self) -> Result<()>` — `Fetch.continueRequest { requestId }`
- `abort(self, reason: AbortReason) -> Result<()>` — `Fetch.failRequest { requestId, errorReason }`
- `respond(self, status: u16, headers: Vec<(String, String)>, body: Vec<u8>) -> Result<()>` — `Fetch.fulfillRequest { requestId, responseCode, responseHeaders, body: base64(body) }`
- `modify_and_continue(self, overrides: RequestOverrides) -> Result<()>` — `Fetch.continueRequest { requestId, url?, method?, headers?, postData? }`
- `body(&self) -> Result<Vec<u8>>` — `Fetch.getResponseBody { requestId }` → base64 decode

All consume `self` except `body` (which keeps the handle alive — useful for at_response stage logging without actioning).

Note on `tab` field: `PausedRequest` needs to dispatch on the tab's session. Either store `Tab` directly (clone) or store `SessionHandle`. Pick SessionHandle (smaller; doesn't need full Tab).

**Tests (2):** mock-driven continue_ dispatches Fetch.continueRequest; respond dispatches Fetch.fulfillRequest with base64 body.

**Commit:** `feat(interception): PausedRequest + continue_/abort/respond/modify methods`

## Task 5: Rule enum + InterceptBuilder skeleton

**Files:** `crates/zendriver-interception/src/rule.rs`, `crates/zendriver-interception/src/builder.rs`
**Spec:** Section "Public API" → InterceptBuilder + Actor

**Implement:**
- `rule.rs`: `enum Rule { Block { pattern: UrlPattern }, Redirect { from: UrlPattern, to: String }, Respond { pattern: UrlPattern, status: u16, headers: Vec<(String, String)>, body: Vec<u8> }, Modify { pattern: UrlPattern, modify: Arc<dyn Fn(&RequestInfo) -> RequestOverrides + Send + Sync> } }`. Method `Rule::matches(url: &str) -> bool` checks the pattern.
- `builder.rs`: `InterceptBuilder<'tab>` with fields `tab, patterns: Vec<RequestPattern>, rules: Vec<Rule>`. Builder methods per spec: `pattern/at_request/at_response/resource/block/redirect/respond/modify_request`. `start()` + `subscribe()` deferred to T7. Just type + builder for now.

**Tests:** 1 mock-style — register 3 rules + assert rule count.

**Commit:** `feat(interception): Rule enum + InterceptBuilder skeleton`

## Task 6: Interception actor + InterceptHandle

**Files:** `crates/zendriver-interception/src/actor.rs`
**Spec:** Section "Actor implementation"

**Implement:** Background task fn `pub(crate) async fn run_actor(session: SessionHandle, rules: Vec<Rule>, cancel: CancellationToken)`. Per spec:
1. Sends `Fetch.enable { patterns: <derived from rules + explicit>, handleAuthRequests: false }`. Subscribe to `Fetch.requestPaused` events BEFORE the enable (P4 pattern — avoid race).
2. Loop: receive event → walk rules in order → first match wins → dispatch via `Fetch.failRequest`/`continueRequest`/`fulfillRequest` per rule kind. No match → `Fetch.continueRequest` to let through.
3. On cancel → `Fetch.disable` → exit.

`InterceptHandle { cancel: CancellationToken }` with `Drop` cancelling. Method `stop(self) -> Result<()>` cancels + awaits a oneshot signaling actor exit.

**Tests:** mock-driven — register Block rule + emit Fetch.requestPaused with matching URL → assert Fetch.failRequest dispatched with errorReason=BlockedByClient.

**Commit:** `feat(interception): background actor + InterceptHandle (RAII cancel)`

## Task 7: InterceptBuilder::start + subscribe + Tab::intercept

**Files:** `crates/zendriver-interception/src/builder.rs`, `crates/zendriver/src/tab.rs`
**Spec:** Section "InterceptBuilder"

**Implement:**
- `InterceptBuilder::start(self) -> Result<InterceptHandle>` — spawns the actor task from T6 with the registered rules + a fresh CancellationToken. Returns the handle.
- `InterceptBuilder::subscribe(self) -> impl Stream<Item = PausedRequest> + Send` — alt path. Enables Fetch with declared patterns, subscribes to Fetch.requestPaused, returns a stream that yields PausedRequest per event. Caller drives + dispatches manually.
- `Tab::intercept(&self) -> InterceptBuilder<'_>` — gated `#[cfg(feature = "interception")]`. In `crates/zendriver/src/tab.rs`.

Also re-export `InterceptBuilder, PausedRequest, InterceptHandle, RequestStage, ResourceType, AbortReason, RequestInfo, RequestOverrides` from `crates/zendriver/src/lib.rs` under `#[cfg(feature = "interception")]`.

**Tests:** 2 — start spawns actor + Tab::intercept().block("*").start() integration on mock (block all → respond all to Fetch.failRequest).

**Commit:** `feat(interception): InterceptBuilder::start + subscribe + Tab::intercept`

## Task 8: expect/ scaffolding + UrlMatcher

**Files:** `crates/zendriver/src/expect/mod.rs` (NEW), `crates/zendriver/src/lib.rs`
**Spec:** Section "expect()"

**Implement:**
- `pub mod expect;` declaration in `crates/zendriver/src/lib.rs` gated `#[cfg(feature = "expect")]`.
- `mod.rs`: module declarations for request/response/dialog/download submodules + `pub use ...` re-exports. `UrlMatcher` enum with `Substring(String)` + `Regex(regex::Regex)` variants + `From<&str>`/`From<String>`/`From<regex::Regex>` impls + a `matches(&self, url: &str) -> bool` method.

Stub the 4 submodule files with `//! Populated in subsequent Phase 5 tasks.`.

**Tests:** UrlMatcher basic — Substring matches if url contains needle; Regex matches per pattern; From impls work.

**Commit:** `feat(expect): scaffolding + UrlMatcher`

## Task 9: RequestExpectation + Tab::expect_request

**Files:** `crates/zendriver/src/expect/request.rs`, `crates/zendriver/src/tab.rs`
**Spec:** Section "expect()" → RequestExpectation

**Implement:**
- `MatchedRequest { url, method, headers: HashMap, post_data: Option<Vec<u8>>, request_id }` struct.
- `RequestExpectation { rx: oneshot::Receiver<MatchedRequest>, timeout: Duration }` + `timeout(dur)` builder + `matched()` method = await impl Future.
- `impl Future for RequestExpectation { type Output = Result<MatchedRequest>; }` using the rx channel + timeout. On timeout returns `ZendriverError::Timeout`.
- `Tab::expect_request(&self, pattern: impl Into<UrlMatcher>) -> RequestExpectation` — gated `#[cfg(feature = "expect")]`. Subscribes to `Network.requestWillBeSent` events on the tab's session, filters by UrlMatcher, sends first match through oneshot then cancels subscription. Spawns the subscriber task at call time so registration is alive when caller returns.

`Network.enable` must be on for these events. Tab construction already does it via InFlightTracker (P4) — verify, otherwise enable here too.

**Tests:** mock-driven — register expectation, emit matching Network.requestWillBeSent, await resolves with the matched request. Test non-matching event doesn't resolve. Test timeout fires.

**Commit:** `feat(expect): RequestExpectation + Tab::expect_request`

## Task 10: ResponseExpectation + body fetching + Tab::expect_response

**Files:** `crates/zendriver/src/expect/response.rs`, `crates/zendriver/src/tab.rs`
**Spec:** Section "expect()" → ResponseExpectation

**Implement:**
- `MatchedResponse { url, status: u16, status_text, headers: HashMap, request_id, tab: Tab }` struct.
- `MatchedResponse::body(&self) -> Result<Vec<u8>>` — `Network.getResponseBody { requestId }` → base64 decode.
- `ResponseExpectation` same shape as RequestExpectation but for `Network.responseReceived`.
- `Tab::expect_response(&self, pattern: impl Into<UrlMatcher>) -> ResponseExpectation`.

**Tests (2):** mock-driven — expect_response resolves on matching event; MatchedResponse::body dispatches Network.getResponseBody + base64-decodes.

**Commit:** `feat(expect): ResponseExpectation + body fetching + Tab::expect_response`

## Task 11: DialogExpectation + Tab::expect_dialog

**Files:** `crates/zendriver/src/expect/dialog.rs`, `crates/zendriver/src/tab.rs`
**Spec:** Section "expect()" → DialogExpectation

**Implement:**
- `MatchedDialog { dialog_type, message, default_prompt, url, tab: Tab }` struct.
- `MatchedDialog::accept(self, prompt_text: Option<String>) -> Result<()>` — `Page.handleJavaScriptDialog { accept: true, promptText? }`.
- `MatchedDialog::dismiss(self) -> Result<()>` — `Page.handleJavaScriptDialog { accept: false }`.
- `DialogType { Alert, Beforeunload, Confirm, Prompt }`.
- `DialogExpectation` subscribes to `Page.javascriptDialogOpened` events.
- `Tab::expect_dialog(&self) -> DialogExpectation` — no pattern (dialogs don't have URLs to match on; any dialog matches).

Note: Page.javascriptDialogOpened requires `Page.enable`. Should already be on from P1's Tab::goto.

**Tests (2):** mock-driven — expect_dialog resolves on Page.javascriptDialogOpened; accept dispatches Page.handleJavaScriptDialog with accept=true.

**Commit:** `feat(expect): DialogExpectation + accept/dismiss + Tab::expect_dialog`

## Task 12: DownloadExpectation + Tab::expect_download

**Files:** `crates/zendriver/src/expect/download.rs`, `crates/zendriver/src/tab.rs`
**Spec:** Section "expect()" → DownloadExpectation

**Implement:**
- `MatchedDownload { url, suggested_filename, guid, state: tokio::sync::Mutex<DownloadState> }` struct.
- `DownloadState { received_bytes: u64, total_bytes: u64, state: enum Receiving | Completed | Canceled }`.
- `MatchedDownload::path(&self) -> Option<PathBuf>` — None until completed.
- `MatchedDownload::save_to(self, path: PathBuf) -> Result<()>` — copies from temp download path to user path.
- `DownloadExpectation` subscribes to `Page.downloadWillBegin` events + tracks `Page.downloadProgress` updates.

CDP needs `Browser.setDownloadBehavior { behavior: "allowAndName", downloadPath: <temp> }` set first to actually capture downloads. Set on first `expect_download` call per Tab.

**Tests (1):** mock-driven — expect_download resolves on Page.downloadWillBegin; path is None until completed.

**Commit:** `feat(expect): DownloadExpectation + save_to + Tab::expect_download`

## Task 13: zendriver-cloudflare scaffolding + ClearanceOutcome

**Files:** `crates/zendriver-cloudflare/src/lib.rs`, `crates/zendriver-cloudflare/src/error.rs`, `crates/zendriver-cloudflare/src/bypass.rs` (stub)
**Spec:** Section "zendriver-cloudflare"

**Implement:**
- `lib.rs`: module declarations + re-exports `CloudflareBypass, CloudflareError, ClearanceOutcome`.
- `error.rs`: per spec — `CloudflareError { NoChallenge, ClearanceTimeout, Tab(#[source] Box<ZendriverError>) }`. Note: Box is needed since CloudflareError is in zendriver-cloudflare but wraps ZendriverError from zendriver core.

But — zendriver-cloudflare can't depend on zendriver core (cycle). So `Tab` variant should wrap a string or wrap `zendriver_transport::CallError` instead. Adjust: `Tab(#[from] zendriver_transport::CallError)`.

- `bypass.rs`: stub `pub struct CloudflareBypass<'tab> { ... }` + `pub enum ClearanceOutcome { TokenAcquired(String), ChallengeGone }`. Just types — methods land in T14/T15.

**Tests (2):** CloudflareError display variants.

**Commit:** `feat(cloudflare): scaffolding + CloudflareError + ClearanceOutcome`

## Task 14: Shadow-DOM detection JS + is_challenge_present

**Files:** `crates/zendriver-cloudflare/src/detection.rs`, `crates/zendriver-cloudflare/src/detect.js`
**Spec:** Section "Bypass implementation"

**Implement:**
- `detect.js`: JS function `(function() { /* walk shadow roots looking for iframes whose src includes challenges.cloudflare.com */ })()` returns either `null` or `{ x, y, width, height }` of the Turnstile iframe.
- `detection.rs`: `pub(crate) async fn detect_challenge(tab: &Tab) -> Result<Option<BoundingBox>, CloudflareError>` — calls `tab.evaluate_main::<Option<BoundingBox>>(include_str!("detect.js")).await?`.
- `pub async fn CloudflareBypass::is_challenge_present(&self) -> Result<bool, CloudflareError>` — returns `Ok(detect_challenge(...).await?.is_some())`.

Note: this needs `&Tab` from `zendriver` crate. zendriver-cloudflare can't depend on zendriver (cycle). Workaround: take a `&SessionHandle` instead and call `session.call("Runtime.evaluate", ...)` directly. Or accept that we need a tab-shaped trait/interface defined in zendriver-transport.

Simpler: the public API is `CloudflareBypass<'tab>` where the lifetime is whatever the user passes. Internally store `SessionHandle` not `Tab`. Tab::cloudflare() in T15 constructs with `tab.session().clone()`.

**Tests:** mock detection — emit Runtime.evaluate response with non-null bbox → is_challenge_present returns Ok(true).

**Commit:** `feat(cloudflare): shadow-DOM detection JS + is_challenge_present`

## Task 15: wait_for_clearance + click + Tab::cloudflare

**Files:** `crates/zendriver-cloudflare/src/{bypass,click}.rs`, `crates/zendriver/src/tab.rs`
**Spec:** Section "Bypass implementation"

**Implement:**
- `click.rs`: `pub(crate) async fn click_at(session: &SessionHandle, x: f64, y: f64) -> Result<(), CloudflareError>` — raw mouse dispatch (not realistic Bezier; raw click on visible checkbox). Sends Input.dispatchMouseEvent mouseMoved → mousePressed → mouseReleased with left button at (x, y).
- `bypass.rs`: full `CloudflareBypass` impl. `wait_for_clearance(self, timeout)`:
  1. detect challenge bbox via detection.rs
  2. compute click point at bbox.left + bbox.width * 0.15, bbox.top + bbox.height * 0.50
  3. click via click.rs
  4. poll every poll_interval (default 500ms): evaluate_main checks for `document.querySelector('[name="cf-turnstile-response"]')?.value` truthy → ClearanceOutcome::TokenAcquired(token); OR challenge container disappears → ClearanceOutcome::ChallengeGone
  5. timeout → ClearanceTimeout
- `Tab::cloudflare(&self) -> CloudflareBypass<'_>` in `crates/zendriver/src/tab.rs` gated `#[cfg(feature = "cloudflare")]`.

**Tests:** 1 mock-driven — bbox found + click dispatched + first poll sees token → ClearanceOutcome::TokenAcquired.

**Commit:** `feat(cloudflare): wait_for_clearance + raw click + Tab::cloudflare`

## Task 16: zendriver-fetcher scaffolding + types

**Files:** `crates/zendriver-fetcher/src/{lib,error,platform,version}.rs`
**Spec:** Section "zendriver-fetcher"

**Implement:**
- `lib.rs`: module declarations + re-exports `Fetcher, FetcherError, FetcherProgress, FetcherPhase, Channel, Platform, VersionSpec`.
- `error.rs`: `FetcherError` enum per spec (Http/Io/Manifest/VersionNotFound/UnsupportedPlatform/IntegrityFailed/Extraction).
- `platform.rs`: `Platform { LinuxX64, MacX64, MacArm64, Win32, Win64 }` enum + `auto_detect() -> Option<Self>` using `cfg!(target_os)` + `cfg!(target_arch)` + `cfg!(target_pointer_width)`. `as_cft_str(&self) -> &'static str` returns CFT manifest's platform key ("linux64", "mac-x64", "mac-arm64", "win32", "win64").
- `version.rs`: `VersionSpec { Latest, Stable, Channel(Channel), Explicit(String) }` enum. `Channel { Stable, Beta, Dev, Canary }` enum.

**Tests:** Platform::auto_detect returns Some on current host; Channel/VersionSpec enums compile cleanly.

**Commit:** `feat(fetcher): scaffolding + FetcherError + Platform/Channel/VersionSpec`

## Task 17: Chrome for Testing manifest fetcher

**Files:** `crates/zendriver-fetcher/src/manifest.rs`
**Spec:** Section "Implementation flow" step 1

**Implement:**
- `KnownGoodVersionsResponse { versions: Vec<VersionEntry> }` with serde derive matching CFT JSON shape.
- `VersionEntry { version, revision, downloads: Downloads }`.
- `Downloads { chrome: Vec<Download> }`.
- `Download { platform: String, url: String }`.
- `pub(crate) async fn fetch_manifest() -> Result<KnownGoodVersionsResponse, FetcherError>` — `reqwest::get("https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json")` → parse JSON.

**Test:** stub via wiremock — serve known good JSON; assert parses into Rust structs.

**Commit:** `feat(fetcher): Chrome for Testing manifest fetcher`

## Task 18: Version + platform → download URL resolution

**Files:** `crates/zendriver-fetcher/src/{version.rs, platform.rs}` extensions, maybe new `resolver.rs`

**Implement:**
- `pub(crate) async fn resolve_download_url(spec: VersionSpec, platform: Platform) -> Result<(String, String), FetcherError>` (returns version string + url). Given VersionSpec:
  - `Latest`: pick last entry from manifest.versions
  - `Stable`: same (CFT manifest only ships stable; channels need separate API). For now alias to Latest.
  - `Channel(Stable)`: same as Stable
  - `Channel(Beta/Dev/Canary)`: separate JSON at `https://googlechromelabs.github.io/chrome-for-testing/latest-versions-per-milestone.json` — but for P5 simplicity, return `UnsupportedPlatform` error and document
  - `Explicit("123.0...")`: find that exact version
  Then walk `version.downloads.chrome[]` for matching `platform.as_cft_str()` and return its url.

**Test:** with a stub manifest, resolve_download_url(Latest, LinuxX64) returns the right URL.

**Commit:** `feat(fetcher): version + platform → download URL resolution`

## Task 19: HTTP download with progress

**Files:** `crates/zendriver-fetcher/src/download.rs`

**Implement:** `pub(crate) async fn download(url: &str, dest_path: &Path, progress_cb: Option<&(dyn Fn(FetcherProgress) + Send + Sync)>) -> Result<(), FetcherError>`. Uses `reqwest::get(url)` → stream the response body to dest_path via `tokio::fs::File`. Emits FetcherProgress callbacks every ~100KB or 100ms (whichever first). Phase = Downloading during; final state = Done.

`FetcherProgress { downloaded, total: Option<u64>, phase: FetcherPhase }`. Total comes from `Content-Length` header.

**Test:** with a stub server returning a small payload, download writes the file correctly + progress callback called at least once.

**Commit:** `feat(fetcher): HTTP download via reqwest with progress callbacks`

## Task 20: Zip extraction (Linux/Windows + macOS)

**Files:** `crates/zendriver-fetcher/src/extract.rs`

**Implement:** `pub(crate) async fn extract(archive_path: &Path, dest_dir: &Path) -> Result<(), FetcherError>`. Uses the `zip` crate to unzip. Chrome for Testing ships zip on all platforms (Linux + macOS + Windows).

Block-on `tokio::task::spawn_blocking` since `zip` is sync. Each file extracted via `zip.by_index(i)` → write to `dest_dir.join(file.name())`. Preserve Unix executable bits via `file.unix_mode()`.

**Test:** with a tiny test zip containing one file, extract recovers the file with correct contents.

**Commit:** `feat(fetcher): zip extraction (Linux/Windows/macOS)`

## Task 21: Atomic cache layout + ensure_chrome

**Files:** `crates/zendriver-fetcher/src/{cache.rs, fetcher.rs}`

**Implement:**
- `cache.rs`: `pub(crate) fn default_cache_dir() -> PathBuf` — `dirs::cache_dir().unwrap_or(env::temp_dir()).join("zendriver/chrome")`. `pub(crate) fn binary_path(cache_dir: &Path, version: &str, platform: Platform) -> PathBuf` — composes `cache_dir/<version>/chrome-<platform_cft>/chrome` (Linux), `chrome-<platform_cft>/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing` (macOS), `chrome-<platform_cft>/chrome.exe` (Windows). Exact paths from CFT zip layout.
- `fetcher.rs`: full `Fetcher` struct + builder methods + `ensure_chrome()`:
  1. Resolve cache_dir (default if not set).
  2. Resolve platform (auto if not set).
  3. Resolve version + URL via T17/T18.
  4. Compute target binary path. If exists + executable → return early.
  5. Download to temp file in cache_dir (`<version>.tmp`).
  6. Extract to `cache_dir/<version>.tmp/`.
  7. Atomic rename `<version>.tmp` → `<version>`.
  8. Set executable bit (Unix).
  9. Return path.

**Test:** end-to-end with a stub manifest + stub download server + temp cache_dir → ensure_chrome returns the executable path.

**Commit:** `feat(fetcher): atomic cache layout + Fetcher::ensure_chrome end-to-end`

## Task 22: BrowserBuilder::ensure_chrome convenience

**Files:** `crates/zendriver/src/browser.rs`

**Implement:** `#[cfg(feature = "fetcher")]` `impl BrowserBuilder { pub async fn ensure_chrome(self) -> Result<Self, ZendriverError> }` — calls `zendriver_fetcher::Fetcher::new().ensure_chrome().await?`, then `self.executable(path)`.

**Test:** mock not needed — just verify the method compiles + returns the right type. Real use is integration only.

**Commit:** `feat(zendriver): BrowserBuilder::ensure_chrome convenience (feature: fetcher)`

## Task 23: P5 integration tests

**Files:** `crates/zendriver/tests/integration_phase5.rs` (NEW, gated `integration-tests`)

**Implement:** 5-7 integration tests covering each P5 sub-area:
1. `interception_block_rule_prevents_request` — wiremock fixture, intercept block "*/blocked/*", goto a page that fetches /blocked/x.json, assert it was blocked.
2. `interception_respond_serves_fake_response` — register respond rule, fetch from URL, get the fake body.
3. `expect_response_returns_matched_response` — page with a fetch, expect_response(pattern), click trigger button, assert resolves.
4. `expect_dialog_resolves_on_alert` — page with `<script>alert('hi')</script>`, expect_dialog, accept.
5. `cloudflare_is_challenge_present_returns_false_on_normal_page` — basic page, assert is_challenge_present returns false.
6. (Optional) `fetcher_with_stub_manifest_downloads_chrome` — local mock CFT manifest + tiny "chrome.zip" → ensure_chrome works.

All `#[serial]` + `#[tokio::test]` + gated `integration-tests` feature.

**Verify:** `cargo build --tests --workspace --features integration-tests --locked` clean.

**Commit:** `test(zendriver): P5 integration tests (interception/expect/cloudflare/fetcher)`

## Task 24: P5-flavored examples

**Files:** `crates/zendriver/examples/*.rs`

**Implement:** 5 examples (synthesize where Python lacks equivalents):
1. `intercept_block_ads.rs` — `tab.intercept().block("*/ads/*").start().await?` then goto example.com.
2. `intercept_modify_headers.rs` — `tab.intercept().modify_request("*/api/*", |_| RequestOverrides { headers: Some(...) }).start()`.
3. `expect_login_response.rs` — fill login form, expect_response, assert status 200.
4. `cloudflare_bypass.rs` — goto a CF-protected URL, tab.cloudflare().wait_for_clearance(30s).
5. `fetcher_demo.rs` — `Fetcher::new().version(Latest).ensure_chrome()` + launch with that binary.

Each compiles via `cargo build --examples --workspace --all-features --locked`.

**Commit:** `examples(zendriver): port 5 P5 examples (intercept/expect/cloudflare/fetcher)`

## Task 25: Nightly cloudflare-tests CI job

**Files:** `.github/workflows/ci.yml`

**Implement:** Add a new job `nightly-cloudflare-tests` similar to existing `nightly-stealth-tests` from P2 T22. Runs on cron `0 7 * * *` (1h after stealth tests), `continue-on-error: true`, installs Chromium, runs `cargo test --workspace --features cloudflare-tests --test cloudflare_phase5 -- --test-threads=1`. (Or fold into existing stealth-tests job if simpler — your call.)

If folding: add the cloudflare test to `crates/zendriver/tests/stealth_phase2.rs` or create `tests/cloudflare_phase5.rs` gated `cloudflare-tests`.

**Commit:** `ci: nightly cloudflare-tests cron job`

## Task 26: README + snapshot regen + final polish

**Implement:**
- Run `cargo test --workspace --lib --locked` — if snapshots drifted, `cargo insta accept`.
- Run `cargo fmt --all` — apply pending fixes.
- README: Phase 5 → DONE; status → "Phases 1-5 shipped"; example could showcase interception or expect.
- Final gate: cargo test --workspace --lib --doc --locked (+ all features), clippy `-D warnings`, fmt --check. All clean.

**Commit:** `chore: post-P5 snapshot regen + README updates`

---

## Self-review checklist

**Spec coverage:** every section in the spec maps to T0-T26. Interception → T1-T7. expect → T8-T12. Cloudflare → T13-T15. Fetcher → T16-T22. Integration + polish → T23-T26.

**Placeholder scan:** none. T2-T26 are compact but reference spec + show explicit commits.

**Type consistency:** InterceptBuilder/PausedRequest/InterceptHandle/Rule/UrlPattern/UrlMatcher/Expectation/MatchedRequest/MatchedResponse/MatchedDialog/MatchedDownload/Fetcher/Platform/VersionSpec/Channel — names consistent.

---

## Notes for the implementing engineer

1. **zendriver-cloudflare CANNOT depend on zendriver core** (would cycle). Internal API takes `&SessionHandle` not `&Tab`. The `Tab::cloudflare()` accessor in T15 constructs the bypass with `tab.session().clone()`.
2. **Same for zendriver-interception** — internal types take SessionHandle not Tab. PausedRequest internally stores a SessionHandle.
3. **Feature flag gating on tests**: tests for interception module use `#[cfg(feature = "interception")]` not just `#[cfg(test)]`. Otherwise they compile under default features (where the deps don't exist) and break the build.
4. **The `expect` module is in-tree** (not its own crate) but still feature-gated. Live under `crates/zendriver/src/expect/` mod tree, gated via `#[cfg(feature = "expect")]` on the `pub mod expect;` declaration.
5. **CFT manifest URL** = `https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json` (stable). Bookmark.
6. **Chrome for Testing path layout** (post-extraction) — Linux: `chrome-linux64/chrome`. macOS: `chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`. Windows: `chrome-win64/chrome.exe`. Don't guess — verify against a real CFT zip.
7. **Branch is `worktree-phase5-gated`** in worktree under `.claude/worktrees/phase5-gated/`.
