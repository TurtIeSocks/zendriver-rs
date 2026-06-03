# Generative Bayesian Network Extraction — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `zendriver-fingerprints`' minimal 3-node Bayesian network with the real browserforge/apify fingerprint network — fetched on first use and cached — producing coherent desktop personas.

**Architecture:** A generic Bayesian sampler walks the canonical `conditionalProbabilities` `deeper`/`skip` tree over `serde_json::Value`, seeded by a deterministic Mulberry32 PRNG. The root `userAgent` node is restricted to non-mobile UAs so the whole persona is desktop-coherent in one forward pass. Sampled attributes (`*STRINGIFIED*`-decoded) map onto the existing `Persona` subset. The 705 KB network is downloaded on first use (default = apify's monthly file, overridable via `ZENDRIVER_FP_NETWORK_URL`) and cached, reusing the pool's cache pattern locally.

**Tech Stack:** Rust (edition 2024), `serde_json`, `zip` (deflate), `reqwest`, `dirs`, `thiserror`; tests use `tokio` + `wiremock` + `tempfile`.

**Spec:** `docs/superpowers/specs/2026-06-02-generative-bayesian-network-design.md`
**Follow-ups already filed:** #38 (header coherence), #39 (geoip locale).

**Working dir:** already an isolated worktree (`.claude/worktrees/friendly-mahavira-a66a1a`).

**Module layout (all under `crates/zendriver-fingerprints/src/generative/`):**
- `mod.rs` — `Generator`, `Node`, `GenError`, `DEFAULT_NETWORK_URL`, constructors, `generate`, sampler internals (Mulberry32, `walk_cpt`, `leaf_distribution`, `weighted_pick`, `topo_order`), `TEST_NETWORK_ZIP`.
- `mapping.rs` — `destringify_scalar`, `destringify_json`, `is_desktop_ua`, `map_platform`, `persona_from_assignment`.
- `download.rs` — `cache_path`, `fetch_or_cached_bytes`.
- `fixtures/mini-network.json`, `fixtures/mini-network.zip` — test fixtures.

---

## Task 1: Cargo wiring + test fixtures

**Files:**
- Modify: `crates/zendriver-fingerprints/Cargo.toml`
- Create: `crates/zendriver-fingerprints/src/generative/fixtures/mini-network.json`
- Create: `crates/zendriver-fingerprints/src/generative/fixtures/mini-network.zip`

- [ ] **Step 1: Update `Cargo.toml` features + deps**

Replace the `[features]` and `[dependencies]`/dev-deps so the file reads:

```toml
[features]
default = []
pool = ["dep:reqwest", "dep:dirs"]
generative = ["dep:reqwest", "dep:dirs", "dep:zip"]

[dependencies]
zendriver-stealth.workspace = true
serde.workspace             = true
serde_json.workspace        = true
thiserror.workspace         = true
tracing.workspace           = true
fastrand                    = "2"
reqwest  = { workspace = true, optional = true }
dirs     = { workspace = true, optional = true }
zip      = { workspace = true, optional = true }

[dev-dependencies]
tokio    = { workspace = true }
wiremock = { workspace = true }
tempfile = { workspace = true }
```

(Leave the existing `[[example]]` block untouched.)

- [ ] **Step 2: Create the JSON fixture**

Create `crates/zendriver-fingerprints/src/generative/fixtures/mini-network.json` with EXACTLY this content (3 desktop UAs + 1 mobile UA; real canonical schema — `parentNames`, `conditionalProbabilities` `deeper`/`skip`, `*STRINGIFIED*` values):

```json
{
  "nodes": [
    {
      "name": "userAgent",
      "parentNames": [],
      "conditionalProbabilities": {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": 0.5,
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": 0.3,
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": 0.15,
        "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Mobile Safari/537.36": 0.05
      }
    },
    {
      "name": "platform",
      "parentNames": ["userAgent"],
      "conditionalProbabilities": {
        "deeper": {
          "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "Win32": 1.0 },
          "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "MacIntel": 1.0 },
          "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "Linux x86_64": 1.0 },
          "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Mobile Safari/537.36": { "Linux armv8l": 1.0 }
        },
        "skip": null
      }
    },
    {
      "name": "deviceMemory",
      "parentNames": ["userAgent"],
      "conditionalProbabilities": {
        "deeper": {
          "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*16": 0.6, "*STRINGIFIED*8": 0.4 },
          "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*16": 1.0 },
          "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*8": 1.0 },
          "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Mobile Safari/537.36": { "*STRINGIFIED*4": 1.0 }
        },
        "skip": null
      }
    },
    {
      "name": "hardwareConcurrency",
      "parentNames": ["userAgent"],
      "conditionalProbabilities": {
        "deeper": {
          "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*16": 0.5, "*STRINGIFIED*8": 0.5 },
          "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*10": 1.0 },
          "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*12": 1.0 },
          "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Mobile Safari/537.36": { "*STRINGIFIED*8": 1.0 }
        },
        "skip": null
      }
    },
    {
      "name": "videoCard",
      "parentNames": ["userAgent"],
      "conditionalProbabilities": {
        "deeper": {
          "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*{\"renderer\":\"ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)\",\"vendor\":\"Google Inc. (NVIDIA)\"}": 1.0 },
          "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*{\"renderer\":\"ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)\",\"vendor\":\"Google Inc. (Apple)\"}": 1.0 },
          "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*{\"renderer\":\"ANGLE (Mesa, llvmpipe (LLVM 15.0.7 256 bits), OpenGL 4.5)\",\"vendor\":\"Google Inc. (Mesa)\"}": 1.0 },
          "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Mobile Safari/537.36": { "*STRINGIFIED*{\"renderer\":\"Adreno (TM) 730\",\"vendor\":\"Qualcomm\"}": 1.0 }
        },
        "skip": null
      }
    },
    {
      "name": "fonts",
      "parentNames": ["userAgent"],
      "conditionalProbabilities": {
        "deeper": {
          "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*[\"Arial\",\"Calibri\",\"Segoe UI\"]": 1.0 },
          "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*[\"Helvetica Neue\",\"Menlo\",\"Gill Sans\"]": 1.0 },
          "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36": { "*STRINGIFIED*[\"DejaVu Sans\",\"Liberation Mono\"]": 1.0 },
          "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Mobile Safari/537.36": { "*STRINGIFIED*[\"Roboto\"]": 1.0 }
        },
        "skip": null
      }
    }
  ]
}
```

- [ ] **Step 3: Create the ZIP fixture from the JSON**

Run (entry name is irrelevant — the loader reads the first entry):

```bash
cd crates/zendriver-fingerprints/src/generative/fixtures
zip mini-network.zip mini-network.json
cd -
```

Expected: `mini-network.zip` created (~1 KB).

- [ ] **Step 4: Verify the workspace still builds (no code yet uses the new deps)**

Run: `cargo build -p zendriver-fingerprints --features generative`
Expected: builds (the new optional deps are declared but unused until later tasks — that is fine).

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-fingerprints/Cargo.toml crates/zendriver-fingerprints/src/generative/fixtures/
git commit -m "chore(fingerprints): wire generative deps + add mini-network test fixtures"
```

---

## Task 2: `mapping.rs` — decode + persona mapping

**Files:**
- Create: `crates/zendriver-fingerprints/src/generative/mapping.rs`

- [ ] **Step 1: Write `mapping.rs` with implementation + unit tests**

```rust
//! Decode sampled Bayesian-network attribute strings onto a [`Persona`] subset.
//!
//! browserforge encodes complex/non-string leaf values with a literal
//! `*STRINGIFIED*` prefix; strip it and JSON-parse the remainder.

use std::collections::HashMap;

use serde_json::Value;
use zendriver_stealth::{FontSpec, Persona, Platform, WebglSpec};

const STRINGIFIED: &str = "*STRINGIFIED*";

/// Strip the `*STRINGIFIED*` prefix for a scalar (e.g. `*STRINGIFIED*16` -> `16`).
pub(super) fn destringify_scalar(raw: &str) -> &str {
    raw.strip_prefix(STRINGIFIED).unwrap_or(raw)
}

/// Strip `*STRINGIFIED*` and JSON-parse the remainder (objects / arrays).
pub(super) fn destringify_json(raw: &str) -> Option<Value> {
    serde_json::from_str(raw.strip_prefix(STRINGIFIED)?).ok()
}

/// A UA is desktop if it carries no mobile marker.
pub(super) fn is_desktop_ua(ua: &str) -> bool {
    !["Mobile", "Android", "iPhone", "iPad"]
        .iter()
        .any(|m| ua.contains(m))
}

/// Map a sampled `platform` value onto the desktop-only [`Platform`] enum.
pub(super) fn map_platform(v: &str) -> Option<Platform> {
    match v {
        "Win32" => Some(Platform::Win32),
        "MacIntel" => Some(Platform::MacIntel),
        _ if v.starts_with("Linux") => Some(Platform::LinuxX86_64),
        _ => None,
    }
}

/// Build a [`Persona`] from a sampled `node name -> value` assignment.
pub(super) fn persona_from_assignment(a: &HashMap<String, String>) -> Persona {
    let mut p = Persona::default();
    if let Some(v) = a.get("platform") {
        p.platform = map_platform(v);
    }
    if let Some(v) = a.get("deviceMemory") {
        p.device_memory_gb = destringify_scalar(v).parse().ok();
    }
    if let Some(v) = a.get("hardwareConcurrency") {
        p.hardware_concurrency = destringify_scalar(v).parse().ok();
    }
    if let Some(v) = a.get("videoCard") {
        if let Some(obj) = destringify_json(v) {
            let vendor = obj.get("vendor").and_then(Value::as_str).map(String::from);
            let renderer = obj.get("renderer").and_then(Value::as_str).map(String::from);
            if vendor.is_some() || renderer.is_some() {
                p.webgl = Some(WebglSpec {
                    unmasked_vendor: vendor,
                    unmasked_renderer: renderer,
                    ..Default::default()
                });
            }
        }
    }
    if let Some(v) = a.get("fonts") {
        if let Some(Value::Array(arr)) = destringify_json(v) {
            let list: Vec<String> = arr
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
            if !list.is_empty() {
                p.fonts = Some(FontSpec {
                    available: Some(list),
                    ..Default::default()
                });
            }
        }
    }
    p
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn destringify_scalar_strips_prefix() {
        assert_eq!(destringify_scalar("*STRINGIFIED*16"), "16");
        assert_eq!(destringify_scalar("MacIntel"), "MacIntel");
    }

    #[test]
    fn destringify_json_parses_object_and_array() {
        let obj = destringify_json("*STRINGIFIED*{\"vendor\":\"V\",\"renderer\":\"R\"}").unwrap();
        assert_eq!(obj.get("vendor").unwrap(), "V");
        let arr = destringify_json("*STRINGIFIED*[\"a\",\"b\"]").unwrap();
        assert!(arr.is_array());
        assert!(destringify_json("not-stringified").is_none());
    }

    #[test]
    fn is_desktop_ua_filters_mobile() {
        assert!(is_desktop_ua("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/143"));
        assert!(!is_desktop_ua("Mozilla/5.0 (Linux; Android 13) Mobile Safari"));
        assert!(!is_desktop_ua("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0)"));
    }

    #[test]
    fn map_platform_table() {
        assert_eq!(map_platform("Win32"), Some(Platform::Win32));
        assert_eq!(map_platform("MacIntel"), Some(Platform::MacIntel));
        assert_eq!(map_platform("Linux x86_64"), Some(Platform::LinuxX86_64));
        assert_eq!(map_platform("Linux aarch64"), Some(Platform::LinuxX86_64));
        assert_eq!(map_platform("iPhone"), None);
    }

    #[test]
    fn persona_from_assignment_populates_subset() {
        let mut a = HashMap::new();
        a.insert("platform".into(), "MacIntel".into());
        a.insert("deviceMemory".into(), "*STRINGIFIED*16".into());
        a.insert("hardwareConcurrency".into(), "*STRINGIFIED*10".into());
        a.insert(
            "videoCard".into(),
            "*STRINGIFIED*{\"renderer\":\"ANGLE (Apple, Apple M2)\",\"vendor\":\"Google Inc. (Apple)\"}".into(),
        );
        a.insert("fonts".into(), "*STRINGIFIED*[\"Menlo\",\"Gill Sans\"]".into());
        let p = persona_from_assignment(&a);
        assert_eq!(p.platform, Some(Platform::MacIntel));
        assert_eq!(p.device_memory_gb, Some(16));
        assert_eq!(p.hardware_concurrency, Some(10));
        let w = p.webgl.unwrap();
        assert_eq!(w.unmasked_vendor.as_deref(), Some("Google Inc. (Apple)"));
        assert!(w.unmasked_renderer.unwrap().contains("Apple M2"));
        assert_eq!(p.fonts.unwrap().available.unwrap().len(), 2);
    }
}
```

- [ ] **Step 2: Wire the module (temporary) so it compiles**

Task 3 fully replaces `mod.rs` (and declares `mod mapping;` there). For now, add a
single line to the **existing** `crates/zendriver-fingerprints/src/generative/mod.rs`
so `mapping.rs` is part of the crate and its tests run — insert `mod mapping;`
immediately after the module doc-comment block (before `use std::collections::HashMap;`):

```rust
mod mapping;
```

The old sampler keeps compiling alongside; `mapping`'s `pub(super)` fns are
exercised by its own `#[cfg(test)]` tests (so no dead-code error under `cargo test`).

- [ ] **Step 3: Run the mapping tests**

Run: `cargo test -p zendriver-fingerprints --features generative mapping::`
Expected: 5 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-fingerprints/src/generative/mapping.rs crates/zendriver-fingerprints/src/generative/mod.rs
git commit -m "feat(fingerprints): generative attribute decode + persona mapping"
```

---

## Task 3: `mod.rs` — sampler core (types, CPT walk, generate, `from_network_json`)

> **Executor note:** Tasks 3 and 4 are **one commit unit** — `mod.rs` (Task 3) and
> `download.rs` (Task 4) are mutually dependent, so the tree does not build between
> them. Run Tasks 3 and 4 back-to-back; the single green build + commit is at the end
> of Task 4. Do not insert a review checkpoint after Task 3.

**Files:**
- Modify (full rewrite): `crates/zendriver-fingerprints/src/generative/mod.rs`

- [ ] **Step 1: Replace `mod.rs` entirely with the new sampler**

Overwrite `crates/zendriver-fingerprints/src/generative/mod.rs` with:

```rust
//! Generative source (C3): a browserforge/apify Bayesian-network persona generator.
//!
//! Ports the canonical conditional-probability-table form: each node's
//! distribution is conditioned on its parents' sampled values, walked through a
//! nested `conditionalProbabilities` `deeper`/`skip` tree. Sampling is
//! **deterministic** in the [`Seed`] (a seeded Mulberry32 stream) and yields an
//! internally **coherent**, **desktop** [`Persona`]: the single root `userAgent`
//! node is restricted to non-mobile UAs, and every other attribute is sampled
//! conditioned on it.
//!
//! The network is fetched on first use and cached (see [`Generator::load_or_download`]).
//! Determinism holds for a fixed cached network version; a cache refresh (the
//! upstream file is regenerated ~monthly) may change per-seed output.

mod download;
mod mapping;

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use zendriver_stealth::{Persona, Seed};

/// Upstream fingerprint network (apify/fingerprint-suite, Apache-2.0, regenerated
/// ~monthly). Overridable per call via [`Generator::load_or_download`] or the
/// `ZENDRIVER_FP_NETWORK_URL` env var (see [`Generator::load_or_download_default`]).
pub const DEFAULT_NETWORK_URL: &str = "https://raw.githubusercontent.com/apify/fingerprint-suite/master/packages/fingerprint-generator/src/data_files/fingerprint-network-definition.zip";

/// Test-support: bytes of the tiny embedded network zip, for downstream hermetic
/// tests (e.g. `zendriver-mcp`). Not part of the supported API.
#[doc(hidden)]
pub const TEST_NETWORK_ZIP: &[u8] = include_bytes!("fixtures/mini-network.zip");

/// Errors from loading or decompressing the network.
#[derive(Debug, thiserror::Error)]
pub enum GenError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("empty network archive")]
    EmptyArchive,
}

