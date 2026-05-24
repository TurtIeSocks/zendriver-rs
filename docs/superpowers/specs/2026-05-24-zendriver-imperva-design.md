# zendriver-imperva — Imperva WAF Bypass Crate

**Date:** 2026-05-24
**Status:** Approved (participate-mode brainstorming complete, ready for implementation plan)
**Scope:** New workspace crate `zendriver-imperva` + small retrofit to `zendriver-cloudflare` for workspace consistency
**Depends on:** Phases 1–6, all on `main`

## Summary

Add a new `zendriver-imperva` workspace crate that provides a single passive bypass driver for Imperva-protected sites, mirroring the file layout and API shape of `zendriver-cloudflare`. The driver attaches to a `Tab`, detects which Imperva surface is active (modern reese84 bot management, legacy Incapsula `___utmvc` flow, or a CAPTCHA fallback), then waits for clearance via tab polling. An optional Fetch-domain interception escape hatch reduces latency on sites where polling races the cookie set, and an optional CAPTCHA callback lets the caller plug in an external solver. The crate requires `zendriver-stealth` as a hard cargo-feature dependency, and `zendriver-cloudflare` is retrofit to do the same so both bypass crates have consistent stealth posture.

The crate is intentionally a passive observer of a real Chrome browser; the active "compute the reese84 token in pure Rust" approach is out of scope for the workspace.

## Goals

- **New crate `zendriver-imperva`** populated. `Tab::imperva()` returns an `ImpervaBypass` builder. `wait_for_clearance()` runs an embedded JS detect routine in the tab to identify the active Imperva surface, then polls cookies + body markers until both clearance signals hit. Returns `ClearanceOutcome::TokenAcquired { reese84, sessions }`, `ChallengeGone`, or `AlreadyClear` per fast path.
- **Hybrid CAPTCHA path.** Default behavior: detecting a CAPTCHA surface without a registered solver returns `ImpervaError::CaptchaRequired` immediately (no wait). Optional `.on_captcha(callback)` builder method registers an async solver; when set, CAPTCHA detection invokes the callback with a `CaptchaChallenge` (kind + site key + url), receives a `CaptchaSolution` (token + form field), injects it into the page, and resumes polling.
- **Hybrid detection.** Default: tab polling at 250ms intervals via a single `detect.js` evaluate per tick that bundles cookie reads, body marker scan, and script-src checks. Opt-in `.with_interception(&interceptor)` enables a Fetch-domain hook that subscribes to `/_Incapsula_Resource*` and `Reese.js` responses; first signal wins (`tokio::select!` between poll tick and interception signal).
- **Hybrid AND clearance signal.** A clearance outcome of `TokenAcquired` requires both (a) a `reese84` cookie set with non-empty value, scoped to the current eTLD+1, and (b) the page body no longer contains Imperva challenge markers. Avoids false positives (cookie set during pre-clearance redirect) and false negatives (cookie evicted by site CSP) seen with cookie-only or body-only signals.
- **Required `zendriver-stealth` dependency.** The `imperva` feature on the parent `zendriver` crate implies `stealth` and `interception`. Imperva's reese84 sensor is itself a fingerprint check; running without stealth has near-zero empirical success rate on protected sites.
- **Retrofit `zendriver-cloudflare` to require stealth.** Acceptable pre-1.0 breaking change per the project's recorded API-churn allowance through P6 publish. Restores workspace consistency: both bypass crates require stealth.
- **Single-driver, single-surface-enum design.** One `ImpervaBypass` struct dispatches per-surface internally via a `match` on the detected `ImpervaSurface` variant. Mirrors `CloudflareBypass`'s single-struct shape; no per-surface trait/impl proliferation in v0.1.
- **Tests:** unit tests (~25) using `MockConnection` covering surface detection, state-machine transitions, timeout behavior, CAPTCHA callback path, interception fast-path, cookie scoping. Doctests (~5) on lib + public methods. Advisory nightly integration job against 2–3 known stable public Imperva sites.

