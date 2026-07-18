# Stealth

zendriver-rs ships with three stealth profiles selecting different
tradeoffs between launch overhead, detectability, and CSP compatibility.
Pick the profile that matches your target site's detection layer; tweak
the fingerprint with builder methods when you need to pin a specific
identity.

## The three profiles

| Profile     | Launch flags | UA scrub | Emulation overrides | JS bootstrap   | Bypass CSP | Use case                                     |
|-------------|--------------|----------|---------------------|----------------|------------|----------------------------------------------|
| `off()`     | none         | no       | no                  | none           | no         | Reproducing issues in vanilla Chrome.       |
| `native()`  | yes          | yes      | yes                 | none           | no         | Most sites. Default recommendation.         |
| `spoofed()` | yes          | yes      | yes                 | Navigator JS   | yes (on)   | Sites with active fingerprint detection.    |

### `StealthProfile::off()`

```rust,no_run
use zendriver::{Browser, StealthProfile};

# async fn ex() -> zendriver::Result<()> {
let browser = Browser::builder()
    .stealth(StealthProfile::off())
    .launch()
    .await?;
# Ok(()) }
```

No launch flags. No UA scrub. No CDP overrides. `Page.setBypassCSP` is
not called. This is what you get from a stock `chromiumoxide` launch.
Use this when you're debugging whether a bug reproduces in a vanilla
Chrome — if it does, the cause is unrelated to zendriver's stealth
machinery.

### `StealthProfile::native()`

```rust,no_run
use zendriver::{Browser, StealthProfile};

# async fn ex() -> zendriver::Result<()> {
let browser = Browser::builder()
    .stealth(StealthProfile::native())
    .launch()
    .await?;
# Ok(()) }
```

The default recommendation. Patches the layer that protocol-level
fingerprinters see, without touching JS object prototypes:

- **Launch flags** — `--disable-blink-features=AutomationControlled`,
  `--disable-features=IsolateOrigins,site-per-process` (toggleable),
  and a curated list of flags that turn off the "this browser is
  controlled by automation" infobar plus various leaks.
- **UA scrub** — strips the `HeadlessChrome` segment from the User-Agent
  string and the Sec-CH-UA brand list.
- **Emulation overrides** —
  `Emulation.setUserAgentOverride` /
  `Emulation.setHardwareConcurrencyOverride` /
  `Emulation.setDeviceMetricsOverride` set a coherent identity.

Safe against `Function.prototype.toString` detection because it patches
nothing at the JS level — there's no `[native code]` mismatch to detect.
Passes most consumer site detectors. Doesn't pass [sannysoft]'s deeper
Navigator-prototype checks.

[sannysoft]: https://bot.sannysoft.com/

### `StealthProfile::spoofed()`

```rust,no_run
use zendriver::{Browser, StealthProfile};

# async fn ex() -> zendriver::Result<()> {
let browser = Browser::builder()
    .stealth(StealthProfile::spoofed())
    .launch()
    .await?;
# Ok(()) }
```

`native()` plus Navigator-prototype JS patches injected via
`Page.addScriptToEvaluateOnNewDocument`. Restores or overrides:

- `navigator.webdriver` (deletes it so `'webdriver' in navigator` is
  `false`).
- `navigator.permissions.query({ name: "notifications" })` (returns
  `"prompt"` instead of the headless-Chrome `"denied"`).
- `navigator.plugins` + `navigator.mimeTypes` (returns plausible-length
  arrays).