#[derive(Debug, Clone, Deserialize)]
struct RawNetwork {
    nodes: Vec<Node>,
}

#[derive(Debug, Clone, Deserialize)]
struct Node {
    name: String,
    #[serde(default, rename = "parentNames")]
    parent_names: Vec<String>,
    /// Canonical nested CPT, walked dynamically by depth = `parent_names.len()`.
    #[serde(rename = "conditionalProbabilities")]
    cpt: Value,
}

/// A Bayesian network of conditional fingerprint attributes (browserforge form).
#[derive(Debug, Clone)]
pub struct Generator {
    nodes: Arc<Vec<Node>>,
}

impl Generator {
    /// Build from the inner network JSON (seam for fast unit tests).
    pub fn from_network_json(json: &str) -> Result<Self, GenError> {
        let raw: RawNetwork = serde_json::from_str(json)?;
        Ok(Self {
            nodes: Arc::new(raw.nodes),
        })
    }

    /// Build from raw ZIP bytes (decompress the first entry, then parse).
    pub fn from_zip_bytes(bytes: &[u8]) -> Result<Self, GenError> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        if archive.is_empty() {
            return Err(GenError::EmptyArchive);
        }
        let mut entry = archive.by_index(0)?;
        let mut json = String::new();
        entry.read_to_string(&mut json)?;
        Self::from_network_json(&json)
    }

    /// Fetch (or read cached) the network ZIP from `url`, then build.
    pub async fn load_or_download(url: &str) -> Result<Self, GenError> {
        let cache = download::cache_path();
        let bytes = download::fetch_or_cached_bytes(url, &cache).await?;
        Self::from_zip_bytes(&bytes)
    }

    /// Ergonomic [`load_or_download`](Self::load_or_download): uses
    /// `ZENDRIVER_FP_NETWORK_URL` if set, else [`DEFAULT_NETWORK_URL`].
    pub async fn load_or_download_default() -> Result<Self, GenError> {
        let url = std::env::var("ZENDRIVER_FP_NETWORK_URL")
            .unwrap_or_else(|_| DEFAULT_NETWORK_URL.to_string());
        Self::load_or_download(&url).await
    }

    /// Sample a coherent **desktop** persona deterministically from `seed`.
    pub fn generate(&self, seed: Seed) -> Persona {
        let mut rng = Mulberry32::new(seed.value() as u32);
        let mut assigned: HashMap<String, String> = HashMap::new();
        for node in self.topo_order() {
            let leaf = walk_cpt(&node.cpt, &node.parent_names, &assigned);
            // Restrict the single root `userAgent` node to desktop UAs so the
            // whole persona is desktop-coherent without backtracking.
            let dist = leaf_distribution(leaf, node.name == "userAgent");
            if dist.is_empty() {
                continue;
            }
            assigned.insert(node.name.clone(), weighted_pick(&dist, rng.next_f64()));
        }
        mapping::persona_from_assignment(&assigned)
    }

    /// Nodes in parents-before-children order (stable sort by parent count;
    /// ties keep JSON array order — keeps [`generate`](Self::generate)
    /// deterministic).
    fn topo_order(&self) -> Vec<&Node> {
        let mut order: Vec<&Node> = self.nodes.iter().collect();
        order.sort_by_key(|n| n.parent_names.len());
        order
    }
}

