# GPU Device Catalogue Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a caller name a specific GPU, or draw one, and get a coherent identity (renderer string, device ID, WebGPU architecture) composed over one of the already-measured capability tiers.

**Architecture:** A generator crate (`gpu-catalogue-gen`) fetches model names from a pinned `fingerprint-suite` commit and device IDs from a vendored `pci.ids`, then emits a committed Rust table. At runtime the catalogue supplies only *identity*: the capability values still come from `gpu::tiers`. Selection is four small strategies over that one table.

**Tech Stack:** Rust 2024 (MSRV 1.85), `serde_json`, `reqwest` (blocking, generator only), the existing `gpu-tier-gen` / `locale-gen` generator pattern.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-25-gpu-device-catalogue-design.md`. Read it before Task 1.
- **Never invent a fingerprint value.** Every catalogue field traces to the vendored corpus, `pci.ids`, or ANGLE/Dawn source. A wrong value is more detectable than honest absence.
- **No auto behavior.** Named opt-ins defaulting off are fine; silent detect-and-adjust is not. `nearest_to_host()` must be an explicit call, never a default.
- **v1 backends: D3D11 and Metal only.** No Linux/Vulkan entries — ANGLE reads those caps off the physical device. No Intel-Mac entries — Dawn takes a different architecture path when `mDeviceId != 0`.
- **Generated files carry a `DO NOT EDIT` header** and are verified by a CI regeneration diff, exactly as `crates/zendriver-stealth/src/gpu/tiers.rs` is.
- **Probed-only is the default.** Selecting an entry whose feature set is `Estimated` is an error unless the caller opts in.
- Before every push: `cargo fmt --all`, then `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- Any public API change needs a `crates/zendriver-mcp/mcp-coverage-ledger.toml` entry and a regenerated `public-api-baseline.txt`.
- Docs sync is required: READMEs, rustdoc, and `docs/book/src/`.

## Prerequisites

- [ ] **PR #126 is merged**, and the release-plz PR it triggers has merged too.
- [ ] Branch cut from a **freshly fetched** `origin/main` after both land:

```bash
git fetch origin
git worktree add .claude/worktrees/gpu-catalogue -b feat/gpu-device-catalogue origin/main
cd .claude/worktrees/gpu-catalogue
```

- [ ] Confirm the tier work is present before starting (all five tiers must be listed):

```bash
cargo test -p zendriver-stealth gpu:: 2>&1 | tail -3
ls crates/zendriver-stealth/data/gpu-tiers/
```

Expected: tests pass, and the directory lists `d3d11-fl11.json`, `d3d11-fl11-nvidia.json`, `metal-macos.json`, `swiftshader.json`, `vulkan-mesa-intel-iris-pro-580.json`.

## File Structure

| File | Responsibility |
|---|---|
| `crates/gpu-catalogue-gen/Cargo.toml` | Generator crate, `publish = false` |
| `crates/gpu-catalogue-gen/src/lib.rs` | Parse corpus + `pci.ids`, compose strings, emit Rust |
| `crates/gpu-catalogue-gen/src/main.rs` | Pinned source refs, fetch, write output |
| `crates/gpu-catalogue-gen/tests/fixtures/` | Small corpus + `pci.ids` fixtures for unit tests |
| `crates/zendriver-stealth/data/pci.ids` | Vendored PCI ID database (dual BSD-3/GPLv2) |
| `crates/zendriver-stealth/data/NOTICE-pci-ids` | License notice for the above |
| `crates/zendriver-stealth/src/gpu/catalogue.rs` | GENERATED table: `CatalogueEntry`, `FeatureSet` |
| `crates/zendriver-stealth/src/gpu/device_select.rs` | The four selection strategies, hand-written |
| `crates/zendriver-stealth/src/gpu/mod.rs` | Re-exports; `profile_for_entry` |
| `crates/zendriver/tests/gpu_catalogue.rs` | Real-Chrome identity test |

---

### Task 1: Vendor `pci.ids` with its license notice

**Files:**
- Create: `crates/zendriver-stealth/data/pci.ids`
- Create: `crates/zendriver-stealth/data/NOTICE-pci-ids`
- Test: `crates/zendriver-stealth/tests/vendored_data.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `crates/zendriver-stealth/data/pci.ids` at a known path, parseable by Task 2.

- [ ] **Step 1: Download the database**

```bash
curl -fsSL https://pci-ids.ucw.cz/v2.2/pci.ids \
  -o crates/zendriver-stealth/data/pci.ids
head -15 crates/zendriver-stealth/data/pci.ids
```

Expected: a header block naming the file, its version, and its date.

- [ ] **Step 2: Record the license**

Write `crates/zendriver-stealth/data/NOTICE-pci-ids`:

```
The PCI ID Repository (pci.ids)
https://pci-ids.ucw.cz/

Vendored under the terms of the BSD 3-Clause License, at the licensee's
option, or the GNU General Public License v2 or later. This project uses it
under the BSD 3-Clause option.

Used by crates/gpu-catalogue-gen to resolve PCI device IDs for GPU models
named in the catalogue. Device IDs are published identifiers, not measured
values: nothing here contributes a capability number.

Snapshot taken from the v2.2 endpoint. The file's own header carries its
version and date.
```

- [ ] **Step 3: Write the failing test**

```rust
// crates/zendriver-stealth/tests/vendored_data.rs
//! The vendored third-party data files must stay present and attributed.

const PCI_IDS: &str = include_str!("../data/pci.ids");
const NOTICE: &str = include_str!("../data/NOTICE-pci-ids");

#[test]
fn pci_ids_is_vendored_with_a_license_notice() {
    assert!(
        PCI_IDS.len() > 100_000,
        "pci.ids looks truncated ({} bytes)",
        PCI_IDS.len()
    );
    // A vendored file without its notice is a licensing problem, not a nit.
    assert!(NOTICE.contains("BSD 3-Clause"), "notice must name the license");
    assert!(NOTICE.contains("pci-ids.ucw.cz"), "notice must name the source");
}

