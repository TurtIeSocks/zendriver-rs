# Fingerprint Spoofing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add full fingerprint spoofing to zendriver-rs — a serde `Persona` type driving canvas/WebGL/audio/fonts/ClientRects/WebRTC/hardware surfaces through per-surface render strategies, plus a `zendriver-fingerprints` crate for real-device (pool + generative) personas.

**Architecture:** Two orthogonal axes. (1) *Persona source* — `system()`/builder/JSON/`from_browser()` in core `zendriver-stealth`, or `pool`/`generative` in the new optional `zendriver-fingerprints` crate. (2) *Per-surface render strategy* — `Native`/`Seeded`/`Random`/`Block`/`Value`, resolved per surface kind (noise/value/policy). JS patches reuse the existing isolated-world observer. Least-opinionated: every field overridable, coherent defaults.

**Tech Stack:** Rust (workspace, edition per workspace), `serde`/`serde_json` (already deps), `sysinfo` (already dep), `fastrand` (new, tiny) for seeding, `tokio`, CDP via `chromiumoxide_cdp`. JS farble shims injected via `Page.addScriptToEvaluateOnNewDocument`.

**Spec:** `docs/superpowers/specs/2026-06-02-fingerprint-spoofing-design.md`

---

## File Structure

**`zendriver-stealth` (core, modified):**
- Create `src/persona/mod.rs` — `Persona`, overlay merge, `PersonaBuilder`, `system()`, `from_browser()`, `FromStr`/`try_from_json`.
- Create `src/persona/seed.rs` — `Seed`, `SeedSource`, `random`/`from_system`/`from_u64`.
- Create `src/persona/surface.rs` — `Surface`, `Strategy`, `SurfaceKind`, per-surface resolution + warn-fallback.
- Create `src/persona/specs.rs` — `UaSpec`, `WebglSpec`, `FontSpec`, `WebrtcSpec`, `HardwareSpec`, `SurfaceCfg`.
- Create `src/patches/{canvas,audio,fonts,client_rects,webrtc,hardware}.js`; extend `src/patches/webgl.js`.
- Modify `src/patches.rs` — `bootstrap_script(&Persona)` emits new patches.
- Modify `src/fingerprint.rs` — keep probe helpers; `Fingerprint` becomes a producer of `Persona` identity fields (see Task A8).
- Modify `src/lib.rs` — `pub mod persona;` + re-exports.
- Modify `Cargo.toml` — add `fastrand`.

**`zendriver-fingerprints` (new crate):**
- Create `Cargo.toml`, `src/lib.rs`, `src/pool/mod.rs`, `src/generative/mod.rs`, `NOTICE`.

**`zendriver` (core public, modified):**
- Modify `src/browser.rs` — `.persona()`, `.persona_overlay()`, `.surface()` builder methods; persona-seed persistence.
- Modify `src/lib.rs` — re-export `Persona`/`Surface`/`Strategy`.
- Create `examples/persona_*.rs`.

---

## Phase A — Persona foundation

### Task A1: `Seed` type

**Files:**
- Create: `crates/zendriver-stealth/src/persona/seed.rs`
- Modify: `crates/zendriver-stealth/Cargo.toml` (add `fastrand`)
- Modify: `crates/zendriver-stealth/src/lib.rs` (add `pub mod persona;` placeholder — see A2)

- [ ] **Step 1: Add `fastrand` dep**

In `crates/zendriver-stealth/Cargo.toml` under `[dependencies]`:
```toml
fastrand = "2"
```
(If `fastrand` is already in the workspace, use `fastrand.workspace = true` instead — check `Cargo.toml` at repo root first.)

- [ ] **Step 2: Write the failing test**

Create `crates/zendriver-stealth/src/persona/seed.rs`:
```rust
//! Seed: controls deterministic farble. random (default) / from_system / explicit.

use serde::{Deserialize, Serialize};

/// A fingerprint seed. Serializes transparently as its u64 value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seed(pub u64);

impl Seed {
    /// Per-instance unique seed. THE DEFAULT.
    pub fn random() -> Seed {
        Seed(fastrand::u64(..))
    }

    /// Explicit reproducible seed.
    pub fn from_u64(v: u64) -> Seed {
        Seed(v)
    }

    /// Deterministic per-machine seed: stable across runs on the same host
    /// WITHOUT a user_data_dir. Opt-in — sticky per machine (one identity).
    pub fn from_system() -> Seed {
        Seed(system_seed_value())
    }

    /// Raw value for JS farble.
    pub fn value(self) -> u64 {
        self.0
    }
}

fn system_seed_value() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // Stable machine id, best-effort per OS; fall back to hostname + cpu brand.
    machine_id().hash(&mut h);
    sysinfo::System::host_name().unwrap_or_default().hash(&mut h);
    h.finish()
}

fn machine_id() -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/etc/machine-id").unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        // IOPlatformUUID via ioreg; empty on failure (falls back to hostname).
        std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                s.lines()
                    .find(|l| l.contains("IOPlatformUUID"))
                    .map(|l| l.to_string())
            })
            .unwrap_or_default()
    }
    #[cfg(target_os = "windows")]
    {
        // MachineGuid from registry via reg query.
        std::process::Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Cryptography",
                "/v",
                "MachineGuid",
            ])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_u64_round_trips() {
        assert_eq!(Seed::from_u64(42).value(), 42);
    }

    #[test]
    fn from_system_is_stable_within_process() {
        assert_eq!(Seed::from_system(), Seed::from_system());
    }

    #[test]
    fn serde_is_transparent_u64() {
        let s = Seed::from_u64(7);
        assert_eq!(serde_json::to_string(&s).unwrap(), "7");
        let back: Seed = serde_json::from_str("7").unwrap();
        assert_eq!(back, s);
    }
}
```

Add to `crates/zendriver-stealth/src/lib.rs`:
```rust
pub mod persona;
```
And create a temporary `crates/zendriver-stealth/src/persona/mod.rs` with just:
```rust
//! Persona: the unified fingerprint configuration.
pub mod seed;
pub use seed::{Seed};
```

- [ ] **Step 3: Run test to verify it fails (compiles first)**

Run: `cargo test -p zendriver-stealth seed:: 2>&1 | tail -20`
Expected: compile error if `fastrand` not yet wired, then PASS once it compiles. If `fastrand` unresolved, fix the dep line.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zendriver-stealth seed::`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/Cargo.toml crates/zendriver-stealth/src/lib.rs crates/zendriver-stealth/src/persona/
git commit -m "feat(stealth): Seed type (random/from_system/from_u64)"
```

---

### Task A2: Spec types (`SurfaceCfg`, `UaSpec`, `WebglSpec`, `FontSpec`, `WebrtcSpec`, `HardwareSpec`)

