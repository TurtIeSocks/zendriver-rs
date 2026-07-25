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

## No automatic fallback

If `Native` is selected and Chrome's GPU process cannot start — no GPU
present, a crashed GPU process, a missing sandbox — the launch fails with
[`BrowserError::GpuBackendUnavailable`] rather than silently retrying with a
software rasterizer. That's deliberate: falling back automatically would
restore the same incoherent, mixed software/hardware fingerprint that
choosing a backend explicitly exists to avoid. If a launch might land on a
GPU-less host, catch `GpuBackendUnavailable` (along with `DevtoolsParse` and
`EarlyExit`, the other failure shapes a missing GPU process produces) and
retry with `GpuBackend::SwiftShader` or `GpuBackend::Disabled` yourself.

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

`MAX_TEXTURE_SIZE` and the other numeric WebGL caps are not read from the
GPU — ANGLE computes them from constants branched on backend and feature
tier, not from device queries. That's why the SwiftShader row above is
identical regardless of what real GPU sits underneath: it's a software
rasterizer, so there's no real device to query.

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

## MCP

`browser_open` exposes this as the `gpu_backend` option (`"disabled"` |
`"swift_shader"` | `"native"`, default `"disabled"`). See the
[MCP chapter](mcp.md#browser_open-options).
