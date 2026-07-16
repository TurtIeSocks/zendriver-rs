# Auto IP-Geo Resolution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** An opt-in `BrowserBuilder::geo_auto()` that probes the exit IP through the browser's proxy, maps the country to a coherent locale/languages persona overlay, and applies it at launch — plus `geo_resolver()` for custom resolvers.

**Architecture:** A structured `BrowserBuilder::proxy(url)` (reusing `crate::proxy::split_proxy_url` from the per-context work) makes the proxy mirrorable. `IpApiResolver` (in `zendriver`, behind `geo`+`reqwest`) implements the existing `zendriver_stealth::geo::GeoResolver` trait via a proxied `reqwest` GET. `launch()`/`connect()` run the resolver async before `StealthObserver::with_persona`, folding `geo::persona(country)` into `persona_overlay`.

**Tech stack:** Rust 2024, Tokio, `reqwest` (optional dep, already present), `async-trait`, `wiremock` (dev-dep under `integration-tests`), `MockConnection`.

## Global Constraints
- **MSRV 1.85, edition 2024.**
- **`geo` is opt-in; the whole feature is `#[cfg(feature = "geo")]`.** Default-feature build must be unaffected.
- **Reuse, don't duplicate:** `crate::proxy::split_proxy_url` / `ParsedProxy` already exist (per-context proxy work) — reuse for `BrowserBuilder::proxy`.
- **Precedence:** an explicit `geo_locale()`/`persona_overlay()` locale always beats the auto-derived one. Match the exact overlay-merge pattern `geo_locale` already uses (`browser.rs` ~1015-1019) so precedence is correct by construction; if both `geo_auto` and an explicit locale are set, skip the probe.
- **Fail-soft:** probe failure / no network / unknown country → `tracing::warn!` + no overlay; NEVER block or fail `launch()`.
- **Privacy:** the bundled `ip-api.com` probe fires only on `.geo_auto()`; endpoint overridable; `GeoResolver` trait swappable.
- **Before commit:** `cargo fmt --all`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; and since this is feature-gated, `cargo clippy -p zendriver --all-targets --features geo -- -D warnings`. Schema-snapshot + public-api steps per CLAUDE.md when MCP/public API changes (Task 4).

---

### Task 1: `BrowserBuilder::proxy(url)` — structured browser-wide proxy

**Files:** Modify `crates/zendriver/src/browser.rs` (builder field + method + arg emission at launch); Test: same file `#[cfg(test)]`.