/// Walk the nested CPT to the leaf for the parents' sampled values. For each
/// parent, descend `deeper[value]` if present, else `skip`. After
/// `parents.len()` steps the result is the leaf `{ value: probability }` object
/// (or `Null` for a missing/`null` `skip`).
fn walk_cpt<'a>(cpt: &'a Value, parents: &[String], assigned: &HashMap<String, String>) -> &'a Value {
    const NULL: Value = Value::Null;
    let mut cur = cpt;
    for parent in parents {
        let pv = assigned.get(parent).map(String::as_str).unwrap_or_default();
        // Descend `deeper[parent_value]` when present, else the `skip` branch
        // (absent / `null` -> empty leaf). Plain match avoids any borrow ambiguity.
        cur = match cur.get("deeper").and_then(|d| d.get(pv)) {
            Some(next) => next,
            None => cur.get("skip").unwrap_or(&NULL),
        };
    }
    cur
}

/// Flatten a leaf object into a `(value, weight)` distribution. When
/// `desktop_only`, keep only desktop-UA keys (used for the root node).
fn leaf_distribution(leaf: &Value, desktop_only: bool) -> Vec<(String, f64)> {
    let Some(obj) = leaf.as_object() else {
        return Vec::new();
    };
    obj.iter()
        .filter(|(k, _)| !desktop_only || mapping::is_desktop_ua(k))
        .filter_map(|(k, v)| v.as_f64().map(|w| (k.clone(), w)))
        .collect()
}

