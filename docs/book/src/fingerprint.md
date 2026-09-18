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

## The 8 surfaces

| Surface | Kind | Default strategy | What it affects |
|---------|------|-----------------|-----------------|
| `Canvas` | Noise | `Seeded` | Pixel readback: `getImageData`, `toDataURL`, `toBlob`, and probe-shaped `readPixels` |
| `Audio` | Noise | `Seeded` | `AnalyserNode` frequency / time-domain data |
| `ClientRects` | Noise | `Seeded` | `getBoundingClientRect` sub-pixel dimensions |
| `Webgl` | Value | `Value` | Every static device capability `getParameter` reports (WebGL1 + WebGL2), both extension lists, and `getShaderPrecisionFormat` — not just `UNMASKED_VENDOR_WEBGL`/`UNMASKED_RENDERER_WEBGL`. Per-context mutable state stays the backend's |
| `Webgpu` | Value | `Value` | `GPUAdapterInfo` (vendor/architecture/device/description) + optional `.limits`/`.features` |
| `Fonts` | Value | `Value` | `measureText` width noise + `FontFaceSet.check` allow-list |
| `Hardware` | Value | `Value` | Battery level, media-device count, speech voices |
| `Webrtc` | Policy | `Block` | ICE candidate leak suppression / fake IP |

## The 5 strategies

| Strategy | Effect |
|----------|--------|
| `Native` | No patch — raw browser output. |
| `Seeded` | Deterministic per-`(seed, content)` noise: the persona's fixed seed → reproducible across separate runs, and stable across repeat reads of identical content within a page. |
| `Random` | Same content-keyed noise as `Seeded`, but the seed itself is a fresh `Math.random()` draw made once per page load — so repeat reads within one page load are stable, while separate page loads (a new navigation or browser launch) get independent noise. |
| `Block` | Empty / zero output (appropriate for policy surfaces). |
| `Value` | Substitute a specific value from the `Persona` spec. |

Both noise strategies key their PRNG by the surface's own content (pixel
bytes, audio samples, rect geometry) on every read, not one stream that
advances across the whole page — so neither strategy "reseeds on every call"
in a way that makes repeat reads of the same content diverge.

Noise surfaces (Canvas, Audio, ClientRects) accept `Native`, `Seeded`,
`Random`, `Block`. Value surfaces (Webgl, Webgpu, Fonts, Hardware) accept
`Native`, `Value`, `Block`. The policy surface (Webrtc) accepts `Native`,
`Block`, `Value` (fake IP). Requesting a meaningless combination logs a
warning and falls back to the surface's kind default.

