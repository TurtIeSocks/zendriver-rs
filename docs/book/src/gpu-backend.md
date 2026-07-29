# GPU backend

By default zendriver launches Chrome with `--disable-gpu` and no ANGLE
backend named. Bare `--disable-gpu` does not by itself guarantee a working
WebGL context — Chrome ≥116 returns `null` from `canvas.getContext('webgl')`
without `--enable-unsafe-swiftshader`. It's the **spoofed** stealth profile
that forces a working software fallback, by unconditionally adding
`--use-gl=angle --use-angle=swiftshader --enable-unsafe-swiftshader` so a
WebGL context exists at all under headless. That is a safe default for
headless CI, but it produces a GPU surface no real device ever produces:
numeric WebGL capability limits and the SwiftShader renderer string that no
laptop or workstation reports.

`GpuBackend` is an opt-in `BrowserBuilder` option that lets Chrome use the
host's real GPU instead.

```rust,no_run
use zendriver::{Browser, GpuBackend};

# async fn demo() -> Result<(), Box<dyn std::error::Error>> {
let browser = Browser::builder()
    .gpu_backend(GpuBackend::Native)
    .launch()
    .await?;
# Ok(())
# }
```

## The three variants

| Variant | What it does | When to reach for it |
|---|---|---|
| `GpuBackend::Disabled` (default) | `--disable-gpu` under headless, no ANGLE backend forced — Chrome picks its own fallback. Byte-for-byte today's behavior. | The historical default; nothing changes if you never touch this option. |
| `GpuBackend::SwiftShader` | Forces ANGLE's SwiftShader software rasterizer explicitly (`--use-gl=angle --use-angle=swiftshader --enable-unsafe-swiftshader`). | You want a guaranteed-working WebGL context on a host with no GPU, and are fine with a software-rasterizer fingerprint. |
| `GpuBackend::Native` | Drops `--disable-gpu` and names the platform's ANGLE backend (`metal` on macOS, `d3d11` on Windows, `vulkan` elsewhere), so Chrome renders on the host's real GPU. | You have real GPU hardware available and want a coherent, non-software GPU surface — WebGL capability values, WebGPU adapter, and pixel output that all match what a real device reports. |

`Disabled` and `SwiftShader` both still emit `--disable-gpu` under headless;
only `Native` omits it.

## Why one enum owns both decisions

Dropping `--disable-gpu` without also naming an ANGLE backend hangs headless
Chrome — measured directly on this project's dev host, twice, killed at the
30-second mark both times. The two decisions (suppress `--disable-gpu`, name
a backend) are coupled, so `GpuBackend` owns both together rather than
leaving it to the caller to combine two separate flags correctly.

## `Native` reports the host's GPU, not a chosen one