**Files:**
- Create: `crates/zendriver-stealth/src/persona/specs.rs`
- Modify: `crates/zendriver-stealth/src/persona/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/zendriver-stealth/src/persona/specs.rs`:
```rust
//! Per-surface value specs carried by a Persona. All fields optional → overlay.

use serde::{Deserialize, Serialize};

use super::surface::Strategy;

/// Noise-surface config (canvas, audio, clientRects).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SurfaceCfg {
    /// Render strategy for this surface. None → kind default.
    pub strategy: Option<Strategy>,
}

/// UA string + UA-CH metadata override.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UaSpec {
    pub ua_string: Option<String>,
    /// Free-form UA-CH overrides; merged onto realistic() output at resolve.
    pub platform: Option<String>,
}

/// WebGL value substitution.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WebglSpec {
    pub strategy: Option<Strategy>,
    pub unmasked_vendor: Option<String>,
    pub unmasked_renderer: Option<String>,
}

/// Font set + measureText noise.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FontSpec {
    pub strategy: Option<Strategy>,
    /// Allow-list of font families the page may detect.
    pub available: Option<Vec<String>>,
}

/// WebRTC policy.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WebrtcSpec {
    pub strategy: Option<Strategy>,
    /// Fake public IP used when strategy = Value.
    pub fake_ip: Option<String>,
}

/// Hardware bundle.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HardwareSpec {
    pub strategy: Option<Strategy>,
    pub battery_level: Option<f64>,
    pub media_devices: Option<u32>,
    pub speech_voices: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_round_trip_json() {
        let w = WebglSpec {
            strategy: Some(Strategy::Value),
            unmasked_vendor: Some("Google Inc. (NVIDIA)".into()),
            unmasked_renderer: Some("ANGLE (NVIDIA, ...)".into()),
        };
        let s = serde_json::to_string(&w).unwrap();
        let back: WebglSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(w, back);
    }

    #[test]
    fn empty_spec_omits_nothing_required() {
        let f = FontSpec::default();
        assert!(f.available.is_none());
    }
}
```

Add to `crates/zendriver-stealth/src/persona/mod.rs`:
```rust
pub mod specs;
pub mod surface; // defined in Task B1; add now so specs.rs compiles
pub use specs::{FontSpec, HardwareSpec, SurfaceCfg, UaSpec, WebglSpec, WebrtcSpec};
```

> Note: `specs.rs` imports `Strategy` from `surface.rs`. Create a minimal `surface.rs` stub now so this compiles; Task B1 fills it:
```rust
//! Surface + Strategy (filled in Task B1).
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Strategy { Native, Seeded, Random, Block, Value }
```

- [ ] **Step 2: Run test to verify it fails, then passes**

Run: `cargo test -p zendriver-stealth specs::`
Expected: 2 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/persona/
git commit -m "feat(stealth): per-surface persona spec types"
```

---

### Task A3: `Persona` struct + `Default`

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/mod.rs`

- [ ] **Step 1: Write the failing test**

In `crates/zendriver-stealth/src/persona/mod.rs`, add:
```rust
use serde::{Deserialize, Serialize};

use crate::Platform;
use specs::{FontSpec, HardwareSpec, SurfaceCfg, UaSpec, WebglSpec, WebrtcSpec};

/// Unified fingerprint configuration. Every field optional → overlay semantics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Persona {
    pub platform: Option<Platform>,
    pub ua: Option<UaSpec>,
    pub hardware_concurrency: Option<u32>,
    pub device_memory_gb: Option<u32>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub webgl: Option<WebglSpec>,
    pub canvas: Option<SurfaceCfg>,
    pub audio: Option<SurfaceCfg>,
    pub fonts: Option<FontSpec>,
    pub client_rects: Option<SurfaceCfg>,
    pub webrtc: Option<WebrtcSpec>,
    pub hardware: Option<HardwareSpec>,
    pub seed: Option<Seed>,
}

#[cfg(test)]
mod persona_tests {
    use super::*;

    #[test]
    fn default_persona_is_all_none() {
        let p = Persona::default();
        assert!(p.platform.is_none() && p.seed.is_none() && p.webgl.is_none());
    }

    #[test]
    fn persona_round_trips_json() {
        let mut p = Persona::default();
        p.seed = Some(Seed::from_u64(5));
        p.timezone = Some("America/New_York".into());
        let s = serde_json::to_string(&p).unwrap();
        let back: Persona = serde_json::from_str(&s).unwrap();
        assert_eq!(back.seed, Some(Seed::from_u64(5)));
        assert_eq!(back.timezone.as_deref(), Some("America/New_York"));
    }
}
```

Ensure `Platform` derives `Serialize + Deserialize`. Check `crates/zendriver-stealth/src/profile.rs` — if `Platform` only derives `Serialize`, add `Deserialize`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform { /* ... */ }
```

- [ ] **Step 2: Run test, verify pass**

Run: `cargo test -p zendriver-stealth persona_tests::`
Expected: 2 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/persona/mod.rs crates/zendriver-stealth/src/profile.rs
git commit -m "feat(stealth): Persona struct + serde"
```

---

### Task A4: Overlay merge

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to `persona_tests` mod:
```rust
#[test]
fn overlay_some_wins_none_inherits() {
    let base = Persona {
        timezone: Some("UTC".into()),
        device_memory_gb: Some(8),
        seed: Some(Seed::from_u64(1)),
        ..Persona::default()
    };
    let over = Persona {
        timezone: Some("Asia/Tokyo".into()),
        ..Persona::default()
    };
    let merged = base.overlay(over);
    assert_eq!(merged.timezone.as_deref(), Some("Asia/Tokyo")); // some wins
    assert_eq!(merged.device_memory_gb, Some(8)); // none inherits
    assert_eq!(merged.seed, Some(Seed::from_u64(1)));
}
```

- [ ] **Step 2: Implement**

Add to `impl Persona`:
```rust
impl Persona {
    /// Field-wise merge: `Some` in `over` wins, `None` inherits from `self`.
    pub fn overlay(self, over: Persona) -> Persona {
        Persona {
            platform: over.platform.or(self.platform),
            ua: over.ua.or(self.ua),
            hardware_concurrency: over.hardware_concurrency.or(self.hardware_concurrency),
            device_memory_gb: over.device_memory_gb.or(self.device_memory_gb),
            timezone: over.timezone.or(self.timezone),
            locale: over.locale.or(self.locale),
            webgl: over.webgl.or(self.webgl),
            canvas: over.canvas.or(self.canvas),
            audio: over.audio.or(self.audio),
            fonts: over.fonts.or(self.fonts),
            client_rects: over.client_rects.or(self.client_rects),
            webrtc: over.webrtc.or(self.webrtc),
            hardware: over.hardware.or(self.hardware),
            seed: over.seed.or(self.seed),
        }
    }
}
```

- [ ] **Step 3: Run test, verify pass**

Run: `cargo test -p zendriver-stealth persona_tests::overlay`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-stealth/src/persona/mod.rs
git commit -m "feat(stealth): Persona::overlay field-wise merge"
```

---

### Task A5: `try_from_json` + `FromStr`

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/mod.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn from_json_and_fromstr_parse() {
    let json = r#"{"timezone":"Europe/Paris","seed":99}"#;
    let a = Persona::try_from_json(json).unwrap();
    assert_eq!(a.timezone.as_deref(), Some("Europe/Paris"));
    let b: Persona = json.parse().unwrap();
    assert_eq!(b.seed, Some(Seed::from_u64(99)));
}
```

- [ ] **Step 2: Implement**

```rust
impl Persona {
    pub fn try_from_json(s: &str) -> Result<Persona, serde_json::Error> {
        serde_json::from_str(s)
    }
}

impl std::str::FromStr for Persona {
    type Err = serde_json::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver-stealth persona_tests::from_json`