- `navigator.chrome` (installs the runtime object headless Chrome
  doesn't ship).
- WebGL vendor / renderer (returns `"Google Inc. (Intel)"` /
  `"ANGLE (Intel, Mesa Intel(R) UHD Graphics, OpenGL 4.6)"` by
  default).
- ChunkSplit + iframe-contentWindow guards so the patches survive
  cross-realm escape attempts.

Toggles `Page.setBypassCSP` **on by default** so the bootstrap script
can install on pages with strict CSP headers. Pass
`.bypass_csp(false)` to opt out when you want to test against a real
CSP-restricted page.

Passes [sannysoft], [areyouheadless], and most active detectors. Pays
a small per-navigation cost (the JS bootstrap runs on every new
document).

[areyouheadless]: https://arh.antoinevastel.com/bots/areyouheadless

## Customizing the fingerprint

All three profiles return a builder that lets you override individual
fingerprint fields. The values are validated and clamped at resolve
time (e.g. `memory_gb` is clamped to a plausible W3C-rounded value;
`cpu_count` is clamped to `2..=32`).

```rust,no_run
use zendriver::{Browser, StealthProfile};
use zendriver::stealth::Platform;

# async fn ex() -> zendriver::Result<()> {
let profile = StealthProfile::spoofed()
    .memory_gb(8)               // navigator.deviceMemory
    .cpu_count(8)               // navigator.hardwareConcurrency
    .chrome_version(126)        // Chrome major in UA + Sec-CH-UA
    .platform(Platform::Win32)  // navigator.platform + OS in UA
    .locale("en-US")            // navigator.language + --lang flag
    .timezone("America/New_York");

let browser = Browser::builder()
    .stealth(profile)
    .launch()
    .await?;
# Ok(()) }
```

You can also override the User-Agent string verbatim — useful when
you need an exact UA that doesn't match the auto-composed one:

```rust,no_run
use zendriver::{Browser, StealthProfile};

# async fn ex() -> zendriver::Result<()> {
let profile = StealthProfile::native()
    .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ...");

let browser = Browser::builder()
    .stealth(profile)
    .launch()
    .await?;
# Ok(()) }
```

`user_agent()` skips the auto-composition step entirely — prefer
`platform()` + `chrome_version()` unless you need a bit-for-bit
specific UA.

## Opting into real site isolation + real WebGL (`native_isolation`)

`native()` and `spoofed()` both disable Chrome's render-process site
isolation (`--disable-features=IsolateOrigins,...,site-per-process`),
and `spoofed()` additionally patches
`WebGLRenderingContext.getParameter()` /`getSupportedExtensions()` to
report a coherent ANGLE/Direct3D11 Intel identity regardless of the
host's actual GPU. `.native_isolation(true)` opts a profile **out** of
both:

```rust,no_run
use zendriver::{Browser, StealthProfile};

# async fn ex() -> zendriver::Result<()> {
let profile = StealthProfile::spoofed().native_isolation(true);

let browser = Browser::builder()
    .stealth(profile)
    .launch()
    .await?;
# Ok(()) }
```

With this set:

- The launch flags omit `IsolateOrigins`/`site-per-process` from
  `--disable-features=...` — Chrome runs with its normal render-process
  isolation boundary. (The unrelated
  `DisableLoadExtensionCommandLineSwitch` feature name stays disabled
  either way — it controls `--load-extension`, not isolation.)
- For `spoofed()`, the bootstrap script omits the WebGL
  vendor/renderer patch entirely — `getParameter(UNMASKED_VENDOR_WEBGL)`
  etc. return the host's real values instead of the spoofed default.

**This is a trade-off, not a strict stealth improvement.** The defaults
it opts out of exist as anti-detection measures: the WebGL patch in
particular is an anti-WAF *coherence* defense — some WAFs
(Imperva/Incapsula) cross-check the WebGL identity against the rest of
the fingerprint and flag a real, host-specific renderer string as a bot
tell when it doesn't match. Reach for `native_isolation(true)` when you
need the host's actual GPU behavior (WebGL-heavy rendering, screenshot
fidelity, visual regression testing) or want Chrome's stock
process-isolation security boundary, and evasion isn't the priority —
not because it's "more stealthy." It isn't; it removes a defense.

It's off by default on every profile, so existing `native()` /
`spoofed()` callers see no behavior change unless they opt in
explicitly.

**Known caveat — WebGPU is untouched.** The WebGPU coherence patch
(`navigator.gpu`) is driven independently by the
[`Persona`](https://docs.rs/zendriver-stealth/latest/zendriver_stealth/struct.Persona.html)
`webgpu` surface, which defaults to still deriving a spoofed adapter
from the hardcoded Intel/ANGLE renderer. Enabling `native_isolation`
does **not** disable that — pair it with a `Persona` that sets the
`Webgpu` surface strategy to `Native` (via `apply_surface_override`) if
you need full WebGL/WebGPU coherence with the real host GPU.

## End-to-end example

This example launches with a custom UA, locale, and platform, then
reads them back via `navigator.*` to prove the overrides took:

```rust,no_run
{{#include ../../../crates/zendriver/examples/set_user_agent.rs}}
```

Expected output:

```text
My user agent
de
Win32
```

The override surface is intentionally narrow — anything that would let
you set incoherent values (e.g. Linux UA + `navigator.platform = Win32`)
goes through the Fingerprint resolver, which composes a coherent
identity.

## When to use which

- **Headless scraping of public sites** — start with `native()`. Most
  sites don't actively probe Navigator prototypes; the cheaper profile
  is plenty.
- **Sites with active bot detection** (Cloudflare, PerimeterX,
  DataDome, Akamai) — use `spoofed()`. Pair with the `cloudflare`
  Cargo feature when you specifically need Turnstile bypass — see
  [Cloudflare](./cloudflare.md).
- **Sites with strict CSP** — `spoofed()` defaults to
  `bypass_csp = true`, which is normally what you want. If you're
  *testing* a real CSP, override it with `.bypass_csp(false)` (and
  expect the JS bootstrap to fail to install).
- **Sites that read `Function.prototype.toString` looking for
  `[native code]` mismatches** — `native()` rather than `spoofed()`,
  because `spoofed()`'s prototype patches leave detectable
  fingerprints in the function-source readouts. zendriver's bootstrap
  papers over the obvious patches, but a determined adversary will
  still find drift.
