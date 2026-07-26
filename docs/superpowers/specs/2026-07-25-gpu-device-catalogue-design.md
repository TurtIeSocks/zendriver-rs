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
| WebGL parameters (D3D11) | feature level, not card | `renderer11_utils.cpp` switches on `D3D_FEATURE_LEVEL`; every value is a `D3D11_REQ_*` constant |
| WebGL parameters (Metal) | nothing on macOS | `DisplayMtl.mm:727-731`, `TARGET_OS_OSX` branch is compile-time constants |
| WebGL parameters (Vulkan) | **the device** | `vk_caps_utils.cpp:652-684` reads `VkPhysicalDeviceLimits`; two Vulkan captures on one Chrome differ in 21 WebGL2 params |
| WebGPU limits | backend | 5 of 36 differ between Metal and D3D12, all API-level (`maxBufferSize`, storage-buffer counts) |
| WebGPU features | backend | D3D11's 19 are a strict subset of Metal's 22; the 3 extra are ASTC/ETC2 texture formats Apple silicon has and desktop D3D11 lacks |
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
    /// Architecture generation, keying the WebGPU feature set. `None` where
    /// the backend's architecture token is constant and carries no
    /// generation — every Apple silicon part reports `metal-3`.
    pub generation: Option<Generation>,
}
```

Both `Option`s are the same fact seen twice: a Metal entry varies only in its
name, so it has neither a device ID nor a generation. They stay in one table
rather than splitting per backend because selection, share-weighting, and the
round-trip test are then uniform, and the composer already has to branch on
backend to build the string at all.

Feature sets are keyed by generation where one exists, and otherwise fall back
to the tier's own probed set — which is the Metal case, and the reason the key
is an `Option` rather than a second table:

```rust
pub struct FeatureSet {
    /// `None` = this is the tier's own probed set, used by every entry on the
    /// tier whose backend has no generation axis.
    pub generation: Option<Generation>,
    pub tier: Tier,
    pub features: &'static [&'static str],
    pub provenance: FeatureProvenance,
}

pub enum FeatureProvenance {
    /// Measured on real hardware of this generation.
    Probed { chrome: &'static str, device: &'static str },
    /// The tier's probed set minus the silicon-gated names (see below).
    Estimated { carried_from: Tier },
}
```

### How features are keyed, and what "estimated" means

The estimation rule is one sentence, so it can be checked mechanically rather
than argued case by case:

> **An estimated entry reports its tier's probed feature set, minus the
> silicon-gated names.**

The silicon-gated list is a small, explicit constant — `shader-f16`,
`subgroups`, `dual-source-blending`, `clip-distances`, `primitive-index`.
Whether Dawn exposes these depends on shader-model support *and* the installed
driver, so they are the ones an older card on the same tier plausibly lacks.
An estimated entry **omits** them.

Everything else in the set is carried, and the carrying is not inference. The
19 features on the RTX 4090 capture include `core-features-and-limits`,
`texture-formats-tier1`, `texture-formats-tier2`, and
`texture-component-swizzle` — WebGPU *specification* features, present because
that Chrome implements that revision. They have nothing to do with the card.
The remainder (BC compression, the float and depth formats, `timestamp-query`,
`indirect-first-instance`) came off a real adapter on this tier, and carrying
them is a statement about the tier, which is exactly what a tier is for.

Omission of the gated five is deliberate. A page requiring an omitted feature
gets a clean rejection, which is what a card lacking it does; claiming one the
silicon cannot back is an overclaim a page triggers in one call. Under-claiming
errs toward a real configuration.

Note what this rules out: deriving features from a GPU spec database would not
merely risk overclaiming `shader-f16`. It would **omit the four browser-level
features entirely**, producing a list no real Chrome reports. The error runs
both directions, which is why the whole set is anchored to a probe rather than
reasoned about from hardware specs.

The measured D3D11 set is also a strict subset of the measured Metal set — the
three differing names are `texture-compression-astc`,
`texture-compression-astc-sliced-3d`, and `texture-compression-etc2`, all
formats Apple silicon has and desktop D3D11 does not. That is the only
cross-tier feature variation in evidence, and it is a texture-format
distinction, not a capability one.

### Provenance is for the caller, not the page

The page never sees `FeatureProvenance`. An overclaim is equally detectable
however honestly it was labelled. The tag earns its keep only because
something acts on it:

**Probed-only is the default.** Naming a device whose feature set is
`Estimated` is an error unless the caller opts in explicitly. That keeps the
default honest and makes the tag load-bearing rather than decorative.

The two backends land very differently under this rule, which is worth stating
plainly rather than discovering at implementation time:

- **Metal ships fully probed.** Every Apple silicon entry resolves to the
  tier's own probed set, because there is no generation axis to miss. The
  whole M-series name list is usable on the default.
- **D3D11 ships one probed generation** — `lovelace`, from the RTX 4090. Every
  other NVIDIA generation, and every AMD and Intel part, is `Estimated` until
  someone captures one. On the default they are *listed but not selectable*.

That is a real limitation of v1 and not a soft one: the interesting fleet
diversity on Windows is behind an opt-in until more captures exist. It is
still the right default — the alternative is shipping guessed feature sets as
if they were measured, which is the failure this whole design is built to
avoid.

## Selection

Four strategies over one catalogue. Each is small; the catalogue is the
substance.

**Explicit.** `GpuDevice::by_name("RTX 3070")` — fuzzy-matched against model
text, erroring on ambiguity rather than guessing.

**Seeded diversity.** Draws an entry from the catalogue using the persona's
existing `Seed`, so a given seed always yields the same device. Composes with
the seeded farbling already in `Persona`.

**Share-weighted.** Draws proportional to a vendored, dated snapshot derived
from the Steam Hardware Survey, regenerable by a generator in the same shape
as `locale-gen`'s pinned CLDR tag. A stale snapshot is still a real
distribution and decays visibly rather than silently. No network at runtime.

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
3. **Generation and architecture must agree.** Where an entry has a
   `generation`, it must produce the same `architecture` token that
   `adapter_for_renderer` derives from its composed renderer string. Where it
   has none, the backend's constant token must be what that function returns.
   Enforced by a test over the whole catalogue, so a mis-keyed entry cannot
   ship.

## Testing

- The generator's output is committed and a CI step regenerates and fails on
  any diff, exactly as the tier tables do.
- Every catalogue entry composes a renderer string that `device_for_renderer`
  resolves back to the same tier — a round-trip property over the whole table.
- Every entry's generation agrees with the architecture derived from its
  string.
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

**Vendor is an untested axis.** Feature sets are keyed by generation alone.
The evidence for that is an M4 Pro and an RTX 4090 differing by three
features, all texture-compression formats — no sign of a vendor dimension, but
no non-NVIDIA D3D11 capture exists to rule one out. If one later disagrees,
the key widens to `(vendor, generation)`. That is a contained schema change:
`GenerationFeatures` gains a field and its lookup gains an argument.

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