`GpuBackend::Native` gives a fully coherent GPU surface, but it does **not**
give identity control. Chrome reports whatever GPU is physically present.
Every browser launched with `Native` on the same host reports the same
adapter, the same WebGL renderer string, the same capability limits — a
fleet sharing one host shares one GPU fingerprint. If you need distinct GPU
identities across a fleet, `Native` is not that tool; look at
[`WebgpuSpec`](fingerprint.md#webgpu-opt-in-adapter-override--fabrication)
for caller-supplied adapter values instead. `Native` makes the GPU surface
*coherent*; it does not give you *control* over what it says.

## The launch is validated, and there is no automatic fallback

Chrome *starting* is not evidence that it got a GPU. On a GPU-less Ubuntu 24
VM, Chrome 150 launched successfully under `Native` and then returned `null`
from both `canvas.getContext('webgl')` and `getContext('webgl2')`. That is a
browser strictly **more** detectable than the default — a missing WebGL
context is one of the oldest and cheapest headless tells, and the stealth
WebGL patch cannot repair it, because it patches prototypes and there is no
context to patch.

So `Native` verifies the launch. After the CDP handshake, zendriver asks
Chrome what it actually initialized (`SystemInfo.getInfo`, a browser-level
domain — no page, no navigation, one round-trip) and reads
`gpu.featureStatus.webgl`. Measured on Chrome 150.0.7871.186 on the darwin
dev host: `Native` reports `enabled`, `SwiftShader` reports
`enabled_readback`, `Disabled` reports `disabled_off`. Anything but a
hardware status terminates the Chrome that was just spawned and fails the
launch with [`BrowserError::GpuBackendUnavailable`].

If the GPU cannot be *verified* — `SystemInfo.getInfo` unavailable, or
answering with a status string zendriver does not recognize — the launch logs
a warning and proceeds. A missing diagnostic API is not evidence of a missing
GPU, and refusing to launch over one would be worse than the problem the
check addresses.

There is no fallback. Failing rather than retrying on SwiftShader is
deliberate: falling back automatically would serve a software rasterizer's
values under a "native" label — the same incoherent, mixed software/hardware
fingerprint that choosing a backend explicitly exists to avoid. If a launch
might land on a GPU-less host, catch `GpuBackendUnavailable` and retry with
`GpuBackend::SwiftShader` or `GpuBackend::Disabled` yourself.

[`BrowserError::GpuBackendUnavailable`]: https://docs.rs/zendriver/latest/zendriver/enum.BrowserError.html

## Measured comparison

All figures below are from real Chrome (150.0.7871.186) on this project's
darwin dev host (Apple M4 Pro), probed with the
[`probe_gpu` example](#probing-your-own-host). WebGL figures are from a
secure-context (`file://`) page — see the note on `navigator.gpu` below for
why that matters. The `SwiftShader` column is `GpuBackend::SwiftShader`
(equivalently: `GpuBackend::Disabled` with a spoofed stealth profile
attached, since that profile forces the same flags — see the intro above).
Bare `GpuBackend::Disabled` with no spoofed profile was not measured here and
is not shown — it emits no ANGLE flags, so nothing above the "does a WebGL
context exist at all" question applies to it.

| | `SwiftShader` | `Native` |
|---|---|---|
| WebGPU adapter | none (`requestAdapter()` resolves `null`) | real: `vendor: "apple"`, `architecture: "metal-3"` |
| `requestDevice()` | n/a (no adapter) | succeeds |
| WebGPU adapter limits | n/a | ~36 limits |
| WebGL renderer string | `ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (LLVM 10.0.0) (0x0000C0DE)), SwiftShader driver)` | `ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)` |
| WebGL `MAX_TEXTURE_SIZE` | 8192 | 16384 |
| WebGL extension count | 30 | 36 |

Whether `MAX_TEXTURE_SIZE` and the other numeric WebGL caps are read from the
GPU **depends on the backend**, and that is what decides how far one probe
generalizes:

- **D3D11 and Metal-on-macOS: not read from the device.** ANGLE computes them
  from constants branched on the feature level (`renderer11_utils.cpp`) or
  from plain compile-time constants (`DisplayMtl.mm`'s `TARGET_OS_OSX` arm), so
  one probe covers every card on that backend.
- **SwiftShader: no device to read.** It is a software rasterizer, which is why
  the row above is identical regardless of what real GPU sits underneath.
- **Vulkan: read straight off the device.** `vk_caps_utils.cpp` fills its caps
  from `VkPhysicalDeviceLimits`, so a Linux probe describes that GPU under that
  Mesa build and nothing else. zendriver's Vulkan tier is named for its device
  for exactly this reason (Intel Iris Pro Graphics 580, Mesa 25.2.8), and
  covering another Linux GPU means capturing it.

The one part of the SwiftShader row that *is* host-specific is the renderer
string. Re-probing the same flags on Ubuntu 24 (Chrome 150.0.7871.114)
reproduced every capability value above, but reported `SwiftShader Device
(Subzero)` rather than `(LLVM 10.0.0)` — SwiftShader picks its JIT backend at
build time and Chrome prints the choice. Windows reports Subzero too (measured
on Windows 10.0.21996, Chrome 150.0.7871.186).

The spoofed profile now serves neither string by default on any platform: a
`Win32` persona resolves the captured D3D11 tier and a `LinuxX86_64` persona
the captured Mesa/Vulkan one, both real hardware rather than a software
rasterizer. A SwiftShader row is reached only by pinning such a renderer
yourself, and which of the two builds you land on then follows the persona's
platform. See
[the WebGL section of the fingerprint chapter](./fingerprint.md#webgl-full-surface-value-spoof-resolved-from-measured-tiers).

## `navigator.gpu` visibility is governed by the page, not by this option

`navigator.gpu` is `[SecureContext]`-gated. On an opaque-origin page —
`about:blank`, a bare `data:` URL — `navigator.gpu` is absent no matter which
`GpuBackend` you pick, headless or headful. This is easy to misread as a
launch-flag effect because `about:blank` is also zendriver's typical first
page; it isn't one. Load a secure-context page (`https://`, or `file://` for
local probing) before checking `'gpu' in navigator`.

## Probing your own host

The [`probe_gpu` example](https://github.com/TurtIeSocks/zendriver-rs/blob/main/crates/zendriver/examples/probe_gpu.rs)
dumps the full WebGPU + WebGL surface as JSON for any backend:

```sh
cargo run -p zendriver --example probe_gpu -- native
cargo run -p zendriver --example probe_gpu -- swiftshader
cargo run -p zendriver --example probe_gpu -- disabled
```

### From a browser, with no toolchain

The same measurement runs as a page: **[GPU tier probe](probe/index.html)**.
Open it on the machine you want to read and it produces the identical capture
file, which matters when that machine is an old laptop or a phone where
installing a Rust toolchain is the whole obstacle. The measurement runs locally
and nothing is uploaded; the page offers a download, a share sheet where the
platform has one, and a two-step path to opening a pull request.

It refuses to export rather than hand back a plausible wrong answer. A page
served insecurely reports no WebGPU adapter on a machine that has one, and a
privacy extension that farbles WebGL produces a capture that is well-formed and
describes a GPU that does not exist. The page checks for both, along with
whether the renderer is ANGLE's at all — Safari and every iOS browser use
WebKit's own stack, so their numbers are real but belong to a graphics layer
these tiers do not model.

The browser path cannot select a backend the way the arguments above do, since
that needs launch flags. It captures whatever the browser is already using,
which for ordinary Chrome on a working GPU is the native backend — the one
worth capturing.

## MCP

`browser_open` exposes this as the `gpu_backend` option (`"disabled"` |
`"swift_shader"` | `"native"`, default `"disabled"`). See the
[MCP chapter](mcp.md#browser_open-options).