## Non-goals

- **Active reese84 token synthesis.** Reverse-engineering Imperva's obfuscated sensor JS and re-implementing it in Rust is out of scope. The crate is a passive observer of a real Chrome browser. If active synthesis is ever wanted, it belongs in a separate `imperva-sensor-rs`-style crate outside this workspace — different deps (no `zendriver-transport`), different churn profile (re-RE per Imperva release), different audience (high-throughput scraping without Chrome).
- **Clearance-mode tuning knob.** No `.clearance_mode(CookieOnly | BodyOnly | Both)` builder. Hybrid AND is the only mode. YAGNI until a real site demands the looser variant.
- **Cookie persistence helpers in this crate.** `CookieJar` at `Browser` scope already handles save/load (P4); callers persist Imperva sessions via the existing API. The crate exposes the set of relevant cookie snapshots via `ClearanceOutcome::TokenAcquired { sessions }` for inspection but does not duplicate persistence machinery.
- **TLS / JA3 fingerprint handling.** Out of scope; handled at transport/proxy layer.
- **Behavioral-classifier evasion** (mouse trajectories beyond what P3 InputController already does, timing jitter, etc.). Out of scope; existing stealth surface covers what's available.
- **Pre-1.0 SemVer guarantees.** Per `api-churn-acceptable-pre-release` memory, API can churn through subsequent phases.

## Architecture

### Crate layout

```
crates/zendriver-imperva/
├── Cargo.toml
└── src/
    ├── lib.rs          # module docs + re-exports
    ├── bypass.rs       # ImpervaBypass struct + wait_for_clearance + state machine
    ├── detection.rs    # surface detection (reese84/legacy/captcha signatures)
    ├── interception.rs # opt-in Fetch hook for /_Incapsula_Resource
    ├── error.rs        # ImpervaError + helpers
    └── detect.js       # embedded JS for tab.evaluate polling
```

### Cargo dependencies

```toml
[package]
name = "zendriver-imperva"
description = "Imperva WAF / Incapsula bypass for zendriver"

[dependencies]
zendriver-transport.workspace    = true
zendriver-interception.workspace = true
zendriver-stealth.workspace      = true
tokio.workspace                  = true
serde.workspace                  = true
serde_json.workspace             = true
thiserror.workspace              = true
tracing.workspace                = true

[dev-dependencies]
tokio-test.workspace = true
zendriver-transport  = { workspace = true, features = ["testing"] }
```

### Parent `zendriver` crate feature flag

```toml
[features]
imperva    = ["stealth", "interception", "dep:zendriver-imperva"]
cloudflare = ["stealth", "interception", "dep:zendriver-cloudflare"]  # retrofit — adds stealth requirement
```

Existing `cloudflare` feature gains `stealth` requirement. One breaking change; acceptable per pre-release memory.

`Tab::imperva()` convenience method lives in the parent `zendriver` crate behind the `imperva` feature, mirroring the existing `Tab::cloudflare()`.

## Components

### Public types

