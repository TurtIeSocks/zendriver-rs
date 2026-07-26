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

1. **Capture.** One command, same on every platform:

   ```bash
   cargo run -q -p zendriver --example probe_gpu -- native --emit-tier d3d11-fl11
   ```

   On a **Vulkan** capture, add the device and driver — its numbers are read
   off the physical device, so they mean nothing without knowing which:

   ```bash
   cargo run -q -p zendriver --example probe_gpu -- native --emit-tier vulkan-intel-uhd620-mesa24 --driver "Intel UHD Graphics 620, Mesa 24.0.9"
   ```

   This writes `crates/zendriver-stealth/data/gpu-tiers/<name>.json` with the
   Chrome version and OS already stamped into its provenance, and prints the
   renderer string it captured. Leave `--driver` off on D3D11 and Metal, where
   the capture is not device-specific.

   It **refuses** rather than writing a plausible-looking wrong file when:

   - the page is not a secure context — `navigator.gpu` is
     `[SecureContext]`-gated, so the capture would silently lack all WebGPU
     data; or
   - a `native` capture came back as `SwiftShader`, meaning the GPU never
     engaged and the file would describe a different backend than its name
     claims.

   Both failures produce a file that looks fine and is wrong, which is why they
   are refused here instead of left for a reviewer to catch.

   Read the renderer string it prints and confirm it names the backend you
   meant to capture (`Direct3D11`, `Metal`, `Vulkan`) — that string is also
   what names the tier, so if step 2 says you guessed wrong, just re-run with
   the right `--emit-tier` name and delete the stale file.

   > Do not reach for a shell pipeline here. The old form of this step piped
   > stdout through `python3` with a `VAR=x cmd` prefix, which has no
   > PowerShell equivalent; `python3` is `python` on Windows; and `>` in
   > Windows PowerShell 5.1 writes UTF-16 with a BOM, producing a capture that
   > looks correct in an editor and fails to parse. `--emit-tier` writes the
   > same bytes on every platform.

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

   The `--driver` value belongs in the renderer string's own fields — e.g.
   from `ANGLE (Intel, Intel(R) UHD Graphics 620 (KBL GT2), Mesa 24.0.9)`,
   pass `"Intel UHD Graphics 620, Mesa 24.0.9"`.

3. **Register the tier in the code.** Six places, all small. Miss one and the
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

4. **Regenerate and verify.**

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

5. **Commit** the capture, the regenerated `tiers.rs`, and every registration
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
