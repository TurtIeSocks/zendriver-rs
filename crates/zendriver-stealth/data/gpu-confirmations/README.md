# GPU confirmation captures

Probe captures kept as **evidence**, not as capability tables. Nothing here is
in `gpu-tier-gen`'s `CAPTURES` list, nothing here is compiled into
`tiers.rs`, and no persona is ever served these numbers. They are here so a
reader can check a claim the tier rustdocs make instead of taking the doc's
word for it.

A capture earns a place here by supporting a shipped design decision. A
capture that merely reproduces a tier that already exists does not — that is
what the sibling `gpu-tiers/` directory is for, and a duplicate is clutter
rather than corroboration.

| File | Device | Backend | What it establishes |
|---|---|---|---|
| `d3d11-nvidia-maxwell-gm108.json` | NVIDIA GeForce GPU `0x134B` (Maxwell GM108, Surface Book 1) | D3D11 | The NVIDIA tier generalizes across GPU generations. Against the RTX 4090 (Lovelace) the tier was probed on — seven years and three process nodes apart — **nothing differs**: all 82 WebGL1 parameters, all 132 WebGL2, every shader precision, both extension lists in content and order, all 36 WebGPU limits. This is why Intel Gen9 became a third tier rather than a reason to distrust the tier model. |
| `gl-amd-rdna2-vangogh.json` | AMD Custom GPU 0405 (RDNA2 Van Gogh, Steam Deck) | ANGLE over Mesa `radeonsi` **GL** | `MAX_SAMPLES` is a property of the silicon, not the backend: 8 here, the same value the RDNA2 D3D11 capture reports. Also the only record of what default Chrome on SteamOS reports, which no shipped tier covers. |
| `vulkan-amd-rdna2-vangogh.json` | AMD Custom GPU 0405 `0x163F` (RDNA2 Van Gogh, Steam Deck) | ANGLE over Mesa **RADV** | Two things. `MAX_SAMPLES` is 8 on a third backend for the same architecture. And a Vulkan tier genuinely does not generalize: against the shipped `vulkan-mesa-intel-iris-pro-580` tier, same backend and same driver stack on different silicon, 12 parameters differ — `MAX_3D_TEXTURE_SIZE` 8192 against 2048, `UNIFORM_BUFFER_OFFSET_ALIGNMENT` 4 against 64, `MIN`/`MAX_PROGRAM_TEXEL_OFFSET` -32/31 against -8/7. Those are `VkPhysicalDeviceLimits` entries reaching the page unchanged. |

## The `MAX_SAMPLES` table these establish

| Architecture | D3D11 | ANGLE-GL | ANGLE-Vulkan |
|---|---|---|---|
| AMD RDNA2 | 8 | 8 | 8 |
| Intel Gen9 | 16 | — | 16 |

Constant per architecture across every backend measured, and different between
the two architectures on the same backend. That is the measurement behind
`Tier::D3d11Fl11IntelGen9` existing at all.

## Read the flags before reusing a number

`vulkan-amd-rdna2-vangogh.json` was taken under
`--use-gl=angle --use-angle=vulkan`. SteamOS Chrome picks the GL backend on its
own, so that capture describes a browser nobody runs by default. It is sound as
evidence about ANGLE's Vulkan backend and unsound as a description of what a
Steam Deck visitor reports. `gl-amd-rdna2-vangogh.json` is the default-flag one.

Every capture here reports `adapter: null`: Chrome resolves no WebGPU adapter on
Linux/Mesa, under either backend. So none of these can say whether the WebGPU
feature list tracks hardware or driver policy — the open question behind the
Gen9 tier's 16-feature list. Answering it needs silicon on Windows or macOS
that nothing has probed yet.
