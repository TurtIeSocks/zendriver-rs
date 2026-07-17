# Fingerprint spoofing

zendriver-rs ships a first-class fingerprint layer that lets you control what
every browser surface reveals to detection scripts — canvas pixel noise, WebGL
renderer strings, WebRTC IP candidates, hardware hints, and more — without
touching any CDP internals directly.

## Two orthogonal axes

Fingerprint control lives on two independent axes:

| Axis | What it controls | Where it lives |
|------|-----------------|----------------|
| **Persona source** | The identity values injected (UA, platform, WebGL vendor, seed, …) | `zendriver-stealth` (core), or `zendriver-fingerprints` (pool / generative) |
| **Per-surface render strategy** | *How* each surface is modified in-page | `Strategy` enum, set per `Surface` |

You can mix any persona source with any per-surface strategy independently.

## The 7 surfaces

| Surface | Kind | Default strategy | What it affects |
|---------|------|-----------------|-----------------|
| `Canvas` | Noise | `Seeded` | `getImageData`, `toDataURL` pixel data |
| `Audio` | Noise | `Seeded` | `AnalyserNode` frequency / time-domain data |
| `ClientRects` | Noise | `Seeded` | `getBoundingClientRect` sub-pixel dimensions |
| `Webgl` | Value | `Value` | `UNMASKED_VENDOR_WEBGL`, `UNMASKED_RENDERER_WEBGL` |
| `Fonts` | Value | `Value` | `measureText` width noise + `FontFaceSet.check` allow-list |
| `Hardware` | Value | `Value` | Battery level, media-device count, speech voices |
| `Webrtc` | Policy | `Block` | ICE candidate leak suppression / fake IP |

## The 5 strategies

| Strategy | Effect |
|----------|--------|
| `Native` | No patch — raw browser output. |
| `Seeded` | Deterministic per-seed noise; same seed → same output every run. |
| `Random` | Fresh random noise on every call; maximally unpredictable. |
| `Block` | Empty / zero output (appropriate for policy surfaces). |
| `Value` | Substitute a specific value from the `Persona` spec. |

Noise surfaces (Canvas, Audio, ClientRects) accept `Native`, `Seeded`,
`Random`, `Block`. Value surfaces (Webgl, Fonts, Hardware) accept `Native`,
`Value`, `Block`. The policy surface (Webrtc) accepts `Native`, `Block`,
`Value` (fake IP). Requesting a meaningless combination logs a warning and
falls back to the surface's kind default.

## Persona sources

### `Persona::system()` — host-probed, cached

Reads the real machine's platform, CPU count, and memory via `sysinfo`. The
result is cached in a `OnceLock` — first call probes, subsequent calls clone.
A random seed is generated per process.

```rust,no_run
use zendriver::{Browser, Persona};

let browser = Browser::builder()
    .persona(Persona::system())
    .launch().await?;
```

### `Persona::builder()` — explicit

Build any combination of fields; unset fields inherit from `system()`.

```rust,no_run
use zendriver::{Browser, Persona, Seed};

let persona = Persona::builder()
    .seed(Seed::from_u64(42))       // reproducible noise
    .device_memory_gb(16)
    .timezone("America/Los_Angeles")
    .build();

let browser = Browser::builder()
    .persona(persona)
    .launch().await?;
```

### `Persona::from_browser(tab)` — live probe

Read the real browser's values (WebGL renderer, timezone, locale, …) from a
running `Tab` and produce a maximally coherent `Persona`. Useful when you want
to match the identity of an existing browser session.

```rust,no_run
use zendriver::{Browser, Persona};

let browser = Browser::builder().launch().await?;
let tab = browser.main_tab();
tab.goto("about:blank").await?;

let persona = Persona::from_browser(tab).await?;
println!("{:?}", persona.webgl);
```

### `Seed::from_system()` — machine-stable seed

Produces the same seed on every run on the same machine (derived from the
platform machine ID + hostname). Useful when you want a consistent identity
per machine without a `user_data_dir`.

```rust,no_run
use zendriver::{Browser, Persona, Seed};

let persona = Persona::builder()
    .seed(Seed::from_system())
    .build();
```

### Pool + generative sources (`zendriver-fingerprints`)

