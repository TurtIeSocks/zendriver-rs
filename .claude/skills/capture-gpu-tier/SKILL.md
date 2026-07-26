---
name: capture-gpu-tier
description: >-
  Use when capturing a GPU capability tier for the stealth crate's WebGL
  spoofing tables — running the probe on a machine whose GPU backend is not
  yet represented, then regenerating and committing the tables. Triggers on a
  request to "capture a tier", "add a GPU tier", "probe this machine's GPU",
  or a `platform_skew` warning about a persona whose OS has no matching tier.
  Keywords: probe_gpu, gpu-tier-gen, tiers.rs, D3D11, Metal, SwiftShader,
  data/gpu-tiers.
---

# Capture a GPU capability tier

## Overview

`crates/zendriver-stealth/src/gpu/tiers.rs` holds the WebGL capability values
the stealth patch reports. It is **generated** from committed probe captures in
`crates/zendriver-stealth/data/gpu-tiers/*.json`, one per backend tier.

Whether a capture generalizes beyond the machine it was taken on depends
entirely on the backend, because ANGLE decides these values differently per
backend. Know which case you are in before you name the tier:

- **D3D11 — generalizes by feature level.** `renderer11_utils.cpp` branches on
  the `D3D_FEATURE_LEVEL`, so every FL11+ card reports the same numbers whether
  it is Intel, AMD or NVIDIA. One capture from any D3D11 machine covers every
  D3D11 GPU at that feature level.
- **Metal on macOS — generalizes across Macs.** `DisplayMtl.mm`'s
  `TARGET_OS_OSX` arm sets its caps from plain compile-time constants. (The
  runtime `supportsAppleGPUFamily` test that would vary them is in the iOS arm
  — see step 2.)
- **Vulkan — does NOT generalize.** `vk_caps_utils.cpp` reads the caps straight
  off the physical device: `max2DTextureSize` is
  `min(limitsVk.maxFramebufferWidth, limitsVk.maxImageDimension2D)`, the
  viewport bounds come from `limitsVk.maxViewportDimensions`, and that one file
  reads `limitsVk.` around 99 times. A Linux/Vulkan capture describes **that
  GPU under that driver**, and nothing else.

So on D3D11 and Metal, capturing on the right *backend* matters more than on the
right *card*, which is why a handful of captures is enough. On Vulkan there is
no such shortcut: name and document the tier for the specific device and driver
it came from (step 2), record the Mesa/driver version in its provenance
(step 3), and never present it as covering Vulkan generally.

**Never hand-write or edit a tier's values.** A wrong value is more detectable
than no spoof at all, and `tiers.rs` carries a `DO NOT EDIT` header enforced by
a CI regeneration check. The only way to add a tier is to run this skill on
hardware that actually has that backend.

## When a tier is missing

A persona whose platform has no captured tier logs:

```
no captured GPU tier matches this renderer; using the fallback device,
whose capability values come from a different backend
```

That means the persona reports one platform's renderer string above another
backend's numbers — checkable in one line by any page. Capturing the tier is
the fix.

## Procedure

Run this on a machine with the backend you want to capture. It must have a
real GPU: a `Native` launch on a GPU-less host fails rather than falling back.

1. **Confirm the backend engages.**

   ```bash
   cargo run -q -p zendriver --example probe_gpu -- native 2>/dev/null \
     | python3 -c "import json,sys; d=json.load(sys.stdin); w=d['webgl2']; print('secure:', d['isSecureContext']); print('adapter:', d['adapter'] and d['adapter']['vendor']); print('renderer:', w['unmaskedRenderer']); print('MAX_TEXTURE_SIZE:', w['params']['MAX_TEXTURE_SIZE'])"
   ```

   `isSecureContext` must be `true` — `navigator.gpu` is `[SecureContext]`-gated
   and the probe navigates a temp `file://` page for exactly this reason. The
   renderer string must name the backend you meant to capture (`Direct3D11`,
   `Metal`, `Vulkan`). If it says `SwiftShader`, the GPU did not engage and the
   capture is worthless — stop and fix the machine, not the data.

2. **Pick the tier name** from the renderer string, using the existing files as
   precedent: `swiftshader`, `metal-macos`, `d3d11-fl11`, and — the Vulkan
   case, named for its device and driver — `vulkan-mesa-intel-iris-pro-580`.

   Where the values generalize, name by *backend and capability tier*, not by
   card — `d3d11-fl11` rather than `nvidia-rtx-4090`, because every D3D11
   feature-level-11 GPU reports the same numbers. That advice is a *consequence*
   of the generalization above, not a house style, so it inverts where the
   generalization does not hold: a **Vulkan** tier's numbers come off the
   physical device, so the device and the driver *are* the tier and the name has
   to carry both — `vulkan-intel-uhd620-mesa24`, not `vulkan-linux`. A
   backend-general name on a device-specific capture is the worst outcome
   available here: it reads as covering every Linux GPU and quietly serves one
   machine's numbers to all of them.

   Name it after the branch ANGLE actually takes, which means reading the
   `#if`/`else` around the constants and not just the nearest capability test.
   The Metal tier was `metal-apple-family3` until someone did: the
   `supportsAppleGPUFamily(3)` check that chooses 16384 over 8192 is in
   `DisplayMtl.mm`'s **iOS** arm, while macOS takes the `TARGET_OS_OSX` arm
   above it, where the same values are plain compile-time constants. The name
   advertised a distinction no Mac can express, and would have sent someone to
   capture an Intel Mac for a tier that reproduces `metal-macos` exactly.