#[test]
fn pci_ids_carries_the_three_gpu_vendors_the_catalogue_needs() {
    // Vendor lines are unindented `<id>  <name>`; device lines are indented.
    for (id, name) in [("10de", "NVIDIA"), ("1002", "Advanced Micro Devices"), ("8086", "Intel")] {
        assert!(
            PCI_IDS.lines().any(|l| l.starts_with(id) && l.contains(name)),
            "expected a vendor line for {id} ({name})"
        );
    }
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p zendriver-stealth --test vendored_data`
Expected: PASS. If `pci_ids_is_vendored_with_a_license_notice` fails on size, the download was blocked or redirected: check the file's first line before retrying.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/data/pci.ids crates/zendriver-stealth/data/NOTICE-pci-ids crates/zendriver-stealth/tests/vendored_data.rs
git commit -m "chore(stealth): vendor the PCI ID database for the GPU catalogue

Device IDs are published identifiers rather than measured capabilities, so
looking them up is a lookup of facts. Vendored with its notice; the project
takes the BSD 3-Clause option of the dual license."
```

---

### Task 2: Parse `pci.ids` into vendor/device lookups

**Files:**
- Create: `crates/gpu-catalogue-gen/Cargo.toml`
- Create: `crates/gpu-catalogue-gen/src/lib.rs`
- Create: `crates/gpu-catalogue-gen/tests/fixtures/pci-mini.ids`
- Modify: `Cargo.toml` (workspace `members`)

**Interfaces:**
- Consumes: `crates/zendriver-stealth/data/pci.ids` from Task 1.
- Produces:
  - `pub struct PciDevice { pub vendor_id: u32, pub device_id: u32, pub name: String }`
  - `pub fn parse_pci_ids(raw: &str) -> Vec<PciDevice>`

- [ ] **Step 1: Create the crate**

`crates/gpu-catalogue-gen/Cargo.toml`:

```toml
[package]
name = "gpu-catalogue-gen"
version = "0.0.0"
edition.workspace = true
publish = false

[lints]
workspace = true

[dependencies]
reqwest = { workspace = true, features = ["blocking", "json"] }
serde_json.workspace = true
zip.workspace = true
```

Add `"crates/gpu-catalogue-gen"` to the workspace `members` array in the root `Cargo.toml`.

- [ ] **Step 2: Write the fixture**

`crates/gpu-catalogue-gen/tests/fixtures/pci-mini.ids`:

```
# Comment line that must be ignored
10de  NVIDIA Corporation
	2484  GA104 [GeForce RTX 3070]
	2684  AD102 [GeForce RTX 4090]
1002  Advanced Micro Devices, Inc. [AMD/ATI]
	164e  Raphael
8086  Intel Corporation
	3e92  CoffeeLake-S GT2 [UHD Graphics 630]
```

- [ ] **Step 3: Write the failing test**

Append to `crates/gpu-catalogue-gen/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const MINI: &str = include_str!("../tests/fixtures/pci-mini.ids");

    #[test]
    fn parses_vendor_and_device_lines() {
        let devices = parse_pci_ids(MINI);
        assert_eq!(devices.len(), 4, "four indented device lines in the fixture");

        let rtx4090 = devices
            .iter()
            .find(|d| d.device_id == 0x2684)
            .expect("RTX 4090 device line");
        assert_eq!(rtx4090.vendor_id, 0x10de);
        assert_eq!(rtx4090.name, "AD102 [GeForce RTX 4090]");
    }

    #[test]
    fn ignores_comments_and_keeps_devices_under_their_own_vendor() {
        let devices = parse_pci_ids(MINI);
        let raphael = devices.iter().find(|d| d.device_id == 0x164e).unwrap();
        // The bug this guards: carrying the previous vendor across a new
        // unindented line would file Raphael under NVIDIA.
        assert_eq!(raphael.vendor_id, 0x1002);
        assert!(!devices.iter().any(|d| d.name.starts_with('#')));
    }
}
```

- [ ] **Step 4: Run it and watch it fail**

Run: `cargo test -p gpu-catalogue-gen`
Expected: FAIL, `cannot find function parse_pci_ids in this scope`.

- [ ] **Step 5: Implement**

At the top of `crates/gpu-catalogue-gen/src/lib.rs`:

```rust
//! Generator for the GPU device catalogue.
//!
//! Emits `crates/zendriver-stealth/src/gpu/catalogue.rs` from two vendored or
//! pinned sources: driver-reported model names out of the fingerprint corpus,
//! and PCI device IDs out of `pci.ids`. Nothing here invents a value.

/// One `<vendor id>:<device id>` pair and the name `pci.ids` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PciDevice {
    pub vendor_id: u32,
    pub device_id: u32,
    pub name: String,
}

/// Parse the `pci.ids` format: unindented `<hex>  <vendor name>` lines, each
/// followed by tab-indented `<hex>  <device name>` lines. Deeper indentation
/// marks subsystem entries, which carry no device ID of their own and are
/// skipped.
pub fn parse_pci_ids(raw: &str) -> Vec<PciDevice> {
    let mut out = Vec::new();
    let mut vendor_id = None;
    for line in raw.lines() {
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            // Two tabs = subsystem line, which is a (subvendor, subdevice)
            // pair rather than a device ID.
            if line.starts_with("\t\t") {
                continue;
            }
            let Some(vendor_id) = vendor_id else { continue };
            let rest = line.trim_start();
            let Some((id, name)) = rest.split_once("  ") else { continue };
            let Ok(device_id) = u32::from_str_radix(id.trim(), 16) else { continue };
            out.push(PciDevice {
                vendor_id,
                device_id,
                name: name.trim().to_string(),
            });
        } else if let Some((id, _name)) = line.split_once("  ") {
            vendor_id = u32::from_str_radix(id.trim(), 16).ok();
        }
    }
    out
}
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p gpu-catalogue-gen`
Expected: PASS, 2 tests.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/gpu-catalogue-gen
git commit -m "feat(gpu-catalogue-gen): parse the vendored pci.ids database"
```

---

### Task 3: Compose renderer strings the way ANGLE does

**Files:**
- Modify: `crates/gpu-catalogue-gen/src/lib.rs`

**Interfaces:**
- Consumes: `PciDevice` from Task 2.
- Produces:
  - `pub enum Backend { D3d11, Metal }`
  - `pub fn compose_renderer(backend: Backend, vendor: &str, model: &str, device_id: Option<u32>) -> String`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/gpu-catalogue-gen/src/lib.rs`:

```rust
#[test]
fn composes_the_d3d11_string_from_angles_own_format() {
    // Renderer11.cpp:2308-2319 — mDescription, " (", FmtHex(DeviceId), ")",
    // " Direct3D11", " vs_5_0", " ps_5_0". The outer "ANGLE (<vendor>, ..., D3D11)"
    // wrapper comes from the common display layer.
    let got = compose_renderer(Backend::D3d11, "NVIDIA", "NVIDIA GeForce RTX 4090", Some(0x2684));
    assert_eq!(
        got,
        "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 (0x00002684) Direct3D11 vs_5_0 ps_5_0, D3D11)"
    );
}

#[test]
fn composes_the_metal_string_with_no_device_id() {
    // DisplayMtl.mm:188-201 — "ANGLE Metal Renderer" + ": " + MTLDevice.name.
    // The trailing field is the literal getVersionString returns for WebGL
    // contexts (:216), not a version anyone should synthesize.
    let got = compose_renderer(Backend::Metal, "Apple", "Apple M4 Pro", None);
    assert_eq!(
        got,
        "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Pro, Unspecified Version)"
    );
}

#[test]
fn the_composed_strings_match_the_measured_captures_byte_for_byte() {
    // The strongest available check: two captures exist, so composition must
    // reproduce them exactly rather than merely look plausible.
    assert_eq!(
        compose_renderer(Backend::D3d11, "AMD", "AMD Radeon(TM) Graphics", Some(0x164E)),
        "ANGLE (AMD, AMD Radeon(TM) Graphics (0x0000164E) Direct3D11 vs_5_0 ps_5_0, D3D11)"
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p gpu-catalogue-gen composes`
Expected: FAIL, `cannot find function compose_renderer`.

- [ ] **Step 3: Implement**

```rust
/// Which ANGLE backend composes a renderer string, and therefore which format
/// it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    D3d11,
    Metal,
}

/// Build the renderer string ANGLE would report for a device.
///
/// Reconstructed from the source that composes it rather than copied from
/// samples, because the format has changed under the corpus: strings collected
/// on older Chrome omit the device ID current ANGLE always appends. Composing
/// means a future format change is a small, detectable fix here instead of
/// silent rot in the data.
pub fn compose_renderer(
    backend: Backend,
    vendor: &str,
    model: &str,
    device_id: Option<u32>,
) -> String {
    match backend {
        // Renderer11.cpp:2308-2319.
        Backend::D3d11 => {
            let id = device_id.unwrap_or(0);
            format!("ANGLE ({vendor}, {model} (0x{id:08X}) Direct3D11 vs_5_0 ps_5_0, D3D11)")
        }
        // DisplayMtl.mm:188-201, with getVersionString's WebGL literal at :216.
        Backend::Metal => {
            format!("ANGLE ({vendor}, ANGLE Metal Renderer: {model}, Unspecified Version)")
        }
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p gpu-catalogue-gen`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/gpu-catalogue-gen/src/lib.rs
git commit -m "feat(gpu-catalogue-gen): compose renderer strings from ANGLE's format"
```

---

### Task 4: Extract model names from a pinned corpus commit

**Files:**
- Modify: `crates/gpu-catalogue-gen/src/lib.rs`
- Create: `crates/gpu-catalogue-gen/tests/fixtures/corpus-mini.json`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub fn extract_models(network_json: &str) -> Vec<(Backend, String, String)>` returning `(backend, vendor, model)`.

- [ ] **Step 1: Write the fixture**

`crates/gpu-catalogue-gen/tests/fixtures/corpus-mini.json` — the shape
`zendriver-fingerprints` already parses, with its `*STRINGIFIED*` values:

```json
{
  "nodes": [
    {
      "name": "videoCard",
      "parentNames": ["userAgent"],
      "conditionalProbabilities": {
        "deeper": {
          "ua-win": {
            "*STRINGIFIED*{\"renderer\":\"ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)\",\"vendor\":\"Google Inc. (NVIDIA)\"}": 1.0,
            "*STRINGIFIED*{\"renderer\":\"ANGLE (AMD, AMD Radeon RX 6700 XT Direct3D11 vs_5_0 ps_5_0, D3D11)\",\"vendor\":\"Google Inc. (AMD)\"}": 1.0
          },
          "ua-mac": {
            "*STRINGIFIED*{\"renderer\":\"ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)\",\"vendor\":\"Google Inc. (Apple)\"}": 1.0
          },
          "ua-linux": {
            "*STRINGIFIED*{\"renderer\":\"ANGLE (Mesa, llvmpipe (LLVM 15.0.7 256 bits), OpenGL 4.5)\",\"vendor\":\"Google Inc. (Mesa)\"}": 1.0
          }
        }
      }
    }
  ]
}
```

- [ ] **Step 2: Write the failing test**

```rust
#[test]
fn extracts_models_from_the_corpus_and_drops_out_of_scope_backends() {
    const MINI: &str = include_str!("../tests/fixtures/corpus-mini.json");
    let mut models = extract_models(MINI);
    models.sort();

    assert_eq!(
        models,
        vec![
            (Backend::D3d11, "AMD".to_string(), "AMD Radeon RX 6700 XT".to_string()),
            (Backend::D3d11, "NVIDIA".to_string(), "NVIDIA GeForce RTX 3060".to_string()),
            (Backend::Metal, "Apple".to_string(), "Apple M2".to_string()),
        ],
        "the Mesa/OpenGL entry is out of scope for v1 and must be dropped"
    );
}

#[test]
fn strips_the_device_id_older_corpora_may_or_may_not_carry() {
    // Only the model name is taken; the string itself is recomposed. A corpus
    // entry collected on a Chrome that appended the ID must not smuggle it
    // into the model text.
    let with_id = r#"{"nodes":[{"name":"videoCard","conditionalProbabilities":{"deeper":{"x":{
      "*STRINGIFIED*{\"renderer\":\"ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 (0x00002684) Direct3D11 vs_5_0 ps_5_0, D3D11)\",\"vendor\":\"\"}": 1.0}}}}]}"#;
    assert_eq!(
        extract_models(with_id),
        vec![(Backend::D3d11, "NVIDIA".to_string(), "NVIDIA GeForce RTX 4090".to_string())]
    );
}
```

- [ ] **Step 3: Run it and watch it fail**

Run: `cargo test -p gpu-catalogue-gen extracts`
Expected: FAIL, `cannot find function extract_models`.

- [ ] **Step 4: Implement**

```rust
/// Pull `(backend, vendor, model)` triples out of the fingerprint corpus's
/// `videoCard` node.
///
/// Only the model name is taken. The renderer string is recomposed by
/// [`compose_renderer`], because the corpus predates the device ID current
/// ANGLE appends and a copied string would be stamped with whatever Chrome
/// collected it.
pub fn extract_models(network_json: &str) -> Vec<(Backend, String, String)> {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(network_json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for node in root["nodes"].as_array().into_iter().flatten() {
        if node["name"] != "videoCard" {
            continue;
        }
        let deeper = &node["conditionalProbabilities"]["deeper"];
        for bucket in deeper.as_object().into_iter().flatten().map(|(_, v)| v) {
            for key in bucket.as_object().into_iter().flatten().map(|(k, _)| k) {
                let json = key.strip_prefix("*STRINGIFIED*").unwrap_or(key);
                let Ok(card) = serde_json::from_str::<serde_json::Value>(json) else { continue };
                let Some(renderer) = card["renderer"].as_str() else { continue };
                let Some((backend, vendor, model)) = split_renderer(renderer) else { continue };
                if seen.insert((backend, vendor.clone(), model.clone())) {
                    out.push((backend, vendor, model));
                }
            }
        }
    }
    out
}

/// Take a renderer string apart into the three fields the catalogue keeps.
/// Returns `None` for any backend outside v1's scope.
fn split_renderer(renderer: &str) -> Option<(Backend, String, String)> {
    let inner = renderer.strip_prefix("ANGLE (")?.strip_suffix(')')?;
    let (vendor, rest) = inner.split_once(", ")?;

    if let Some(model) = rest.strip_prefix("ANGLE Metal Renderer: ") {
        // Everything before the trailing ", Unspecified Version" field.
        let model = model.split_once(", ").map_or(model, |(before, _)| before);
        return Some((Backend::Metal, vendor.to_string(), model.to_string()));
    }
    if rest.ends_with(", D3D11") {
        let body = rest.trim_end_matches(", D3D11");
        let model = body.split(" Direct3D11").next()?.trim();
        // Strip a device ID if this entry came from a Chrome that appended one.
        let model = model
            .rsplit_once(" (0x")
            .map_or(model, |(before, _)| before)
            .trim();
        return Some((Backend::D3d11, vendor.to_string(), model.to_string()));
    }
    None
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p gpu-catalogue-gen`
Expected: PASS, 7 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/gpu-catalogue-gen
git commit -m "feat(gpu-catalogue-gen): extract model names from the fingerprint corpus"
```

---

### Task 5: Emit the catalogue table

**Files:**
- Modify: `crates/gpu-catalogue-gen/src/lib.rs`
- Create: `crates/gpu-catalogue-gen/src/main.rs`
- Create: `crates/zendriver-stealth/src/gpu/catalogue.rs` (generated)
- Modify: `crates/zendriver-stealth/src/gpu/mod.rs`

**Interfaces:**
- Consumes: everything from Tasks 2 to 4.
- Produces, in `zendriver_stealth::gpu::catalogue`:
  - `pub(crate) struct CatalogueEntry { pub model: &'static str, pub vendor: &'static str, pub device_id: Option<u32>, pub tier: Tier, pub generation: Option<Generation> }`
  - `pub(crate) const CATALOGUE: &[CatalogueEntry]`
  - `pub(crate) enum Generation { Ampere, Lovelace, Rdna2, MetalConstant }`
  - `pub(crate) struct FeatureSet { pub generation: Option<Generation>, pub tier: Tier, pub features: &'static [&'static str], pub provenance: FeatureProvenance }`
  - `pub(crate) enum FeatureProvenance { Probed { chrome: &'static str, device: &'static str }, Estimated { carried_from: Tier } }`

- [ ] **Step 1: Write the pinned entry point**

`crates/gpu-catalogue-gen/src/main.rs`:

```rust
//! Regenerate the GPU device catalogue.
//!
//! Run from the workspace root: `cargo run -p gpu-catalogue-gen`.