For real-device personas drawn from a dataset or a Bayesian network, add
the optional `zendriver-fingerprints` crate and enable the `pool` or
`generative` feature:

```toml
[dependencies]
zendriver-fingerprints = { version = "0.1", features = ["pool"] }
```

```rust,no_run
use zendriver_fingerprints::pool::PoolSet;
use zendriver_stealth::Seed;

// Build from a local JSON array (or load with load_or_download(url)).
let pool = PoolSet::from_json(include_str!("pool.json"))?;
let persona = pool.sample(Seed::from_u64(42));

// Pass to Browser::builder() in the zendriver crate.
```

## Per-surface strategy overrides

Override any surface's render strategy on top of the persona:

```rust,no_run
use zendriver::{Browser, Persona, Seed, Strategy, Surface};

let browser = Browser::builder()
    .persona(Persona::builder().seed(Seed::from_u64(42)).build())
    .surface(Surface::Webrtc, Strategy::Native)  // allow real IP
    .surface(Surface::Canvas, Strategy::Random)  // max entropy
    .launch().await?;
```

## Country → locale + timezone overlay (`geo_locale`)

The optional `geo` feature adds [`BrowserBuilder::geo_locale`], which maps an
ISO 3166-1 alpha-2 country code (e.g. `"US"`, `"de"`) to a coherent `locale` +
`languages` (Accept-Language) set drawn from a bundled CLDR-derived table,
**plus a representative IANA `timezone`** drawn from a bundled tz-database
table (wired through to `Emulation.setTimezoneOverride`). It is layered as a
**persona overlay**, so it composes with `.persona(..)` and is overridden by
an explicit `.persona_overlay(..)` locale. An invalid / unknown country code
is ignored (logged) — the value is never locked.

**Representative-zone caveat:** countries spanning multiple timezones (the
US, Russia, Canada, Australia, Brazil, ...) resolve to a single representative
zone (the country's first `zone1970.tab` entry, with a few curated overrides
— e.g. `RU` → `Europe/Moscow`, not `Europe/Kaliningrad`), not any particular
visitor's actual local zone. Treat it as a coherent default, not a precise
one — set `.persona(Persona::builder().timezone("America/Los_Angeles").build())`
(or `.persona_overlay(..)`) when a specific zone within the country matters.

```toml
[dependencies]
zendriver = { version = "0.1", features = ["geo"] }
```

```rust,no_run
use zendriver::Browser;

let browser = Browser::builder()
    .geo_locale("DE")   // de-DE locale + matching Accept-Language
    .launch().await?;
```

[`BrowserBuilder::geo_locale`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.geo_locale

## Auto IP-geo resolution (`geo_auto`)

`geo_locale` requires knowing the country up front. When you don't — e.g. the
browser is routed through a rotating or third-party proxy pool and you want
the locale to match wherever that proxy happens to exit — use
[`BrowserBuilder::geo_auto`] instead. It probes the exit IP through a bundled
[`IpApiResolver`] (a proxied GET against `ip-api.com`) and folds the resulting
country's locale/languages into the persona overlay, with the exact same
precedence as `geo_locale`: an explicit `.persona(..)`/`.persona_overlay(..)`
locale always wins and skips the probe entirely.

**Timezone precision beats `geo_locale` here:** `ip-api.com`'s response
carries the exit IP's exact IANA `timezone`, not just its country, so
`geo_auto` uses that EXACT zone instead of the country-representative one —
multi-timezone countries (US, RU, CA, AU, BR, ...) get the visitor's real
local zone, not an approximation. (A custom [`GeoResolver`] that can't
determine an exact zone returns `timezone: None`, and `geo_auto` falls back to
the same country-representative zone `geo_locale` uses.) Precedence:
explicit `.persona(..)`/`.persona_overlay(..)` timezone > exact probe
timezone > country-representative timezone.

```rust,no_run
use zendriver::Browser;

let browser = Browser::builder()
    .proxy("http://user:pass@residential-proxy.example:8000")
    .geo_auto()   // probes the exit IP through the proxy above, credentials included
    .launch().await?;
```

`geo_auto()` mirrors the proxy's credentials into the probe too (via
`reqwest::Proxy::basic_auth`, never embedded in a URL string), so an
authenticated proxy like the one above is probed authenticated — the probe
would otherwise 407 silently and fail soft with no overlay.

