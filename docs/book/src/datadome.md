# DataDome

The `datadome` cargo feature (sub-crate: `zendriver-datadome`) provides a
passive bypass driver for sites protected by DataDome. It detects the active
DataDome surface (`device_check`, `captcha`, or `block`), then polls the page
until the `datadome` clearance cookie lands, optionally escalating a CAPTCHA
surface to a caller-supplied async solver.

> **Stealth strongly recommended.** DataDome's dominant surface is an
> **invisible device-check** that scores the browser fingerprint. Without
> `BrowserBuilder::stealth` the device-check will not clear on the vast
> majority of real DataDome-protected sites. The `stealth()` profile now
> includes the `Surface::Webgpu` coherence patch (issue #20) which aligns
> `navigator.gpu` adapter info with the spoofed WebGL renderer — a DataDome
> signal previously unmasked.

## Enabling the feature

```toml
[dependencies]
zendriver = { version = "*", features = ["datadome"] }
```

To run the integration test suite against a real Chrome:

```bash
cargo test -p zendriver --features datadome-tests --test datadome_v0 -- --ignored
```

## Quick start

```rust,no_run
use std::time::Duration;
use zendriver::stealth::StealthProfile;
use zendriver::{Browser, DataDomeClearanceOutcome};

#[tokio::main]
async fn main() -> zendriver::Result<()> {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .launch()
        .await?;
    let tab = browser.main_tab();
    tab.goto("https://protected.example.com").await?;
    tab.wait_for_load().await?;

    let outcome = tab
        .datadome()
        .timeout(Duration::from_secs(60))
        .wait_for_clearance()
        .await?;

    match outcome {
        DataDomeClearanceOutcome::Cleared { datadome } => {
            println!("cleared — datadome cookie: {datadome}")
        }
        DataDomeClearanceOutcome::AlreadyClear => println!("no DataDome surface"),
        DataDomeClearanceOutcome::Blocked => println!("IP banned — change your proxy"),
        DataDomeClearanceOutcome::TimedOut { .. } => println!("timed out"),
        DataDomeClearanceOutcome::ChallengeGone => println!("challenge cleared without cookie"),
    }

    browser.close().await?;
    Ok(())
}
```

## `tab.datadome()` builder

| Method | Default | Description |
|---|---|---|
| `.timeout(Duration)` | 30 s | Maximum total wait for a terminal outcome. |
| `.poll_interval(Duration)` | 250 ms | How often to re-probe the page during the poll loop. |
| `.with_interception()` | off | Enable Fetch-domain fast-path: signals on first 2xx response from `captcha-delivery.com` or any `datadome*` URL. |
| `.on_captcha(solver)` | none | Register an async CAPTCHA solver. Without it, a CAPTCHA surface returns `DataDomeError::CaptchaRequired`. |

Call `.wait_for_clearance().await` to start the drive.

## Surface variants

| Variant | What it is | How it's detected |
|---|---|---|
| `DeviceCheck` | Invisible JS interrogation (`window.dd.t == 'fe'`). Scores the browser fingerprint. | `window.dd` present + no captcha-delivery iframe. |
| `Captcha` | Slider / puzzle / press-hold via `captcha-delivery.com` iframe. | `captcha-delivery.com` iframe src present. |
| `Block` | IP banned (`window.dd.t == 'bv'`). Nothing in-browser clears this. | `window.dd.t == 'bv'`. |
| `None` | No DataDome surface. Fast `AlreadyClear` path. | Default — `window.dd` absent + no iframe. |

Detection precedence: **Block > Captcha > DeviceCheck > None**.

## Clearance signal

`Cleared` requires *both*:

1. The `datadome` cookie is present and non-empty.
2. `window.dd` is absent and no `captcha-delivery.com` iframe is present (`body_clean`).

`ChallengeGone` fires when body markers clear but no `datadome` cookie is
observed (rare / legacy path). Legacy flows that never set the cookie use this
path.

## CAPTCHA handling

Without an `on_captcha` callback, a CAPTCHA surface returns
`DataDomeError::CaptchaRequired` immediately (no waiting). Plug in your
solver:

```rust,no_run
# use zendriver_datadome::{DataDomeSolution, DataDomeBypass};
# async fn ex(tab: &zendriver_transport::SessionHandle) -> Result<(), zendriver_datadome::DataDomeError> {
let _ = DataDomeBypass::new(tab)
    .on_captcha(|challenge| async move {
        // challenge.captcha_url — the captcha-delivery.com iframe URL.
        // challenge.user_agent — must match the page UA (solver requirement).
        // Wire to 2captcha / capsolver / your own service:
        let cookie = call_my_service(&challenge.captcha_url, &challenge.user_agent).await?;
        Ok(DataDomeSolution { datadome_cookie: cookie })
    })
    .wait_for_clearance()
    .await?;
# Ok(()) }
# async fn call_my_service(_: &str, _: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> { Ok(String::new()) }
```

**The solver returns the `datadome` COOKIE value** — DataDome whitelists the
browser by setting this cookie (not a form-field token like hCaptcha/reCAPTCHA).
The driver applies it via `Network.setCookie` scoped to the registrable domain
and reloads the page.

`DataDomeChallenge` carries: `captcha_url`, `site_url`, `user_agent`, `cid`
(DataDome challenge ID), and `hash`. `DataDomeSolution` holds
`datadome_cookie`.

## `Blocked` / `TimedOut` outcomes

- **`Blocked`** — `window.dd.t == 'bv'` means DataDome has banned the IP at
  the edge. Nothing the browser does will clear this. Change your proxy to a
  residential IP, or wait out the ban.
- **`TimedOut { last_surface }`** — the deadline elapsed without reaching a
  terminal state. `last_surface` records the most recent surface the poll loop
  observed. Common causes: fingerprint scoring failure (see stealth note),
  IP reputation, or a CAPTCHA with no solver registered.

## Fetch-domain fast path

`.with_interception()` spawns a `Fetch` subscription that signals on first
2xx response to `captcha-delivery.com` or any `datadome*` URL. Polling
continues in parallel; the first signal wins. Useful on sites where the cookie
is set faster than the default 250 ms poll cadence.

```rust,no_run
# use zendriver_datadome::DataDomeBypass;
# async fn ex(tab: &zendriver_transport::SessionHandle) -> Result<(), zendriver_datadome::DataDomeError> {
let _ = DataDomeBypass::new(tab)
    .with_interception()
    .wait_for_clearance()
    .await?;
# Ok(()) }
```

## WebGPU / issue #20 stealth note

DataDome's device-check probes `navigator.gpu.requestAdapter()` and compares
the reported `GPUAdapterInfo.vendor` + `architecture` against its device
dataset. Before the `Surface::Webgpu` patch (issue #20), Chrome running under
`zendriver` leaked the platform's real GPU adapter info even when the WebGL
renderer was spoofed — the inconsistency read as a bot signal.

`Surface::Webgpu` (shipped with `zendriver-stealth`) derives a coherent
`GPUAdapterInfo` from the spoofed WebGL renderer string and decorates the real
adapter's reported `.info` so both surfaces report the same hardware. This
patch is included in `BrowserBuilder::stealth(StealthProfile::spoofed())`
with no extra configuration required.

**Containers / CI:** WebGPU requires a real GPU. In GPU-less containers,
`requestAdapter()` returns `null` both before and after the patch — which is
itself coherent (no GPU present) and passes DataDome's own consistency check
(both `null`). The patch does not fabricate adapters in GPU-less environments
**by default**. If a GPU-less environment specifically needs
`requestAdapter()` to resolve a non-null adapter (e.g. to match a device
profile DataDome expects), opt into `WebgpuSpec::fabricate_when_absent` with
explicit `vendor` + `limits` — see the
[WebGPU section](fingerprint.md#webgpu-opt-in-adapter-override--fabrication)
of the fingerprint chapter. It's an explicit, caller-supplied override, not
automatic: wrong values are more detectable than the honest `null` this
container caveat already describes.

## Active sensor reverse-engineering — out of scope

Computing DataDome's invisible device-check score in pure Rust (outside of a
real browser) is not in scope for this crate. DataDome updates its obfuscated
JS sensor frequently; maintaining a pure-HTTP solver alongside a
browser-automation library is a poor fit. If you need pure-HTTP DataDome bypass
for high-throughput scraping, build that as a separate crate.