```rust
// detection.rs
pub enum ImpervaSurface {
    /// Modern reese84-based bot mgmt. Most common on Imperva ABP.
    Reese84,
    /// Legacy Incapsula (___utmvc / incap_ses_* / visid_incap_*).
    Legacy,
    /// Visual or invisible CAPTCHA challenge.
    Captcha(CaptchaKind),
    /// No Imperva surface detected.
    None,
}

pub enum CaptchaKind { HCaptcha, Recaptcha, ImpervaNative, Unknown }

// bypass.rs
pub struct ImpervaBypass<'tab> { /* tab handle + builder fields */ }

impl<'tab> ImpervaBypass<'tab> {
    pub fn new(tab: &'tab SessionHandle) -> Self;
    pub fn timeout(self, t: Duration) -> Self;              // default 30s
    pub fn poll_interval(self, t: Duration) -> Self;        // default 250ms
    pub fn with_interception(self, i: &'tab Interceptor) -> Self;
    pub fn on_captcha<F, Fut>(self, f: F) -> Self
        where F: Fn(CaptchaChallenge) -> Fut + Send + Sync + 'static,
              Fut: Future<Output = Result<CaptchaSolution, BoxError>> + Send;

    pub async fn wait_for_clearance(self) -> Result<ClearanceOutcome, ImpervaError>;
}

impl ImpervaBypass<'_> {
    /// Non-blocking surface probe. Useful for fast-path branching.
    pub async fn detect(tab: &SessionHandle) -> Result<ImpervaSurface, ImpervaError>;
}

pub enum ClearanceOutcome {
    /// reese84 cookie acquired AND body markers gone (S3 hybrid).
    TokenAcquired { reese84: String, sessions: Vec<CookieSnapshot> },
    /// Body markers gone but no explicit token (e.g., legacy flow).
    ChallengeGone,
    /// No challenge present at call time — fast path, no waiting.
    AlreadyClear,
}

// captcha types live in bypass.rs (small surface)
pub struct CaptchaChallenge {
    pub kind: CaptchaKind,
    pub site_key: Option<String>,
    pub url: String,
}

pub struct CaptchaSolution {
    pub token: String,
    pub form_field: String,  // e.g., "h-captcha-response"
}

// error.rs
#[derive(thiserror::Error, Debug)]
pub enum ImpervaError {
    #[error("clearance not achieved within {timeout:?}")]
    Timeout { timeout: Duration, last_surface: Option<ImpervaSurface> },
    #[error("CAPTCHA required but no solver registered: {kind:?}")]
    CaptchaRequired { kind: CaptchaKind },
    #[error("CAPTCHA solver failed: {0}")]
    CaptchaSolver(BoxError),
    #[error("interception hook error: {0}")]
    Interception(#[from] InterceptionError),
    #[error("transport error: {0}")]
    Transport(#[from] CallError),
}
```

### Detection internals

`detect.js` is a single embedded JS routine run via `tab.evaluate` per poll tick. It returns a JSON object containing:

- Whether `document.cookie` includes any of: `reese84`, `incap_ses_*`, `visid_incap_*`, `___utmvc`.
- The value of `reese84` if present (for `TokenAcquired` payload).
- All cookie names + value lengths + domain scopes for `sessions` payload.
- Whether the page body contains Imperva challenge markers: `/_Incapsula_Resource` script src, `<meta name="ROBOTS">` challenge stamp, Imperva challenge text patterns, CAPTCHA iframe src patterns (`hcaptcha.com`, `google.com/recaptcha`, Imperva-native CAPTCHA endpoint).

Surface inference precedence in `detection.rs`:

```
Captcha > Reese84 > Legacy > None
```

CAPTCHA wins because Imperva CAPTCHA challenges supersede invisible challenges when escalation has happened — the caller needs to know immediately so they can route to the solver path.

### Interception hook (opt-in)

When `.with_interception(&interceptor)` was set on the builder:

- Subscribe via `interceptor.subscribe()` to Fetch responses matching `*/Reese.js` or `*/_Incapsula_Resource*`.
- Send a unit oneshot to the waiter task on first 2xx response.
- Polling continues in parallel; first signal wins via `tokio::select!`.
- Final clearance verification still uses `detect.js` (single round-trip) to confirm S3 hybrid signal (cookie + body both clean).

If subscription startup fails → hard error (`ImpervaError::Interception`). The caller explicitly asked for the fast path; silent fallback would mask configuration problems.

## Data flow

### Happy path — `wait_for_clearance()`