/// Weighted pick from a (value, weight) distribution given a uniform `r` in
/// `[0, 1)`. Weights need not be normalized.
fn weighted_pick(dist: &[(String, f64)], r: f64) -> String {
    let total: f64 = dist.iter().map(|(_, w)| w).sum();
    if total <= 0.0 {
        return dist.first().map(|(v, _)| v.clone()).unwrap_or_default();
    }
    let mut acc = 0.0;
    for (v, w) in dist {
        acc += w / total;
        if r <= acc {
            return v.clone();
        }
    }
    dist.last().map(|(v, _)| v.clone()).unwrap_or_default()
}

/// Mulberry32 PRNG — tiny, fast, deterministic.
struct Mulberry32(u32);

impl Mulberry32 {
    fn new(seed: u32) -> Self {
        Self(seed)
    }

    /// Next value in `[0, 1)`.
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6D2B_79F5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(1 | t);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use zendriver_stealth::Platform;

    const FIXTURE: &str = include_str!("fixtures/mini-network.json");

    fn gen() -> Generator {
        Generator::from_network_json(FIXTURE).expect("fixture parses")
    }

    #[test]
    fn parses_fixture_with_root_user_agent() {
        let g = gen();
        assert_eq!(g.nodes.len(), 6);
        let roots: Vec<_> = g.nodes.iter().filter(|n| n.parent_names.is_empty()).collect();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].name, "userAgent");
    }

    #[test]
    fn generate_is_deterministic() {
        let g = gen();
        for s in 0..64u64 {
            let a = g.generate(Seed::from_u64(s));
            let b = g.generate(Seed::from_u64(s));
            assert_eq!(serde_json::to_value(&a).unwrap(), serde_json::to_value(&b).unwrap());
        }
    }

    #[test]
    fn personas_are_desktop_coherent() {
        let g = gen();
        for s in 0..256u64 {
            let p = g.generate(Seed::from_u64(s));
            let plat = p.platform.expect("platform assigned");
            assert!(
                matches!(plat, Platform::Win32 | Platform::MacIntel | Platform::LinuxX86_64),
                "non-desktop platform sampled: {plat:?}"
            );
            let w = p.webgl.as_ref().expect("webgl assigned");
            let renderer = w.unmasked_renderer.as_deref().expect("renderer assigned");
            assert!(w.unmasked_vendor.is_some(), "vendor assigned");
            match plat {
                Platform::Win32 => assert!(renderer.contains("D3D11"), "win renderer: {renderer}"),
                Platform::MacIntel => {
                    assert!(renderer.contains("Apple") || renderer.contains("Metal"), "mac renderer: {renderer}")
                }
                Platform::LinuxX86_64 => assert!(renderer.contains("Mesa"), "linux renderer: {renderer}"),
            }
        }
    }

    #[test]
    fn fields_populated() {
        let g = gen();
        let p = g.generate(Seed::from_u64(1));
        assert!(p.device_memory_gb.is_some());
        assert!(p.hardware_concurrency.is_some());
        assert!(p.fonts.as_ref().and_then(|f| f.available.as_ref()).is_some());
    }

    #[test]
    fn different_seeds_can_differ() {
        let g = gen();
        let plats: Vec<_> = (0..64u64).map(|s| g.generate(Seed::from_u64(s)).platform).collect();
        let first = plats[0];
        assert!(plats.iter().any(|p| *p != first), "every seed gave same platform");
    }
}
```

- [ ] **Step 2: Do NOT build/commit yet — `mod.rs` declares `mod download;`**

`mod.rs` now references `mod download;`, and `download.rs` (Task 4) needs
`GenError` + `TEST_NETWORK_ZIP` from this `mod.rs`. The two files are mutually
dependent and **land in a single commit at the end of Task 4**. Proceed directly
to Task 4; the first green build/test + commit happens there.

---

## Task 4: `download.rs` — fetch + cache, then green the suite

**Files:**
- Create: `crates/zendriver-fingerprints/src/generative/download.rs`

- [ ] **Step 1: Write `download.rs` (impl + wiremock cache test)**

```rust
//! Generative-local download-on-first-use + cache (mirrors the pool's pattern;
//! kept local so the pool's currently-untested download path is not perturbed).