**Interfaces:**
- Consumes: `crate::proxy::{split_proxy_url, ParsedProxy}`.
- Produces: `pub fn proxy(self, impl Into<String>) -> Self`; a `pub(crate) proxy: Option<crate::proxy::ParsedProxy>` field on `BrowserBuilder` (read by Task 3's `geo_auto`).

- [ ] **Step 1: Failing test** — assert `Browser::builder().proxy("http://user:pass@host:3128")` (a) stores a `ParsedProxy { server: "http://host:3128", credentials: Some(("user","pass")) }`, and (b) at launch emits `--proxy-server=http://host:3128` (userinfo-stripped) into the Chrome args, and (c) auto-populates `proxy_auth` from the userinfo when not already set. Model arg-inspection on the existing `--proxy-server` test (`browser.rs:4054`) and the per-context builder tests.

```rust
#[test]
fn proxy_stores_parsed_and_strips_userinfo_arg() {
    let b = Browser::builder().proxy("http://bob:pw@host:3128");
    let p = b.proxy.as_ref().unwrap();
    assert_eq!(p.server, "http://host:3128");
    assert_eq!(p.credentials, Some(("bob".into(), "pw".into())));
    // proxy_auth auto-wired from userinfo
    assert_eq!(b.proxy_auth, Some(("bob".into(), "pw".into())));
}
```
(Add a launch-arg assertion following the `--proxy-server` arg-inspection pattern already in the file.)

- [ ] **Step 2: Run — fails** (`no field proxy` / `no method proxy`). `cargo test -p zendriver --lib proxy_stores_parsed --features geo`

- [ ] **Step 3: Implement.** Add field near `proxy_auth`:
```rust
    /// Structured browser-wide proxy (userinfo-stripped server + optional
    /// credentials), set via [`BrowserBuilder::proxy`]. Emitted as
    /// `--proxy-server=` at launch and mirrored by `geo_auto`'s probe.
    pub(crate) proxy: Option<crate::proxy::ParsedProxy>,
```
Initialize `proxy: None` at every `BrowserBuilder` constructor (grep the builder's `Default`/`new`; there is typically one). Add the method:
```rust
    /// Route the browser through `proxy` (`scheme://[user:pass@]host:port`).
    /// Emits `--proxy-server=<host:port>` (Chrome ignores userinfo there) and,
    /// when the URL carries credentials and `proxy_auth` is unset, auto-wires
    /// them. The structured form lets `geo_auto()` probe the exit IP through
    /// the same proxy.
    ///
    /// # Errors
    /// Silently ignores an unparseable URL (logs a warning) to keep the
    /// builder chainable; the bad value simply isn't applied.
    #[must_use]
    pub fn proxy(mut self, proxy: impl Into<String>) -> Self {
        let raw = proxy.into();
        match crate::proxy::split_proxy_url(&raw) {
            Ok(parsed) => {
                if self.proxy_auth.is_none() {
                    if let Some((u, p)) = parsed.credentials.clone() {
                        self.proxy_auth = Some((u, p));
                    }
                }
                self.proxy = Some(parsed);
            }
            Err(e) => tracing::warn!(error = %e, "proxy: ignoring invalid proxy URL"),
        }
        self
    }
```
At launch arg assembly, when `self.proxy` is set and no explicit `--proxy-server` arg is present, push `format!("--proxy-server={}", parsed.server)`. (Find where args are finalized before spawn; follow the existing pattern for injected flags.)

- [ ] **Step 4: Run — passes** (both feature sets: default build unaffected because the field/method are un-gated but harmless; `--features geo` for the resolver later). `cargo test -p zendriver --lib proxy_stores_parsed`

- [ ] **Step 5: Commit** — `feat(browser): structured BrowserBuilder::proxy reusing split_proxy_url`

---

### Task 2: `IpApiResolver` — proxied exit-IP → country probe

**Files:** Create `crates/zendriver/src/geo_resolver.rs`; Modify `crates/zendriver/src/lib.rs` (`#[cfg(feature="geo")] mod geo_resolver;` + re-export), `crates/zendriver/Cargo.toml` (`geo` pulls `reqwest`); Test: inline `#[cfg(test)]` using `wiremock`.

**Interfaces:**
- Consumes: `zendriver_stealth::geo::{GeoResolver, Country}`, `reqwest`.
- Produces: `pub struct IpApiResolver` with `new()`, `endpoint(self, impl Into<String>)`, `timeout(self, Duration)`, `pub(crate) fn with_proxy(self, Option<String>)`; `impl GeoResolver for IpApiResolver`.

- [ ] **Step 1: Cargo** — change `crates/zendriver/Cargo.toml`: `geo = ["zendriver-stealth/geo", "dep:reqwest"]`. Run `cargo build -p zendriver --features geo` to confirm reqwest resolves.

- [ ] **Step 2: Failing test** (wiremock canned body):
```rust
#[tokio::test]
async fn resolves_country_from_ipapi_json() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(wiremock::ResponseTemplate::new(200)
            .set_body_string(r#"{"countryCode":"DE"}"#))
        .mount(&server).await;
    let r = IpApiResolver::new().endpoint(server.uri());
    assert_eq!(r.country().await, Some(Country::try_from("DE").unwrap()));
}

#[tokio::test]
async fn bad_body_yields_none() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("nope"))
        .mount(&server).await;
    assert_eq!(IpApiResolver::new().endpoint(server.uri()).country().await, None);
}
```
(`wiremock` is a dev-dep under `integration-tests`; gate the test module `#[cfg(all(test, feature = "geo", feature = "integration-tests"))]` OR confirm wiremock is available to the `geo` test build — if not, add `wiremock` to dev-deps unconditionally and gate the test on `feature="geo"`.)

- [ ] **Step 3: Run — fails.**

- [ ] **Step 4: Implement** `crates/zendriver/src/geo_resolver.rs`:
```rust
//! `IpApiResolver`: derive the exit-IP country via a proxied HTTP probe.
use std::time::Duration;
use async_trait::async_trait;
use zendriver_stealth::geo::{Country, GeoResolver};

/// Resolves the apparent country by querying an IP-geolocation service
/// (default `ip-api.com`) through the browser's proxy. Opt-in via
/// [`crate::BrowserBuilder::geo_auto`]; endpoint overridable; swap the whole
/// thing out with a custom [`GeoResolver`].
pub struct IpApiResolver {
    endpoint: String,
    proxy: Option<String>,
    timeout: Duration,
}
impl Default for IpApiResolver { fn default() -> Self { Self::new() } }
impl IpApiResolver {
    #[must_use] pub fn new() -> Self {
        Self { endpoint: "http://ip-api.com/json".into(), proxy: None, timeout: Duration::from_secs(5) }
    }
    #[must_use] pub fn endpoint(mut self, url: impl Into<String>) -> Self { self.endpoint = url.into(); self }
    #[must_use] pub fn timeout(mut self, d: Duration) -> Self { self.timeout = d; self }
    #[must_use] pub(crate) fn with_proxy(mut self, proxy: Option<String>) -> Self { self.proxy = proxy; self }
}
#[async_trait]
impl GeoResolver for IpApiResolver {
    async fn country(&self) -> Option<Country> {
        let mut b = reqwest::Client::builder().timeout(self.timeout);
        if let Some(p) = &self.proxy {
            match reqwest::Proxy::all(p) {
                Ok(px) => b = b.proxy(px),
                Err(e) => { tracing::warn!(error=%e, "geo probe: bad proxy; skipping"); return None; }
            }
        }
        let client = b.build().ok()?;
        let resp = match client.get(&self.endpoint).send().await {
            Ok(r) => r, Err(e) => { tracing::warn!(error=%e, "geo probe request failed"); return None; }
        };
        let body: serde_json::Value = resp.json().await.ok()?;
        let cc = body.get("countryCode").and_then(|v| v.as_str())?;
        match Country::try_from(cc) {
            Ok(c) => Some(c),
            Err(_) => { tracing::warn!(country=%cc, "geo probe: unrecognized country code"); None }
        }
    }
}
```
Wire `mod geo_resolver;` (gated) + `pub use geo_resolver::IpApiResolver;` in `lib.rs`. Confirm `Country: TryFrom<&str>` exists (it does — `geo_locale` uses `TryInto`); if the concrete `TryFrom<&str>` isn't public, use the same conversion `geo_locale` uses.

- [ ] **Step 5: Run — passes.** `cargo test -p zendriver --lib --features geo geo_resolver::`

- [ ] **Step 6: Commit** — `feat(geo): add IpApiResolver (proxied exit-IP country probe)`

---

### Task 3: `geo_auto()` / `geo_resolver()` + launch integration

**Files:** Modify `crates/zendriver/src/browser.rs` (builder fields/methods + launch/connect wiring); Test: same file `#[cfg(test)]`.

**Interfaces:**
- Consumes: `IpApiResolver` (Task 2), `self.proxy` (Task 1), `zendriver_stealth::geo::{GeoResolver, persona}`, `self.persona_overlay`, `resolved_persona()`.
- Produces: `pub fn geo_auto(self) -> Self`, `pub fn geo_resolver(self, impl GeoResolver + 'static) -> Self`; `pub(crate) geo_resolver: Option<std::sync::Arc<dyn GeoResolver>>` field.

- [ ] **Step 1: Failing test** — with a stub resolver, launch resolves the persona locale:
```rust
#[cfg(feature = "geo")]
#[tokio::test]
async fn geo_resolver_augments_persona_at_launch() {
    struct StubDe;
    #[async_trait::async_trait]
    impl zendriver_stealth::geo::GeoResolver for StubDe {
        async fn country(&self) -> Option<zendriver_stealth::geo::Country> {
            Some(zendriver_stealth::geo::Country::try_from("DE").unwrap())
        }
    }
    // Build a BrowserBuilder with .geo_resolver(StubDe), then drive the
    // same overlay-resolution path launch() uses (extract it into a testable
    // async helper `apply_geo_overlay(&mut self)` so the test needs no Chrome),
    // and assert resolved_persona().locale == Some("de-DE").
}
```
Design note for the implementer: extract the async overlay-merge into a `#[cfg(feature="geo")] async fn apply_geo_overlay(&mut self)` so it's unit-testable without launching Chrome; `launch()`/`connect()` call it before `StealthObserver::with_persona`. Also add: an explicit `geo_locale("US")` + `geo_resolver(StubDe)` → locale stays `en-US` (explicit wins, probe skipped).

- [ ] **Step 2: Run — fails.**

- [ ] **Step 3: Implement.** Fields + methods:
```rust
    #[cfg(feature = "geo")]
    pub(crate) geo_resolver: Option<std::sync::Arc<dyn zendriver_stealth::geo::GeoResolver>>,
```
(init `None` at the builder constructor(s), gated.)
```rust
    /// Auto-derive locale/languages from the exit IP's country via a proxied
    /// probe to `ip-api.com`. Opt-in; makes ONE outbound request at launch
    /// only when set. Overridden by an explicit `geo_locale`/`persona_overlay`
    /// locale (which also skips the probe). Mirrors the proxy set via
    /// [`Self::proxy`].
    #[cfg(feature = "geo")]
    #[must_use]
    pub fn geo_auto(mut self) -> Self {
        let proxy = self.proxy.as_ref().map(|p| p.server.clone());
        self.geo_resolver = Some(std::sync::Arc::new(
            crate::IpApiResolver::new().with_proxy(proxy),
        ));
        self
    }

    /// Supply a custom [`GeoResolver`] (your own service / offline DB).
    #[cfg(feature = "geo")]
    #[must_use]
    pub fn geo_resolver(mut self, resolver: impl zendriver_stealth::geo::GeoResolver + 'static) -> Self {
        self.geo_resolver = Some(std::sync::Arc::new(resolver));
        self
    }
```
The overlay helper (match `geo_locale`'s existing merge direction so explicit wins):
```rust
    #[cfg(feature = "geo")]
    async fn apply_geo_overlay(&mut self) {
        let Some(resolver) = self.geo_resolver.clone() else { return };
        // explicit locale already set → skip the probe entirely
        if self.persona_overlay.as_ref().is_some_and(|p| p.locale.is_some()) { return; }
        if let Some(country) = resolver.country().await {
            let derived = zendriver_stealth::geo::persona(country);
            self.persona_overlay = Some(match self.persona_overlay.take() {
                Some(existing) => existing.overlay(derived), // SAME direction as geo_locale
                None => derived,
            });
        }
    }
```
Call `self.apply_geo_overlay().await;` in both `launch()` (before browser.rs:2536) and `connect()` (before ~2846). **Verify the overlay direction against `geo_locale` (browser.rs ~1015): use whichever of `existing.overlay(derived)` / `derived.overlay(existing)` `geo_locale` uses — they must match so precedence is identical.**

- [ ] **Step 4: Run — passes** (`--features geo`).

- [ ] **Step 5: Commit** — `feat(geo): geo_auto/geo_resolver builder + launch-time exit-IP resolution`

---

### Task 4: MCP coverage + docs + backlog

**Files:** `crates/zendriver-mcp/src/tools/*` (browser_open geo fields), `mcp-coverage-ledger.toml`, `public-api-baseline.txt`, snapshots; `docs/book/src/*geo*`, `README.md`, `docs/superpowers/deferred-backlog.md`.

- [ ] **Step 1: MCP.** Add `geo_auto: bool` (and optional `geo_endpoint: Option<String>`) to `browser_open` under the `geo` MCP feature, mirroring how `geo_country` was wired in PR #45 (grep `geo_country` in `crates/zendriver-mcp/`). Ledger the `geo_resolver(impl GeoResolver)` method as `excluded` (a trait object has no wire form); `geo_auto`/`proxy` get `covered` entries.
- [ ] **Step 2: Regenerate + accept snapshots + public-api baseline** per CLAUDE.md (`cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked` + `cargo insta accept --all`; `cargo +nightly public-api …` if flagged).
- [ ] **Step 3: Docs.** Rustdoc already inline (Tasks 1-3). mdBook geo chapter: auto-resolution section + the privacy note + a custom-`GeoResolver` example + `.proxy()` mention. README geo bullet. `mdbook build docs/book`.
- [ ] **Step 4: Backlog.** Move the §1 "Auto IP-geo resolution / Option B exit-IP probe" item to the `✅ closed` section (shipped via `geo_auto`/`IpApiResolver`); note timezone-from-geo remains open.
- [ ] **Step 5: Final gates + commit** — `fmt`; `clippy --workspace` + `-p zendriver --features geo` + `-p zendriver-mcp --all-features`; `cargo test -p zendriver --features geo`. Commit `docs(geo): expose geo_auto over MCP + sync docs and backlog`.

---

## Self-Review
- **Structured proxy** (Task 1) → prerequisite for proxy-mirrored probe. ✓ reuses split_proxy_url.
- **IpApiResolver** (Task 2) → the probe; wiremock-tested, fail-soft. ✓
- **geo_auto/geo_resolver + launch wiring** (Task 3) → async resolve, explicit-wins precedence, testable helper. ✓
- **MCP + docs + backlog** (Task 4). ✓
- **Precedence correctness** hinges on matching `geo_locale`'s overlay direction — Task 3 Step 3 explicitly instructs verifying it against `browser.rs:~1015`. Flagged.
- **Types:** `ParsedProxy`/`split_proxy_url` (Task 1), `IpApiResolver`/`GeoResolver`/`Country` (Task 2), `geo_resolver` field + `apply_geo_overlay` (Task 3) consistent across tasks.
- **Open risk:** `wiremock` availability under the `geo` (non-`integration-tests`) test build — Task 2 Step 2 tells the implementer to reconcile (add to dev-deps or gate the test).