```
User                ImpervaBypass        Tab (Chrome)         Imperva edge
 │                       │                    │                    │
 │ tab.imperva()         │                    │                    │
 │ .wait_for_clearance() │                    │                    │
 ├──────────────────────>│                    │                    │
 │                       │ detect.js evaluate │                    │
 │                       ├───────────────────>│                    │
 │                       │<───── surface ─────┤                    │
 │                       │                    │                    │
 │                  surface == None?          │                    │
 │                  → AlreadyClear (fast)     │                    │
 │                       │                    │                    │
 │                  surface == Reese84:       │                    │
 │                       │ poll loop (250ms)  │                    │
 │                       ├──cookie+body check>│                    │
 │                       │                    │ JS sensor compute  │
 │                       │                    ├───────────────────>│
 │                       │                    │<── reese84 cookie ─┤
 │                       │<── both signals ───┤                    │
 │<── TokenAcquired ─────┤                    │                    │
 │                       │                    │                    │
 │                  surface == Captcha:       │                    │
 │                       │ on_captcha set?    │                    │
 │                       │   yes → callback   │                    │
 │                       │   no  → Err        │                    │
 │                       │                    │                    │
 │                  timeout?                  │                    │
 │                       │ → Err Timeout      │                    │
```

### Three flow variants by surface

1. **Reese84 / Legacy (invisible).** Detect → poll loop → emit `TokenAcquired` (reese84 case) or `ChallengeGone` (legacy case where no reese84 ever sets).
2. **CAPTCHA + callback registered.** Detect → invoke `on_captcha(challenge)` → await user-provided solution → inject `solution.token` into `solution.form_field` via `tab.evaluate` → resume poll loop → emit outcome on next clearance signal.
3. **CAPTCHA + no callback.** Detect → immediate `Err(CaptchaRequired { kind })`. No timeout wait.

### Poll loop pseudocode

```rust
loop {
    if elapsed > timeout {
        return Err(Timeout { timeout, last_surface });
    }

    let snapshot = run_detect_js(&tab).await?;  // single evaluate

    match (snapshot.has_reese84, snapshot.body_clean) {
        (true, true)  => return Ok(TokenAcquired { reese84, sessions }),
        (false, true) => return Ok(ChallengeGone),
        _ => last_surface = Some(snapshot.surface),
    }

    tokio::select! {
        _ = tokio::time::sleep(poll_interval) => {},
        _ = interception_signal.recv() => {},   // if D3 hook active
    }
}
```

One CDP round-trip per tick: `detect.js` bundles all checks (cookies, body markers, script srcs) into one `tab.evaluate`. Prevents per-tick CDP storms.

## Error handling

### Returned errors

- **`Timeout { timeout, last_surface }`** — returned, never panics. Default timeout 30s, caller-tunable via `.timeout(...)`. `last_surface` aids caller diagnostics (e.g., "stuck on Reese84 for 30s" vs "saw CAPTCHA at end").
- **`CaptchaRequired { kind }`** — fast-path error within ~250ms when CAPTCHA detected without registered callback.
- **`CaptchaSolver(BoxError)`** — wraps any error from user-supplied callback. Original recoverable via `.source()`.
- **`Interception(InterceptionError)`** — hard fail if `.with_interception()` was set and Fetch subscription cannot be wired at startup. Silent fallback to polling would mask configuration issues.
- **`Transport(CallError)`** — wraps all CDP failures (`tab.evaluate` serialization errors, connection drops, etc.).

### Defensive (no error) behaviors

- Multiple `reese84` cookies across subdomains → pick the one whose `domain` attribute is most specific match to current eTLD+1.
- `document.cookie` race during navigation → catch + retry on next poll tick.
- `tab.evaluate` returns null/undefined when page mid-navigation → treat as "no signal yet", continue polling.
- Cookie set but value is `""` or literal `"undefined"` string → treat as not-yet-set.

### Telemetry

- `tracing::debug!` per poll iteration with surface + signal state.
- `tracing::info!` on successful clearance with elapsed time.
- `tracing::warn!` on first CAPTCHA surface detection (handled or not).
- `tracing::error!` on truly unexpected failures (interception subscription, CDP transport drops).

## Testing