3. **Capture with provenance.**

   The probe reports its own `userAgent`, so the Chrome version lands in the
   provenance string without you looking it up per-platform. On **Vulkan** the
   Chrome version is not enough: the numbers came off this physical device
   through this driver, so set `DRIVER` to the GPU model and Mesa/driver version
   (both are in the renderer string's fields, e.g. `ANGLE (Intel, Intel(R) UHD
   Graphics 620 (KBL GT2), Mesa 24.0.9)`). Leave `DRIVER` unset on D3D11 and
   Metal, where the capture is not device-specific.

   ```bash
   TIER=d3d11-fl11   # <- set this
   DRIVER=           # <- Vulkan only, e.g. "Intel UHD Graphics 620, Mesa 24.0.9"
   cargo run -q -p zendriver --example probe_gpu -- native 2>/dev/null \
     | TIER=$TIER DRIVER=$DRIVER python3 -c "
import json,sys,os,platform,re
d=json.load(sys.stdin)
assert d['isSecureContext'], 'not a secure context; WebGPU data would be missing'
r=d['webgl2']['unmaskedRenderer']
assert 'SwiftShader' not in r, f'GPU did not engage, got: {r}'
m=re.search(r'Chrome/([\d.]+)', d.get('userAgent',''))
chrome=m.group(1) if m else 'unknown'
driver=os.environ.get('DRIVER','').strip()
prov=f'probed: Chrome {chrome} on {platform.system()} {platform.release()}'
if driver: prov += f' / {driver}'
print(json.dumps({'tier': os.environ['TIER'], 'provenance': prov,
                  'capture': d}, indent=2, sort_keys=True))
" > crates/zendriver-stealth/data/gpu-tiers/$TIER.json
   ```

   The two asserts are the point of this step: a capture from a non-secure
   context silently lacks WebGPU data, and a capture that fell back to
   SwiftShader describes the wrong backend entirely. Both produce a file that
   looks fine and is wrong.

4. **Register the tier in the code.** Six places, all small. Miss one and the
   tier is half-registered:
   - `crates/zendriver-stealth/src/gpu/types.rs` — add a `Tier` variant.
   - `crates/zendriver-stealth/src/gpu/types.rs` — add it to `Tier::ALL` too.
     **This is the one whose omission is silent**: `ALL` is what every
     all-tiers invariant sweep and the alias-equality check in `enum_names`
     iterate, so a tier left out of it simply skips all of them and every test
     still passes.
   - `crates/zendriver-stealth/src/gpu/mod.rs` — add the variant to `tier_key`,
     returning the same string as the capture's filename.
   - `crates/gpu-tier-gen/src/lib.rs` — add the `(name, include_str!(...))`
     pair to `CAPTURES`.
   - `crates/zendriver-stealth/src/gpu/devices.rs` — add a `DeviceRow` with the
     captured `unmasked_vendor`/`unmasked_renderer`, the new `tier`, its
     `match_token` (a lowercase substring unique to this backend's renderer
     string), and the WebGPU vendor/architecture the capture reported.
   - `crates/zendriver-stealth/src/gpu/invariants.rs` — add the tier to
     `platform_skew`'s coherent-pair arm if its backend belongs to one OS
     (Metal → `MacIntel`, D3D11 → `Win32`), or return early like SwiftShader
     if it is platform-neutral. Otherwise every persona on the new tier logs a
     skew warning.

5. **Regenerate and verify.**

   ```bash
   cargo run -p gpu-tier-gen
   cargo test -p gpu-tier-gen
   cargo test -p zendriver-stealth
   cargo fmt --all
   cargo clippy --workspace --all-targets --locked -- -D warnings
   ```

   The generator's capture-derived guard will fail if the new capture contains
   a parameter no one has classified against the WebGL spec — that failure is
   the point, not an obstacle. Classify it in `gl_type_for` (if JSON cannot
   represent its type faithfully) or `VERIFIED_PLAIN_PARAMS`, and say which in
   the commit message.

   `shipped_tiers_are_all_coherent` must also pass. If it fails, the capture
   disagrees with a relation real hardware satisfies — investigate the capture
   before touching the invariant.

6. **Commit** the capture, the regenerated `tiers.rs`, and every registration
   edit together, so the generated file never lands out of sync with its
   input:

   ```bash
   git add crates/zendriver-stealth/data/gpu-tiers crates/zendriver-stealth/src/gpu crates/gpu-tier-gen
   git commit -m "feat(stealth): capture the <tier> GPU capability tier

   Probed on <machine/OS/Chrome version>. <N> WebGL2 parameters, of which
   <M> differ from the shared base."
   ```

## What not to do

- **Do not capture on a GPU-less host or a VM without passthrough.** You get
  SwiftShader's numbers under a hardware renderer name, which is worse than
  having no tier: it looks captured but describes a different backend.
- **Do not edit `tiers.rs`.** CI regenerates it and fails on any diff.
- **Do not invent values for a tier you cannot probe.** The whole design rests
  on every number having been measured somewhere.
- **Do not reuse a tier name across backends.** The name is the join key
  between the capture, `tier_key`, and the device row.
- **Do not give a Vulkan capture a backend-general name or provenance.** Its
  numbers are read off one physical device (`vk_caps_utils.cpp`), so a
  `vulkan-linux` tier claims a generality it does not have — see the Overview.