use std::fs;
use std::path::{Path, PathBuf};

use super::GenError;

/// Cache location for the network ZIP — same root as the pool / fetcher.
pub(super) fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("zendriver/fingerprints/fingerprint-network.zip")
}

/// Read `cache` if present and non-empty, else GET `url` and atomically cache it.
pub(super) async fn fetch_or_cached_bytes(url: &str, cache: &Path) -> Result<Vec<u8>, GenError> {
    if let Ok(bytes) = fs::read(cache) {
        if !bytes.is_empty() {
            tracing::debug!(path = %cache.display(), "fp network cache hit");
            return Ok(bytes);
        }
    }
    tracing::debug!(url, "fp network cache miss — downloading");
    let bytes = reqwest::get(url).await?.bytes().await?.to_vec();
    if let Some(parent) = cache.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = cache.with_extension("tmp");
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, cache)?;
    tracing::debug!(path = %cache.display(), "fp network cached");
    Ok(bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn downloads_then_serves_from_cache() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(super::super::TEST_NETWORK_ZIP))
            .expect(1) // exactly one network hit — second call must hit cache
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("net.zip");

        let first = fetch_or_cached_bytes(&server.uri(), &cache).await.unwrap();
        let second = fetch_or_cached_bytes(&server.uri(), &cache).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(first, super::super::TEST_NETWORK_ZIP);
        // `.expect(1)` is verified on server drop.
    }
}
```

- [ ] **Step 2: Run the full generative test suite**

Run: `cargo test -p zendriver-fingerprints --features generative`
Expected: PASS — `mapping::` (5), `generative::tests` (5), `download::tests` (1).

- [ ] **Step 3: Add a `from_zip_bytes` test**

Append to the `#[cfg(test)] mod tests` in `mod.rs`:

