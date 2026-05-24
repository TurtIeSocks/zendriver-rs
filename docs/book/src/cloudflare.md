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

The driver returns a [`ClearanceOutcome`] on success:

- **`TokenAcquired(token)`** — the `cf-turnstile-response` input picked
  up a non-empty value. The page can now proceed; the token is also
  forwarded to Cloudflare server-side on the next request.
- **`ChallengeGone`** — the challenge container disappeared without
  yielding a token, typically because Cloudflare honored a clearance
  cookie and short-circuited the gate.

[`ClearanceOutcome`]: https://docs.rs/zendriver/latest/zendriver/enum.ClearanceOutcome.html

Errors are typed via [`CloudflareError`]:

- **`NoChallenge`** — no Turnstile iframe was detected at call time.
  Often means the page already cleared you (an existing cookie) or
  there's no CF gate present. Treat as a no-op, not a failure.
- **`ClearanceTimeout`** — the deadline elapsed without resolution.
  Usually means Cloudflare escalated to the deeper anti-bot path that
  this driver doesn't handle.

[`CloudflareError`]: https://docs.rs/zendriver/latest/zendriver/enum.CloudflareError.html

## How it works

The driver runs four stages internally:

1. **Detect.** A shadow-DOM-aware walk of the page's main world looks
   for the Turnstile iframe (`<iframe>` whose `src` matches Cloudflare's
   Turnstile widget origin). It surfaces the bounding box.
2. **Click.** A raw `mousedown` / `mouseup` is dispatched at offset
   `(bbox.x + bbox.width * 0.15, bbox.y + bbox.height * 0.5)` — the
   canonical 15%-from-left, 50%-from-top position of the Turnstile
   checkbox inside the iframe. No Bezier-path motion; Cloudflare wants a
   real click on a real checkbox.
3. **Poll.** Every 500 ms (override via
   [`poll_interval`](https://docs.rs/zendriver/latest/zendriver/struct.CloudflareBypass.html#method.poll_interval)),
   the driver checks both `cf-turnstile-response` (for a non-empty
   token) and the challenge container (for removal from the DOM).
4. **Return.** First condition to fire wins; deadline elapsed →
   `ClearanceTimeout`.

## Limitations

This driver **only handles the visible interactive Turnstile checkbox**.
It does not solve:

- **Silent / invisible Turnstile** (no UI element to click — relies on
  passive fingerprinting). For those, stealth alone is your only
  defense; pair `StealthProfile::spoofed()` with a clean residential IP.
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
    .await
{
    Ok(_) => { /* cleared */ }
    Err(CloudflareError::NoChallenge) => { /* already cleared, fine */ }
    Err(e) => return Err(e.into()),
}

// Now your normal scraping / interaction code.
let data = tab.find().css(".product-grid").one().await?;
```

`NoChallenge` is informational, not an error — code should treat it as
success. The other variants of [`CloudflareError`] should propagate.

## Tuning

- `.poll_interval(Duration::from_millis(200))` — tighter polling burns
  more CPU but reacts faster to clearance. Defaults to 500 ms which
  balances responsiveness against load.
- Pass a generous `wait_for_clearance` timeout (30-60 s) for the first
  challenge; subsequent navigations on the same `user_data_dir` are
  usually cookie-shortcut clears and resolve in &lt;1 s via
  `ChallengeGone`.