**Privacy:** the bundled `ip-api.com` probe fires ONLY when `.geo_auto()` (or
`.geo_resolver()`) is called — it is fully opt-in, never implicit. Failure
(no network, proxy down, unrecognized country) is fail-soft: a
`tracing::warn!` is logged and `launch()` proceeds with no overlay; it never
blocks or fails the launch. The default endpoint (`http://ip-api.com/json`)
is **plaintext HTTP** — a proxy operator can observe or tamper with the
response in transit; override [`IpApiResolver::endpoint`] to an HTTPS service
if response integrity matters for your threat model.

### Structured `proxy(..)`

[`BrowserBuilder::proxy`] parses a `scheme://[user:pass@]host:port` URL,
strips the userinfo before emitting `--proxy-server=` (Chrome ignores
credentials there), and auto-wires `proxy_auth` from the userinfo when set
(requires the `interception` feature to actually answer the
`Fetch.authRequired` challenge). It also makes `geo_auto()`'s probe traffic
mirror the same upstream proxy the browser itself will use, so the resolved
country matches the exit IP Chrome actually sees.

### Custom resolver (`geo_resolver`)

Swap the bundled `ip-api.com` probe for your own service, an offline
MaxMind-style DB, or a test double by implementing
[`zendriver_stealth::geo::GeoResolver`] and passing it to
[`BrowserBuilder::geo_resolver`]. `resolve()` returns a
[`ResolvedGeo`][`zendriver_stealth::geo::ResolvedGeo`] — the country plus an
optional exact `timezone`; return `timezone: None` if your source can't
determine one more precise than the country-representative zone:

```rust,no_run
use async_trait::async_trait;
use zendriver::Browser;
use zendriver_stealth::geo::{Country, GeoResolver, ResolvedGeo};

struct MyResolver;

#[async_trait]
impl GeoResolver for MyResolver {
    async fn resolve(&self) -> Option<ResolvedGeo> {
        // Query your own service / offline DB instead of ip-api.com.
        Some(ResolvedGeo {
            country: Country::try_from("DE").ok()?,
            timezone: Some("Europe/Berlin".to_string()),
        })
    }
}

let browser = Browser::builder()
    .geo_resolver(MyResolver)
    .launch().await?;
```

Only ONE of `geo_auto()` / `geo_resolver(..)` takes effect (the last one
called wins — both set the same underlying resolver slot).

[`BrowserBuilder::geo_auto`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.geo_auto
[`BrowserBuilder::geo_resolver`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.geo_resolver
[`BrowserBuilder::proxy`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.proxy
[`IpApiResolver`]: https://docs.rs/zendriver/latest/zendriver/struct.IpApiResolver.html
[`IpApiResolver::endpoint`]: https://docs.rs/zendriver/latest/zendriver/struct.IpApiResolver.html#method.endpoint
[`zendriver_stealth::geo::GeoResolver`]: https://docs.rs/zendriver-stealth/latest/zendriver_stealth/geo/trait.GeoResolver.html
[`zendriver_stealth::geo::ResolvedGeo`]: https://docs.rs/zendriver-stealth/latest/zendriver_stealth/geo/struct.ResolvedGeo.html

## JSON persona (`try_from_json`)

Any `Persona` can be expressed as a JSON object and round-trips cleanly.
Fields are snake_case; all fields are optional. Useful for configuration files
or environment variables:

```rust,no_run
use zendriver::Persona;

let persona: Persona = Persona::try_from_json(r#"{
  "timezone": "Europe/Berlin",
  "device_memory_gb": 8,
  "seed": 12345,
  "webgl": {
    "unmasked_vendor":   "Google Inc. (NVIDIA)",
    "unmasked_renderer": "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)"
  },
  "webrtc": { "strategy": "Block" }
}"#).unwrap();
```

You can also parse via `FromStr`:

```rust,no_run
use zendriver::Persona;

let persona: Persona = r#"{"seed": 99, "timezone": "UTC"}"#.parse().unwrap();
```