```rust
    #[test]
    fn from_zip_bytes_loads_fixture() {
        let g = Generator::from_zip_bytes(TEST_NETWORK_ZIP).expect("zip loads");
        assert!(g.generate(Seed::from_u64(3)).platform.is_some());
    }
```

- [ ] **Step 4: Add a `load_or_download` wiremock smoke test**

Append to the `#[cfg(test)] mod tests` in `mod.rs`:

```rust
    #[tokio::test]
    async fn load_or_download_builds_from_mock() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(TEST_NETWORK_ZIP))
            .mount(&server)
            .await;
        let g = Generator::load_or_download(&server.uri()).await.expect("load");
        assert!(g.generate(Seed::from_u64(5)).platform.is_some());
    }
```

Note: `load_or_download` writes to the real `cache_path()`; this test downloads
once. That is acceptable for a smoke test (it overwrites the shared cache with the
fixture, then later runs re-read it). If flakiness appears in CI from the shared
cache, gate this test behind `#[ignore]`; the cache logic itself is covered by
`download::tests`.

- [ ] **Step 5: Run the suite again**

Run: `cargo test -p zendriver-fingerprints --features generative`
Expected: PASS (now +2 tests).

- [ ] **Step 6: Commit Tasks 3 + 4 together**

```bash
git add crates/zendriver-fingerprints/src/generative/mod.rs crates/zendriver-fingerprints/src/generative/download.rs
git commit -m "feat(fingerprints): canonical Bayesian sampler + download-on-first-use loader"
```

---

## Task 5: Remove the old embedded network + NOTICE

**Files:**
- Delete: `crates/zendriver-fingerprints/src/generative/network.json`
- Modify: `crates/zendriver-fingerprints/NOTICE`

- [ ] **Step 1: Delete the stale 3-node network**

```bash
git rm crates/zendriver-fingerprints/src/generative/network.json
```

(The new code never references it — `include_str!("network.json")` is gone.)

- [ ] **Step 2: Note the bundled fixture provenance in NOTICE**

Append to `crates/zendriver-fingerprints/NOTICE`:

```
The generative source downloads the fingerprint Bayesian network from
fingerprint-suite at runtime (default: the apify GitHub raw URL); a tiny derived
sample is embedded for tests. Both are Apache-2.0.
```

- [ ] **Step 3: Verify build + tests still pass**