/// Pinned so the catalogue cannot change without this constant changing.
/// The runtime cache in `zendriver-fingerprints` tracks `master`, which is
/// fine for a cache and unacceptable for a generator.
const CORPUS_COMMIT: &str = "REPLACE_WITH_RESOLVED_SHA";
const CORPUS_URL: &str = "https://raw.githubusercontent.com/apify/fingerprint-suite/\
                          REPLACE_WITH_RESOLVED_SHA/packages/fingerprint-generator/src/\
                          data_files/fingerprint-network-definition.zip";
const PCI_IDS: &str = include_str!("../../zendriver-stealth/data/pci.ids");
const OUT: &str = "crates/zendriver-stealth/src/gpu/catalogue.rs";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let zipped = reqwest::blocking::get(CORPUS_URL)?.error_for_status()?.bytes()?;
    let network = gpu_catalogue_gen::unzip_network_json(&zipped)?;
    let entries = gpu_catalogue_gen::build_catalogue(&network, PCI_IDS);
    println!("emitting {} catalogue entries to {OUT}", entries.len());
    std::fs::write(OUT, gpu_catalogue_gen::emit_rust(CORPUS_COMMIT, &entries))?;
    Ok(())
}
```

Resolve the pin before running:

```bash
gh api repos/apify/fingerprint-suite/commits/master --jq '.sha'
```

Paste the returned SHA into both constants, replacing `REPLACE_WITH_RESOLVED_SHA`.

- [ ] **Step 2: Write the failing test for the emitter**

```rust
#[test]
fn emitted_source_carries_a_do_not_edit_header_and_the_pin() {
    let entries = vec![CatalogueRow {
        model: "NVIDIA GeForce RTX 3070".into(),
        vendor: "NVIDIA".into(),
        device_id: Some(0x2484),
        tier: "D3d11Fl11Nvidia".into(),
        generation: Some("Ampere".into()),
    }];
    let src = emit_rust("abc123", &entries);
    assert!(src.contains("DO NOT EDIT"), "generated files must say so");
    assert!(src.contains("abc123"), "the corpus pin must be recorded in the output");
    assert!(src.contains("NVIDIA GeForce RTX 3070"));
    assert!(src.contains("Some(0x2484)"));
    assert!(src.contains("Tier::D3d11Fl11Nvidia"));
}