### Unit tests (in-crate, `MockConnection` via `zendriver-transport/testing`)

State machine + flow coverage:

- `surface_none_returns_already_clear`
- `surface_reese84_polls_until_cookie_set_plus_body_clean`
- `surface_reese84_cookie_only_no_clearance`  *(S3 requires both)*
- `surface_reese84_body_only_no_clearance`
- `surface_legacy_returns_challenge_gone_when_markers_disappear`
- `surface_captcha_no_callback_errors_fast`
- `surface_captcha_with_callback_invokes_and_resumes`
- `timeout_fires_with_last_surface_in_error`
- `multiple_reese84_cookies_picks_most_specific_domain`
- `interception_signal_beats_polling`
- `interception_subscription_failure_is_hard_error`
- `cookie_value_empty_string_treated_as_unset`

Detection logic coverage:

- `detect_js_matches_known_imperva_script_src_patterns`
- `detect_js_distinguishes_hcaptcha_vs_recaptcha_vs_native`
- `detect_js_does_not_false_positive_on_clean_pages`  *(regression suite — feed N non-Imperva page DOMs)*

Target: ≥25 unit tests.

### Doctests

- `lib.rs` module-level usage example (compile-checked, `no_run`).
- `ImpervaBypass::wait_for_clearance` per-method doctest.
- `ImpervaBypass::with_interception` doctest.
- `ImpervaBypass::on_captcha` callback usage doctest.
- `ImpervaBypass::detect` non-blocking probe doctest.

Target: ~5 doctests.

### Nightly integration (advisory)

New workflow `.github/workflows/imperva-tests.yml`:

- Schedule offset 2h from the existing cloudflare nightly, 1h from the stealth nightly.
- Hits 2–3 known stable public Imperva-protected sites (specific list compiled during plan phase).
- Each test asserts clearance achievable within 60s timeout with `stealth + imperva` features enabled.
- Marked **advisory**: flakes do not block PRs. Public Imperva sites change challenge configurations unpredictably.

### CI gating

- Unit tests + doctests = blocking on PRs.
- Nightly integration = informational only.

## Retrofit: `zendriver-cloudflare` stealth requirement

To restore workspace consistency:

- `cloudflare` feature on parent `zendriver` crate adds `stealth` to its requirements list.
- `crates/zendriver-cloudflare/Cargo.toml` adds `zendriver-stealth.workspace = true` (currently absent).
- Existing `Tab::cloudflare()` documentation updated to mention stealth is implied.
- One existing breaking change for any user already opting into `cloudflare` without `stealth`. Pre-1.0, no published users, acceptable per memory.

Also flagged for cleanup in the same PR: `crates/zendriver-cloudflare/Cargo.toml` currently declares `zendriver-interception.workspace = true` but never imports it. Drop the stale dep.

## Out-of-scope but worth recording

- **Active reese84 synthesis** as a separate `imperva-sensor-rs` crate, if ever wanted. Should not share workspace.
- **`clearance_mode` builder knob** if a real site emerges that needs cookie-only or body-only.
- **Imperva ABP "newer" surfaces** beyond reese84 / legacy / CAPTCHA, if Imperva introduces a fresh challenge family.
- **Cookie-jar convenience** filtering only Imperva-relevant cookies for save/load — minor sugar over existing `CookieJar` API.

## Implementation phases (rough)

1. Workspace wiring: new crate Cargo.toml + skeleton + lib.rs re-exports + parent feature flag updates + cloudflare retrofit.
2. `detection.rs` + `detect.js` + unit tests for surface detection.
3. `bypass.rs` poll loop + `wait_for_clearance` + state machine + unit tests for flow variants.
4. `interception.rs` Fetch hook + unit tests for fast-path signal.
5. CAPTCHA callback wiring + unit tests for callback path.
6. Error type, telemetry, doctests, README snippet, mdBook chapter draft.
7. Nightly integration workflow + target site list.

Detailed task breakdown will land in the implementation plan.