Run: `cargo test -p zendriver-fingerprints --features generative,pool`
Expected: PASS (pool tests unaffected; generative tests green).

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-fingerprints/
git commit -m "chore(fingerprints): drop stale embedded network; note provenance"
```

---

## Task 6: Update the MCP `browser_fingerprint_generate` tool

**Files:**
- Modify: `crates/zendriver-mcp/src/tools/fingerprints.rs`
- Modify: `crates/zendriver-mcp/src/server.rs` (~line 1056 tool description)
- Modify: `crates/zendriver-mcp/Cargo.toml` (dev-deps)

- [ ] **Step 1: Add `wiremock` + `tempfile` dev-deps to `zendriver-mcp`**

In `crates/zendriver-mcp/Cargo.toml`, under `[dev-dependencies]` (create the
section if missing; keep existing entries), ensure these lines exist:

```toml
wiremock = { workspace = true }
tempfile = { workspace = true }
```

(`tokio` is already a dependency of this crate for the async server.)

- [ ] **Step 2: Rewrite `tools/fingerprints.rs` (call site + doc-comments + hermetic test)**

Replace the file body so the source uses the async loader and a URL seam, and
the doc-comments no longer claim "offline/embedded":

```rust
//! `browser_fingerprint_generate` — produce a Persona JSON from a real-device
//! source (pool / generative). Gated by the `fingerprints` cargo feature.

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where the persona comes from.
#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FpSource {
    /// Synthesize a coherent persona from the browserforge Bayesian network,
    /// downloaded on first use and cached locally.
    Generative,
    /// Sample a real-device persona from the published pool dataset. Requires
    /// the pool asset to be downloadable — see POOL_URL and issue #25.
    Pool,
}

/// Input for `browser_fingerprint_generate`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerateInput {
    /// Where the persona comes from. `generative` synthesizes one from the
    /// browserforge Bayesian network (downloaded + cached on first use); `pool`
    /// samples a downloaded real-device set (requires the published pool asset —
    /// see issue #25).
    pub source: FpSource,
    /// Optional seed for reproducibility. Omit for a random persona.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

/// Output for `browser_fingerprint_generate`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GenerateOutput {
    /// A Persona JSON — pass to `browser_open`'s `persona` field (inspect /
    /// tweak the JSON first if desired).
    pub persona: serde_json::Value,
}

// TODO(#25): real asset URL once the fingerprint-pool release asset is published.
const POOL_URL: &str =
    "https://github.com/TurtIeSocks/zendriver-rs/releases/latest/download/fingerprint-pool.json";

/// Resolve the generative network URL: `ZENDRIVER_FP_NETWORK_URL` if set, else the
/// crate default.
fn network_url() -> String {
    std::env::var("ZENDRIVER_FP_NETWORK_URL")
        .unwrap_or_else(|_| zendriver_fingerprints::generative::DEFAULT_NETWORK_URL.to_string())
}

/// Produce a [`GenerateOutput`] carrying a Persona JSON from the chosen
/// [`FpSource`].
///
/// `generative` synthesizes a persona from the browserforge Bayesian network,
/// which is downloaded on first use and cached. `pool` samples the published
/// real-device dataset (also downloaded on first use) — and, until the pool asset
/// is hosted (issue #25), surfaces an `internal_error`. An optional `seed` makes
/// either source reproducible. Takes no `SessionState` — fingerprint generation
/// is browser-independent.
pub async fn generate(input: GenerateInput) -> Result<GenerateOutput, ErrorData> {
    generate_from(input, &network_url()).await
}

/// Inner generator with an injectable generative network URL (test seam).
async fn generate_from(input: GenerateInput, network_url: &str) -> Result<GenerateOutput, ErrorData> {
    use zendriver::Seed;
    let seed = input.seed.map_or_else(Seed::random, Seed::from_u64);
    let persona = match input.source {
        FpSource::Generative => zendriver_fingerprints::generative::Generator::load_or_download(network_url)
            .await
            .map_err(|e| {
                ErrorData::internal_error(format!("generative network load failed: {e}"), None)
            })?
            .generate(seed),
        FpSource::Pool => {
            // NOTE: The pool release asset does not exist yet (tracked in issue
            // #25). This fails at runtime with a clear error until it is hosted.
            let set = zendriver_fingerprints::pool::load_or_download(POOL_URL)
                .await
                .map_err(|e| {
                    ErrorData::internal_error(
                        format!(
                            "pool load failed (the pool asset may not be published yet — see issue #25): {e}"
                        ),
                        None,
                    )
                })?;
            set.sample(seed)
        }
    };
    let value = serde_json::to_value(&persona)
        .map_err(|e| ErrorData::internal_error(format!("persona serialize: {e}"), None))?;
    Ok(GenerateOutput { persona: value })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn generative_via_mock_is_non_null_and_deterministic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(zendriver_fingerprints::generative::TEST_NETWORK_ZIP),
            )
            .mount(&server)
            .await;

        let a = generate_from(GenerateInput { source: FpSource::Generative, seed: Some(7) }, &server.uri())
            .await
            .expect("a");
        assert!(a.persona.is_object());

        let b = generate_from(GenerateInput { source: FpSource::Generative, seed: Some(7) }, &server.uri())
            .await
            .expect("b");
        assert_eq!(a.persona, b.persona);
    }
}
```

- [ ] **Step 3: Update the tool registration description in `server.rs`**

Open `crates/zendriver-mcp/src/server.rs` around line 1053-1056. Replace the
`description = "..."` string for the fingerprint tool so it no longer says
"works offline" / "embedded". Use exactly:

```rust
        description = "Generate a realistic fingerprint Persona JSON from a real-device `source`. `generative` synthesizes a coherent persona from the browserforge Bayesian network, downloaded on first use and cached locally (override the URL with the ZENDRIVER_FP_NETWORK_URL env var). `pool` samples a downloaded real-device set (requires the published pool asset — see issue #25; returns an error until the dataset is hosted). Optional `seed` (u64) for reproducibility — omit for a random persona. Returns `{ persona }` — pass it to `browser_open`'s `persona` field (inspect / tweak the JSON first if desired).",