#[test]
fn a_metal_row_emits_no_device_id_and_no_generation() {
    let entries = vec![CatalogueRow {
        model: "Apple M2".into(),
        vendor: "Apple".into(),
        device_id: None,
        tier: "MetalMacos".into(),
        generation: None,
    }];
    let src = emit_rust("abc123", &entries);
    assert!(src.contains("device_id: None"));
    assert!(src.contains("generation: None"));
}
```

- [ ] **Step 3: Run it and watch it fail**

Run: `cargo test -p gpu-catalogue-gen emitted`
Expected: FAIL, `cannot find type CatalogueRow`.

- [ ] **Step 4: Implement the emitter**

```rust
/// One row on its way to the generated file. Strings rather than the
/// stealth crate's own enums, because the generator does not depend on it.
#[derive(Debug, Clone)]
pub struct CatalogueRow {
    pub model: String,
    pub vendor: String,
    pub device_id: Option<u32>,
    pub tier: String,
    pub generation: Option<String>,
}

/// Render the committed catalogue module.
pub fn emit_rust(corpus_commit: &str, rows: &[CatalogueRow]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "// GENERATED by `cargo run -p gpu-catalogue-gen`. DO NOT EDIT.\n\
         //\n\
         // Model names come from the fingerprint corpus pinned at {corpus_commit}.\n\
         // Device IDs come from the vendored `data/pci.ids`. Renderer strings are\n\
         // composed from ANGLE's own format at read time, never copied.\n\n\
         use super::types::Tier;\n\n"
    ));
    out.push_str("pub(crate) const CATALOGUE: &[CatalogueEntry] = &[\n");
    for row in rows {
        let device_id = row
            .device_id
            .map_or_else(|| "None".to_string(), |id| format!("Some(0x{id:04X})"));
        let generation = row
            .generation
            .as_ref()
            .map_or_else(|| "None".to_string(), |g| format!("Some(Generation::{g})"));
        out.push_str(&format!(
            "    CatalogueEntry {{ model: {:?}, vendor: {:?}, device_id: {device_id}, \
             tier: Tier::{}, generation: {generation} }},\n",
            row.model, row.vendor, row.tier
        ));
    }
    out.push_str("];\n");
    out
}
```

Write `build_catalogue` and `unzip_network_json` alongside it: `unzip_network_json` reads the single JSON member out of the zip archive, and `build_catalogue` joins `extract_models` output against `parse_pci_ids` by matching a model name against a `pci.ids` device name, assigning `tier` from the backend and vendor (`Backend::D3d11` plus vendor `NVIDIA` gives `D3d11Fl11Nvidia`, any other D3D11 vendor gives `D3d11Fl11`, `Backend::Metal` gives `MetalMacos`).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p gpu-catalogue-gen`
Expected: PASS, 9 tests.

