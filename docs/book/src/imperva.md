# Imperva WAF / Incapsula

The `imperva` cargo feature (sub-crate: `zendriver-imperva`) provides a
passive bypass driver for sites protected by Imperva WAF / Incapsula. It
detects which Imperva surface is active (modern reese84 bot management,
legacy Incapsula `___utmvc` flow, or a CAPTCHA escalation), then polls
the page until both clearance signals — `reese84` cookie set AND body
markers cleared — are observed.

> **Stealth required.** Imperva's reese84 sensor is itself a browser
> fingerprint check. Run with `BrowserBuilder::stealth` enabled or the
> bypass will fail on nearly all real Imperva-protected sites.

## Limitations

This crate observes the page; it does not modify Imperva's response. A
`TokenAcquired` result means the JS challenge completed and the
`reese84` cookie landed — it does *not* guarantee the next request will
be accepted. Imperva runs a second validation pass on every request
that ships the token; if its scoring of the fingerprint collected
during the challenge falls below threshold, the next request returns a
403 challenge page with `edet=15` / `B15(x,y,z)` (general bot
protection). Root causes for that are upstream of this crate:

- **Browser stealth gaps.** Missing fingerprint shim — canvas, audio,
  WebGL, client hints — leaks "automation" even when the challenge JS
  itself runs to completion.
- **IP reputation.** Residential / datacenter pools flagged at the
  edge return `B15` before the JS challenge runs at all.
- **UA-vs-binary drift.** Claiming `Chrome/146` from a Chromium 148
  binary leaks JS-API behavior inconsistent with the claimed version.

If `wait_for_clearance` returns `TokenAcquired` but subsequent requests
still hit `edet=15`, look upstream — the fingerprint the browser
emitted is the problem, not the clearance detection.

## Quick start

```rust,no_run
use std::time::Duration;
use zendriver::stealth::StealthProfile;
use zendriver::{Browser, ImpervaClearanceOutcome};

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
        .imperva()
        .timeout(Duration::from_secs(60))
        .wait_for_clearance()
        .await?;

    match outcome {
        ImpervaClearanceOutcome::TokenAcquired { reese84, .. } => {
            println!("got token: {reese84}")
        }
        ImpervaClearanceOutcome::ChallengeGone => println!("legacy cleared"),
        ImpervaClearanceOutcome::AlreadyClear => println!("no challenge"),
    }

    browser.close().await?;
    Ok(())
}
```

## Surface variants

| Variant | What it is | How it's detected |
|---|---|---|
| `Reese84` | Modern Imperva ABP bot management. Invisible JS challenge → reese84 sensor token. | `reese84` cookie name OR `Reese.js` body marker. |
| `Legacy` | Older Incapsula `___utmvc` / `incap_ses_*` flow. | `___utmvc` / `incap_ses_*` / `visid_incap_*` cookies, or `/_Incapsula_Resource` body marker. |
| `Captcha(kind)` | Escalation to hCaptcha, reCAPTCHA, or Imperva's native CAPTCHA. | iframe src patterns + `g-recaptcha` / `h-captcha` DOM markers. |
| `None` | No Imperva surface present. | Default — fast `AlreadyClear` path. |

Detection precedence: **Captcha > Reese84 > Legacy > None**.

## Clearance signal (S3 hybrid AND)

`TokenAcquired` requires *both*:

1. A non-empty `reese84` cookie scoped to the current site.
2. The page body no longer contains Imperva challenge markers.

This avoids false positives (cookie set during pre-clearance redirect)
and false negatives (cookie evicted by site CSP). Legacy flows that
never set `reese84` resolve to `ChallengeGone` once body markers clear.

## CAPTCHA handling

Without an `on_captcha` callback, a CAPTCHA surface returns
`ImpervaError::CaptchaRequired { kind }` immediately (no waiting). Plug
in your own solver:

```rust,no_run
# use zendriver_imperva::{CaptchaSolution, ImpervaBypass};
# async fn ex(tab: &zendriver_transport::SessionHandle) -> Result<(), zendriver_imperva::ImpervaError> {
let _ = ImpervaBypass::new(tab)
    .on_captcha(|challenge| async move {
        // Call 2captcha / anticaptcha / your own service here.
        Ok(CaptchaSolution {
            token: "...".into(),
            form_field: "h-captcha-response".into(),
        })
    })
    .wait_for_clearance()
    .await?;
# Ok(()) }
```

`CaptchaChallenge` carries `kind`, `site_key` (when extractable), and
`url`. `CaptchaSolution` is the token + form field name your solver
returns.

## Fetch-domain fast path

`with_interception()` spawns a `Fetch` subscription that signals on
first 2xx response to `Reese.js` or `_Incapsula_Resource*`. Polling
continues in parallel; first signal wins. Useful on sites where the
token cookie is set faster than the default 250ms poll cadence.

```rust,no_run
# use zendriver_imperva::ImpervaBypass;
# async fn ex(tab: &zendriver_transport::SessionHandle) -> Result<(), zendriver_imperva::ImpervaError> {
let _ = ImpervaBypass::new(tab)
    .with_interception()
    .wait_for_clearance()
    .await?;
# Ok(()) }
```

## Active sensor synthesis — out of scope

Reverse-engineering Imperva's obfuscated reese84 sensor JS and computing
tokens in pure Rust is *not* in scope for this crate. The maintenance
burden (Imperva ships new obfuscated builds frequently) and the lack of
CAPTCHA fallback in a pure-HTTP design make it a poor fit alongside a
browser-automation library. If you need pure-HTTP Imperva bypass for
high-throughput scraping, build that as a separate crate.
