# Cloudflare

The `cloudflare` Cargo feature ships a driver that bypasses Cloudflare's
**interactive Turnstile challenge** — the "verify you are human"
checkbox page that gates many sites behind a CDN. It is not a generic
anti-Cloudflare solution; it specifically automates clicking the visible
Turnstile checkbox iframe and waiting for the resulting clearance token.

Enable it in `Cargo.toml`:

```toml
[dependencies]
zendriver = { version = "0.1", features = ["cloudflare"] }
```

The entry point is [`Tab::cloudflare`], which constructs a
[`CloudflareBypass`] driver scoped to that tab's session. Call
[`wait_for_clearance`] with a timeout to run the full detect-click-poll
flow.

[`Tab::cloudflare`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.cloudflare
[`CloudflareBypass`]: https://docs.rs/zendriver/latest/zendriver/struct.CloudflareBypass.html
[`wait_for_clearance`]: https://docs.rs/zendriver/latest/zendriver/struct.CloudflareBypass.html#method.wait_for_clearance

## Usage

```rust,no_run
{{#include ../../../crates/zendriver/examples/cloudflare_bypass.rs}}
```

The driver returns a [`ClearanceOutcome`] on success. All three are
ordinary terminals, the last one included:

- **`TokenAcquired(token)`** — the `cf-turnstile-response` input picked
  up a non-empty value. The page can now proceed; the token is also
  forwarded to Cloudflare server-side on the next request.
- **`ChallengeGone`** — the challenge went away without yielding a
  token, typically because Cloudflare honored a clearance cookie and
  short-circuited the gate. Either an iframe the driver clicked was torn
  down, or every challenge marker seen on an earlier tick has vanished.
- **`TimedOut { saw_challenge }`** — the deadline elapsed.
  `saw_challenge` separates the two cases worth telling apart: `true`
  means a real challenge sat on the page and never resolved, `false`
  means no Cloudflare marker ever appeared and the bypass was probably
  called on a page with no gate on it.

[`ClearanceOutcome`]: https://docs.rs/zendriver/latest/zendriver/enum.ClearanceOutcome.html

A deadline is not an error here, so [`CloudflareError`] is left to carry
genuine faults, of which there are two:

- **`Call`** — the underlying CDP call failed, usually a dead tab or a
  closed connection.
- **`JsError`** — the in-page evaluator raised, or handed back a payload
  the driver could not decode.

The enum is `#[non_exhaustive]`, so a `match` on it needs a wildcard
arm. There is no `NoChallenge` or `ClearanceTimeout` variant — if you
are looking for those, the first is `TimedOut { saw_challenge: false }`
and the second is `TimedOut { saw_challenge: true }`.

[`CloudflareError`]: https://docs.rs/zendriver/latest/zendriver/enum.CloudflareError.html

## How it works

The driver runs a single poll loop, and each tick costs one CDP
round-trip. A shadow-DOM-aware walk of the page's main world reports
three things at once: the
`cf-turnstile-response` token if one is present, the challenge iframe's
box if that iframe is a valid click target, and whether any Cloudflare
marker at all (container, hidden input, or live iframe) is on the page.
The tick then resolves in this order:

1. **A token wins outright.** A non-empty token returns `TokenAcquired`
   immediately. This is also the invisible-Turnstile path, where no
   iframe mounts and Cloudflare's loader script fills the field in
   without anything to click.
2. **Scroll, re-measure, click.** Raw mouse events carry viewport
   coordinates, so a widget below the fold has to be scrolled in and
   re-measured before a click can land on it. The click goes to
   `(bbox.x + bbox.width * 0.15, bbox.y + bbox.height * 0.5)` — the
   canonical 15%-from-left, 50%-from-top position of the checkbox inside
   the iframe. No Bezier-path motion; Cloudflare wants a real click on a
   real checkbox. A widget that is mounted but hidden or zero-sized is
   never clicked, since there is no meaningful point to click.
3. **Retry, up to a cap.** Cloudflare drops clicks that land while the
   widget is still booting, so one run spends up to three of them, each
   at least four poll ticks after the last (about two seconds at the
   default interval). A swallowed first click no longer strands the run
   until the deadline.
4. **Otherwise keep polling** — every 500 ms by default, override via
   [`poll_interval`](https://docs.rs/zendriver/latest/zendriver/struct.CloudflareBypass.html#method.poll_interval)
   — until a terminal fires or the deadline passes.

After ten consecutive ticks with no progress the driver logs a warning
asking whether `BrowserBuilder::stealth` is on, which is the answer most
of the time. See [Pairing with stealth](#pairing-with-stealth) below.

## Limitations

The driver **clicks the visible interactive Turnstile checkbox**, and
picks up the token on the invisible path when Cloudflare's own script
produces one. It does not solve:

- **Silent / invisible Turnstile.** There is no UI element to click and
  the verdict comes from passive fingerprinting. The loop returns
  `TokenAcquired` the moment the field is populated, but nothing here
  makes Cloudflare populate it — that is stealth's job. Pair
  `StealthProfile::spoofed()` with a clean residential IP.
- **Cloudflare's full Pro / Enterprise managed challenge** (which can
  escalate to image puzzles or even hCaptcha).
- **Bot Fight Mode soft blocks** that issue 403s without a UI.
- **Rate-limit blocks** (1015 errors) that don't expose a challenge UI
  at all.

If the bypass times out, switch to a real browser session, manually
inspect the page, and confirm whether the gate is the interactive
checkbox flow. If it's not, this driver can't help and you'll need a
different strategy (better stealth, rotating residential proxies, or
giving up on that target).

## Pairing with stealth

Cloudflare's challenge logic checks several signals before deciding
whether to show the visible checkbox or escalate to the silent flow:
TLS JA3 fingerprint, User-Agent, header order, `navigator.webdriver`,
etc. Out-of-the-box headless Chrome trips most of those, so it tends to
get the harder challenge path — sometimes one this driver can't pass.

Pair the bypass with `StealthProfile::spoofed()` for the best results:

```rust,ignore
use zendriver::{Browser, StealthProfile};

let browser = Browser::builder()
    .stealth(StealthProfile::spoofed())  // patches navigator.webdriver etc.
    .launch()
    .await?;
let tab = browser.main_tab();
tab.goto("https://target.example.com").await?;
tab.wait_for_load().await?;

tab.cloudflare()
    .wait_for_clearance(std::time::Duration::from_secs(30))
    .await?;
```

`spoofed` patches the Navigator-prototype tells that Cloudflare also
checks during the protocol-level challenge — together they pass most
consumer-site Cloudflare gates. See [Stealth](./stealth.md) for the
profile tradeoffs.

## When to call it

Call `wait_for_clearance` **after** the navigation completes but
**before** any post-challenge code that depends on being past the gate.
The typical sequence:

```rust,ignore
tab.goto(url).await?;
tab.wait_for_load().await?;

match tab.cloudflare()
    .wait_for_clearance(Duration::from_secs(30))
    .await?
{
    ClearanceOutcome::TokenAcquired(_) | ClearanceOutcome::ChallengeGone => {
        // Past the gate.
    }
    ClearanceOutcome::TimedOut { saw_challenge: false } => {
        // No Cloudflare marker ever appeared — this page has no gate.
        // Usually fine to continue.
    }
    ClearanceOutcome::TimedOut { saw_challenge: true } => {
        // A real challenge that never resolved. Worth failing the job.
        return Err("cloudflare challenge did not clear".into());
    }
}

// Now your normal scraping / interaction code.
let data = tab.find().css(".product-grid").one().await?;
```

The `?` propagates the two real faults, `Call` and `JsError`. Everything
else is a terminal you decide about, and the `saw_challenge` split is
the one that changes what you do: "no gate on this page" and "a gate we
could not pass" want opposite handling.

## Tuning

- `.poll_interval(Duration::from_millis(200))` — tighter polling burns
  more CPU but reacts faster to clearance. Defaults to 500 ms which
  balances responsiveness against load.
- Pass a generous `wait_for_clearance` timeout (30-60 s) for the first
  challenge; subsequent navigations on the same `user_data_dir` are
  usually cookie-shortcut clears and resolve in &lt;1 s via
  `ChallengeGone`.