- [ ] **Step 6: Generate and inspect**

```bash
cargo run -p gpu-catalogue-gen
cargo fmt -p zendriver-stealth
head -30 crates/zendriver-stealth/src/gpu/catalogue.rs
```

Expected: a `DO NOT EDIT` header naming the pinned commit, then `CATALOGUE` rows. Add `mod catalogue;` to `crates/zendriver-stealth/src/gpu/mod.rs`.

- [ ] **Step 7: Commit**

```bash
git add crates/gpu-catalogue-gen crates/zendriver-stealth/src/gpu
git commit -m "feat(gpu-catalogue-gen): emit the committed device catalogue"
```

---

### Task 6: Feature sets keyed by generation, with provenance

**Files:**
- Modify: `crates/gpu-catalogue-gen/src/lib.rs`
- Modify: `crates/zendriver-stealth/src/gpu/catalogue.rs` (regenerated)

**Interfaces:**
- Consumes: `Tier`, `Generation` from Task 5.
- Produces: `pub(crate) const FEATURE_SETS: &[FeatureSet]`, and
  `pub(crate) fn features_for(tier: Tier, generation: Option<Generation>) -> &'static FeatureSet`.

- [ ] **Step 1: Write the failing test**

In `crates/zendriver-stealth/src/gpu/catalogue.rs`'s sibling test module (create `crates/zendriver-stealth/src/gpu/catalogue_tests.rs` and `#[cfg(test)] mod catalogue_tests;` in `mod.rs`, so the generated file stays generated):

