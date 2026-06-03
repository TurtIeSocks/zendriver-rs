# Generative fingerprints: extract full browserforge/apify Bayesian network

**Issue:** [#25](https://github.com/TurtIeSocks/zendriver-rs/issues/25) — follow-up to PR1 (#24).
**Date:** 2026-06-02
**Scope:** `zendriver-fingerprints`, `generative` feature only. Purely additive — no
public API change, no `Persona` type change, no JS-patch change.

## 1. Goal

Replace the minimal hand-authored 3-node Bayesian network with the real
browserforge/apify network so generated personas reflect real-world fingerprint
distributions. The loader and sampler are rewritten to the canonical schema; the
embedded data becomes the upstream 705 KB network; the attribute→`Persona`
mapping is widened to the subset `Persona` already models.

## 2. Current state (verified)

- `crates/zendriver-fingerprints/src/generative/mod.rs` (235 lines): a flat-CPT
  sampler. `Node { name, parents, cpt: HashMap<parent_join, Vec<(value, weight)>> }`,
  Mulberry32 PRNG, `weighted_pick`, `persona_from_assignment` mapping
  platform/webglRenderer/deviceMemory.
- `crates/zendriver-fingerprints/src/generative/network.json` (1.1 KB, 3 nodes:
  platform → deviceMemory/webglRenderer).
- `Generator::embedded() -> Self` parses the embedded JSON; `generate(&self, Seed) -> Persona`.
- Feature wiring: `generative = []` (no deps). `flate2`, `miniz_oxide`, and `zip`
  are already present in `Cargo.lock` (via `zendriver-fetcher`/`reqwest`).

## 3. Canonical network — verified facts

Source: `fingerprint-network-definition.zip`, Apache-2.0, from
`apify/fingerprint-suite/packages/fingerprint-generator/src/data_files/`.
Downloaded and inspected:

- ZIP is **705 185 bytes**; inner `network.json` is **13 MB** (issue estimated
  3–5 MB — actual is larger).
- Top-level shape: `{ "nodes": [ Node, … ] }`, **25 nodes**.
- Node shape: `{ name, parentNames: string[], possibleValues: string[],
  conditionalProbabilities: CPT }`. (Field is `parentNames`, **not** `parents`.)
- **`userAgent` is the single root** (`parentNames: []`). Every Persona-relevant
  node — `platform`, `deviceMemory`, `hardwareConcurrency`, `videoCard`, `fonts`
  — has `parentNames: ["userAgent"]`. The UA conditions the whole fingerprint.
- **CPT walk (matches apify `getProbabilitiesGivenKnownValues`):** start at
  `conditionalProbabilities`; for each parent name in order, if the parent's
  sampled value is a key of `deeper`, descend into `deeper[value]`, else descend
  into `skip`. After `parentNames.len()` steps the node is a **leaf**:
  `{ valueString: probability }` (probabilities sum to ~1).
  - Root (0 parents): the CPT *is* the leaf — a flat `{ UA: prob }` map.
  - `skip` may be `null` (observed on `platform`) — treat as an empty leaf.
- **Sampling (matches apify `sample`):** `anchor = rng()` in `[0,1)`; walk leaf
  entries accumulating probability; return the first value whose cumulative sum
  exceeds `anchor`; fall back to the first entry.
- **`*STRINGIFIED*` encoding:** complex/non-string leaf values are prefixed with
  the literal `*STRINGIFIED*`. Decode = strip the 13-char prefix, then
  JSON-parse the remainder. Examples:
  - `deviceMemory`: `*STRINGIFIED*16` → `16`
  - `hardwareConcurrency`: `*STRINGIFIED*8` → `8`
  - `videoCard`: `*STRINGIFIED*{"renderer":"…","vendor":"…"}` → object
  - `fonts`: `*STRINGIFIED*["Arial",…]` → array
  - `platform` values are **plain strings** (not stringified): `MacIntel`,
    `Win32`, `Linux x86_64`, `iPhone`, `iPad`, `Linux armv8l`, … (9 total).
- **Mobile present:** of 235 UA strings, **156 are non-mobile** (no
  `Mobile`/`Android`/`iPhone`/`iPad` marker). `platform` values include `iPhone`,
  `iPad`, and several ARM Linux variants. `Persona`/`Platform` is desktop-only
  (`Win32`, `MacIntel`, `LinuxX86_64`).
- **No `languages` node.** The fingerprint network carries no locale/language —
  those live in browserforge's separate *header* network. The issue's mapping
  list named "languages/locale", but this data file cannot supply it.

## 4. Design decisions

### 4.1 Ship the upstream ZIP unmodified; decompress with the existing `zip` dep

`include_bytes!("fingerprint-network-definition.zip")` embeds apify's file
**byte-for-byte**. Decompress in-memory at load via the `zip` crate
(`ZipArchive::new(Cursor::new(bytes))`, first entry), which is **already a
workspace dependency** (`zendriver-fetcher`).

Rejected alternatives:
- *Recompress as gzip + `flate2`* (661 KB): saves 44 KB but adds `flate2` as a
  workspace dep and a recompress step that muddies provenance. Not worth it.
- *zstd* (305 KB): best ratio but pulls a C-bindings dep into a `--locked` CI.
- *Commit raw 13 MB JSON*: bloats the crate tarball and repo.
- *Download-on-first-use* (reuse the pool's fetch/cache): undermines the whole
  point of `generative` — unlimited **offline** synthesis — and would pull
  `reqwest` into the `generative` feature (currently `pool`-only).

Feature change: `generative = ["dep:zip"]`.

### 4.2 Sampler — generic, faithful, deterministic

Rewrite `Node`/`Generator` to the canonical schema:

```rust
#[derive(Deserialize)]
struct Node {
    name: String,
    #[serde(default, rename = "parentNames")]
    parent_names: Vec<String>,
    #[serde(rename = "conditionalProbabilities")]
    cpt: serde_json::Value,         // walked dynamically by depth
    // possibleValues intentionally not deserialized — leaf keys suffice.
}

pub struct Generator { nodes: Arc<Vec<Node>> }   // Arc → cheap clone, parse once
```

- `Generator::embedded() -> Self` keeps its **signature** (no public API change).
  A module-level `OnceLock<Arc<Vec<Node>>>` decompresses + parses the 13 MB JSON
  **once**; `embedded()` returns a `Generator` holding a cheap `Arc` clone.
- `generate(&self, seed) -> Persona`: Mulberry32 seeded by `seed.value() as u32`;
  iterate nodes in a stable topological order (stable sort by `parent_names.len()`,
  preserving JSON array order on ties); for each node walk the CPT to its leaf,
  build a `Vec<(&str, f64)>` distribution, `weighted_pick`, record the assignment.
- CPT walk helper returns `&serde_json::Value` (the leaf), handling missing
  `deeper[value]` → `skip`, and `skip: null`/absent → empty leaf (node yields
  nothing; mapping treats it as unset). Mirrors apify exactly.
- Leaf-iteration order: `serde_json::Map` defaults to `BTreeMap` (no
  `preserve_order` feature), so leaf keys iterate in a **stable sorted order** →
  deterministic without needing `indexmap`. We do not need to match apify's RNG
  (we use Mulberry32, not `Math.random`); we only need *our* output stable.

**Determinism contract:** same `Seed` → same `Persona`, **for a fixed embedded
network version**. Updating the vendored network (or the mapped-field set) may
change the per-seed output. Documented in the module header. (This differs from
the pool source, whose modulo index is stable across versions.)

### 4.3 Desktop-only via a root restriction (no backtracking)

Because `userAgent` is the *single* root and everything is conditioned on it,
restricting the root's distribution to **non-mobile UAs** makes the entire
sampled persona coherent-desktop in one forward pass. No `sampleAccordingTo
Restrictions` backtracking is needed for downstream nodes — every real desktop
UA has populated child distributions.

```rust
fn is_desktop_ua(ua: &str) -> bool {
    !["Mobile", "Android", "iPhone", "iPad"].iter().any(|m| ua.contains(m))
}
```

Applied only to the root node: filter its leaf to desktop UAs, renormalize via
`weighted_pick` (weights need not sum to 1). 156 UAs remain — a healthy pool.
`map_platform` additionally folds any `Linux *` variant → `LinuxX86_64` and
returns `None` for any stray mobile value (defensive; shouldn't occur).

### 4.4 Attribute → `Persona` mapping (subset)

| BN node | `Persona` field | Decode |
|---|---|---|
| `platform` | `platform` | `Win32`→`Win32`, `MacIntel`→`MacIntel`, `Linux *`→`LinuxX86_64`, else `None` |
| `deviceMemory` | `device_memory_gb` | strip `*STRINGIFIED*`, parse `u32` |
| `hardwareConcurrency` | `hardware_concurrency` | strip `*STRINGIFIED*`, parse `u32` |
| `videoCard` | `webgl.unmasked_vendor` + `unmasked_renderer` | strip, JSON-parse `{renderer,vendor}` |
| `fonts` | `fonts.available` | strip, JSON-parse `[String]` |

**Deliberately excluded** (recorded as assumptions, §9):
- `userAgent` — **not** mapped to `ua.ua_string`. Stealth derives UA-CH
  (`sec-ch-ua*`) metadata from `platform`, not from a raw UA string; emitting a
  BN UA (e.g. `Chrome/143`) while UA-CH is composed for a default major would
  create a UA-vs-UA-CH mismatch. The issue's subset omits UA. The realistic()
  path already produces a platform-coherent UA. UA is still *sampled* (it
  conditions every child) — just not emitted.
- `locale`/`languages` — not present in this network (header network, separate).
- `screen`, `audioCodecs`, `videoCodecs`, `pluginsData`, `multimediaDevices`,
  `battery`, `extraProperties`, `userAgentData`, etc. — `Persona` has no
  corresponding value fields (the optional larger follow-up in #25).

### 4.5 `*STRINGIFIED*` decode helpers

```rust
const STRINGIFIED: &str = "*STRINGIFIED*";
// scalar: "*STRINGIFIED*16" -> "16"
fn destringify_scalar(raw: &str) -> &str { raw.strip_prefix(STRINGIFIED).unwrap_or(raw) }
// json:   "*STRINGIFIED*{…}" -> serde_json::Value
fn destringify_json(raw: &str) -> Option<serde_json::Value> {
    serde_json::from_str(raw.strip_prefix(STRINGIFIED)?).ok()
}
```

`videoCard` → read `.renderer` / `.vendor` strings. `fonts` → collect array into
`Vec<String>`.

## 5. Files touched

- `crates/zendriver-fingerprints/src/generative/mod.rs` — rewrite sampler +
  loader + mapping.
- `crates/zendriver-fingerprints/src/generative/fingerprint-network-definition.zip`
  — **new** vendored blob (705 KB).
- `crates/zendriver-fingerprints/src/generative/network.json` — **delete**.
- `crates/zendriver-fingerprints/Cargo.toml` — `generative = ["dep:zip"]`,
  add optional `zip` dep.
- `crates/zendriver-fingerprints/NOTICE` — already attributes
  browserforge/fingerprint-suite Apache-2.0; confirm wording covers the bundled
  network file (add the file name).

## 6. Testing

Rewrite/extend the module tests:
- **Parse:** embedded ZIP decompresses + deserializes; 25 nodes; `userAgent` is
  the sole root.
- **Determinism:** `generate(seed) == generate(seed)` (full `Persona` equality)
  across many seeds.
- **Coherence — platform/webgl:** over 0..256 seeds, every persona's
  `platform ∈ {Win32, MacIntel, LinuxX86_64}` (never mobile), and the webgl
  renderer string is plausible for that platform (Win → `Direct3D11`/`D3D11`;
  Mac → `Apple`/`Metal`; Linux → `ANGLE`/`Mesa`/`OpenGL`). Assert renderer +
  vendor are both `Some`.
- **Coherence — desktop restriction:** sampled internal UA (test-only accessor or
  a seam) always passes `is_desktop_ua`. (If no seam, assert the *consequence*:
  platform is always desktop — sufficient given the single-root structure.)
- **Spread:** across seeds, not all personas are identical (platform varies; or
  webgl renderer varies).
- **Decode units:** `destringify_scalar`/`destringify_json` on the three shapes;
  `map_platform` table.
- **Field population:** a generated persona has `device_memory_gb`,
  `hardware_concurrency`, `webgl{vendor,renderer}`, and `fonts.available`
  populated.

No `insta` snapshots (no wire-shape change; `Persona` JSON is unchanged).

## 7. Cargo / CI gates

- `cargo fmt --all`, `cargo clippy --workspace --all-targets --locked -D warnings`.
- `generative` is feature-gated; also run
  `cargo clippy -p zendriver-fingerprints --features generative --all-targets -- -D warnings`.
- `cargo test -p zendriver-fingerprints --features generative`.

## 8. MCP coverage

No `zendriver` public-API change (this is `zendriver-fingerprints`-internal;
`Generator::embedded`/`generate` signatures preserved). No new MCP tool, no
`public-api-baseline.txt` regen, no schema snapshots. The `generative` source is
not currently surfaced through an MCP tool; that gap (if any) predates this issue
and is unchanged by it — no ledger entry is created or modified here.

## 9. Assumptions (delegate-mode judgement calls)

1. **Ship the full 25-node network**, not a trimmed subset, even though only 6
   nodes are mapped — keeps provenance trivial (byte-identical upstream) and
   readies the optional full-coverage follow-up. Sampler is generic/faithful.
2. **`userAgent` is sampled but not emitted** to `ua.ua_string` (UA-CH mismatch
   risk; matches the issue's subset). Deviates from a literal reading that might
   map every available attribute.
3. **`locale` is not mapped** — absent from this network. The issue listed
   "languages/locale"; the data file cannot supply it without also vendoring the
   header network (out of scope).
4. **All `Linux *` platform variants fold to `LinuxX86_64`**, the only Linux
   value in the `Platform` enum.
5. **Desktop-only generator.** `generate` bakes in the non-mobile UA restriction;
   no public knob to request mobile personas (would need `Platform`/`Persona`
   mobile support — out of scope).
6. **`device_memory_gb` carries the raw BN value** (which can be 16/32); stealth's
   resolve already snaps `navigator.deviceMemory` to the spec max of 8. Persona
   stays least-opinionated (per project fingerprint philosophy).
7. **`zip` (existing dep) over `flate2`** for decompression — no new workspace
   dependency; ship apify's ZIP unmodified.
8. **Determinism is per-network-version**, not cross-version — acceptable and
   documented; a future network refresh re-rolls seeds.

## 10. Out of scope (tracked elsewhere / follow-ups)

- Full attribute coverage (screen metrics via `Emulation.setDeviceMetricsOverride`,
  navigator extras, audio) + `Persona`/JS-patch expansion — the issue's "optional
  larger follow-up (~1 week+, separate issue)".
- Header-network port for real `languages`/`Accept-Language` coherence.
- Mobile personas (`Platform` mobile variants).
- Download-on-first-use variant of the generative blob.

## 11. Risks

- **13 MB parse + `serde_json::Value` CPT trees** → tens of MB transient RAM and
  ~100–300 ms on first `embedded()`. Mitigated by parse-once `OnceLock`. Acceptable
  for a generator loaded once per process.
- **Vendoring fetchability** — the upstream ZIP must be downloaded once at
  implementation time and committed. Verified reachable
  (`raw.githubusercontent.com/apify/fingerprint-suite/master/…`).
- **Borrow lifetimes** in the sampler (assignments borrow `&str` from `self`'s
  CPT `Value`s) — resolvable; if it fights the borrow checker, store assignments
  as owned `String` (cheap; ≤25 entries).
