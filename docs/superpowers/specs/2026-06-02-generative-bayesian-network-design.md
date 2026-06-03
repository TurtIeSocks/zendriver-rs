# Generative fingerprints: extract full browserforge/apify Bayesian network

**Issue:** [#25](https://github.com/TurtIeSocks/zendriver-rs/issues/25) — follow-up to PR1 (#24).
**Follow-ups filed:** [#38](https://github.com/TurtIeSocks/zendriver-rs/issues/38)
(header-network request-header coherence), [#39](https://github.com/TurtIeSocks/zendriver-rs/issues/39)
(geo-IP-derived locale).
**Date:** 2026-06-02
**Scope:** `zendriver-fingerprints` `generative` feature + its MCP surface. No
`Persona` type change, no JS-patch change, no `zendriver` public-API change.

## 1. Goal

Replace the minimal hand-authored 3-node Bayesian network with the real
browserforge/apify fingerprint network so generated personas reflect real-world
fingerprint distributions. Rewrite the loader + sampler to the canonical schema;
**fetch the network on first use and cache it** (do not bundle); widen the
attribute→`Persona` mapping to the subset `Persona` already models.

## 2. Current state (verified)

- `crates/zendriver-fingerprints/src/generative/mod.rs` (235 lines): a flat-CPT
  sampler. `Node { name, parents, cpt: HashMap<parent_join, Vec<(value, weight)>> }`,
  Mulberry32 PRNG, `weighted_pick`, `persona_from_assignment`.
- `crates/zendriver-fingerprints/src/generative/network.json` (1.1 KB, 3 nodes).
- `Generator::embedded() -> Self` (sync, `include_str!`); `generate(&self, Seed) -> Persona`.
- `crates/zendriver-fingerprints/src/pool/mod.rs` already does
  **download-on-first-use**: `pool::load_or_download(url).await`,
  `pool::cache_path()` (`dirs::cache_dir()/zendriver/fingerprints/pool.json`),
  atomic tmp→rename write, `reqwest::get`. Note: this crate has **no dev-deps**
  and pool's download path is **currently untested**; the `zendriver-fetcher`
  crate is the in-repo precedent for `wiremock`-based download tests.
- **MCP surfaces generative:** `crates/zendriver-mcp/src/tools/fingerprints.rs:59`
  calls `Generator::embedded().generate(seed)`; the tool description
  (`server.rs:1056`) advertises it as offline/embedded.
- Feature wiring: `generative = []`. `reqwest`, `dirs`, `zip` are all already
  workspace deps (pool uses `reqwest`+`dirs`; fetcher uses `zip`) and present in
  `Cargo.lock` — no new dependency trees.

## 3. Canonical network — verified facts

Source: `fingerprint-network-definition.zip`, Apache-2.0, from
`apify/fingerprint-suite/packages/fingerprint-generator/src/data_files/`.
Downloaded and inspected:

- ZIP is **705 185 bytes**; inner `network.json` is **13 MB** (issue estimated
  3–5 MB — actual is larger). **Updated monthly** (commit history: a new build on
  the 1st of every month) — a strong reason *not* to freeze it into a release.
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
  `{ valueString: probability }`.
  - Root (0 parents): the CPT *is* the leaf — a flat `{ UA: prob }` map.
  - `skip` may be `null` (observed on `platform`) — treat as an empty leaf.
- **Sampling (matches apify `sample`):** `anchor = rng()` in `[0,1)`; walk leaf
  entries accumulating probability; return the first value whose cumulative sum
  exceeds `anchor`; fall back to the first entry.
- **`*STRINGIFIED*` encoding:** complex leaf values are prefixed with the literal
  `*STRINGIFIED*`. Decode = strip the 13-char prefix, then JSON-parse the
  remainder. `deviceMemory`→`*STRINGIFIED*16`; `hardwareConcurrency`→`*STRINGIFIED*8`;
  `videoCard`→`*STRINGIFIED*{"renderer":"…","vendor":"…"}`;
  `fonts`→`*STRINGIFIED*["Arial",…]`. `platform` values are **plain strings**.
- **Mobile present:** of 235 UA strings, **156 are non-mobile**. `platform` has 9
  values incl. `iPhone`, `iPad`, ARM Linux. `Persona`/`Platform` is desktop-only
  (`Win32`, `MacIntel`, `LinuxX86_64`).
- **No `languages` node.** Neither the fingerprint, header, nor input networks
  carry a language distribution — browserforge builds `Accept-Language` from a
  `locale` *input* (default `en-US`). Locale is therefore **not generatable** from
  this data (see §4.6 + #39).

## 4. Design decisions

### 4.1 Download-on-first-use + cache (no bundled blob)

The 705 KB network is fetched on first use and cached locally, **reusing the
pool's existing fetch/cache machinery**. Rationale (revised from an initial
bundle plan after finding the monthly cadence):

- The data **churns monthly**; a bundled copy ships stale Chrome majors that grow
  *more* detectable over a release's lifetime. Fetching keeps personas current.
- This is an online browser-automation tool — a one-time fetch is acceptable; the
  "offline synthesis" argument for bundling was weak and is dropped.
- The pool source already establishes the pattern (download + cache + atomic
  write). Consistency + code reuse.
- `build.rs`/compile-time download is **rejected**: it breaks sandboxed
  `cargo install`, docs.rs, `--offline`/`--locked` CI, and reproducibility.

**Fetch helper (generative-local).** A small private `generative/download.rs`
mirrors the pool's cache pattern — `cache_path()`
(`dirs::cache_dir()/zendriver/fingerprints/fingerprint-network.zip`) and
`async fetch_or_cached_bytes(url, cache) -> Result<Vec<u8>, GenError>` (cache-hit
read, else `reqwest::get` bytes + atomic tmp→rename write). **Pool is left
untouched** — its download path is currently untested, so refactoring it into a
shared module would add risk for no behavior gain; ~12 lines of cache/fetch are
duplicated instead. Cache stores the compressed zip; decompress on load. Like the
pool, the cache is kept indefinitely; refreshing = deleting the cache file (a TTL
/ `force_refresh` knob is a possible later enhancement, out of scope here).

### 4.2 Loader + sampler API (async; generic, faithful, deterministic)

```rust
/// Default upstream network (apify, Apache-2.0, regenerated ~monthly).
pub const DEFAULT_NETWORK_URL: &str =
    "https://raw.githubusercontent.com/apify/fingerprint-suite/master/\
     packages/fingerprint-generator/src/data_files/fingerprint-network-definition.zip";

pub struct Generator { nodes: Arc<Vec<Node>> }   // Arc → cheap clone

impl Generator {
    /// Fetch (or read cached) the network from `url`, then build.
    pub async fn load_or_download(url: &str) -> Result<Generator, GenError>;
    /// Ergonomic: use the `ZENDRIVER_FP_NETWORK_URL` env override if set, else
    /// `DEFAULT_NETWORK_URL`. The env knob doubles as a production mirror
    /// override and a hermetic test seam (point it at a local/wiremock URL).
    pub async fn load_or_download_default() -> Result<Generator, GenError>;
    /// Build from raw ZIP bytes (decompress first entry → from_network_json). Seam for the HTTP path + zip-fixture tests.
    pub fn from_zip_bytes(bytes: &[u8]) -> Result<Generator, GenError>;
    /// Build from the inner network JSON. Seam for fast unit tests (no zip/network).
    pub fn from_network_json(json: &str) -> Result<Generator, GenError>;
    /// Sample a coherent **desktop** persona deterministically.
    pub fn generate(&self, seed: Seed) -> Persona;
}

#[derive(Deserialize)]
struct Node {
    name: String,
    #[serde(default, rename = "parentNames")]
    parent_names: Vec<String>,
    #[serde(rename = "conditionalProbabilities")]
    cpt: serde_json::Value,           // walked dynamically by depth
    // possibleValues intentionally not deserialized — leaf keys suffice.
}
```

- `from_zip_bytes`: `zip::ZipArchive::new(Cursor::new(bytes))`, read the first
  entry to a `String`, hand to `from_network_json`.
- `generate`: Mulberry32 seeded by `seed.value() as u32`; iterate nodes in a
  stable topo order (stable sort by `parent_names.len()`, preserving JSON array
  order on ties); walk each node's CPT to its leaf; build a `Vec<(&str, f64)>`
  distribution; `weighted_pick`; record the assignment.
- CPT-walk helper returns the leaf `&serde_json::Value`, handling missing
  `deeper[value]` → `skip`, and `skip: null`/absent → empty leaf (node yields
  nothing; mapping treats it as unset).
- Leaf order: `serde_json::Map` is `BTreeMap` by default (no `preserve_order`),
  so leaf iteration is stably ordered → deterministic without `indexmap`. We do
  not match apify's RNG (we use Mulberry32), only *our own* stability.
- **No process-wide cache.** Callers construct a `Generator` once and reuse it for
  many `generate()` calls (the MCP tool builds per request — acceptable; a future
  `OnceCell` memoization is possible but unnecessary here).

**Determinism contract:** same `Seed` → same `Persona`, for a fixed *cached
network version*. A cache refresh (new monthly build) may change per-seed output.
Documented in the module header.

`GenError`: `Http(reqwest::Error)`, `Io(std::io::Error)`, `Zip(zip::result::ZipError)`,
`Json(serde_json::Error)`.

### 4.3 Desktop-only via a root restriction (no backtracking)

`userAgent` is the single root, so restricting *its* distribution to **non-mobile
UAs** makes the whole sampled persona coherent-desktop in one forward pass — no
`sampleAccordingToRestrictions` backtracking needed (every real desktop UA has
populated child distributions).

```rust
fn is_desktop_ua(ua: &str) -> bool {
    !["Mobile", "Android", "iPhone", "iPad"].iter().any(|m| ua.contains(m))
}
```

Applied only to the root node: filter its leaf to desktop UAs, then
`weighted_pick` (weights need not normalize). 156 UAs remain. `map_platform`
folds any `Linux *` → `LinuxX86_64` and returns `None` for stray mobile values
(defensive). Mobile personas are out of scope (the issue's separate follow-up;
needs `Platform`/`Persona` expansion across ~54 desktop-assuming match sites).

### 4.4 Attribute → `Persona` mapping (subset)

| BN node | `Persona` field | Decode |
|---|---|---|
| `platform` | `platform` | `Win32`→`Win32`, `MacIntel`→`MacIntel`, `Linux *`→`LinuxX86_64`, else `None` |
| `deviceMemory` | `device_memory_gb` | strip `*STRINGIFIED*`, parse `u32` |
| `hardwareConcurrency` | `hardware_concurrency` | strip `*STRINGIFIED*`, parse `u32` |
| `videoCard` | `webgl.unmasked_vendor` + `unmasked_renderer` | strip, JSON-parse `{renderer,vendor}` |
| `fonts` | `fonts.available` | strip, JSON-parse `[String]` |

### 4.5 `userAgent` sampled but not emitted

`userAgent` is sampled (it conditions every child) but **not** written to
`ua.ua_string`. Stealth derives UA-CH (`sec-ch-ua*`) from `platform`, not from a
raw UA string; emitting a BN UA (e.g. `Chrome/143`) while UA-CH is composed for a
default major would create a UA-vs-UA-CH mismatch. The issue's subset omits UA,
and the realistic() path already produces a platform-coherent UA.

### 4.6 Locale dropped from generative

The data carries no locale distribution (§3). A **random** locale is a detection
liability — anti-bot systems flag exit-IP-geo vs `navigator.language` mismatch.
Generative leaves `Persona.locale = None`; the user/overlay supplies it via the
existing builder `.locale()`. Coherent auto-locale (geo-IP-derived) is tracked in
**#39**; header-coherence (`Accept`, `sec-ch-ua`, order) in **#38**.

### 4.7 `*STRINGIFIED*` decode helpers

```rust
const STRINGIFIED: &str = "*STRINGIFIED*";
fn destringify_scalar(raw: &str) -> &str { raw.strip_prefix(STRINGIFIED).unwrap_or(raw) }
fn destringify_json(raw: &str) -> Option<serde_json::Value> {
    serde_json::from_str(raw.strip_prefix(STRINGIFIED)?).ok()
}
```

`videoCard` → read `.renderer`/`.vendor`. `fonts` → collect array → `Vec<String>`.

## 5. Files touched

- `crates/zendriver-fingerprints/src/generative/mod.rs` — rewrite loader (async
  download) + sampler (CPT walk) + `Generator`/`Node`/`GenError`/`DEFAULT_NETWORK_URL`.
- `crates/zendriver-fingerprints/src/generative/mapping.rs` — **new** decode
  helpers (`destringify_*`), `map_platform`, `is_desktop_ua`, `persona_from_assignment`.
- `crates/zendriver-fingerprints/src/generative/download.rs` — **new**
  generative-local `cache_path` + `fetch_or_cached_bytes`.
- `crates/zendriver-fingerprints/src/generative/fixtures/mini-network.json` +
  `mini-network.zip` — **new** tiny real-schema fixtures (a few desktop UAs + one
  mobile UA + the 5 mapped child nodes, ~KB) used by `#[cfg(test)]` via
  `include_str!`/`include_bytes!`. Pool left untouched.
- `crates/zendriver-fingerprints/src/generative/network.json` — **delete**.
- `crates/zendriver-fingerprints/Cargo.toml` —
  `generative = ["dep:reqwest", "dep:dirs", "dep:zip"]`; add `zip` optional dep;
  add `[dev-dependencies]` `tokio` + `wiremock` (none exist today) for the async
  download test.
- `crates/zendriver-mcp/src/tools/fingerprints.rs` — (a) call site
  `Generator::embedded().generate(seed)` →
  `Generator::load_or_download_default().await.map_err(…)?.generate(seed)` (the
  fn is already `async`; pool branch already awaits + maps errors); (b) fix the
  **JsonSchema doc-comments** that say "embedded"/"works offline" on
  `FpSource::Generative`, `GenerateInput.source`, and the `generate` fn doc — these
  text strings are emitted into the tool schema.
- `crates/zendriver-mcp/src/server.rs` — tool description (~line 1056): drop
  "works offline"/"embedded"; state it downloads the model once and caches.
- `crates/zendriver-mcp/Cargo.toml` — add `wiremock` (and `tokio` if absent)
  dev-deps for the hermetic generative test.
- `crates/zendriver-mcp/tests/snapshots/*.snap` — regenerate + accept (the schema
  `description` fields change). Input/output **types** are unchanged
  (`{source, seed}` → `{persona}`).
- `crates/zendriver-fingerprints/NOTICE` — already attributes browserforge
  Apache-2.0; add the network file name.

## 6. Testing

Module unit tests (fast path, `from_network_json` + the `mini-network.json`
fixture — no zip, no network):
- **Parse:** fixture deserializes; `userAgent` is the sole root.
- **Determinism:** `generate(seed) == generate(seed)` (full `Persona` equality)
  across many seeds.
- **Coherence:** over 0..256 seeds, `platform ∈ {Win32, MacIntel, LinuxX86_64}`
  (never mobile); webgl renderer plausible for platform (Win→`D3D11`,
  Mac→`Apple`/`Metal`, Linux→`Mesa`/`ANGLE`); vendor + renderer both `Some`.
- **Spread:** platform (or renderer) varies across seeds.
- **Decode units:** `destringify_scalar`/`destringify_json` on the three shapes;
  `map_platform` table; `is_desktop_ua`.
- **Field population:** `device_memory_gb`, `hardware_concurrency`,
  `webgl{vendor,renderer}`, `fonts.available` all populated.

HTTP/zip path (new `tokio`+`wiremock` dev-deps; mirrors the `zendriver-fetcher`
download tests — this also becomes the first test of the shared download path,
which pool never had):
- **`wiremock`** serves `mini-network.zip`; `download::fetch_or_cached_bytes(url, tmp)`
  (cache path = a `tempfile` dir) downloads on the first call and reads cache on
  the second (assert the mock saw exactly one request). A thin
  `load_or_download(mock_url)` smoke test asserts it returns a populated persona.

MCP (`tools/fingerprints.rs` existing tests): keep `generative_produces_non_null_persona`
/ `generative_is_deterministic`, but make them **hermetic** — a `wiremock` server
serves `mini-network.zip` and the test sets `ZENDRIVER_FP_NETWORK_URL` to the mock
URL (so `generate()` never touches live apify). Determinism still holds (same seed,
same cached fixture).

No new `insta` `Persona`-shape snapshots (the `Persona` wire shape is unchanged);
only the MCP tool-description schema snapshot is regenerated.

## 7. Cargo / CI gates

- `cargo fmt --all`; `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- Feature-gated clippy:
  `cargo clippy -p zendriver-fingerprints --features generative --all-targets -- -D warnings`
  and `cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings`.
- `cargo test -p zendriver-fingerprints --features generative,pool`.
- Schema snapshots:
  `cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked`
  then `cargo insta accept --all`.
- Public-API baseline: this touches `zendriver-fingerprints` (not `zendriver`), so
  the `zendriver` baseline is unaffected; no regen.

## 8. MCP coverage

The generative source **is** MCP-surfaced (the `fingerprint_persona` tool). This
PR keeps it covered: the call site moves to the async loader and the description
is corrected. No new tool, no I/O **type** change (so no ledger entry); the
description-text change is captured by regenerating the schema snapshot.

## 9. Assumptions (delegate-mode judgement calls)

1. **Download-on-first-use, not bundle** — driven by the monthly cadence; reuse
   the pool's fetch/cache. Default URL points at apify's raw master file;
   caller-overridable via `load_or_download(url)`.
2. **`load_or_download_default()`** convenience added so callers needn't pass the
   const (user-requested ergonomics). It honors a `ZENDRIVER_FP_NETWORK_URL` env
   override — a production mirror knob that also makes the MCP tests hermetic.
3. **Desktop-only.** Root UA restricted to non-mobile; all `Linux *` → `LinuxX86_64`.
   Mobile is a separate follow-up.
4. **`userAgent` sampled but not emitted** (UA-CH mismatch risk; matches subset).
5. **Locale dropped** (no data; random locale is a liability) → #39.
6. **Header coherence out of scope** → #38.
7. **`device_memory_gb` carries the raw BN value** (can be 16/32); stealth snaps
   `navigator.deviceMemory` to the spec max of 8 at resolve. Persona stays
   least-opinionated.
8. **Cache kept indefinitely** (like pool); refresh = delete cache file. TTL is a
   later enhancement.
9. **`Generator::embedded()` removed** (replaced by async loaders). Breaking change
   to the `generative` API — acceptable pre-release (no published users until P6).
10. **Determinism is per-cached-network-version**, documented.

## 10. Out of scope (tracked)

- **#38** — header-network request-header coherence (`Accept`, `sec-ch-ua`, order).
- **#39** — geo-IP-derived coherent locale.
- Full attribute coverage (screen metrics, navigator extras, audio) + `Persona`/
  JS-patch expansion — the issue's "optional larger follow-up".
- Mobile personas (`Platform` mobile variants).
- Cache TTL / forced refresh.

## 11. Risks

- **13 MB parse + `serde_json::Value` CPT trees** → tens of MB transient RAM and
  ~100–300 ms per `from_network_json`. The MCP tool builds a `Generator` per
  request — acceptable; memoize later if it shows up in profiles.
- **Upstream URL drift** — apify could move the master path (they moved it once).
  Mitigated by the cache (one good fetch persists) and the overridable URL; a
  project-hosted mirror is a fallback if it becomes a problem.
- **CI hermeticity** — tests must not hit the live network; enforced by the
  `wiremock` + fixture approach. Audit that no test calls `load_or_download_default`
  against the real URL.
- **Borrow lifetimes** in the sampler (assignments borrow `&str` from `self`'s CPT
  `Value`s) — if it fights the borrow checker, store assignments as owned `String`
  (≤25 entries, cheap).
