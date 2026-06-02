# Fingerprint Spoofing — Design (PR1)

- **Date:** 2026-06-02
- **Status:** Approved (brainstorming), pending implementation plan
- **Upstream driver:** zendriver (Python) issues #241, #108, #18, #202, #101; v1.0.0 thread #170
- **Scope of this spec:** the fingerprint-spoofing feature **only** (upstream TODO #1). The other seven prioritized TODOs ship as a separate PR2 (see *Scope*).

---

## 1. Context & motivation

`zendriver-rs` already ships a 9-patch JS stealth shim (`webdriver`, `plugins`, `chrome`, `webgl`-flags, `permissions`, `codecs`, `navigator_props`, `user_agent_data`, `broken_image`) plus 3-tier profiles, UA-CH metadata, and emulation. That defeats naive detectors (sannysoft/intoli) but does **not** spoof the high-entropy fingerprint surfaces that modern anti-bot vendors read: canvas, WebGL renderer/readback, AudioContext, font metrics, ClientRects, WebRTC, and assorted hardware APIs.

Fingerprint spoofing is the single most-requested upstream feature (#241 has the most reactions in the repo; the v1.0.0 thread repeatedly ends "any updates on fingerprint spoofing?"). #108 specifically names canvas + fonts; #241 asks to "support browserforge natively."

This is the natural P2-stealth extension for the Rust port.

## 2. Guiding principle

**Least opinionated; everything overridable; coherent defaults/templates on top.**

The realistic user is a power user doing per-site / per-WAF tuning — a process that is tedious and varies enormously by target. The library must therefore:

- Never lock a value. Every persona field is independently overridable.
- Ship coherent, sensible defaults so the common case is one line.
- Make "copy a real device, tweak a field, feed it" a first-class workflow.
- Protect coherence by **default**, not by removing capability. Power users keep every escape hatch; the foot-guns are documented, not forbidden.

## 3. Scope

The feature is organized around **two orthogonal axes** — keep them distinct throughout:

1. **Persona source** — *where the values come from*: `system()` host-probe, builder, JSON paste, `from_browser()` live-probe, or the `zendriver-fingerprints` crate's **pool** / **generative** sources. A source yields a whole coherent `Persona` (values + seeds).
2. **Per-surface render strategy** — *how a resolved persona is applied to each surface in the page*: `Native` / `Seeded` / `Random` / `Block` / `Value` (see §6).

"Real-device pool" from the early Q&A is a *source* (axis 1), rendered through `Value`/`Seeded` (axis 2) — not a fifth per-surface strategy. All of the originally-requested behaviors are present: seeded farble, per-read random, block/empty (axis 2) and real-device pool + generative (axis 1).

### In scope (PR1)

- Seven fingerprint surfaces: **canvas, WebGL, audio, fonts, ClientRects, WebRTC, hardware** (battery / gamepad / mediaDevices / SpeechSynthesis voices).
- Five per-surface render strategies (interpreted by surface *kind*, see §6): **`Native`, `Seeded`, `Random`, `Block`, `Value`**.
- A unified, serde-(de)serializable **`Persona`** type with builder + JSON + host-probe + live-browser-probe constructors and field-wise overlay merge.
- A new **`zendriver-fingerprints`** crate exposing the real-device persona source behind two cargo features: **`pool`** (finite set, download-on-first-use) and **`generative`** (browserforge Bayesian-network port).
- Seed model with `random` (default), `from_system` (machine-sticky, opt-in), and explicit reproducible seeds; persona-seed persistence alongside `user_data_dir`.

### Out of scope / deferred

- **PR2** — the other seven prioritized upstream TODOs: persistent network monitor (#223), browser-context HTTP request API (#189), nested-iframe traversal (#239/#246), CDP protocol freshness / forward-compat deser (nodriver #33/#34), bs4-like find ergonomics (#55), `datadome` bypass crate (#20), disable save-password/popups (#13). Each gets its own spec → plan → PR cycle.
- Real-device-data **collection** pipeline. We ship a curated release asset, not a scraper.
- Mobile personas. Desktop-first.
- Auto-refreshing the pool on a schedule.

## 4. Crate & module architecture

```
zendriver-stealth/                  (core, exists; serde + serde_json already hard deps)
├── persona/                        NEW module
│   ├── mod.rs        Persona, overlay merge, builder, FromStr/try_from_json,
│   │                 system(), from_browser()
│   ├── surface.rs    Surface enum, Strategy enum, per-surface resolution
│   └── seed.rs       Seed: random / from_system / from_u64
├── patches/                        existing dir; ADD:
│   ├── canvas.js  audio.js  fonts.js  client_rects.js  webrtc.js  hardware.js
│   └── webgl.js      (extended: value substitution, not just ANGLE/SwiftShader flags)
└── patches.rs        bootstrap_script(&Persona) emits the new patches from resolved values

zendriver-fingerprints/             NEW crate (optional, higher layer)
├── Cargo.toml        features = ["pool", "generative"]; depends on zendriver-stealth
├── pool/             download-on-first-use finite set → Pool::sample(seed) -> Persona
└── generative/       browserforge BN port      → Generator::generate(seed) -> Persona
```

**`Persona` lives in `zendriver-stealth` (core).** `zendriver-fingerprints` depends on stealth and *produces* `Persona`. Stealth never depends on the optional data crate, so builder/JSON/seeded-farble users never compile the heavy pieces.

**serde is not feature-gated.** `serde` + `serde_json` are already mandatory deps of `zendriver-stealth` (UA-CH metadata serializes into `Emulation.setUserAgentOverride`). Gating them saves nothing, so `Persona` derives `Serialize + Deserialize` unconditionally and both the builder and JSON paths are always available. The feature boundary that earns its keep is the `zendriver-fingerprints` crate, not serde.

## 5. The `Persona` type & merge model

```rust
/// Every field Option<_> → overlay semantics. Serialize + Deserialize always on.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Persona {
    // identity / system (today's Fingerprint folds in here)
    pub platform: Option<Platform>,
    pub ua: Option<UaSpec>,                  // ua_string + ua_metadata
    pub hardware_concurrency: Option<u32>,
    pub device_memory_gb: Option<u32>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    // surfaces
    pub webgl: Option<WebglSpec>,            // unmasked_vendor/renderer, params, extensions
    pub canvas: Option<SurfaceCfg>,          // noise surface
    pub audio: Option<SurfaceCfg>,           // noise surface
    pub fonts: Option<FontSpec>,             // available list + measureText noise
    pub client_rects: Option<SurfaceCfg>,    // noise surface
    pub webrtc: Option<WebrtcSpec>,          // policy + optional fake IP
    pub hardware: Option<HardwareSpec>,      // battery/gamepad/mediaDevices/speechVoices
    // control
    pub seed: Option<Seed>,
}
```

**Overlay merge:** `resolved = base.overlay(over)` is field-wise — `Some` in `over` wins, `None` inherits from `base`. Power-user flow: `Persona::system().overlay(my_json)` or `pool_device.overlay(tweaks)`.

**Constructors (all always available):**

```rust
impl Persona {
    /// Host-probed (sysinfo): platform, cpu, memory, tz, locale. Cached in a
    /// OnceLock so repeated calls are cheap clones (const-like ergonomics,
    /// runtime-correct — NOT a build-script const).
    pub fn system() -> Persona;

    /// Idiomatic fluent builder.
    pub fn builder() -> PersonaBuilder;

    /// No-opinion paste-a-real-device path. Also `impl FromStr`.
    pub fn try_from_json(s: &str) -> Result<Persona, serde_json::Error>;

    /// Highest-fidelity: probe the LIVE Chrome on this host for its real
    /// webgl/canvas/audio/font values. Maximally coherent — it IS the host.
    /// Async; needs a tab (reuses the browser you are already launching).
    pub async fn from_browser(tab: &Tab) -> Result<Persona, StealthError>;
}
```

> **Why not a build script / static const for `system()`:** a `build.rs`-baked host persona captures the *build* machine, not the run machine (CI / Docker / a distributed binary all differ), breaks under cross-compilation, and a build-time seed const would be identical across every run of a shipped binary (all users share one fingerprint). Host facts must be runtime; `OnceLock` gives the const-like access ergonomics without the wrongness.

## 6. Surface kinds (the key semantic)

A single `Strategy` enum is used for overrides, but it means different things per surface **kind**. Resolution interprets it; a meaningless combination logs a `tracing::warn` and falls back to the kind's default — it never errors (least-opinionated).

```rust
pub enum Strategy { Native, Seeded, Random, Block, Value }

pub enum Surface { Canvas, Webgl, Audio, Fonts, ClientRects, Webrtc, Hardware }
```

| Kind | Surfaces | Meaningful strategies | Default |
|---|---|---|---|
| **Noise** | canvas, audio, clientRects | `Seeded` (stable noise from persona seed) · `Random` (per-read) · `Block` (constant) · `Native` (off) | `Seeded` |
| **Value** | webgl, fonts, hardware | `Value` (substitute concrete value from persona) · `Block` (empty) · `Native` | `Value` |
| **Policy** | webrtc | `Block` (deny local-IP candidates) · `Value` (fake IP from persona) · `Native` | `Block` |

Pool and generative strategies preserve coherence by **supplying both** the noise seeds *and* the substituted values from one device record, so the whole persona tells one story (e.g. WebGL `UNMASKED_RENDERER` matches the canvas rendering traits and `navigator.platform`).

## 7. JS patch injection

Reuses the **existing** isolated-world observer (`zendriver-stealth/src/observer.rs`), which already injects the 9 current patches into every world/frame via `addScriptToEvaluateOnNewDocument` plus per-world re-injection. `bootstrap_script(&Persona)` grows to template the six new patches from the resolved persona; `webgl.js` is upgraded from flag-only to value substitution. **No new injection machinery** — same path that already passes sannysoft across frames.

## 8. `zendriver-fingerprints` crate

- **`pool` feature** — finite real-device set shipped as a **release asset**, fetched + cached on first use using the *same* cache directory/pattern as `zendriver-fetcher` (which already downloads Chrome for Testing). `Pool::sample(seed) -> Persona`. Refresh = publish a new asset; no code release. Seeded sampling is reproducible.
- **`generative` feature** — port browserforge's Bayesian-network sampler; vendor its network JSON (Apache-2.0; attribution in a crate `NOTICE`). `Generator::generate(seed) -> Persona`. Synthesizes unlimited coherent personas. Most directly answers #241's "support browserforge natively."
- Both emit the **same `Persona`**, so they slot straight into `overlay`.

## 9. Seed & reproducibility

```rust
pub enum SeedSource { Random, System, Explicit(u64) }

impl Seed {
    pub fn random() -> Seed;       // DEFAULT — per-instance unique
    pub fn from_system() -> Seed;  // deterministic per-machine:
                                   //   hash(machine-id / IOPlatformUUID / MachineGuid,
                                   //   fallback hostname + cpu model)
    pub fn from_u64(x: u64) -> Seed;
}
```

- Default seed is random per browser instance.
- `from_system()` yields the same fingerprint every run on this box **without** a `user_data_dir` — great for one persistent identity, wrong if you want to look like many users. Documented tradeoff; opt-in only.
- When a `user_data_dir` is set, the persona seed persists alongside it so a profile keeps one identity across runs.
- Determinism is unit-tested: same seed → byte-identical farble + identical pool sample.

## 10. Public API entry points

```rust
Browser::builder()
    .stealth(StealthProfile::spoofed())          // unchanged default
    .persona(Persona::system())                  // NEW; defaults to system() if omitted
    .persona_overlay(json_or_builder)            // NEW; sparse field-wise override
    .surface(Surface::Webrtc, Strategy::Native)  // NEW; per-surface escape hatch
```

Omit everything → coherent host-derived seeded-farble persona (sensible default). Real-device strategies are used by constructing a `Persona` from `zendriver-fingerprints` and passing it to `.persona(...)`.

## 11. Testing

- **Unit:** overlay merge precedence; seed determinism; per-surface strategy resolution incl. warn-fallback on meaningless combos; `Persona` JSON round-trip.
- **Integration (feature-gated, headful):** farbled readback differs from native and is **stable across repeat reads** under `Seeded`; differs under `Random`; empty under `Block`; untouched under `Native`. Probe targets: sannysoft, creepjs, browserscan (reuse the existing `browserscan.rs` example).
- **Snapshot:** `Persona` JSON wire shape via `insta` (matches the repo's existing schema-snapshot discipline; regenerate + accept per CLAUDE.md).
- **`from_browser`:** a headful test asserting the probed persona's webgl/canvas values match a second independent read from the same Chrome.

## 12. Alternatives considered

- **Strategy as a single global vs per-surface map:** resolved to *global default + unrestricted per-surface override*. With unrestricted overrides, a global default is a strict superset of a per-surface map (it can express any full map) plus better ergonomics; the only thing a restricted form would buy is forbidding incoherent multi-source mixes, which we instead handle via coherent defaults + documentation.
- **Real-device pool: bundle vs download vs separate crate vs generative model:** resolved to a *separate crate offering both `pool` (download) and `generative` (browserforge port) behind features*, keeping the core crate lean and offering both the lightweight finite-set path and the unlimited generative path.
- **`system()` at build time vs runtime:** runtime only (see §5 note).

## 13. Licensing

browserforge / fingerprint-suite data and model are Apache-2.0. Vendoring is permitted with attribution; ship a `NOTICE` in `zendriver-fingerprints`. The Rust workspace is dual MIT/Apache-2.0 — compatible.

## 14. Open questions for the plan stage

- Exact shape of `WebglSpec` / `FontSpec` / `HardwareSpec` (field lists per surface).
- Whether `Hardware` stays one surface or splits into sub-surfaces (battery/gamepad/mediaDevices/speech) for finer override; PR1 treats it as one `HardwareSpec`.
- Pool asset format (JSON vs a compact binary) and where it is hosted (zendriver-rs GitHub release vs a dedicated repo).
- How much of browserforge's BN to port vs vendor as precomputed tables.