**`Native` on `Webgl` also silences the `Webgpu` value spoof.** A native
WebGL surface reports the host's real renderer, so a substituted
`GPUAdapterInfo` — derived from the renderer that was *not* applied — would
have `navigator.gpu` naming a GPU that `getParameter(UNMASKED_RENDERER_WEBGL)`
never claimed, which is exactly the cross-API mismatch a fingerprinter looks
for. The same coupling applies to
[`native_isolation`](stealth.md#opting-into-real-site-isolation--real-webgl-native_isolation),
which drops the WebGL patch profile-wide. An explicit `Webgpu` `Block` names
no GPU at all, so it stays honored either way.

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

// Build from a local JSON array (or load with load_or_download(url, policy)).
let pool = PoolSet::from_json(include_str!("pool.json"))?;
let persona = pool.sample(Seed::from_u64(42));

// Pass to Browser::builder() in the zendriver crate.
```

### Cache freshness (`CachePolicy`)

`pool::load_or_download` and `generative::Generator::load_or_download` both
download-on-first-use into a local cache file
(`dirs::cache_dir()/zendriver/fingerprints/...`). Freshness is controlled by a
`CachePolicy`, checked **on access** (at load time) — there is no background
scheduler:

```rust,no_run
use zendriver_fingerprints::CachePolicy;
use zendriver_fingerprints::pool::load_or_download;
use std::time::Duration;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
// Default: permanent cache — a cache hit is used forever (unchanged from
// before this knob existed).
let pool = load_or_download("https://example.com/pool.json", CachePolicy::default()).await?;

// Re-download once the cached file is older than a day.
let pool = load_or_download(
    "https://example.com/pool.json",
    CachePolicy::with_ttl(Duration::from_secs(86_400)),
)
.await?;

// Always re-download, ignoring any cache hit.
let pool = load_or_download("https://example.com/pool.json", CachePolicy::force_refresh()).await?;
# let _ = pool;
# Ok(())
# }
```

`CachePolicy::default()` is byte-for-byte identical to the pre-`CachePolicy`
behavior: permanent cache, and — since `ttl: None` short-circuits before any
mtime read — zero added filesystem calls. Clock skew (a cache file with a
future-dated mtime) fails **closed**: it's treated as stale and re-downloaded,
never as fresh and never a panic.

## Per-surface strategy overrides

Override any surface's render strategy on top of the persona:

```rust,no_run
use zendriver::{Browser, Persona, Seed, Strategy, Surface};

let browser = Browser::builder()
    .persona(Persona::builder().seed(Seed::from_u64(42)).build())
    .surface(Surface::Webrtc, Strategy::Native)  // allow real IP
    .surface(Surface::Canvas, Strategy::Random)  // fresh per-page-load seed
    .launch().await?;
```

## Canvas (pixel readback farbling)

The `Canvas` surface perturbs pixels on their way out, so the image a probe
hashes differs per persona while the visible canvas is untouched. The
perturbation is a **palette**: one table per colour channel, built from the
seed, mapping each 8-bit value to itself plus or minus one. Alpha is left
alone, since perturbing it changes compositing and it rarely carries
fingerprint weight.

**A palette rather than per-pixel noise, because per-pixel noise is
self-refuting.** Keying the noise on a pixel's position gives every pixel an
independent draw, and that fails two checks a page can run in a few lines:

- **A flat fill must come back flat.** Rendering is a function of its input, so
  identical pixels come back identical on every real GPU. Position-keyed noise
  broke that. Measured before the rewrite: a uniform WebGL clear read back with
  three different red values.
- **Every readback path must agree.** `readPixels` returns rows bottom-up and
  `getImageData` returns them top-down, so a position-keyed scheme cannot agree
  with itself across paths even in principle. A page could render one scene,
  read it both ways and compare. A palette has no position to disagree about.

Anti-linkage survives the change: a different seed permutes the palette
differently, so the hash still differs per persona, and it stays stable across
repeat reads because the mapping is a pure function.

**`readPixels` is farbled only when the read looks like a probe** — a whole
small drawing buffer, read as `RGBA`/`UNSIGNED_BYTE`. GPU picking reads a 1x1
rectangle off a large buffer and compares it against an exact id colour, so
perturbing that breaks real pages; float readbacks are compute output, where a
one-LSB change is meaningless at best. The residue, stated rather than hidden: a
page that reads 1x1 *and* reads the full buffer can see that only one of them
moved. That is a contrived probe, and the alternative is breaking picking on
sites that use it.

## WebGL (full-surface value spoof, resolved from measured tiers)

The `Webgl` surface's default `Value` strategy no longer substitutes a
handful of hand-picked numbers. It resolves and serves every static **device
capability** a page can read — 18 `getParameter` values on a WebGL1 context
and 47 on a WebGL2 one — plus both contexts' `getSupportedExtensions` lists
(in Chrome's own order, which is itself a fingerprint input) and every
`getShaderPrecisionFormat` result. That is the whole surface that identifies
the GPU, not just the vendor/renderer pair.

**The rest of `getParameter` is deliberately left to the real backend.** The
other ~85 values it answers are not device capabilities: they are per-context
*mutable state* (`VIEWPORT`, `BLEND`, `SCISSOR_BOX`, the `STENCIL_*`,
`PACK_*` and `UNPACK_*` families, `DRAW_BUFFERn`, …), values fixed by the
context attributes the page asked for (`RED_BITS`, `STENCIL_BITS`, `SAMPLES`
— `getContext('webgl', {stencil: true})` changes them), or the
extension-dependent `COMPRESSED_TEXTURE_FORMATS`. Every real Chrome reports
the same defaults for all of them, so they carry no entropy to spoof (with one
partial exception, `DRAW_BUFFERn`, covered below), and serving them from a
table would be a tell rather than a disguise:
`gl.enable(gl.BLEND); gl.getParameter(gl.BLEND)` would answer `false`
forever, and a resized canvas would report a stale viewport beside its real
`drawingBufferWidth`. Delegating also keeps real WebGL pages working —
state-caching renderers (deck.gl's `withParameters`, Babylon's state cache)
save and restore through `getParameter`.

**Values come from measured capability tiers, not per-parameter guesses.**
How far one capture reaches depends on the backend, because ANGLE (Chrome's GL
layer) decides these values differently per backend. Six tiers ship today, and
all but the last generalize:

- `SwiftShader` — Chrome's software rasterizer, the same numbers on every OS.
- Metal on macOS — ANGLE's Metal backend, covering every Mac. The values are
  compile-time constants in ANGLE's `DisplayMtl.mm` `TARGET_OS_OSX` branch, so
  an Intel Mac and an Apple silicon one report the same numbers.
- D3D11 (feature level 11_0+) — ANGLE's Direct3D 11 backend on Windows, in
  three tiers. ANGLE derives almost all of these values from `D3D11_REQ_*`
  constants rather than from the card, so a catch-all tier covers most parts at
  that feature level. Two refinements sit above it, and both were found by
  probing a second device rather than predicted:
  - NVIDIA. `renderer11_utils.cpp` enables `skipVSConstantRegisterZero` when
    and only when the vendor is NVIDIA, which docks
    `MAX_VERTEX_UNIFORM_VECTORS` from 4096 to 4095 and shifts the two values
    derived from it. Nothing else moves: probing an RTX 4090 and an AMD Radeon
    on one machine found no other difference in either WebGL parameter set, the
    extension lists, the shader precisions, or any WebGPU limit or feature.
  - Intel Gen9. Two values on this backend are read off the device rather than
    off the feature level, and an Intel HD Graphics 520 reports both
    differently from the Radeon: `MAX_SAMPLES` is 16 against 8 (ANGLE fills it
    by asking `CheckMultisampleQualityLevels` per renderable format), and the
    WebGPU adapter enumerates 16 features against 19, lacking `shader-f16`,
    `subgroups` and `bgra8unorm-storage`. Everything else matched, including
    all 82 WebGL1 parameters, the other 131 WebGL2 ones, both extension lists
    in content and order, and all 36 WebGPU limits.

  **Only Gen9 is routed to that third tier**, which is a deliberate limit
  rather than an oversight. Gen11 (Iris Plus G4/G7) and Gen12 (Iris Xe, Arc)
  stay on the catch-all one: nobody has probed them, both of the values that
  moved are device-derived, and Iris Xe is the heaviest single entry in the
  device catalogue — so a guess there would be wrong at the largest available
  scale. Closing that gap needs a Gen11 and a Gen12 capture.

  The limit applies backwards too, and it turns on a detail worth knowing
  before extending the routing. Intel spelled Broadwell (Gen8) with four
  digits, `HD Graphics 5500`, where Gen9 used three, `HD Graphics 520` — so
  matching on an `HD Graphics 5` prefix quietly takes a generation nobody
  probed. Routing counts the digits instead, and Broadwell stays on the
  catch-all tier.
- Intel Iris Pro Graphics 580 (Skylake GT4e) under Mesa 25.2.8 — ANGLE's
  **Vulkan** backend on Linux. This one covers **that GPU under that driver and
  nothing else**: `vk_caps_utils.cpp` fills its caps straight from
  `VkPhysicalDeviceLimits` (`max2DTextureSize` is
  `min(limitsVk.maxFramebufferWidth, limitsVk.maxImageDimension2D)`, the
  viewport bounds come from `limitsVk.maxViewportDimensions`), so a different
  Intel part — or the same part on a different Mesa release — is a different
  tier and needs its own capture. That is why it is named for the device and
  the driver rather than for the backend, and why no "Linux" or "Vulkan" tier
  exists to generalize it.

  That reasoning has since been measured rather than left as a reading of
  ANGLE's source. A second Mesa/Vulkan device — AMD RDNA2 under RADV, same
  Chrome build — differs from this tier in 12 WebGL2 parameters, including
  `MAX_3D_TEXTURE_SIZE` 8192 against 2048 and
  `UNIFORM_BUFFER_OFFSET_ALIGNMENT` 4 against 64, and the two disagree on
  extensions as well. Worth having, because `D3D11_REQ_*` was the other
  source-argument in this chapter and it turned out to have two escapes.

The measurement shows the split rather than just asserting it. The Vulkan
capture is closest to SwiftShader (7 of 82 WebGL1 parameters differ and 21 of
132 WebGL2, against 10/26 for D3D11 and 9/23 for Metal), because SwiftShader's
renderer string also says `Vulkan 1.3.0` and it runs through the same ANGLE
backend. The 21 that remain between two Vulkan-backed captures on one Chrome
build are exactly the device-derived limits.

A tier is shared *capability values*, never shared *identity*. Pin an Intel or
AMD D3D11 renderer and you get that tier's numbers above your own vendor and
renderer strings — `UNMASKED_VENDOR_WEBGL` is derived from the renderer you
pinned (`ANGLE (Intel, …)` → `Google Inc. (Intel)`), not from the NVIDIA card
the tier happened to be captured on.

**When a persona names no renderer, the default is chosen from its
platform.** A renderer string is read beside `navigator.platform`, so the two
have to be a pair Chrome can actually produce. A `MacIntel` persona gets the
Apple Metal row, a `Win32` persona the D3D11 row, and a `LinuxX86_64` persona
the Intel Iris Pro 580 Mesa/Vulkan row — all three ordinary hardware
identities whose name and numbers come from the same probe.

Linux used to get a SwiftShader row instead, which was real (Chrome reports it
on a GPU-blocklisted machine, a VM, or a headless container) but announced "no
usable GPU", something some fingerprinters weight on its own. Capturing the
Vulkan tier retired that last fallback: every platform default is now hardware.

**SwiftShader's numbers are platform-independent; its renderer string is
not.** Probing Ubuntu 24 (Chrome 150.0.7871.114, GPU-less VM) against the
flags used for the macOS capture reproduced it exactly — no WebGL1 or WebGL2
parameter differed, and the extension and precision lists matched — while the
renderer string differed in one token, because SwiftShader chooses its JIT
backend at build time and Chrome prints the choice:

| SwiftShader build | renderer string |
|---|---|
| Linux and Windows (both measured) | `ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)` |
| macOS | `ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (LLVM 10.0.0) (0x0000C0DE)), SwiftShader driver)` |

So one capability tier ships under two identity strings. Neither picks a
default any more — every platform resolves captured hardware — but the split
still decides which row a persona lands on when it pins a SwiftShader renderer
itself, and it must be the build that platform's Chrome really prints. Windows
Chrome's SwiftShader build prints Subzero too (measured on Windows 10.0.21996,
Chrome 150.0.7871.186).

**A renderer you pin yourself that matches no tier falls back to your
platform's default device and logs a warning.** Serving an unrecognized
renderer's *name* above a different backend's *numbers* is its own
incoherence. A desktop-GL string is expected to be unmatched, and so is any
Vulkan device other than the captured Iris Pro — widening that row to cover
Linux generally is exactly what its device-derived limits forbid. Adding a tier
requires probing real hardware with that backend; values are never invented.
See the
[`capture-gpu-tier` skill](https://github.com/TurtIeSocks/zendriver-rs/blob/main/.claude/skills/capture-gpu-tier/SKILL.md)
for the procedure if you hit this warning and have the hardware to fix it.

**The tables describe the claimed device, not the host that runs the page.**
A served capability can exceed what the backend underneath can actually do,
and nothing in the tables can change that — the numbers are read from a table,
the work is done by real hardware. `MAX_TEXTURE_SIZE` 16384 sits above a
backend that fails a 16384 `texImage2D`; `MAX_SAMPLES` 8 sits above one whose
`getInternalformatParameter` offers fewer; `MAX_DRAW_BUFFERS` 8 sits above one
with six real `DRAW_BUFFERn` enums. The last of those is cheap enough to close
outright, and the patch does close it (it answers the ES 3.0 default for any
index the served cap claims and the backend has no constant for). The rest are
not: a script that *exercises* a limit rather than reading it can tell the
claim from the capability, and no table can fabricate the capability itself.
Where that fidelity matters, pair the persona's tier with a
[`gpu_backend`](gpu-backend.md) that really has those limits — a `MacIntel`
persona (Apple Metal tier), a `Win32` one (D3D11 tier), or a `LinuxX86_64` one
(the Mesa/Vulkan tier) on `GpuBackend::Native` hardware, rather than over
SwiftShader, which is the default. No platform resolves the SwiftShader tier
by default any more, so no platform is trivially coherent with that backend;
pinning a SwiftShader renderer yourself is what makes it so.

**`Persona.gpu: Option<GpuProfile>` lets you pin a whole coherent device.**
Unset, it resolves from the persona's WebGL renderer string, matched against
the shipped devices above. Set, it overlays the resolved tier key-wise,
so a partial profile only overrides the keys it sets. It merges as one
atomic value across personas (like `screen`), never field-by-field —
composing two devices' values could describe hardware that exists nowhere.
The finer-grained `WebglSpec` (the `unmasked_vendor`/`unmasked_renderer`
strings) still overlays on top of whatever `Persona.gpu` produces, so you can
pin just the renderer string without restating an entire device's parameter
table.

**`Strategy::Native` on `Webgl` emits no WebGL patch at all** — `getParameter`
and friends return the host's real, unmodified values. As covered above,
this also suppresses the `Webgpu` value spoof, so neither surface serves a
value that contradicts the other.

**The tables are generated, never hand-edited.**
`crates/zendriver-stealth/src/gpu/tiers.rs` is produced by the `gpu-tier-gen`
crate from committed probe captures
(`crates/zendriver-stealth/data/gpu-tiers/*.json`); a CI step regenerates it
and fails the build on any diff, so the shipped file can never drift from
its source captures.

### What the tier tables buy, and what they do not

Worth stating plainly, because the mechanism above is easy to mistake for more
than it is.

**What they remove is a positive signal.** Before, a persona reported
`MAX_VIEWPORT_DIMS` from one backend beside `MAX_TEXTURE_SIZE` from another — a
pair no device produces, checkable in two lines. Same for NVIDIA's 4095 under an
AMD name, and for `DRAW_BUFFER6` answering past its own advertised cap. Those
were *detectably fake*, and they are gone. Going from wrong to not-wrong is
worth doing, and it is not the same as convincing.

**What they cannot do is survive a render.** A page that hashes canvas or WebGL
pixels reads what actually rendered, and on the default software rasterizer that
is not what the claimed device would produce. No amount of metadata fixes it,
and nothing here could: matching a specific GPU's pixels means reproducing a
proprietary shader compiler's optimisation choices bit-for-bit and tracking them
across driver releases. Fused multiply-add alone rounds differently from a
separate multiply and add, which is a different last bit and a different hash.

So treat this as a floor rather than a defence. It is sufficient against a
checker that reads values only — common, because rendering and reading back
costs real time — and insufficient against one that renders. For the latter the
honest answer is [`GpuBackend::Native`](gpu-backend.md) on hardware that matches
the persona, where pixels and values agree because both come from the same
machine.

**The catalogue's strongest justification is a different one.** It is not
fooling a hardware check. Before it existed, every zendriver user on Windows
reported the same RTX 4090 — a constant shared across the entire user base,
which is a *library* fingerprint rather than a GPU one. That signal is
independent of pixels, no render defeats it, and the catalogue kills it.

### Naming a GPU from the device catalogue

The tiers decide what a device *can do*. The catalogue decides *which device*
it is — 482 identities, against the one default each platform used to get.

```rust,no_run
use zendriver::stealth::{GpuDevice, Persona, Platform};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let persona = Persona {
    platform: Some(Platform::Win32),
    ..Persona::builder()
        .gpu_device(GpuDevice::by_name("NVIDIA GeForce RTX 4090")?)
        .build()
};
# Ok(())
# }
```

`gpu_device` sets nothing but the renderer string, because that string is a
device's whole contribution: it already selects the capability tier, the WebGPU
adapter, and the vendor through the machinery above. There is no separate GPU
field to keep in sync.

**Drawing one instead of naming it.** A fleet wants variety, and ten personas
that agree on every readable GPU value are ten personas that can be grouped:

```rust,no_run
use zendriver::stealth::{GpuDevice, Platform, Seed};

// Same seed, same device, every run.
let uniform = GpuDevice::from_seed(Seed(42), Platform::Win32);
// Weighted by how common the device actually is.
let realistic = GpuDevice::by_share(Seed(42), Platform::Win32);
```

`by_share` is usually the one you want. Over 5000 Win32 draws it yields 238
distinct devices led by Intel UHD Graphics at 15%, Iris Xe at 15% and AMD
integrated at 9% — a browser population, laptop iGPUs first. `from_seed` draws
uniformly, which makes a GeForce 210 as likely as the commonest laptop chip.

Those weights come from the same fingerprint corpus the device names do, as
marginal probabilities over its user-agent prior. They are deliberately *not*
Steam Hardware Survey numbers: Steam skews toward discrete gaming cards, and
this is a browser tool.

### Why the weighting matters more than it looks

The instinct is that a spoof succeeds by being *correct*. For fingerprinting it
succeeds by being *common*, and those are different targets.

A detection vendor sitting in the request path sees hundreds of millions of real
sessions, each handing over a renderer string, a canvas hash, a font list, an
audio hash, and timings, all correlated. That is a continuously-updating census
of what real browsers report, and it costs them nothing beyond being there — it
even self-updates through driver releases, because real users update drivers.
Nobody needs a reference lab of every GPU on every driver version when the
population reports itself.

So the practical check is not "is this what an RTX 4090 should render?" but
"does this combination show up across thousands of unrelated sessions?" Rarity
is the signal, not incorrectness. A device nobody else reports is conspicuous
even when every value in it is internally perfect.

That is the real argument for `by_share`: it lands in the dense part of the
distribution. A uniform draw is just as coherent and considerably rarer, and a
GeForce 210 in current traffic is a small population to hide in.

It also sharpens a tradeoff the surface farbling makes. Per-persona noise
defeats *linkage* — two sessions cannot be tied together by a shared canvas
hash — while making each session's hash unique, and uniqueness is the anomaly a
consensus check looks for. Those goals genuinely oppose each other. Defending
against being followed and defending against being spotted are not the same
problem, and no single setting wins both.

(Reasoning about incentives and cost, not inside knowledge of any vendor's
implementation. The conclusion is robust either way: a common device is never
the worse choice.)

**What the catalogue will not do.**

- **Invent a device id.** A D3D11 renderer string carries one by construction,
  so a model the sources never pair with an id is dropped rather than given a
  placeholder. The generated table names the ones it dropped.
- **Cross a platform.** Every draw is filtered through the same skew check the
  invariants use, so a Win32 persona cannot draw an Apple identity.
- **Cover Linux.** ANGLE's Vulkan backend reads its limits off the physical
  device, so there is no shared tier for a Linux identity to layer over and
  `from_seed` answers `None` there.
- **Claim a feature level it does not have.** ANGLE writes the feature level
  into the renderer string as its shader model, so pre-FL11 cards are excluded
  rather than filed under an FL11 tier.

**Matching the host's own GPU.** `zendriver::nearest_gpu_device()` launches a
short-lived browser on the native backend, reads what this machine reports, and
returns the closest catalogued device — exact identity first, then the same
model, then the same vendor on the same backend, then the same backend.

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
if let Some(device) = zendriver::nearest_gpu_device().await? {
    println!("closest catalogued GPU: {}", device.model());
}
# Ok(())
# }
```

It is a function you call, never a default. Probing the host to decide what a
persona claims is detect-and-adjust, which this project does not do implicitly;
the named-opt-in shape is the same one `geo_auto` uses. It answers `None` rather
than reaching for something plausible when the host's backend has no catalogue —
Linux and software rendering — because a Windows GPU is not a reasonable answer
for a Linux host merely because one was requested.

Pairing it with [`GpuBackend::Native`](gpu-backend.md) is the one configuration
where the values and the pixels agree by construction, because both come off the
same physical card. Everywhere else they are two separate claims that happen to
be consistent with each other, and only one of them survives being rendered. If
a target is known to run WebGL challenges, this pairing is the answer and no
amount of catalogue work substitutes for it.

`by_name` refuses ambiguity rather than guessing — `"rtx 40"` returns every
candidate — while an exact model name always wins, since several catalogued
names are prefixes of others.

## WebGPU (opt-in adapter override / fabrication)

By default (`Persona.webgpu = None`, or `Some(WebgpuSpec::default())`), the
`Webgpu` surface only DECORATES a real `navigator.gpu` adapter's `.info` with
a vendor/architecture DERIVED from the `Webgl` surface's renderer (never
fabricated) — the same behavior it always had. Only a renderer naming a GPU
family zendriver recognizes yields a vendor and architecture; anything else —
including any SwiftShader renderer — derives both as `""`, which is what Chrome
itself reports for an adapter it cannot classify. A software rasterizer has no
vendor, and naming one beside a SwiftShader WebGL renderer would be the
cross-API contradiction this derivation exists to prevent. The Mesa/Vulkan row
a `LinuxX86_64` persona defaults to derives `intel` with an empty
architecture — Intel is what its renderer names, and nothing measured that
part's architecture token, so it stays empty rather than guessed.

**`.limits` and `.features` come from the same measured tier the WebGL surface
serves.** The probe captures each tier's `navigator.gpu` adapter alongside its
WebGL blocks, in one run on one machine, so the two APIs answer for one device:
a `Win32` persona reports the D3D11 tiers' 2 GiB `maxBufferSize` and 19 features
(16 on Intel Gen9), a `MacIntel` persona the Metal tier's 4 GiB - 4 and its 22.
Before this they were left at the host's, so an adapter could name an NVIDIA
card above a Metal buffer limit — the same gap the tier tables closed for
WebGL.

Two tiers serve **neither**, and in both cases that is the measurement rather
than a hole. Chrome on SwiftShader resolves `requestAdapter()` to `null`, and
the Linux machine the Mesa/Vulkan tier was probed on has WebGPU off by default
(`navigator.gpu` exists, `requestAdapter()` resolves `null`) — so neither has
an adapter to describe, and the host's own values are left untouched.
Substituting a neighbouring tier's numbers would hand a persona that just told
WebGL it has a software rasterizer, or a machine with WebGPU disabled, a
hardware adapter's capabilities.

One honest caveat, the mirror of the D3D11 tier's WebGL story — and one that has
since been half-measured. Most WebGL values generalize across FL11+ cards
because ANGLE derives them from `D3D11_REQ_*` constants, and the WebGPU *limits*
generalize the same way (Dawn's D3D12 backend derives them from binding-tier
constants). A few *features* are genuinely hardware-gated, though —
`shader-f16`, `subgroups`, `bgra8unorm-storage` — and the catch-all tier's list
was measured on desktop hardware.

Taking the capture this predicted is what closed half of it: an Intel HD
Graphics 520 enumerates exactly those three fewer features, and Gen9 iGPUs now
resolve their own tier with its own 16-feature list rather than borrowing the
desktop one. What is still open is Gen11 and Gen12, which remain on the
catch-all tier because nobody has probed them — so pinning an Iris Xe renderer
still serves a feature list measured on another card. If that fidelity matters
before someone captures those, pin `features` yourself through `WebgpuSpec`.

`WebgpuSpec` (mirroring `WebglSpec`'s strategy+values shape) adds two OPT-IN
capabilities on top:

1. **Caller-supplied adapter identity.** Set `vendor` / `architecture` /
   `device` / `description` / `limits` / `features` explicitly instead of
   letting `vendor`/`architecture` derive from the WebGL renderer and
   `limits`/`features` come from its tier. `limits` overlays key-wise, so
   pinning one limit keeps the tier's value for every other; `features`
   replaces the tier's list wholesale.
2. **Synthetic adapter fabrication** (`fabricate_when_absent: true`) — when
   the host has no real WebGPU adapter, resolve a synthetic one built from
   your supplied values. This covers **both** GPU-less shapes:
   - `navigator.gpu` **entirely absent** (`'gpu' in navigator === false`).
     This is governed by the **page**, not by launch flags: `navigator.gpu` is
     `[SecureContext]`-gated, so it is absent on an opaque origin such as
     `about:blank` or a `data:` URL no matter what hardware or flags are in
     play. On a secure page under zendriver's default flags, `'gpu' in
     navigator` is `true` and `requestAdapter()` merely resolves `null`
     (measured on Chrome 150). Where it is absent, a synthetic `navigator.gpu`
     is *created* on `Navigator.prototype`, flipping `'gpu' in navigator` to
     **true** — coherent for a modern-Chrome persona, since real modern Chrome
     exposes `navigator.gpu` even with no usable GPU.
   - `navigator.gpu` present but `requestAdapter()` returns `null`: the real
     `requestAdapter` is wrapped so a `null` result falls back to the
     synthetic adapter (a real adapter passes through untouched).

   Requires BOTH `vendor` AND `limits` to be set explicitly — a bare
   `fabricate_when_absent: true` with nothing else is refused (no-op): this
   project never auto-invents fingerprint values.

```rust,no_run
use zendriver::{Browser, Persona, WebgpuSpec};

let persona = Persona {
    webgpu: Some(WebgpuSpec {
        vendor: Some("apple".into()),
        architecture: Some("metal-3".into()),
        ..Default::default()
    }),
    ..Persona::default()
};

let browser = Browser::builder().persona(persona).launch().await?;
```

Or via JSON (works with `fabricate_when_absent` + `limits`/`features` too):

```rust,no_run
use zendriver::Persona;

let persona: Persona = Persona::try_from_json(r#"{
  "webgpu": {
    "vendor": "apple",
    "architecture": "metal-3",
    "limits": { "maxTextureDimension2D": 16384 },
    "features": ["texture-compression-bc"],
    "fabricate_when_absent": true
  }
}"#).unwrap();
```

**You own value accuracy.** Every `WebgpuSpec` field is caller-supplied —
nothing is probed or invented from a real GPU. A `vendor`/`limits`/`features`
combination that doesn't correspond to any real device is **more detectable
than leaving the field `None`**: fingerprinting scripts cross-check
`GPUAdapterInfo` against `GPUSupportedLimits`/`GPUSupportedFeatures` and
against the WebGL renderer string, so an incoherent combination reads as a
bot faster than honest absence does. Only set these to values verified
against a real device.

**v1 limitations:** a fabricated synthetic adapter's `requestDevice()` always
REJECTS — there is no way to fabricate a working `GPUDevice` without a real
GPU behind it. Fabrication only makes `requestAdapter()` resolve a coherent
adapter for detection scripts that stop there; it does not unlock actual
WebGPU rendering on a GPU-less host.

`adapter.limits` and `adapter.features` are the **real** `GPUSupportedLimits`
and `GPUSupportedFeatures` objects wherever those classes exist: the patch
overrides their prototypes' accessors and setlike members rather than handing
back a plain object and a `Set`, so `constructor.name`, `instanceof`,
`Object.prototype.toString` and own-property count all read as a genuine
adapter's while the values are the claimed device's (verified against real
Chrome in `crates/zendriver/tests/gpu_profile.rs`). What still differs: the
iterators `features.keys()` / `values()` / `entries()` return are ordinary
`Array Iterator`s rather than `GPUSupportedFeatures Iterator`s — `has`, `size`,
spread and `for...of` all read correctly, only the iterator's own type tag
differs.

**`requestDevice()` is held to the same claim.** A served limit that only
survives being *read* is a fingerprint of its own: the adapter advertises the
tier's numbers, so a page can ask for exactly what it was just told, and the
request would go straight to hardware that never had it (a `Win32` persona
advertises 16 storage buffers per shader stage; a Metal host supports 10). On
the decorate path the patch wraps `requestDevice` so both directions agree with
the advertisement — a `requiredLimits` / `requiredFeatures` request beyond it
rejects with Chrome's own error and message, and one within it is translated
down to what the hardware can actually give so the call succeeds. The resulting
`GPUDevice` reports the **requested** values on its `.limits` (and the spec
defaults for everything it did not request, exactly as a real device does), on a
genuine `GPUSupportedLimits`. Its `adapterInfo` names the same adapter too —
before this it answered the host's, so `adapter.info.vendor` read `nvidia` and
`device.adapterInfo.vendor` read `apple` one line later.

That closes the *interrogation* divergence, not the capability gap. A page that
goes on to **allocate** at the claimed capability still fails, because no patch
can conjure hardware — the same honest limit as SwiftShader's pixels not being
an NVIDIA GPU's pixels.

**On a GPU-equipped host, [`GpuBackend::Native`](gpu-backend.md) sidesteps
`WebgpuSpec` fabrication entirely.** Instead of faking an adapter and
accepting the `requestDevice()`-rejects / no-real-rendering limitations
above, `Native` has Chrome render on the host's real GPU: a real adapter, a
real working `requestDevice()`, real limits and features — no patch
involved. The trade-off moves in the other direction: `Native` reports
whatever GPU the host actually has, with no caller-supplied identity, so it
doesn't help when you need a *specific* (rather than *coherent*) GPU
identity. See the [GPU backend](gpu-backend.md) chapter for the full
comparison.

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

**Order matters here.** `geo_auto()` reads the proxy configured at the moment
you call it, so it has to come *after* `.proxy(..)` — as above. Reversed,
there is nothing to mirror: the probe leaves over your own connection, which
hands `ip-api.com` your real IP and derives a locale for the wrong country.

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

**It fails closed.** The URL is parsed by `launch()` / `connect()` rather
than by the setter, and one that can't be parsed fails that call — before
Chrome is spawned or an endpoint is dialled. Unlike geo resolution, which
degrades to a less coherent persona, a dropped proxy means requests leave
from your own IP while you believe otherwise. Error messages redact any
userinfo the URL carried, so a typo in a proxy password does not end up in
a log. (`browser_context().proxy(..)` has always behaved this way — its
`build()` returns the same parse error.)

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