```rust
use super::catalogue::*;
use super::types::Tier;

/// The five names ANGLE/Dawn gate on shader model and driver rather than on
/// the backend. An estimated entry omits them; a probed one may carry them.
const SILICON_GATED: &[&str] = &[
    "shader-f16",
    "subgroups",
    "dual-source-blending",
    "clip-distances",
    "primitive-index",
];

#[test]
fn an_estimated_feature_set_omits_every_silicon_gated_name() {
    for set in FEATURE_SETS {
        if matches!(set.provenance, FeatureProvenance::Estimated { .. }) {
            for gated in SILICON_GATED {
                assert!(
                    !set.features.contains(gated),
                    "estimated set for {:?} claims {gated}, which nothing measured",
                    set.generation
                );
            }
        }
    }
}

#[test]
fn every_probed_set_carries_the_browser_level_features() {
    // Present because a given Chrome implements that spec revision, not
    // because of the card. Their absence would describe no real browser.
    for set in FEATURE_SETS {
        if matches!(set.provenance, FeatureProvenance::Probed { .. }) {
            for browser_level in ["core-features-and-limits", "texture-formats-tier1"] {
                assert!(
                    set.features.contains(&browser_level),
                    "probed set for {:?} is missing {browser_level}",
                    set.generation
                );
            }
        }
    }
}

#[test]
fn the_probed_sets_match_the_captures_they_came_from() {
    // Lovelace and RDNA2 were both measured, on the same machine and Chrome,
    // and reported identical 19-feature sets.
    let lovelace = features_for(Tier::D3d11Fl11Nvidia, Some(Generation::Lovelace));
    let rdna2 = features_for(Tier::D3d11Fl11, Some(Generation::Rdna2));
    assert_eq!(lovelace.features, rdna2.features);
    assert_eq!(lovelace.features.len(), 19);
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p zendriver-stealth catalogue`
Expected: FAIL, `FEATURE_SETS` not found.

- [ ] **Step 3: Extend the emitter**

Emit `FEATURE_SETS` from the committed captures: for each capture with an
adapter, one `Probed` set carrying its measured feature list, its Chrome
version, and its device. For each generation named in `CATALOGUE` with no
capture, one `Estimated` set equal to its tier's probed features minus
`SILICON_GATED`. Emit `features_for` as a linear scan over `FEATURE_SETS`
matching on `(tier, generation)`, falling back to the tier's own set when the
generation is `None`.

- [ ] **Step 4: Regenerate and run**

