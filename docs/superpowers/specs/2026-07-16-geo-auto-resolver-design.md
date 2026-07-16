# Design: auto IP-geo resolution (the `GeoResolver` "option B" exit-IP probe)

**Date:** 2026-07-16
**Status:** delegate-mode design; assumptions flagged for veto at review
**Crates:** `zendriver` (builder + `IpApiResolver` + launch wiring), `zendriver-stealth` (the `GeoResolver` trait already ships here)
**Feature:** `geo` (extended to pull `reqwest` for the HTTP probe)
**Backlog item:** §1 geo — "Auto IP-geo resolution / Option B exit-IP probe" (deferred in PR #45, which shipped only the `GeoResolver` seam).

## Problem

PR #45 shipped the caller-supplies-country path (`BrowserBuilder::geo_locale("US")`) and an **empty `GeoResolver` seam** (`zendriver-stealth/src/geo/mod.rs`: `#[async_trait] pub trait GeoResolver { async fn country(&self) -> Option<Country>; }`), explicitly deferring the auto-resolving implementation ("`IpApiResolver` + structured proxy URL + outbound probe") to a follow-up. This is that follow-up: derive a coherent `locale`/`languages` automatically from the **exit IP's** country — critically, the *proxy's* exit IP, not the host's.

Today `geo_locale` layers a geo persona overlay at builder time (sync). Auto-resolution is inherently async (a network probe), so it must run during `launch()`/`connect()`.

## Goals / non-goals

- **Goal:** an opt-in `BrowserBuilder::geo_auto()` that probes the exit IP through the browser's proxy, maps the country to a persona overlay via the existing `geo::persona(country)`, and applies it before the first navigation. Plus `geo_resolver(...)` for a fully custom resolver.
- **Non-goal (v1):** per-context geo (each `BrowserContext` its own exit IP). Browser-wide only; per-context is a later extension (post-launch tab probe).
- **Non-goal:** bundling an offline GeoIP database. Users who want offline resolution implement `GeoResolver` themselves.

## The exit-IP subtlety (why a structured proxy is a prerequisite)

To learn the *proxy's* exit country, the probe must egress **through the same proxy** as the browser. Today the browser-wide proxy is a raw `.arg("--proxy-server=…")` — unstructured, so nothing can mirror it. So v1 adds:

### `BrowserBuilder::proxy(impl Into<String>)`  (prerequisite)
A structured browser-wide proxy. Parses via the existing `crate::proxy::split_proxy_url` (built for per-context proxy auth), then:
- emits `--proxy-server=<server>` (userinfo-stripped) into the launch args,
- if the URL carried userinfo and no explicit `proxy_auth` is set, auto-wires `proxy_auth(user, pass)` (mirrors the per-context builder's ergonomic),
- stores the parsed proxy on the builder so the `IpApiResolver` can mirror it.

`proxy(...)` and a manual `.arg("--proxy-server=…")` are both honored; if both are set, `proxy(...)` wins for the resolver-mirroring purpose (document it). Precedence and coexistence with `proxy_auth` mirror the per-context design.

## The resolver

### `GeoResolver` trait — unchanged
Stays in `zendriver-stealth` (pure, no network dep): `async fn country(&self) -> Option<Country>`.

### `IpApiResolver` — new, in `zendriver` core (behind `geo`)
Lives in `zendriver` (which has `reqwest`), not `zendriver-stealth` (which must stay network-free). Implements `GeoResolver`.

```rust
pub struct IpApiResolver {
    endpoint: String,          // default "http://ip-api.com/json"
    proxy: Option<String>,     // mirror the browser proxy (set by geo_auto from BrowserBuilder::proxy)
    timeout: Duration,         // default 5s
}
impl IpApiResolver {
    pub fn new() -> Self { /* defaults */ }
    pub fn endpoint(self, url: impl Into<String>) -> Self { … }   // override the service
    pub fn timeout(self, d: Duration) -> Self { … }
}
```
`country()`:
1. Build a `reqwest::Client` with `.proxy(reqwest::Proxy::all(proxy)?)` when `proxy` is set, and the timeout.
2. GET the endpoint; parse the JSON `countryCode` field (ip-api.com returns e.g. `{"countryCode":"US",...}`).
3. `Country::try_from(country_code).ok()` — on any error (network, parse, invalid code) return `None` + `tracing::warn!`. Never panics, never blocks beyond the timeout.

**Privacy:** this makes an outbound call to a third-party service (`ip-api.com`). It fires ONLY when the user calls `.geo_auto()`. The endpoint is overridable, and the `GeoResolver` trait lets users swap in their own service or an offline DB. Per the stealth-hardening philosophy (volatile/external → expose config, never hardcode).

## Builder API

- `BrowserBuilder::geo_auto()` — use the default `IpApiResolver`, mirroring the browser proxy set via `BrowserBuilder::proxy(...)` (if any). Sugar over `geo_resolver(IpApiResolver::new()...)`.
- `BrowserBuilder::geo_resolver(impl GeoResolver + 'static)` — supply any resolver (stored as `Option<Arc<dyn GeoResolver>>`).
- Both gated `#[cfg(feature = "geo")]`.
- **Precedence:** the auto-derived overlay is applied with *lower* precedence than an explicit `geo_locale(...)` / `persona_overlay(...)` locale — explicit always wins. If both `geo_auto` and `geo_locale` are set, `geo_locale` wins (and the probe is skipped, to avoid a pointless network call — document it).

## Launch integration

In `launch()` and `connect()`, immediately before the `StealthObserver::with_persona(…, self.resolved_persona(), …)` calls (browser.rs:2536 / 2846):

```rust
#[cfg(feature = "geo")]
if let Some(resolver) = self.geo_resolver.take() {
    // skip if an explicit geo/locale overlay already set the locale
    if !self.persona_overlay.as_ref().is_some_and(|p| p.locale.is_some()) {
        if let Some(country) = resolver.country().await {
            let derived = zendriver_stealth::geo::persona(country);
            self.persona_overlay = Some(match self.persona_overlay.take() {
                Some(existing) => derived.overlay(existing), // existing wins over derived
                None => derived,
            });
        }
    }
}
```
Then `resolved_persona()` picks up the augmented overlay exactly as today. Fail-soft: a `None` country leaves the overlay untouched and launch proceeds.

**Latency/robustness:** launch now waits up to the probe timeout (default 5s) when `geo_auto` is set. Bounded, opt-in. A failed probe never blocks launch.

## Feature gating

`geo` gains `reqwest`: `geo = ["zendriver-stealth/geo", "dep:reqwest"]` in `crates/zendriver/Cargo.toml`. `reqwest` is already an optional dep (used by `tracker-blocking`), so this only compiles it in when `geo` is on. The `GeoResolver` trait and `Country` stay in stealth (no network dep). (Alternative considered: a separate `geo-auto` sub-feature to keep `geo` network-free — rejected for v1 as feature-sprawl; revisit if the reqwest pull on `geo` bothers downstreams.)

## Errors

Reuse `tracing::warn!` + `Option` returns throughout (mirrors `geo::persona`'s unknown-country handling). `IpApiResolver::country` never returns an error type — it logs and yields `None`. `BrowserBuilder::proxy` reuses `ZendriverError::Navigation` for an unparseable URL (same as the per-context builder).

## Testing

- **Unit (no network):** `IpApiResolver` country parsing from a canned JSON body (inject the body / use a `wiremock` server already a dev-dep under `integration-tests`); proxy-mirroring config assembled correctly; precedence logic (geo_locale set → probe skipped); overlay merge keeps explicit locale winning.
- **`BrowserBuilder::proxy`:** emits the stripped `--proxy-server=` arg + auto-wires proxy_auth from userinfo + stores the parsed proxy (MockConnection / arg inspection, mirroring the per-context proxy tests).
- **Launch integration (MockConnection):** a stub `GeoResolver` returning `Some("DE")` → the resolved persona carries `de-DE`; returning `None` → persona untouched.
- **Integration (`#[ignore]`, real network):** `geo_auto()` against a live `ip-api.com` (behind a known proxy if available) resolves a plausible country. Joins the nightly `#[ignore]` set.

## MCP coverage

Add `geo_auto` + a `geo_resolver`/`geo_proxy` shape to `browser_open` under the existing `geo` MCP feature (mirrors how `geo_country` was added in PR #45), OR ledger the `GeoResolver`-trait-taking method as `excluded` (a trait object doesn't fit a wire tool) while exposing `geo_auto` (a bool) + an optional endpoint override on `browser_open`. Regenerate schema snapshots + public-api baseline per CLAUDE.md.

## Docs

Rustdoc (no_run) on `proxy`, `geo_auto`, `geo_resolver`, `IpApiResolver`. mdBook: extend the geo chapter (auto-resolution + the privacy note + custom-resolver example). README geo bullet. Flip the backlog §1 "Auto IP-geo" item to closed on ship.

## Assumptions (delegate-mode calls — flag any to veto at review)

1. **Bundled opt-in `ip-api.com` default** for `geo_auto()`, endpoint-overridable + trait-swappable. (The one fork surfaced to the user; absent a redirect, this ships. Alternative: trait-only, no bundled service.)
2. **New `BrowserBuilder::proxy(url)`** structured field as a prerequisite, reusing `split_proxy_url` + auto-wiring `proxy_auth` from userinfo.
3. **Browser-wide, pre-launch proxied `reqwest` probe** (not a post-launch browser-tab probe); per-context geo deferred.
4. **`geo` feature pulls `reqwest`** (no separate sub-feature for v1).
5. **Explicit `geo_locale`/`persona_overlay` locale beats auto**, and setting both skips the probe.
6. **5s default probe timeout**, fail-soft (never blocks launch).