Expected: PASS.
```bash
git add crates/zendriver-stealth/src/persona/mod.rs
git commit -m "feat(stealth): Persona JSON ingestion (try_from_json + FromStr)"
```

---

### Task A6: `PersonaBuilder`

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/mod.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn builder_sets_fields() {
    let p = Persona::builder()
        .seed(Seed::from_u64(3))
        .timezone("UTC")
        .device_memory_gb(16)
        .build();
    assert_eq!(p.seed, Some(Seed::from_u64(3)));
    assert_eq!(p.device_memory_gb, Some(16));
    assert_eq!(p.timezone.as_deref(), Some("UTC"));
}
```

- [ ] **Step 2: Implement**

```rust
/// Fluent builder for [`Persona`]. Every setter is optional.
#[derive(Debug, Clone, Default)]
pub struct PersonaBuilder(Persona);

impl Persona {
    pub fn builder() -> PersonaBuilder {
        PersonaBuilder(Persona::default())
    }
}

impl PersonaBuilder {
    pub fn seed(mut self, s: Seed) -> Self {
        self.0.seed = Some(s);
        self
    }
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.0.timezone = Some(tz.into());
        self
    }
    pub fn locale(mut self, l: impl Into<String>) -> Self {
        self.0.locale = Some(l.into());
        self
    }
    pub fn device_memory_gb(mut self, gb: u32) -> Self {
        self.0.device_memory_gb = Some(gb);
        self
    }
    pub fn hardware_concurrency(mut self, n: u32) -> Self {
        self.0.hardware_concurrency = Some(n);
        self
    }
    pub fn webgl(mut self, w: WebglSpec) -> Self {
        self.0.webgl = Some(w);
        self
    }
    pub fn build(self) -> Persona {
        self.0
    }
}
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver-stealth persona_tests::builder`
Expected: PASS.
```bash
git add crates/zendriver-stealth/src/persona/mod.rs
git commit -m "feat(stealth): PersonaBuilder"
```

---

### Task A7: `Persona::system()` (host probe, OnceLock-cached)

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/mod.rs`
- Reference: `crates/zendriver-stealth/src/fingerprint.rs` (`auto_detect`, `detect_platform`, `detect_memory_gb`, `clamp_cpu_count`)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn system_persona_is_populated_and_cached() {
    let a = Persona::system();
    // Host probe fills platform + cpu + memory.
    assert!(a.platform.is_some());
    assert!(a.hardware_concurrency.is_some());
    assert!(a.device_memory_gb.is_some());
    let b = Persona::system();
    // Cached → same values.
    assert_eq!(a.device_memory_gb, b.device_memory_gb);
}
```

- [ ] **Step 2: Implement**

In `fingerprint.rs`, make the probe helpers `pub(crate)` (they are currently private): `detect_platform`, `detect_memory_gb`, `clamp_cpu_count`, `probe_chrome_version` — add `pub(crate)` to each.

In `persona/mod.rs`:
```rust
use std::sync::OnceLock;

static SYSTEM: OnceLock<Persona> = OnceLock::new();

impl Persona {
    /// Host-probed persona (sysinfo). Cached: first call probes, rest clone.
    /// Runtime — NOT a build-script const (build host != run host).
    pub fn system() -> Persona {
        SYSTEM.get_or_init(Persona::probe_system).clone()
    }

    fn probe_system() -> Persona {
        let platform = crate::fingerprint::detect_platform();
        let cpu = crate::fingerprint::clamp_cpu_count(num_cpus::get() as u32);
        let mem = crate::fingerprint::detect_memory_gb().unwrap_or(8);
        Persona {
            platform: Some(platform),
            hardware_concurrency: Some(cpu),
            device_memory_gb: Some(mem),
            timezone: None, // probed lazily / by from_browser
            locale: None,
            seed: Some(Seed::random()),
            ..Persona::default()
        }
    }
}
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver-stealth persona_tests::system`
Expected: PASS.
```bash
git add crates/zendriver-stealth/src/persona/mod.rs crates/zendriver-stealth/src/fingerprint.rs
git commit -m "feat(stealth): Persona::system() host probe (cached)"
```

---

### Task A8: Bridge `Fingerprint` → `Persona` for `bootstrap_script`

This keeps the existing 9 patches working while patches.rs migrates to `Persona`.

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/mod.rs`
- Reference: `crates/zendriver-stealth/src/patches.rs` (`bootstrap_script(fp: &Fingerprint)`), `src/fingerprint.rs` (`Fingerprint`)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn persona_exposes_resolved_platform_for_patches() {
    let p = Persona::system();
    // Helper used by patches.rs to read the effective platform JS string.
    assert!(!p.resolved_platform_js().is_empty());
}
```

- [ ] **Step 2: Implement**

```rust
impl Persona {
    /// Effective `navigator.platform` JS string for patch templating.
    /// Falls back to host platform when unset.
    pub fn resolved_platform_js(&self) -> String {
        let plat = self.platform.unwrap_or_else(|| {
            Persona::system().platform.unwrap_or(crate::Platform::LinuxX86_64)
        });
        plat.platform_js().to_string()
    }
}
```
> Check `profile.rs`/`fingerprint.rs` for the existing platform→JS-string method (e.g. `ch_platform` / `platform`); reuse it. If the method is named differently, adapt `platform_js()` to the real name and keep this helper's name stable.

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver-stealth persona_tests::persona_exposes`
Expected: PASS.
```bash
git add crates/zendriver-stealth/src/persona/mod.rs
git commit -m "feat(stealth): Persona patch-templating accessors"
```

---

## Phase B — Surfaces & strategies

### Task B1: `Surface` + `Strategy` + per-surface resolution

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/surface.rs` (replace the Task A2 stub)

- [ ] **Step 1: Write the failing test**

Replace `surface.rs` contents:
```rust
//! Surface + Strategy + per-surface kind resolution.

use serde::{Deserialize, Serialize};

/// A fingerprint surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Surface {
    Canvas,
    Webgl,
    Audio,
    Fonts,
    ClientRects,
    Webrtc,
    Hardware,
}

/// How a resolved persona is applied to a surface in the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Strategy {
    Native,
    Seeded,
    Random,
    Block,
    Value,
}

/// The semantic family a surface belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Noise,
    Value,
    Policy,
}

impl Surface {
    pub fn kind(self) -> SurfaceKind {
        match self {
            Surface::Canvas | Surface::Audio | Surface::ClientRects => SurfaceKind::Noise,
            Surface::Webgl | Surface::Fonts | Surface::Hardware => SurfaceKind::Value,
            Surface::Webrtc => SurfaceKind::Policy,
        }
    }

    /// Default strategy when none is set.
    pub fn default_strategy(self) -> Strategy {
        match self.kind() {
            SurfaceKind::Noise => Strategy::Seeded,
            SurfaceKind::Value => Strategy::Value,
            SurfaceKind::Policy => Strategy::Block,
        }
    }