```bash
cargo run -p gpu-catalogue-gen && cargo fmt -p zendriver-stealth
cargo test -p zendriver-stealth catalogue
```

Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/gpu-catalogue-gen crates/zendriver-stealth/src/gpu
git commit -m "feat(stealth): key catalogue feature sets by generation with provenance"
```

---

### Task 7: The four selection strategies

**Files:**
- Create: `crates/zendriver-stealth/src/gpu/device_select.rs`
- Modify: `crates/zendriver-stealth/src/gpu/mod.rs`

**Interfaces:**
- Consumes: `CATALOGUE`, `features_for` from Tasks 5 and 6.
- Produces:
  - `pub struct GpuDevice(&'static CatalogueEntry)`
  - `pub fn by_name(query: &str) -> Result<GpuDevice, DeviceLookupError>`
  - `pub fn from_seed(seed: Seed, platform: Platform) -> Option<GpuDevice>`
  - `pub fn by_share(seed: Seed, platform: Platform) -> Option<GpuDevice>`
  - `pub fn nearest_to_host() -> Option<GpuDevice>`
  - `pub enum DeviceLookupError { NotFound(String), Ambiguous(Vec<&'static str>), EstimatedNotAllowed(&'static str) }`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn by_name_matches_case_insensitively_on_a_substring() {
    let device = by_name("rtx 3070").expect("catalogued");
    assert!(device.model().contains("RTX 3070"));
}

#[test]
fn by_name_refuses_an_ambiguous_query_rather_than_guessing() {
    // "rtx 30" matches the whole Ampere line. Picking one silently would put
    // an arbitrary card behind an ambiguous request.
    match by_name("rtx 30") {
        Err(DeviceLookupError::Ambiguous(matches)) => {
            assert!(matches.len() > 1, "expected several candidates, got {matches:?}");
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

#[test]
fn by_name_refuses_an_estimated_entry_by_default() {
    // Probed-only is the default; an estimated feature set is opt-in.
    let estimated = CATALOGUE
        .iter()
        .find(|e| {
            matches!(
                features_for(e.tier, e.generation).provenance,
                FeatureProvenance::Estimated { .. }
            )
        })
        .expect("at least one estimated entry ships");
    assert!(matches!(
        by_name(estimated.model),
        Err(DeviceLookupError::EstimatedNotAllowed(_))
    ));
}

#[test]
fn the_same_seed_always_draws_the_same_device() {
    let a = from_seed(Seed(42), Platform::Win32);
    let b = from_seed(Seed(42), Platform::Win32);
    assert_eq!(a.map(|d| d.model()), b.map(|d| d.model()));
}

#[test]
fn seeded_selection_respects_the_platform() {
    for seed in 0..50 {
        if let Some(device) = from_seed(Seed(seed), Platform::MacIntel) {
            assert_eq!(device.tier(), Tier::MetalMacos, "seed {seed} crossed platforms");
        }
    }
}
```

- [ ] **Step 2: Run and watch them fail**

Run: `cargo test -p zendriver-stealth device_select`
Expected: FAIL, module not found.

- [ ] **Step 3: Implement**

`by_name` lowercases both sides and collects every entry whose model contains
the query, returning `NotFound` on zero, `Ambiguous` on more than one, and
`EstimatedNotAllowed` when the single match resolves to an `Estimated` feature
set. `from_seed` filters `CATALOGUE` to the platform's tiers (via the existing
`invariants::platform_skew` returning `None`), then indexes with
`seed.0 % candidates.len()`. `by_share` does the same over a weighted table
(Task 8). `nearest_to_host` probes the host renderer and picks the entry whose
composed string shares the most leading tokens, and is never called by
default.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p zendriver-stealth device_select`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/src/gpu
git commit -m "feat(stealth): add catalogue selection strategies"
```

---

### Task 8: Share-weighted selection from a dated snapshot

**Files:**
- Create: `crates/zendriver-stealth/data/gpu-share-2026-07.json`
- Modify: `crates/zendriver-stealth/src/gpu/device_select.rs`

**Interfaces:**
- Consumes: `CATALOGUE`, `from_seed` from Task 7.
- Produces: `pub fn by_share(seed: Seed, platform: Platform) -> Option<GpuDevice>`.

- [ ] **Step 1: Write the snapshot**

A dated JSON file mapping catalogue model names to shares, derived from the
Steam Hardware Survey. The date is in the filename so staleness is visible:

```json
{
  "source": "Steam Hardware Survey",
  "captured": "2026-07",
  "shares": {
    "NVIDIA GeForce RTX 3060": 0.0412,
    "NVIDIA GeForce RTX 4090": 0.0091
  }
}
```

- [ ] **Step 2: Write the failing test**

```rust
#[test]
fn every_share_entry_names_a_catalogued_device() {
    // A distribution naming a device the catalogue lacks would silently
    // reweight everything else.
    let snapshot: serde_json::Value = serde_json::from_str(SHARE_SNAPSHOT).unwrap();
    for model in snapshot["shares"].as_object().unwrap().keys() {
        assert!(
            CATALOGUE.iter().any(|e| e.model == model),
            "share data names {model}, which is not in the catalogue"
        );
    }
}

#[test]
fn share_weighted_selection_is_deterministic_per_seed() {
    let a = by_share(Seed(7), Platform::Win32);
    let b = by_share(Seed(7), Platform::Win32);
    assert_eq!(a.map(|d| d.model()), b.map(|d| d.model()));
}
```

- [ ] **Step 3: Run and watch them fail**

Run: `cargo test -p zendriver-stealth share`
Expected: FAIL, `SHARE_SNAPSHOT` not found.

- [ ] **Step 4: Implement**

`include_str!` the snapshot, parse it once into a cumulative-weight vector
filtered to the platform, and index with `seed.0` scaled across the total.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p zendriver-stealth share`
Expected: PASS, 2 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/zendriver-stealth/data crates/zendriver-stealth/src/gpu
git commit -m "feat(stealth): draw catalogue devices by market share"
```

---

### Task 9: Coherence rules over the whole catalogue

**Files:**
- Modify: `crates/zendriver-stealth/src/gpu/invariants.rs`

**Interfaces:**
- Consumes: `CATALOGUE`, `compose_renderer` behavior, `adapter_for_renderer`.
- Produces: no new public API; three sweep tests.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn every_entry_round_trips_to_its_own_tier() {
    // The catalogue may only widen identity, never capability. An entry whose
    // composed string resolves to a different tier would serve one device's
    // name over another's numbers.
    for entry in CATALOGUE {
        let renderer = entry.renderer_string();
        assert_eq!(
            crate::gpu::devices::device_for_renderer(&renderer).map(|d| d.tier),
            Some(entry.tier),
            "{renderer} resolved the wrong tier"
        );
    }
}

#[test]
fn every_entry_agrees_with_the_architecture_derived_from_its_string() {
    for entry in CATALOGUE {
        let renderer = entry.renderer_string();
        let derived = crate::gpu::devices::adapter_for_renderer(&renderer);
        assert_eq!(
            derived.architecture,
            entry.expected_architecture(),
            "{renderer} derives {:?} but the entry claims otherwise",
            derived.architecture
        );
    }
}

#[test]
fn no_entry_is_platform_skewed_against_its_own_tier() {
    for entry in CATALOGUE {
        let platform = entry.platform();
        assert!(
            platform_skew(platform, entry.tier).is_none(),
            "{} claims {platform:?} over {:?}",
            entry.model,
            entry.tier
        );
    }
}
```

- [ ] **Step 2: Run and watch them fail**

Run: `cargo test -p zendriver-stealth invariants`
Expected: FAIL on missing `renderer_string` / `expected_architecture` / `platform`.

- [ ] **Step 3: Implement the entry accessors**

Add to `CatalogueEntry`: `renderer_string()` calling the same composition
`gpu-catalogue-gen` uses (share the format by keeping composition in the
stealth crate and having the generator emit only fields, so the two cannot
drift), `expected_architecture()` returning the generation's Dawn token or
`""`, and `platform()` mapping tier to `Platform`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p zendriver-stealth`
Expected: PASS, whole crate.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/src/gpu
git commit -m "test(stealth): enforce catalogue coherence over every entry"
```

---

### Task 10: Wire the catalogue into `Persona`

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/mod.rs`
- Modify: `crates/zendriver-stealth/src/patches.rs`

**Interfaces:**
- Consumes: `GpuDevice` from Task 7.
- Produces: `Persona.gpu_device: Option<GpuDevice>`, resolved ahead of
  `Persona.gpu` and below `WebglSpec`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_catalogued_device_supplies_identity_and_its_tier_supplies_values() {
    let device = by_name("rtx 4090").expect("catalogued");
    let persona = Persona {
        platform: Some(Platform::Win32),
        gpu_device: Some(device),
        ..Persona::default()
    };
    let js = crate::patches::build(&persona);
    assert!(js.contains("NVIDIA GeForce RTX 4090 (0x00002684)"));
    // 4095, not 4096: the NVIDIA tier, selected by the entry rather than guessed.
    assert!(js.contains("4095"));
}

#[test]
fn an_explicit_webgl_spec_still_overrides_a_catalogued_device() {
    let device = by_name("rtx 4090").expect("catalogued");
    let persona = Persona {
        platform: Some(Platform::Win32),
        gpu_device: Some(device),
        webgl: Some(WebglSpec {
            unmasked_renderer: Some("ANGLE (Pinned)".into()),
            ..Default::default()
        }),
        ..Persona::default()
    };
    let js = crate::patches::build(&persona);
    assert!(js.contains("ANGLE (Pinned)"), "the finest layer must win");
}
```

- [ ] **Step 2: Run and watch them fail**

Run: `cargo test -p zendriver-stealth persona`
Expected: FAIL, no field `gpu_device`.

- [ ] **Step 3: Implement**

Add the field with rustdoc stating the precedence, and resolve it in
`push_webgl` / `push_webgpu` between the tier lookup and `Persona::gpu`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p zendriver-stealth`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/src
git commit -m "feat(stealth): resolve a catalogued device on Persona"
```

---

### Task 11: Real-Chrome verification

**Files:**
- Create: `crates/zendriver/tests/gpu_catalogue.rs`
- Modify: `.github/workflows/ci.yml:242`

**Interfaces:**
- Consumes: everything above.
- Produces: one ignored-by-default real-browser test, run by the
  `test-gpu-coherence` job.

- [ ] **Step 1: Write the test**

```rust
//! A catalogued identity must reach the page intact, on all three surfaces at
//! once: the renderer string, the WebGPU architecture, and the tier's values.

#[tokio::test]
#[ignore = "requires a real Chrome"]
async fn a_catalogued_device_reaches_the_page_coherently() {
    let device = zendriver::GpuDevice::by_name("rtx 4090").expect("catalogued");
    let persona = Persona::builder()
        .platform(Platform::Win32)
        .gpu_device(device)
        .build();
    let browser = Browser::builder().persona(persona).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto("about:blank").await.unwrap();

    // MUST be evaluate_main: `evaluate` runs in an isolated world the patch
    // does not reach, so it reads the unpatched surface and proves nothing.
    let got: serde_json::Value = tab
        .evaluate_main(
            r#"(() => {
              const gl = document.createElement('canvas').getContext('webgl2');
              const dbg = gl.getExtension('WEBGL_debug_renderer_info');
              return JSON.stringify({
                renderer: gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL),
                vectors: gl.getParameter(gl.MAX_VERTEX_UNIFORM_VECTORS),
              });
            })()"#,
        )
        .await
        .unwrap();

    assert!(got["renderer"].as_str().unwrap().contains("RTX 4090 (0x00002684)"));
    assert_eq!(got["vectors"].as_u64(), Some(4095), "the NVIDIA tier's value");
    browser.close().await.ok();
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p zendriver --features integration-tests --test gpu_catalogue -- --ignored`
Expected: PASS.

- [ ] **Step 3: Add it to the blocking CI job**

Extend the filter at `.github/workflows/ci.yml:242` to
`-E 'binary(gpu_profile) or binary(gpu_backend) or binary(gpu_catalogue)'`.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver/tests/gpu_catalogue.rs .github/workflows/ci.yml
git commit -m "test(zendriver): verify a catalogued device in a real browser"
```

---

### Task 12: Regeneration check, docs, ledger, and final gates

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `README.md`, `crates/zendriver-mcp/README.md`
- Modify: `docs/book/src/fingerprint.md`
- Modify: `crates/zendriver-mcp/mcp-coverage-ledger.toml`
- Modify: `crates/zendriver-mcp/public-api-baseline.txt`

- [ ] **Step 1: Add the regeneration diff**

Beside the existing `gpu-tier-gen` check near `.github/workflows/ci.yml:32`:

```yaml
          cargo run -p gpu-catalogue-gen
          git diff --exit-code crates/zendriver-stealth/src/gpu/catalogue.rs
```

- [ ] **Step 2: Decide MCP exposure**

Add a `browser_open` option selecting a catalogued device by name, or record
the deliberate omission in the ledger:

```toml
[[api]]
item = "zendriver::GpuDevice"
covered = "browser_open"
```

If a tool changed, regenerate the snapshots:

```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo insta accept --all
```

- [ ] **Step 3: Update the three doc surfaces**

The book's `fingerprint.md` gains a catalogue section covering the four
selection strategies, the probed-only default, and the v1 backend scope.
Rustdoc on every new public item. README feature matrix and tool count if a
tool was added.

- [ ] **Step 4: Regenerate the public-API baseline**

```bash
cargo +nightly public-api -p zendriver --all-features > crates/zendriver-mcp/public-api-baseline.txt
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked
```

- [ ] **Step 5: Run every gate**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings
cargo test --workspace
mdbook build docs/book
```

Expected: all pass.

- [ ] **Step 6: Commit and open the PR**

```bash
git add -A
git commit -m "docs: document the GPU device catalogue"
git push -u origin feat/gpu-device-catalogue
```

---

## Self-Review

**Spec coverage.** Problem statement, Tasks 7 and 8. Sourcing, Tasks 1 to 4.
Data model, Tasks 5 and 6. Selection, Tasks 7 and 8. Coherence, Task 9.
Testing, Tasks 9 and 11. Scope limits (D3D11 + Metal, no Linux, no Intel Mac)
are enforced by `split_renderer` returning `None` in Task 4 and asserted in
its test.

**Known gaps the implementer must resolve, not paper over:**

1. **Task 5's `build_catalogue` joins model names against `pci.ids` names, and
   they do not match textually.** `pci.ids` says `GA104 [GeForce RTX 3070]`
   where the driver says `NVIDIA GeForce RTX 3070`. The join needs a
   normalization step: strip the vendor prefix from the driver name, then look
   for it inside the bracketed part of the `pci.ids` name. Entries that fail to
   join must be **dropped with a logged count**, never given a fabricated
   device ID. Expect a meaningful drop rate and report it.
2. **Task 8's share snapshot needs real numbers.** The two rows shown are
   placeholders for format only. Pull the actual Steam Hardware Survey figures
   when implementing, and if they cannot be obtained, ship `by_share` returning
   `None` with a documented reason rather than inventing a distribution.
3. **Generation assignment is unspecified above.** Deriving `Ampere` vs
   `Lovelace` from a model name needs a table; take the tokens from Dawn's
   `gpu_info.json` as `adapter_for_renderer` already does, and reuse that
   function rather than writing a second mapping.

**Type consistency.** `CatalogueEntry` fields are consistent across Tasks 5, 7,
9, and 10. `Backend` (generator-side) and `Tier` (stealth-side) stay distinct
on purpose: the generator does not depend on the stealth crate, which is why
`CatalogueRow.tier` is a `String` that Task 5 renders as `Tier::{}`.
