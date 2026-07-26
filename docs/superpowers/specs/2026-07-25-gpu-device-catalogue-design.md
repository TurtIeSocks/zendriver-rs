# GPU device catalogue — design

> **Status:** design, 2026-07-25. Follow-up to the capability tier tables
> (PR #126). Depends on that work being merged.

## Problem

The tier tables make one GPU identity coherent. They do not make a *fleet*
diverse. Every persona on a given platform reports the same device — the M4
Pro on macOS, the RTX 4090 on Windows, the Iris Pro 580 on Linux — because a
tier ships exactly one device row per platform.

A shared renderer string is a cluster identifier. Ten personas that agree on
every readable GPU value are ten personas that can be grouped, however
coherent each one is on its own.

This design adds a catalogue: a caller names a device, or draws one, and gets
a coherent identity composed over an existing measured tier.

## What is actually variable

This is the load-bearing section. The catalogue is only sound because most of
the GPU surface does **not** vary per device, and the parts that do are small
and enumerable.

Measured across the four committed captures:

| surface | varies by | evidence |
|---|---|---|
| WebGL parameters (D3D11) | feature level, **plus one vendor bit** | `renderer11_utils.cpp` switches on `D3D_FEATURE_LEVEL` for the values themselves, but `skipVSConstantRegisterZero` is keyed on `isNvidia` and docks `maxVertexUniformVectors` by 1 — measured: an RTX 4090 and an AMD Radeon on one machine differ in that cap and the two derived from it, and in nothing else |
| WebGL parameters (Metal) | nothing on macOS | `DisplayMtl.mm:727-731`, `TARGET_OS_OSX` branch is compile-time constants |
| WebGL parameters (Vulkan) | **the device** | `vk_caps_utils.cpp:652-684` reads `VkPhysicalDeviceLimits`; two Vulkan captures on one Chrome differ in 21 WebGL2 params |
| WebGPU limits | backend | 5 of 36 differ between Metal and D3D12, all API-level (`maxBufferSize`, storage-buffer counts) |
| WebGPU features | backend, **not vendor** | D3D11's 19 are a strict subset of Metal's 22; the 3 extra are ASTC/ETC2 formats Apple silicon has and desktop D3D11 lacks. AMD RDNA2 and NVIDIA Lovelace report identical 19-feature sets |
| WebGPU limits | **not vendor** | 0 of 36 differ between AMD RDNA2 and NVIDIA Lovelace |
| renderer string | **the device** | model name and PCI device ID |
| WebGPU `architecture` | generation on D3D11 (`ampere`, `lovelace`); **constant** on Apple silicon (`metal-3`) | `PhysicalDeviceMTL.mm` picks it by capability family, not generation — see Scope |

So a catalogue entry is not a capability set. It is an **identity** —
a renderer string, a device ID, an architecture token — layered over a
capability tier that already exists.

That is why this is tractable. Cataloguing five hundred cards does not mean
five hundred captures; it means five hundred names over three or four
measured tiers.

## Scope

**In scope for v1: D3D11 and Metal.** Both backends derive capabilities from
constants, so one tier genuinely covers every device on it, and an entry adds
only identity.

The two are not symmetric, and Metal is by far the smaller job. Dawn's
`PhysicalDeviceMTL.mm` picks the architecture token like this:

```cpp
if (mDeviceId == 0) {
    if (@available(macOS 13.0, iOS 16.0, *)) {
        if ([*mDevice supportsFamily:static_cast<::MTLGPUFamily>(5001)]) {
            mArchitectureName = "metal-3";   // MTLGPUFamilyMetal3
        }
    } else if ([*mDevice supportsFamily:MTLGPUFamilyCommon3]) { ... }
}
```

Two things follow. First, this branch is reached only when `mDeviceId == 0` —
which is exactly the Apple silicon case, independently confirming that these
parts carry no PCI ID. Second, `metal-3` is a **capability-family test, not a
generation test**: every Apple silicon Mac that supports the Metal 3 family
reports the same token, M1 through M4 alike. It is nothing like NVIDIA's
`ampere` / `lovelace`, which do track generation.

So a Metal entry varies in exactly one field, `MTLDevice.name`, over a
constant architecture and a constant capability tier. There is no generation
axis to key, and the whole M-series name set ships without needing a capture
per chip.

**Intel Macs are out of scope.** They have a real device ID, so `mDeviceId`
is non-zero and Dawn takes the other path entirely, reading the name from a
`gpu_info` lookup table. Different mechanism, different design, no evidence
here.

**Out of scope for v1: Linux and Vulkan.** ANGLE's Vulkan backend reads caps
off the physical device, so a Linux entry cannot be identity-over-a-shared-tier
— each device would need its own full capture. That is a different model, not
an extension of this one, and it needs more than one hardware Vulkan capture
to design against. The Vulkan renderer string also embeds a driver version
(`Vulkan 1.4.318 (…), Intel open-source Mesa driver`), so a Linux entry carries
a Mesa-version axis that D3D11 entries do not.

Linux personas keep the single measured Iris Pro 580 identity until that
design exists.

## Sourcing

Nothing in the catalogue is invented. Every field traces to a citable source,
and the string is **reconstructed from the ANGLE code that composes it**
rather than pattern-matched from samples. The two in-scope backends compose
their strings differently, so they source differently too.

> **Revised during implementation: device IDs come from the corpus, not
> `pci.ids`.** The section below was written expecting `pci.ids` to supply
> every device ID by name lookup. Measuring showed the corpus already carries
> 467 exact `(name, id)` pairs covering 308 of the 310 D3D11 names.
>
> Preferring those is a fidelity gain, not a shortcut. A name lookup can only
> reconstruct a *plausible* ID, and the names are wildly ambiguous —
> `Intel(R) Graphics` matches 30 candidates, so choosing among them is a rule
> rather than a fact. The corpus instead reports the pair a real machine
> emitted, and reports **every** SKU a marketing name spans: 77 names appear
> with several IDs, so the catalogue carries 476 D3D11 rows over 310 names.
>
> `pci.ids` remains, demoted to a fallback for names the corpus never pairs
> with an ID, and still earns its place by resolving 9 such rows. Two names
> resist both sources and are dropped and reported.

### The string format comes from ANGLE's source

D3D11, `Renderer11.cpp:2308-2319`:

```
mDescription + " (" + FmtHex(DeviceId) + ")" + " Direct3D11" + " vs_5_0 ps_5_0"
```

Metal, `DisplayMtl.mm:188-201`:

```
"ANGLE Metal Renderer" + ": " + MTLDevice.name
```

with the trailing version field the literal string `"Unspecified Version"` —
`getVersionString` returns it verbatim for WebGL contexts (`:216`), because
Chrome requires *something* there. It is not a version and must not be
synthesized as one.

So D3D11 has two variables (`Description`, `DeviceId`) and **Metal has one**
(`MTLDevice.name`). Apple Silicon exposes no PCI device ID, and ANGLE would
have nowhere to put one if it did.

Reading the format from source is what makes a Chrome format change a small,
detectable fix instead of silent rot. It is also not hypothetical: the
fixture corpus in this repo carries

```
ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)
```

with **no device ID**, while the measured RTX 4090 capture on current Chrome
has `(0x00002684)`. The format changed under the corpus. That is the whole
argument for composing rather than copying: a copied string is stamped with
whatever Chrome collected it, and nothing detects the drift.

### Model names come from a corpus of real strings

`zendriver-fingerprints`' generative network carries a `videoCard` node whose
values are driver-reported renderer strings. Those give the exact
`Description` / `MTLDevice.name` text a driver reports, which `pci.ids` does
not: `pci.ids` says `GA104 [GeForce RTX 3070]`, the driver says
`NVIDIA GeForce RTX 3070`.

The corpus is **not vendored** — `DEFAULT_NETWORK_URL`
(`generative/mod.rs:29`) downloads it on first use, and it points at
`fingerprint-suite`'s `master`, not a tag. An unpinned upstream is fine for a
runtime cache and unacceptable for a generator: the catalogue would change
without the input changing. The generator therefore **pins a commit** and
records it in the generated header, the same discipline as `locale-gen`'s
pinned CLDR tag. Extraction happens at generation time; the committed table is
what ships, and nothing downloads at runtime.

### Device IDs come from `pci.ids`

Dual BSD-3/GPLv2, vendored with a NOTICE. PCI device IDs are published
identifiers, not measured capabilities — using them is a lookup of facts.
**D3D11 entries only**; Metal entries have no device ID field to fill.

Where the corpus and `pci.ids` disagree on which model a device ID belongs to,
the corpus wins for the name and `pci.ids` for the ID, because each is
authoritative for its own half.

## Data model

A catalogue entry:

```rust
pub struct CatalogueEntry {
    /// Driver-reported model text: `Description` on D3D11, `MTLDevice.name`
    /// on Metal. E.g. "NVIDIA GeForce RTX 3070", "Apple M2".
    pub model: &'static str,
    /// ANGLE vendor token, e.g. "NVIDIA", "Apple".
    pub vendor: &'static str,
    /// PCI device ID, e.g. 0x2484. `None` on Metal, which has no PCI ID and
    /// no field in the renderer string to carry one.
    pub device_id: Option<u32>,
    /// Which measured capability tier this device runs on.
    pub tier: Tier,
    /// Share of the corpus population, for the share-weighted draw. See
    /// Selection.
    pub weight: f64,
}
```

`device_id` is `Option` because a Metal entry varies only in its name. Both
backends stay in one table rather than splitting, because selection,
share-weighting, and the round-trip test are then uniform, and the composer
already has to branch on backend to build the string at all.

### Feature sets come from the tier. There is no generation axis.

> **Revised during implementation.** This section previously specified a
> `Generation` key, a `FeatureSet` table, a `Probed`/`Estimated` provenance
> tag, and a rule that estimated entries omit five silicon-gated names
> (`shader-f16`, `subgroups`, `dual-source-blending`, `clip-distances`,
> `primitive-index`). All of it is removed. **Nothing measured supports a
> generation axis, and this project does not model distinctions it has not
> measured.**

The original design hedged against two axes at once, vendor and generation,
because only one device had been probed. Both have since been tested:

- **Vendor is flat within a backend.** An AMD Radeon (RDNA2) and an
  RTX 4090 (Lovelace), captured on the same machine under the same Chrome,
  report **identical** 19-feature sets and identical 36 limits — including all
  five names the original rule would have withheld.
- **Generation was never separately observed at all.** RDNA2 and Lovelace are
  two years and two vendors apart and still agree, which is evidence the
  WebGPU feature set is decided by the backend rather than by the silicon.

So a catalogue entry takes its features from its tier, which is measured for
every tier that ships. That is the whole rule. There is no second table, no
key, no provenance tag, and no entry that is "listed but not selectable" — the
probed-only default has nothing left to exclude, because every entry resolves
to a probed set.

What is genuinely carried, and why it is not inference: the 19 features on the
D3D11 captures include `core-features-and-limits`, `texture-formats-tier1`,
`texture-formats-tier2` and `texture-component-swizzle`, which are WebGPU
*specification* features present because that Chrome implements that revision.
They have nothing to do with the card. The rest came off a real adapter on that
tier, and carrying them is a statement about the tier, which is what a tier is
for.

This is also why deriving features from a GPU spec database would be wrong in
both directions: it would not merely risk overclaiming `shader-f16`, it would
omit those four browser-level features entirely and produce a list no real
Chrome reports.

The measured D3D11 set is a strict subset of the measured Metal set, differing
only in `texture-compression-astc`, `texture-compression-astc-sliced-3d` and
`texture-compression-etc2` — formats Apple silicon has and desktop D3D11 does
not. That is the only cross-tier feature variation in evidence, and it is a
texture-format distinction rather than a capability one.

**The ceiling this leaves, stated plainly.** Both probed parts are modern. The
catalogue spans older hardware, and an entry for an early feature-level-11 card
is served a feature set measured on a 2022 part. Nothing has measured whether
those agree. The honest options were to model the gap without evidence or to
document it, and modelling it was what produced the design this note replaces.
Feature-level-10 cards, at least, are excluded outright: ANGLE writes the
feature level into the renderer string as its shader model, so they are
detectable and are dropped rather than filed under an FL11 tier.

## Selection

Four strategies over one catalogue. Each is small; the catalogue is the
substance.

**Explicit.** `GpuDevice::by_name("RTX 3070")` — fuzzy-matched against model
text, erroring on ambiguity rather than guessing.

**Seeded diversity.** Draws an entry from the catalogue using the persona's
existing `Seed`, so a given seed always yields the same device. Composes with
the seeded farbling already in `Persona`.

**Share-weighted.** Draws proportional to the corpus's own frequencies.

> **Revised during implementation.** This originally specified a vendored,
> dated snapshot of the Steam Hardware Survey. That is no longer needed: the
> fingerprint corpus already pinned for model names is a Bayesian network, and
> its `videoCard` node carries real frequencies for exactly the devices being
> catalogued.
>
> `videoCard` is conditioned on `userAgent`, which is parentless with a prior
> summing to 1, so each device's weight is the marginal
> `Σ_ua P(ua) · P(device | ua)`. Summing the conditionals raw would
> over-weight anything common to many user agents.
>
> Three reasons this is better rather than merely cheaper. It removes a source
> (Valve publishes no API, and their page is not content-addressed, so it could
> not be pinned the way everything else here is). It removes a licensing
> question, since the community CSV archives carry no explicit license. And it
> is the *right population*: Steam skews hard toward discrete gaming cards,
> while a browser-automation tool wants what browsers report — the corpus's top
> entries are Intel Iris Xe and Apple silicon, which is what the web actually
> looks like.
>
> The stored weight is the raw marginal, so it does not sum to 1 over the
> catalogue: the excluded categories (iOS, Windows-on-ARM, WARP, VM adapters,
> non-modelled backends) hold the remainder. Selection renormalizes over
> whatever it is drawing from.

**Nearest-to-host.** `GpuDevice::nearest_to_host()` — probes the running
machine and picks the closest catalogue entry. **Explicit opt-in only.**
Probing the host to choose a persona is detect-and-adjust, which this project
forbids as a default; the named-opt-in shape matches `geo_auto`.

## Coherence

The catalogue widens the space of expressible identities, so it widens the
space of incoherent ones. Three rules, each enforced rather than documented:

1. **Entry and tier must agree.** An entry's `tier` is a field, not a guess,
   and a test asserts every entry's tier matches its backend.
2. **Platform skew.** A macOS persona selecting a D3D11 entry is incoherent.
   The existing `platform_skew` check already covers this shape and extends to
   catalogue entries.
3. **The composed string must resolve its own identity.** Passing an entry's
   composed renderer string back through `adapter_for_renderer` must derive
   the vendor and architecture that entry claims, and through
   `device_for_renderer` must resolve the tier it was filed under. Enforced by
   a test over the whole catalogue, so an entry cannot ship describing one
   device and resolving as another.

   This replaces a rule about a `generation` field, which no longer exists.
   Deriving the architecture from the string rather than storing it also keeps
   one source of truth: `adapter_for_renderer` already owns that mapping, and a
   second copy in the generator would be free to drift.

## Testing

- The generator's output is committed and a CI step regenerates and fails on
  any diff, exactly as the tier tables do.
- Every catalogue entry composes a renderer string that `device_for_renderer`
  resolves back to the same tier — a round-trip property over the whole table.
- Every entry's composed string derives the vendor and architecture the entry
  claims, via the existing `adapter_for_renderer`.
- The share snapshot's entries all exist in the catalogue; an entry named in
  the distribution but absent from the catalogue fails the build.
- A real-Chrome test that a catalogued device's identity reaches the page
  intact: renderer string, `adapter.info.architecture`, and the tier's
  capability values together.
- Seeded selection is deterministic: one seed, one device, across runs.

## Risks

**The corpus is version-stamped and unpinned.** Its renderer strings were
collected under older Chrome and omit the device ID current ANGLE appends, and
its URL tracks `master`. Both are handled in Sourcing — pin a commit, take only
the model name, compose the rest — but a generator that skipped either would
produce a catalogue that drifts without its input changing, which is the
failure mode hardest to notice.

**Vendor is a tested axis now, and the answer was mixed.** This section
originally recorded it as untested, pending a non-NVIDIA D3D11 capture. That
capture exists: an AMD Radeon (Raphael integrated, `0x164E`) probed on the
same machine and Chrome build as the RTX 4090.

For **WebGPU it settles the question in the design's favour** — 0 of 36 limits
and 0 of 19 features differ between RDNA2 and Lovelace, including all five
names this spec had originally flagged as silicon-gated and unsafe to estimate
(`shader-f16`, `subgroups`, `dual-source-blending`, `clip-distances`,
`primitive-index`). Feature sets stay keyed without a vendor dimension, and
D3D11 estimates are considerably safer than written below.

For **WebGL it found a real vendor split**, though a narrow one:
`MAX_VERTEX_UNIFORM_VECTORS` is 4095 on NVIDIA against 4096 elsewhere, with
two derived values following. The cause is
`ANGLE_FEATURE_CONDITION(features, skipVSConstantRegisterZero, isNvidia)` —
the vendor and nothing else, so the split is binary and predictable. Two
capability tiers ship for it (`d3d11-fl11`, `d3d11-fl11-nvidia`), both
measured, and a catalogue entry's `tier` field already carries which one a
device is on.

The generalisable lesson is about method, not about NVIDIA: the split was not
predicted from reading ANGLE, it was found by probing a second vendor and
diffing, and the explaining condition was looked up afterwards. Every
"generalizes by backend" claim in this document is worth the same treatment
before it is leaned on.

**`metal-3` is expected, not measured, on pre-M4 Apple silicon.** The
mechanism is known exactly (see Scope): it is a capability test, and Apple
documents the *newer* Metal 4 as M1-or-later, so Metal 3's floor is no higher.
An M1 reporting something other than `metal-3` on macOS 13+ would contradict
both. But no M1 was probed here, and "follows from the mechanism" is a weaker
warrant than "was measured." If a capture ever disagrees, the fix is local —
Metal entries gain the architecture axis that D3D11 entries already have.

**Catalogue breadth invites incoherent picks.** Mitigated by the three
coherence rules, but a caller who pins a renderer *and* a conflicting
`WebglSpec` can still construct a mismatch. The existing spec-overlay
precedence makes that the caller's explicit choice.

**Share data decays.** A dated snapshot goes stale as the market moves. It
fails visibly — the date is in the file — rather than silently, and
regenerating is one command.

## What this does not do

- **No new capability values.** Every entry runs on an already-measured tier.
  A device whose backend has no tier cannot be catalogued.
- **No Linux and no Intel-Mac entries.** Both take mechanisms this design has
  no evidence for. See Scope.
- **No claim of undetectability.** A catalogued identity is coherent across
  the readable surface. It does not make rendered pixels match the claimed
  device, and it cannot: that ceiling is unchanged from the tier work.