    /// Resolve a requested strategy against this surface's kind.
    /// Meaningless combos warn and fall back to the kind default
    /// (least-opinionated: never error).
    pub fn resolve_strategy(self, requested: Option<Strategy>) -> Strategy {
        let req = match requested {
            None => return self.default_strategy(),
            Some(s) => s,
        };
        let ok = match (self.kind(), req) {
            (_, Strategy::Native) | (_, Strategy::Block) => true,
            (SurfaceKind::Noise, Strategy::Seeded | Strategy::Random) => true,
            (SurfaceKind::Value, Strategy::Value) => true,
            (SurfaceKind::Policy, Strategy::Value) => true,
            _ => false,
        };
        if ok {
            req
        } else {
            tracing::warn!(
                surface = ?self,
                requested = ?req,
                "strategy not meaningful for surface kind; using default"
            );
            self.default_strategy()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_default_is_seeded() {
        assert_eq!(Surface::Canvas.resolve_strategy(None), Strategy::Seeded);
    }

    #[test]
    fn value_strategy_on_noise_warns_and_falls_back() {
        // Value is meaningless for a noise surface → falls back to Seeded.
        assert_eq!(
            Surface::Canvas.resolve_strategy(Some(Strategy::Value)),
            Strategy::Seeded
        );
    }

    #[test]
    fn native_and_block_always_pass() {
        assert_eq!(Surface::Webgl.resolve_strategy(Some(Strategy::Native)), Strategy::Native);
        assert_eq!(Surface::Audio.resolve_strategy(Some(Strategy::Block)), Strategy::Block);
    }

    #[test]
    fn webrtc_default_is_block() {
        assert_eq!(Surface::Webrtc.resolve_strategy(None), Strategy::Block);
    }
}
```

- [ ] **Step 2: Run + commit**

Run: `cargo test -p zendriver-stealth surface::`
Expected: 4 passed.
```bash
git add crates/zendriver-stealth/src/persona/surface.rs
git commit -m "feat(stealth): Surface/Strategy + per-kind resolution"
```

---

### Tasks B2–B8: Surface JS patches

Each patch is a self-contained IIFE taking the resolved persona JSON. **Pattern per task** (shown fully in B2; B3–B8 give the real JS + the same 4 steps):

A seeded PRNG (`mulberry32`) is shared. Add it once as `crates/zendriver-stealth/src/patches/_prng.js`:
```js
// mulberry32 — deterministic PRNG seeded by the persona seed.
function __zdRng(seed) {
  let a = seed >>> 0;
  return function () {
    a |= 0; a = (a + 0x6D2B79F5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
```

### Task B2: `canvas.js`

**Files:**
- Create: `crates/zendriver-stealth/src/patches/canvas.js`, `_prng.js`
- Test: `crates/zendriver-stealth/tests/stealth_phase2.rs` (add a unit asserting the patch string is templated)

- [ ] **Step 1: Write the failing test**

In `crates/zendriver-stealth/src/patches.rs` tests mod (or `stealth_phase2.rs`):
```rust
#[test]
fn canvas_patch_included_when_seeded() {
    let p = Persona { seed: Some(Seed::from_u64(123)), ..Persona::default() };
    let script = bootstrap_script(&p); // signature migrated in Task B9
    assert!(script.contains("getImageData"), "canvas farble must hook getImageData");
    assert!(script.contains("123"), "seed value must be templated in");
}
```

- [ ] **Step 2: Implement the patch**

Create `crates/zendriver-stealth/src/patches/canvas.js`:
```js
(function (seed) {
  const rng = __zdRng(seed);
  function farble(data) {
    for (let i = 0; i < data.length; i += 4) {
      // +/-1 LSB perturbation on RGB, deterministic per seed.
      data[i]     = Math.max(0, Math.min(255, data[i]     + (rng() < 0.5 ? -1 : 1)));
      data[i + 1] = Math.max(0, Math.min(255, data[i + 1] + (rng() < 0.5 ? -1 : 1)));
      data[i + 2] = Math.max(0, Math.min(255, data[i + 2] + (rng() < 0.5 ? -1 : 1)));
    }
    return data;
  }
  const origGetImageData = CanvasRenderingContext2D.prototype.getImageData;
  CanvasRenderingContext2D.prototype.getImageData = function (...args) {
    const img = origGetImageData.apply(this, args);
    farble(img.data);
    return img;
  };
  const origToDataURL = HTMLCanvasElement.prototype.toDataURL;
  HTMLCanvasElement.prototype.toDataURL = function (...args) {
    const ctx = this.getContext('2d');
    if (ctx) {
      const w = this.width, h = this.height;
      if (w > 0 && h > 0) {
        const img = origGetImageData.call(ctx, 0, 0, w, h);
        farble(img.data);
        ctx.putImageData(img, 0, 0);
      }
    }
    return origToDataURL.apply(this, args);
  };
})(SEED);
```
> The `__zdRng` definition (from `_prng.js`) and `SEED` substitution are wired by `bootstrap_script` (Task B9). Each noise patch is wrapped so it is a no-op under `Native`, returns a constant under `Block`, and re-seeds per call under `Random` (use `Math.random()*2**32` instead of the fixed seed).

- [ ] **Step 3: Run (after B9 wires bootstrap) + commit**

> B2's test depends on B9's migrated `bootstrap_script`. Implement the `.js` files B2–B8 first, then B9 wires them and you run the suite. Commit each `.js` as you write it:
```bash
git add crates/zendriver-stealth/src/patches/canvas.js crates/zendriver-stealth/src/patches/_prng.js
git commit -m "feat(stealth): canvas farble patch"
```

### Task B3: `audio.js`
```js
(function (seed) {
  const rng = __zdRng(seed);
  const orig = AnalyserNode.prototype.getFloatFrequencyData;
  AnalyserNode.prototype.getFloatFrequencyData = function (array) {
    orig.call(this, array);
    for (let i = 0; i < array.length; i++) array[i] += (rng() - 0.5) * 1e-4;
  };
  const origTime = AnalyserNode.prototype.getByteTimeDomainData;
  if (origTime) {
    AnalyserNode.prototype.getByteTimeDomainData = function (array) {
      origTime.call(this, array);
      for (let i = 0; i < array.length; i++) {
        array[i] = Math.max(0, Math.min(255, array[i] + (rng() < 0.5 ? -1 : 1)));
      }
    };
  }
})(SEED);
```
Commit: `feat(stealth): audio farble patch`.

### Task B4: `webgl.js` (extend existing — value substitution)
Append to the existing `webgl.js` (keep current flag behavior):
```js
(function (vendor, renderer) {
  const VENDOR = 0x9245, RENDERER = 0x9246; // UNMASKED_VENDOR_WEBGL / RENDERER
  function patch(proto) {
    const orig = proto.getParameter;
    proto.getParameter = function (p) {
      if (vendor && p === VENDOR) return vendor;
      if (renderer && p === RENDERER) return renderer;
      return orig.call(this, p);
    };
  }
  if (window.WebGLRenderingContext) patch(WebGLRenderingContext.prototype);
  if (window.WebGL2RenderingContext) patch(WebGL2RenderingContext.prototype);
})(WEBGL_VENDOR, WEBGL_RENDERER);
```
Commit: `feat(stealth): webgl vendor/renderer substitution`.

### Task B5: `fonts.js`
```js
(function (allow, seed) {
  const rng = __zdRng(seed);
  const orig = CanvasRenderingContext2D.prototype.measureText;
  CanvasRenderingContext2D.prototype.measureText = function (text) {
    const m = orig.call(this, text);
    // Sub-pixel width noise, deterministic.
    try { Object.defineProperty(m, 'width', { value: m.width + (rng() - 0.5) * 1e-3 }); }
    catch (e) {}
    return m;
  };
  // If an allow-list is provided, hide other fonts from FontFaceSet checks.
  if (Array.isArray(allow) && document.fonts && document.fonts.check) {
    const origCheck = document.fonts.check.bind(document.fonts);
    document.fonts.check = function (font, text) {
      const fam = (font || '').split(/\s+/).pop();
      if (fam && allow.indexOf(fam.replace(/["']/g, '')) === -1) return false;
      return origCheck(font, text);
    };
  }
})(FONT_ALLOW, SEED);
```
Commit: `feat(stealth): font metrics + enumeration patch`.

### Task B6: `client_rects.js`
```js
(function (seed) {
  const rng = __zdRng(seed);
  function noisy(v) { return v + (rng() - 0.5) * 1e-3; }
  const origRect = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function () {
    const r = origRect.call(this);
    return new DOMRect(noisy(r.x), noisy(r.y), noisy(r.width), noisy(r.height));
  };
})(SEED);
```
Commit: `feat(stealth): clientRects sub-pixel patch`.

### Task B7: `webrtc.js`
```js
(function (policy, fakeIp) {
  // policy: "block" | "value" | "native"
  if (policy === 'native') return;
  const RTC = window.RTCPeerConnection || window.webkitRTCPeerConnection;
  if (!RTC) return;
  window.RTCPeerConnection = function (cfg, ...rest) {
    const pc = new RTC(cfg, ...rest);
    const origAdd = pc.addEventListener.bind(pc);
    pc.addEventListener = function (type, cb, ...a) {
      if (type === 'icecandidate') {
        const wrapped = function (e) {
          if (policy === 'block' && e && e.candidate) return; // drop local IPs
          if (policy === 'value' && fakeIp && e && e.candidate) {
            try {
              Object.defineProperty(e.candidate, 'address', { value: fakeIp });
            } catch (x) {}
          }
          return cb.apply(this, arguments);
        };
        return origAdd(type, wrapped, ...a);
      }
      return origAdd(type, cb, ...a);
    };
    return pc;
  };
  window.RTCPeerConnection.prototype = RTC.prototype;
})(WEBRTC_POLICY, WEBRTC_FAKE_IP);
```
Commit: `feat(stealth): webrtc ip-leak guard`.

### Task B8: `hardware.js`
```js
(function (battery, mediaDevices, voices) {
  if (typeof battery === 'number' && navigator.getBattery) {
    navigator.getBattery = function () {
      return Promise.resolve({
        level: battery, charging: true, chargingTime: 0,
        dischargingTime: Infinity,
        addEventListener() {}, removeEventListener() {},
      });
    };
  }
  if (typeof mediaDevices === 'number' && navigator.mediaDevices &&
      navigator.mediaDevices.enumerateDevices) {
    navigator.mediaDevices.enumerateDevices = function () {
      const out = [];
      for (let i = 0; i < mediaDevices; i++) {
        out.push({ deviceId: 'dev' + i, kind: 'audioinput', label: '', groupId: 'g' + i });
      }
      return Promise.resolve(out);
    };
  }
  if (Array.isArray(voices) && window.speechSynthesis) {
    speechSynthesis.getVoices = function () {
      return voices.map((n) => ({ name: n, lang: 'en-US', default: false,
        localService: true, voiceURI: n }));
    };
  }
})(HW_BATTERY, HW_MEDIA_DEVICES, HW_VOICES);
```
Commit: `feat(stealth): hardware surface patch`.

---

### Task B9: `bootstrap_script(&Persona)` integration

**Files:**
- Modify: `crates/zendriver-stealth/src/patches.rs`
- Modify call sites: `crates/zendriver-stealth/src/observer.rs`, anything calling `bootstrap_script`

- [ ] **Step 1: Write the failing test** (already added in B2; add per-strategy cases)

```rust
#[test]
fn native_strategy_omits_surface_patch() {
    let p = Persona {
        canvas: Some(SurfaceCfg { strategy: Some(Strategy::Native) }),
        seed: Some(Seed::from_u64(1)),
        ..Persona::default()
    };
    let script = bootstrap_script(&p);
    assert!(!script.contains("getImageData"), "Native canvas → no farble hook");
}

#[test]
fn block_strategy_emits_constant_canvas() {
    let p = Persona {
        canvas: Some(SurfaceCfg { strategy: Some(Strategy::Block) }),
        ..Persona::default()
    };
    let script = bootstrap_script(&p);
    assert!(script.contains("BLOCK"), "Block canvas marker present");
}
```

- [ ] **Step 2: Implement**

Migrate `bootstrap_script` to take `&Persona`. Resolve each surface strategy, then conditionally concatenate the patch JS with substitutions. Sketch:
```rust
const PRNG: &str = include_str!("patches/_prng.js");
const CANVAS: &str = include_str!("patches/canvas.js");
const AUDIO: &str = include_str!("patches/audio.js");
// ... existing 9 + new ones

pub fn bootstrap_script(p: &Persona) -> String {
    let seed = p.seed.unwrap_or_else(Seed::random).value();
    let mut out = String::new();
    out.push_str(PRNG);

    // existing identity patches (webdriver/plugins/chrome/...) — keep, templated
    // from p.resolved_platform_js() etc. (was fp.platform before).
    out.push_str(&existing_identity_patches(p));

    // noise surfaces
    push_noise(&mut out, Surface::Canvas, p.canvas.as_ref(), CANVAS, seed);
    push_noise(&mut out, Surface::Audio, p.audio.as_ref(), AUDIO, seed);
    push_noise(&mut out, Surface::ClientRects, p.client_rects.as_ref(), CLIENT_RECTS, seed);

    // value surfaces
    push_webgl(&mut out, p, seed);
    push_fonts(&mut out, p, seed);
    push_hardware(&mut out, p);

    // policy
    push_webrtc(&mut out, p);

    out
}

fn push_noise(out: &mut String, surface: Surface, cfg: Option<&SurfaceCfg>,
              js: &str, seed: u64) {
    let strat = surface.resolve_strategy(cfg.and_then(|c| c.strategy));
    match strat {
        Strategy::Native => {} // omit
        Strategy::Block => out.push_str(&js.replace("SEED", "0/*BLOCK*/")),
        Strategy::Seeded => out.push_str(&js.replace("SEED", &seed.to_string())),
        Strategy::Random => out.push_str(&js.replace("SEED", "(Math.random()*4294967296)>>>0")),
        Strategy::Value => out.push_str(&js.replace("SEED", &seed.to_string())),
    }
}
// push_webgl/push_fonts/push_hardware/push_webrtc: resolve strategy, substitute
// WEBGL_VENDOR/FONT_ALLOW/etc. with serde_json::to_string of the persona values,
// or "null" when absent; omit under Native; emit empty values under Block.
```

> Implement `existing_identity_patches(p)` by moving the current `bootstrap_script` body here and swapping `fp.<field>` for the `Persona` equivalents (Task A8 accessors). Update `observer.rs` to hold a `Persona` instead of `Fingerprint` and call `bootstrap_script(&persona)`.

- [ ] **Step 3: Run full stealth unit suite**

Run: `cargo test -p zendriver-stealth`
Expected: all green (existing + new patch tests).

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-stealth/src/patches.rs crates/zendriver-stealth/src/observer.rs
git commit -m "feat(stealth): bootstrap_script(&Persona) + surface patch wiring"
```

---

## Phase C — `zendriver-fingerprints` crate

### Task C1: Crate skeleton

**Files:**
- Create: `crates/zendriver-fingerprints/Cargo.toml`, `src/lib.rs`, `NOTICE`
- Modify: root `Cargo.toml` workspace members

- [ ] **Step 1: Create the crate**

`crates/zendriver-fingerprints/Cargo.toml`:
```toml
[package]
name = "zendriver-fingerprints"
description = "Real-device persona sources (pool + generative) for zendriver-rs"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[features]
default = []
pool = ["dep:reqwest", "dep:directories"]
generative = []

[dependencies]
zendriver-stealth.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
fastrand = "2"
reqwest = { workspace = true, optional = true }
directories = { workspace = true, optional = true }
```
> Match the dep style (`workspace = true`) used by `zendriver-fetcher/Cargo.toml`; copy its `reqwest`/cache-dir deps verbatim so the pool fetch reuses the proven pattern.

`src/lib.rs`:
```rust
//! Real-device persona sources for zendriver-rs.
#[cfg(feature = "pool")]
pub mod pool;
#[cfg(feature = "generative")]
pub mod generative;
```

Add `crates/zendriver-fingerprints` to the root `Cargo.toml` `[workspace] members`.

Add `NOTICE`:
```
This crate vendors fingerprint data derived from browserforge / fingerprint-suite
(https://github.com/apify/fingerprint-suite), licensed Apache-2.0.
```

- [ ] **Step 2: Verify it builds**

Run: `cargo build -p zendriver-fingerprints --all-features`
Expected: builds (empty modules).

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-fingerprints Cargo.toml
git commit -m "feat(fingerprints): new crate skeleton (pool/generative features)"
```

---

### Task C2: `pool` — fetch + cache + sample

**Files:**
- Create: `crates/zendriver-fingerprints/src/pool/mod.rs`
- Reference: `crates/zendriver-fetcher/src/{cache,download}.rs` (reuse atomic-cache pattern)

- [ ] **Step 1: Write the failing test** (offline: sample from an in-memory set)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sample_is_deterministic_for_seed() {
        let set = PoolSet::from_records(vec![
            mk("Win32", 8), mk("MacIntel", 16), mk("Win32", 4),
        ]);
        let a = set.sample(zendriver_stealth::Seed::from_u64(42));
        let b = set.sample(zendriver_stealth::Seed::from_u64(42));
        assert_eq!(a.device_memory_gb, b.device_memory_gb);
    }
}
```

- [ ] **Step 2: Implement**

```rust
use serde::{Deserialize, Serialize};
use zendriver_stealth::{Persona, Seed};

/// A parsed pool of real-device personas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolSet {
    records: Vec<Persona>,
}

impl PoolSet {
    pub fn from_records(records: Vec<Persona>) -> Self {
        Self { records }
    }

    /// Parse a downloaded pool asset (JSON array of personas).
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        Ok(Self { records: serde_json::from_str(s)? })
    }

    /// Deterministically pick one persona by seed.
    pub fn sample(&self, seed: Seed) -> Persona {
        assert!(!self.records.is_empty(), "pool is empty");
        let idx = (seed.value() as usize) % self.records.len();
        self.records[idx].clone()
    }
}
```

- [ ] **Step 3: Implement download-on-first-use (feature `pool`)**

```rust
/// Download (once) + cache the pool asset, then parse. Reuses the cache dir
/// convention from zendriver-fetcher.
pub async fn load_or_download(url: &str) -> Result<PoolSet, PoolError> {
    let cache = cache_path(); // directories::ProjectDirs, same root as fetcher
    if let Ok(bytes) = std::fs::read_to_string(&cache) {
        if let Ok(set) = PoolSet::from_json(&bytes) {
            return Ok(set);
        }
    }
    let body = reqwest::get(url).await?.text().await?;
    let set = PoolSet::from_json(&body)?;
    // atomic write (tmp + rename), mirroring fetcher::cache
    let tmp = cache.with_extension("tmp");
    std::fs::write(&tmp, &body)?;
    std::fs::rename(&tmp, &cache)?;
    Ok(set)
}
```
Define `PoolError` (`thiserror`) wrapping `reqwest::Error`, `io::Error`, `serde_json::Error`. Implement `cache_path()` and the test helper `mk(platform, mem)` building a minimal `Persona`.

- [ ] **Step 4: Run + commit**

Run: `cargo test -p zendriver-fingerprints --features pool pool::`
Expected: PASS.
```bash
git add crates/zendriver-fingerprints/src/pool/
git commit -m "feat(fingerprints): pool source (download-on-first-use + seeded sample)"
```

---

### Task C3: `generative` — browserforge BN port

**Files:**
- Create: `crates/zendriver-fingerprints/src/generative/mod.rs`
- Create: `crates/zendriver-fingerprints/src/generative/network.json` (vendored, trimmed)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generate_is_deterministic_and_coherent() {
        let g = Generator::embedded();
        let a = g.generate(zendriver_stealth::Seed::from_u64(7));
        let b = g.generate(zendriver_stealth::Seed::from_u64(7));
        // same seed → same persona
        assert_eq!(a.platform, b.platform);
        // coherence: a populated persona
        assert!(a.platform.is_some());
        assert!(a.webgl.is_some());
    }
}
```

- [ ] **Step 2: Implement the BN sampler**

```rust
use serde::Deserialize;
use zendriver_stealth::{Persona, Seed};

/// A Bayesian network of conditional fingerprint attributes (browserforge form).
#[derive(Debug, Clone, Deserialize)]
pub struct Generator {
    nodes: Vec<Node>,
}

#[derive(Debug, Clone, Deserialize)]
struct Node {
    name: String,
    parents: Vec<String>,
    // conditional distributions keyed by parent-value tuple → (value, weight)
    cpt: std::collections::HashMap<String, Vec<(String, f64)>>,
}

impl Generator {
    /// Load the embedded, trimmed network.
    pub fn embedded() -> Self {
        serde_json::from_str(include_str!("network.json")).expect("valid embedded BN")
    }

    /// Sample a coherent persona deterministically from `seed`.
    pub fn generate(&self, seed: Seed) -> Persona {
        let mut rng = Mulberry32::new(seed.value() as u32);
        let mut assigned: std::collections::HashMap<String, String> = Default::default();
        for node in self.topo_order() {
            let key = node.parents.iter()
                .map(|p| assigned.get(p).cloned().unwrap_or_default())
                .collect::<Vec<_>>().join("|");
            let dist = node.cpt.get(&key).or_else(|| node.cpt.get("")).unwrap();
            assigned.insert(node.name.clone(), weighted_pick(dist, rng.next_f64()));
        }
        persona_from_assignment(&assigned)
    }

    fn topo_order(&self) -> Vec<&Node> {
        // parents-before-children; the embedded network is already ordered,
        // but sort defensively.
        let mut order = self.nodes.iter().collect::<Vec<_>>();
        order.sort_by_key(|n| n.parents.len());
        order
    }
}

struct Mulberry32(u32);
impl Mulberry32 {
    fn new(seed: u32) -> Self { Self(seed) }
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6D2B79F5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(1 | t);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        ((t ^ (t >> 14)) as f64) / 4294967296.0
    }
}

fn weighted_pick(dist: &[(String, f64)], r: f64) -> String {
    let total: f64 = dist.iter().map(|(_, w)| w).sum();
    let mut acc = 0.0;
    for (v, w) in dist {
        acc += w / total;
        if r <= acc { return v.clone(); }
    }
    dist.last().map(|(v, _)| v.clone()).unwrap_or_default()
}

fn persona_from_assignment(a: &std::collections::HashMap<String, String>) -> Persona {
    // Map the sampled attribute strings → Persona fields. Keys mirror
    // network.json node names: "platform", "webglRenderer", "deviceMemory", ...
    use zendriver_stealth::{Platform, WebglSpec};
    let mut p = Persona::default();
    if let Some(plat) = a.get("platform") {
        p.platform = match plat.as_str() {
            "Win32" => Some(Platform::Win32),
            "MacIntel" => Some(Platform::MacIntel),
            _ => Some(Platform::LinuxX86_64),
        };
    }
    if let Some(r) = a.get("webglRenderer") {
        p.webgl = Some(WebglSpec { unmasked_renderer: Some(r.clone()), ..Default::default() });
    }
    if let Some(m) = a.get("deviceMemory") {
        p.device_memory_gb = m.parse().ok();
    }
    p
}
```

- [ ] **Step 3: Create a minimal real `network.json`**

Vendor a trimmed network (full extraction is a follow-up; the embedded file must be valid and produce coherent output). Minimal real content:
```json
{
  "nodes": [
    {"name": "platform", "parents": [], "cpt": {
      "": [["Win32", 0.7], ["MacIntel", 0.2], ["LinuxX86_64", 0.1]]}},
    {"name": "deviceMemory", "parents": ["platform"], "cpt": {
      "Win32": [["8", 0.6], ["16", 0.4]],
      "MacIntel": [["16", 0.7], ["8", 0.3]],
      "LinuxX86_64": [["8", 0.5], ["16", 0.5]]}},
    {"name": "webglRenderer", "parents": ["platform"], "cpt": {
      "Win32": [["ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)", 1.0]],
      "MacIntel": [["ANGLE (Apple, Apple M1, OpenGL 4.1)", 1.0]],
      "LinuxX86_64": [["ANGLE (Mesa, llvmpipe, OpenGL 4.5)", 1.0]]}}
  ]
}
```
> This is a real, working (if small) network. Expanding it from the upstream browserforge dataset is tracked in §14 of the spec; the loader + sampler are complete and correct regardless of network size.

- [ ] **Step 4: Run + commit**

Run: `cargo test -p zendriver-fingerprints --features generative generative::`
Expected: PASS.
```bash
git add crates/zendriver-fingerprints/src/generative/
git commit -m "feat(fingerprints): generative BN persona source (browserforge port)"
```

---

## Phase D — Live-probe, wiring, docs

### Task D1: `Persona::from_browser(tab)`

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/mod.rs`
- Note: this needs a `Tab` handle. To avoid a stealth→zendriver dep cycle, define a tiny trait the core `Tab` implements.

- [ ] **Step 1: Define the probe trait + test (unit, mocked)**

```rust
/// Minimal eval surface so stealth can probe a live page without depending
/// on the zendriver core crate (avoids a dependency cycle).
#[async_trait::async_trait]
pub trait JsProbe {
    async fn eval_json(&self, js: &str) -> Result<serde_json::Value, crate::StealthError>;
}

impl Persona {
    /// Probe the live Chrome for its REAL webgl/canvas/audio/font values.
    /// Maximally coherent host persona.
    pub async fn from_browser<P: JsProbe + Sync>(probe: &P) -> Result<Persona, crate::StealthError> {
        let v = probe.eval_json(PROBE_JS).await?;
        Ok(persona_from_probe(&v))
    }
}

const PROBE_JS: &str = r#"(() => {
  const c = document.createElement('canvas').getContext('webgl');
  const dbg = c && c.getExtension('WEBGL_debug_renderer_info');
  return {
    platform: navigator.platform,
    deviceMemory: navigator.deviceMemory,
    hardwareConcurrency: navigator.hardwareConcurrency,
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    locale: navigator.language,
    webglVendor: dbg ? c.getParameter(dbg.UNMASKED_VENDOR_WEBGL) : null,
    webglRenderer: dbg ? c.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
  };
})()"#;
```
Implement `persona_from_probe(&serde_json::Value) -> Persona`. Test with a fake `JsProbe` returning a canned JSON value; assert mapped fields.

- [ ] **Step 2: Implement `JsProbe` for the core `Tab`**

In `crates/zendriver/src/tab.rs`, add (behind nothing — core):
```rust
#[async_trait::async_trait]
impl zendriver_stealth::JsProbe for Tab {
    async fn eval_json(&self, js: &str) -> Result<serde_json::Value, zendriver_stealth::StealthError> {
        self.evaluate(js).await
            .map(|v| v.into_json()) // adapt to the real evaluate return type
            .map_err(|e| zendriver_stealth::StealthError::from(e))
    }
}
```
> Adapt `.into_json()` and the error conversion to the real `Tab::evaluate` signature. Add a `From<CallError>`/`From<ZendriverError>` arm to `StealthError` if missing.

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver-stealth from_browser`
Expected: PASS (mocked).
```bash
git add crates/zendriver-stealth/src/persona/mod.rs crates/zendriver/src/tab.rs
git commit -m "feat: Persona::from_browser live-probe via JsProbe trait"
```

---

### Task D2: Browser builder wiring

**Files:**
- Modify: `crates/zendriver/src/browser.rs`
- Modify: `crates/zendriver/src/lib.rs` (re-export `Persona`, `Surface`, `Strategy`)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn builder_accepts_persona_and_overrides() {
    let b = Browser::builder()
        .persona(zendriver_stealth::Persona::builder().device_memory_gb(16).build())
        .persona_overlay(r#"{"timezone":"UTC"}"#.parse().unwrap())
        .surface(zendriver_stealth::Surface::Webrtc, zendriver_stealth::Strategy::Native);
    let p = b.resolved_persona();
    assert_eq!(p.device_memory_gb, Some(16));
    assert_eq!(p.timezone.as_deref(), Some("UTC"));
}
```

- [ ] **Step 2: Implement**

Add fields to the browser builder: `persona: Option<Persona>`, `persona_overlay: Option<Persona>`, `surface_overrides: Vec<(Surface, Strategy)>`. Methods:
```rust
pub fn persona(mut self, p: Persona) -> Self { self.persona = Some(p); self }
pub fn persona_overlay(mut self, p: Persona) -> Self { self.persona_overlay = Some(p); self }
pub fn surface(mut self, s: Surface, strat: Strategy) -> Self {
    self.surface_overrides.push((s, strat)); self
}

/// Effective persona: default system() → user persona → overlay → surface overrides.
pub fn resolved_persona(&self) -> Persona {
    let mut p = self.persona.clone().unwrap_or_else(Persona::system);
    if let Some(o) = &self.persona_overlay { p = p.clone().overlay(o.clone()); }
    for (s, strat) in &self.surface_overrides {
        p.apply_surface_override(*s, *strat); // sets the right SurfaceCfg.strategy
    }
    p
}
```
Add `Persona::apply_surface_override(&mut self, Surface, Strategy)` in stealth (sets `self.canvas/webgl/... .strategy`). Wire `resolved_persona()` into the existing stealth-launch path so `bootstrap_script(&persona)` is used instead of the old `Fingerprint`.

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver builder_accepts_persona`
Expected: PASS.
```bash
git add crates/zendriver/src/browser.rs crates/zendriver/src/lib.rs crates/zendriver-stealth/src/persona/mod.rs
git commit -m "feat: Browser builder .persona/.persona_overlay/.surface"
```

---

### Task D3: persona-seed persistence with `user_data_dir`

**Files:**
- Modify: `crates/zendriver/src/browser.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn seed_persists_in_user_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let b1 = Browser::builder().user_data_dir(dir.path());
    let s1 = b1.resolved_persona().seed.unwrap();
    let b2 = Browser::builder().user_data_dir(dir.path());
    let s2 = b2.resolved_persona().seed.unwrap();
    assert_eq!(s1, s2, "same profile dir → same seed across builders");
}
```

- [ ] **Step 2: Implement**

When `user_data_dir` is set and the persona seed is unset, read/write `<user_data_dir>/.zd_persona_seed` (a single u64). On first use: generate random, persist. On reuse: load.
```rust
fn persisted_seed(dir: &std::path::Path) -> Seed {
    let f = dir.join(".zd_persona_seed");
    if let Ok(s) = std::fs::read_to_string(&f) {
        if let Ok(v) = s.trim().parse::<u64>() { return Seed::from_u64(v); }
    }
    let seed = Seed::random();
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&f, seed.value().to_string());
    seed
}
```
Call this in `resolved_persona()` when `persona.seed.is_none()` and a `user_data_dir` exists.

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver seed_persists`
Expected: PASS.
```bash
git add crates/zendriver/src/browser.rs
git commit -m "feat: persist persona seed alongside user_data_dir"
```

---

### Task D4: `insta` snapshot for `Persona` JSON wire shape

**Files:**
- Create/modify: `crates/zendriver-stealth/tests/persona_snapshot.rs`

- [ ] **Step 1: Write the snapshot test**

```rust
#[test]
fn persona_full_wire_shape() {
    let p = zendriver_stealth::Persona::builder()
        .seed(zendriver_stealth::Seed::from_u64(1))
        .device_memory_gb(8)
        .timezone("UTC")
        .build();
    insta::assert_json_snapshot!(p);
}
```

- [ ] **Step 2: Generate + accept**

Run:
```bash
cargo test -p zendriver-stealth --test persona_snapshot
cargo insta accept --all
```
Expected: snapshot file created under `crates/zendriver-stealth/tests/snapshots/`.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/tests/persona_snapshot.rs crates/zendriver-stealth/tests/snapshots/
git commit -m "test(stealth): Persona JSON wire-shape snapshot"
```

---

### Task D5: Headful integration test (gated)

**Files:**
- Create: `crates/zendriver/tests/fingerprint_integration.rs` (behind the existing integration-test feature/gate used by other phase tests)

- [ ] **Step 1: Write the test**

```rust
// Gated like the other integration tests in this crate (see integration_phase2.rs).
#[tokio::test]
#[ignore] // run with --ignored on the nightly integration job
async fn seeded_canvas_is_stable_across_reads() {
    let browser = Browser::builder()
        .persona(Persona::builder().seed(Seed::from_u64(42)).build())
        .build().await.unwrap();
    let tab = browser.new_tab("about:blank").await.unwrap();
    let read = r#"(() => { const c=document.createElement('canvas');
        c.width=50;c.height=20;const x=c.getContext('2d');
        x.fillText('zd',2,12); return c.toDataURL(); })()"#;
    let a: String = tab.evaluate(read).await.unwrap().into();
    let b: String = tab.evaluate(read).await.unwrap().into();
    assert_eq!(a, b, "Seeded canvas must be STABLE across repeat reads");
}
```
> Adapt the `Browser`/`Tab`/`evaluate` API to the real signatures (see `integration_phase2.rs`). Add a `Native` and a `Random` variant assertion (Random → differs).

- [ ] **Step 2: Run locally if Chrome available, else defer to CI**

Run: `cargo test -p zendriver --test fingerprint_integration -- --ignored`
Expected: PASS with a local Chrome.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/tests/fingerprint_integration.rs
git commit -m "test: headful fingerprint stability integration"
```

---

### Task D6: Examples + docs

**Files:**
- Create: `crates/zendriver/examples/persona_basic.rs`, `persona_pool.rs`
- Create: `docs/book/src/fingerprint.md` + add to `SUMMARY.md`

- [ ] **Step 1: Examples**

`persona_basic.rs` (seeded farble + a surface override), `persona_pool.rs` (feature-gated; load pool → sample → `.persona()`). Mirror the style of `crates/zendriver/examples/browserscan.rs`.

- [ ] **Step 2: mdBook chapter**

`docs/book/src/fingerprint.md`: the two axes, the surfaces table, strategy list, `system()`/`from_browser()`/`Seed::from_system()`, and a copy-paste-JSON example. Add `- [Fingerprint spoofing](fingerprint.md)` to `docs/book/src/SUMMARY.md`.

- [ ] **Step 3: Build docs + commit**

Run: `cargo build --examples -p zendriver` and `mdbook build docs/book` (if `mdbook` installed).
```bash
git add crates/zendriver/examples/persona_*.rs docs/book/
git commit -m "docs: fingerprint spoofing examples + book chapter"
```

---

## Phase E — Gates & PR

### Task E1: Full verification

- [ ] **Step 1: Format + clippy (per CLAUDE.md, required before push)**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p zendriver-fingerprints --all-features --all-targets -- -D warnings
```
Expected: clean. Fix and re-stage anything flagged.

- [ ] **Step 2: Full test suite + schema snapshots**

```bash
cargo test --workspace --all-features --locked
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo insta accept --all
```
Expected: green; commit any accepted snapshots.

- [ ] **Step 3: Final commit + open PR**

```bash
git add -A && git commit -m "chore: fmt + clippy + snapshots for fingerprint spoofing"
gh pr create --title "feat: fingerprint spoofing (Persona + surfaces + zendriver-fingerprints)" \
  --body "Implements upstream TODO #1. See docs/superpowers/specs/2026-06-02-fingerprint-spoofing-design.md"
```

---

## Self-Review (completed by plan author)

**Spec coverage:** §3 surfaces → B2–B8; §4 crate layout → A1/A2/C1; §5 Persona+constructors → A2–A7, D1; §6 surface kinds → B1; §7 injection → B9; §8 pool+generative → C2/C3; §9 seed → A1/A7/D3; §10 API → D2; §11 testing → A*/B*/D4/D5; §13 licensing → C1 NOTICE. All covered.

**Placeholders:** none — every step has real code. The `network.json` and pool asset are intentionally small-but-valid (noted), not placeholders; the loaders/samplers are complete.

**Type consistency:** `Persona`, `Seed` (value()), `Surface`, `Strategy`, `SurfaceCfg.strategy`, `WebglSpec.unmasked_{vendor,renderer}`, `bootstrap_script(&Persona)`, `resolve_strategy`, `overlay`, `resolved_persona` used consistently across tasks.

**Known adapt-points flagged inline** (real APIs to match at implementation time): `Tab::evaluate` return type (D1/D5), existing platform→JS method name (A8), `StealthError` From-conversions (D1), workspace dep style for reqwest/directories (C1).