```

- [ ] **Step 4: Run the MCP tool tests**

Run: `cargo test -p zendriver-mcp --features fingerprints fingerprints::`
Expected: PASS — `generative_via_mock_is_non_null_and_deterministic`.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-mcp/src/tools/fingerprints.rs crates/zendriver-mcp/src/server.rs crates/zendriver-mcp/Cargo.toml
git commit -m "feat(mcp): generative persona via async network loader; drop offline wording"
```

---

## Task 7: Regenerate MCP schema snapshots

**Files:**
- Modify: `crates/zendriver-mcp/tests/snapshots/*.snap` (regenerated)

- [ ] **Step 1: Regenerate + accept snapshots**

The tool description + `FpSource`/`GenerateInput` doc-comments changed, so the
emitted JSON-schema `description` fields change.

```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo insta accept --all
```

- [ ] **Step 2: Eyeball the diff**

Run: `git diff --stat crates/zendriver-mcp/tests/snapshots/`
Expected: only `description` text changes for the fingerprint tool (no field
add/remove/rename — input is still `{source, seed}`, output `{persona}`).

- [ ] **Step 3: Re-run snapshot test to confirm clean**

Run: `cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked`
Expected: PASS, no pending snapshots.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-mcp/tests/snapshots/
git commit -m "test(mcp): accept fingerprint tool schema snapshot (description update)"
```

---

## Task 8: Full gate run (fmt, clippy, tests)

**Files:** none (verification + any fmt fixes)

- [ ] **Step 1: Format**

```bash
cargo fmt --all
```

- [ ] **Step 2: Clippy — default features (CI baseline)**

```bash
cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged
cargo clippy --workspace --all-targets --locked -- -D warnings
```
Expected: no warnings.

- [ ] **Step 3: Clippy — feature-gated code**

```bash
cargo clippy -p zendriver-fingerprints --features generative,pool --all-targets -- -D warnings
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings
```
Expected: no warnings.

- [ ] **Step 4: Tests (parallel-safe set)**

Run in one batch:
```bash
cargo test -p zendriver-fingerprints --features generative,pool
cargo test -p zendriver-mcp --all-features
```
Expected: all PASS.

- [ ] **Step 5: Public-API check (no `zendriver` surface change expected)**

```bash
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked
```
Expected: PASS (this PR touches `zendriver-fingerprints` + `zendriver-mcp`, not
the `zendriver` public API baseline). If it fails because the baseline genuinely
moved, STOP and review — do not blindly regenerate.

- [ ] **Step 6: Commit any formatting changes**

```bash
git add -A
git commit -m "style: cargo fmt" || echo "nothing to commit"
```

---

## Self-review notes (already reconciled against the spec)

- **Download-on-first-use + cache** → Tasks 3-4 (`load_or_download*`, `download.rs`).
- **`ZENDRIVER_FP_NETWORK_URL` override** → `load_or_download_default` (Task 3) + MCP `network_url()` (Task 6).
- **Canonical CPT walk + `skip: null`** → `walk_cpt` (Task 3).
- **`*STRINGIFIED*` decode** → `mapping.rs` (Task 2).
- **Desktop restriction (root only)** → `leaf_distribution(.., desktop_only)` (Task 3).
- **Mapping subset (platform/memory/cores/webgl/fonts); UA + locale excluded** → `persona_from_assignment` (Task 2).
- **Determinism** → Mulberry32 + stable topo order; tested (Task 3).
- **MCP ripple (call site + 4 doc-comments + description + snapshots)** → Tasks 6-7.
- **Hermetic tests (wiremock + `TEST_NETWORK_ZIP`)** → Tasks 4, 6.
- **No `Persona`/JS-patch/`zendriver` public-API change** → confirmed (Task 8 step 5).
- Follow-ups #38 (header), #39 (geoip locale) intentionally out of scope.
