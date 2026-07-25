# GPU Backend + Probe Tooling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship an opt-in `GpuBackend` that lets Chrome use the host GPU, plus the probe tooling and drift canaries that later phases depend on.

**Architecture:** A `GpuBackend` enum in `zendriver-stealth` owns the ANGLE launch-flag decision and the per-OS backend mapping. `BrowserBuilder` is the single authority: it suppresses `--disable-gpu` and propagates the backend into the stealth profile at launch. A probe example dumps a real browser's GPU surface as JSON, and two `#[ignore]` real-Chrome tests pin ANGLE's behavior so later table work has a baseline that fails loudly when Chrome moves.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), tokio, `insta` snapshots, `rmcp` for the MCP layer, Chrome DevTools Protocol.

**Source spec:** `docs/superpowers/specs/2026-07-24-gpu-spoofing-design.md` (phases 1–2).

## Global Constraints

- Edition 2024, MSRV 1.85. Nine-crate workspace.
- `GpuBackend::Disabled` is the default and MUST reproduce today's flags byte-for-byte. The existing `insta` flag snapshots are the regression guard — if `native_profile_flags`, `spoofed_profile_flags`, or `off_profile_flags` change, the default path broke.
- No auto behavior. Never detect a GPU and switch backends. Never silently fall back from `Native` to SwiftShader.
- Before any push: `cargo fmt --all` then `cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged`, then confirm `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings` both pass.
- Any MCP input/output type change requires `cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked` then `cargo insta accept --all`, committing the updated `crates/zendriver-mcp/tests/snapshots/*.snap`.
- Any public-API change requires a ledger entry in `crates/zendriver-mcp/mcp-coverage-ledger.toml` (`covered` or `excluded`) and a regenerated `crates/zendriver-mcp/public-api-baseline.txt`.
- Real-Chrome tests are `#[ignore]` by convention in this repo and must skip cleanly (not fail) when the host lacks a GPU.

---

### Task 1: `GpuBackend` enum and flag mapping

Pure, dependency-free logic. No wiring yet, so it can be tested exhaustively on any host regardless of its actual GPU.

**Files:**
- Modify: `crates/zendriver-stealth/src/flags.rs` (add above `shared_stealth_flags`, currently line 19)
- Modify: `crates/zendriver-stealth/src/lib.rs` (re-export)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub enum GpuBackend { Disabled, SwiftShader, Native }` — `Default` is `Disabled`; derives `Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize` with `#[serde(rename_all = "snake_case")]`.
  - `pub fn GpuBackend::angle_flags(self) -> Vec<String>`
  - `pub fn GpuBackend::allows_disable_gpu(self) -> bool`
  - private `fn angle_backend_for_os(os: &str) -> &'static str`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/zendriver-stealth/src/flags.rs` (the block starts at line 93):

```rust
    // --- GpuBackend ---------------------------------------------------------

    #[test]
    fn gpu_backend_default_is_disabled() {
        assert_eq!(GpuBackend::default(), GpuBackend::Disabled);
    }

    #[test]
    fn disabled_backend_emits_no_angle_flags() {
        assert!(GpuBackend::Disabled.angle_flags().is_empty());
    }

    #[test]
    fn swiftshader_backend_emits_todays_three_flags() {
        assert_eq!(
            GpuBackend::SwiftShader.angle_flags(),
            vec![
                "--use-gl=angle".to_string(),
                "--use-angle=swiftshader".to_string(),
                "--enable-unsafe-swiftshader".to_string(),
            ]
        );
    }

    #[test]
    fn native_backend_maps_os_to_angle_backend() {
        assert_eq!(angle_backend_for_os("macos"), "metal");
        assert_eq!(angle_backend_for_os("windows"), "d3d11");
        assert_eq!(angle_backend_for_os("linux"), "vulkan");
        // Unknown platforms take the Linux path rather than emitting nothing —
        // an empty backend would silently fall back to Chrome's default pick.
        assert_eq!(angle_backend_for_os("freebsd"), "vulkan");
    }

    #[test]
    fn native_backend_emits_angle_flags_for_current_os() {
        let flags = GpuBackend::Native.angle_flags();
        assert_eq!(flags[0], "--use-gl=angle");
        assert!(
            flags[1].starts_with("--use-angle="),
            "expected an explicit backend, got: {flags:?}"
        );
        // Never SwiftShader under Native — that is the whole point.
        assert!(!flags.iter().any(|f| f.contains("swiftshader")), "got: {flags:?}");
    }

    #[test]
    fn only_native_forbids_disable_gpu() {
        // Measured: dropping --disable-gpu without naming a backend hangs
        // Chrome, and keeping it with a backend suppresses the GPU entirely.
        // So the two decisions are coupled and Native owns both.
        assert!(GpuBackend::Disabled.allows_disable_gpu());
        assert!(GpuBackend::SwiftShader.allows_disable_gpu());
        assert!(!GpuBackend::Native.allows_disable_gpu());
    }

    #[test]
    fn gpu_backend_round_trips_json_as_snake_case() {
        let json = serde_json::to_string(&GpuBackend::SwiftShader).unwrap();
        assert_eq!(json, "\"swift_shader\"");
        let back: GpuBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(back, GpuBackend::SwiftShader);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p zendriver-stealth --lib flags::tests 2>&1 | tail -20
```

Expected: FAIL — `cannot find type GpuBackend in this scope`.

- [ ] **Step 3: Write the implementation**

Insert into `crates/zendriver-stealth/src/flags.rs`, immediately after the `use crate::ProfileKind;` line:

```rust
use serde::{Deserialize, Serialize};

/// Which GPU backend Chrome should render WebGL / WebGPU with.
///
/// Defaults to [`Disabled`](Self::Disabled), which reproduces zendriver's
/// historical launch flags exactly. This is an explicit opt-in: zendriver
/// never probes the host for a GPU and never switches backends on its own.
///
/// # Why the backend must be named explicitly
///
/// Removing `--disable-gpu` without also naming an ANGLE backend **hangs**
/// headless Chrome (measured on darwin; see the design spec's Measurements
/// section). The two decisions are therefore coupled and owned together here,
/// rather than left to the caller to combine correctly.
///
/// ```no_run
/// use zendriver::GpuBackend;
/// // Use the host's real GPU instead of a software rasterizer.
/// let builder = zendriver::Browser::builder().gpu_backend(GpuBackend::Native);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuBackend {
    /// Today's behavior: `--disable-gpu` under headless and no ANGLE backend
    /// forced. Chrome picks its own fallback.
    #[default]
    Disabled,
    /// Force SwiftShader's CPU rasterizer. Guarantees a working WebGL context
    /// on a host with no GPU, at the cost of a software-rasterizer
    /// fingerprint that no real device produces.
    SwiftShader,
    /// Render on the host GPU through the platform's ANGLE backend (macOS →
    /// Metal, Windows → D3D11, otherwise Vulkan).
    ///
    /// Gives a fully coherent GPU surface — real capabilities, real pixels,
    /// real timings, a working `requestDevice()` — but reports **the host's**
    /// GPU, not a chosen one. A fleet sharing one host shares one fingerprint.
    ///
    /// Requires a usable GPU. There is no automatic fallback: if the GPU
    /// process cannot start, the launch fails with an actionable error rather
    /// than silently degrading to software rendering.
    Native,
}

/// ANGLE backend token for an OS name as reported by [`std::env::consts::OS`].
///
/// Unknown platforms take the Vulkan path rather than returning nothing —
/// an absent backend is what causes the launch hang described on
/// [`GpuBackend`].
fn angle_backend_for_os(os: &str) -> &'static str {
    match os {
        "macos" => "metal",
        "windows" => "d3d11",
        _ => "vulkan",
    }
}

impl GpuBackend {
    /// Launch flags selecting this backend. Empty for
    /// [`Disabled`](Self::Disabled).
    #[must_use]
    pub fn angle_flags(self) -> Vec<String> {
        match self {
            Self::Disabled => Vec::new(),
            Self::SwiftShader => vec![
                "--use-gl=angle".into(),
                "--use-angle=swiftshader".into(),
                // Chrome >= 116 refuses the SwiftShader fallback without this.
                "--enable-unsafe-swiftshader".into(),
            ],
            Self::Native => vec![
                "--use-gl=angle".into(),
                format!("--use-angle={}", angle_backend_for_os(std::env::consts::OS)),
            ],
        }
    }

    /// Whether `--disable-gpu` may still be emitted under headless.
    ///
    /// False only for [`Native`](Self::Native), where the flag would defeat
    /// the entire point of selecting a hardware backend.
    #[must_use]
    pub fn allows_disable_gpu(self) -> bool {
        !matches!(self, Self::Native)
    }
}
```

- [ ] **Step 4: Re-export from the crate root**

`crates/zendriver-stealth/src/lib.rs` currently re-exports nothing from `flags`. Add a new line immediately after line 57 (`pub use profile::{Platform, ProfileKind, StealthProfile};`):

```rust
pub use flags::GpuBackend;
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p zendriver-stealth --lib flags::tests 2>&1 | tail -20
```

Expected: PASS, including the seven new tests.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/zendriver-stealth/src/flags.rs crates/zendriver-stealth/src/lib.rs
git commit -m "feat(stealth): add GpuBackend with per-OS ANGLE flag mapping"
```

---

### Task 2: Thread `GpuBackend` through `flags_for_profile` and `StealthProfile`

`StealthProfile` already carries two comparable opt-ins (`native_isolation`, `native_webgl`), so this follows that established shape.

**Files:**
- Modify: `crates/zendriver-stealth/src/flags.rs:66` (`flags_for_profile` signature and `Spoofed` arm at lines 83–85)
- Modify: `crates/zendriver-stealth/src/profile.rs:80-95` (struct), `:516` (`build_flags`)

**Interfaces:**
- Consumes: `GpuBackend`, `GpuBackend::angle_flags` (Task 1).
- Produces:
  - `pub fn flags_for_profile(kind: ProfileKind, native_isolation: bool, gpu_backend: GpuBackend) -> Vec<String>`
  - `pub fn StealthProfile::gpu_backend(self, backend: GpuBackend) -> Self` (consuming builder setter, `#[must_use]`)
  - `pub(crate) gpu_backend: GpuBackend` field on `StealthProfile`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/zendriver-stealth/src/flags.rs`:

```rust
    #[test]
    fn spoofed_default_backend_still_emits_swiftshader_flags() {
        // Regression guard: the default path must be byte-for-byte unchanged.
        let flags = flags_for_profile(ProfileKind::Spoofed, false, GpuBackend::Disabled);
        assert!(flags.iter().any(|f| f == "--use-angle=swiftshader"));
        assert!(flags.iter().any(|f| f == "--enable-unsafe-swiftshader"));
    }

    #[test]
    fn spoofed_native_backend_replaces_swiftshader_flags() {
        let flags = flags_for_profile(ProfileKind::Spoofed, false, GpuBackend::Native);
        assert!(
            !flags.iter().any(|f| f.contains("swiftshader")),
            "Native must not carry SwiftShader flags, got: {flags:?}"
        );
        assert!(flags.iter().any(|f| f.starts_with("--use-angle=")));
    }

    #[test]
    fn native_profile_gains_angle_flags_only_when_backend_selected() {
        // The Native *profile* emits no GPU flags today; selecting a backend
        // is what adds them, for every non-Off profile kind.
        let default = flags_for_profile(ProfileKind::Native, false, GpuBackend::Disabled);
        assert!(!default.iter().any(|f| f.starts_with("--use-angle=")));

        let native_gpu = flags_for_profile(ProfileKind::Native, false, GpuBackend::Native);
        assert!(native_gpu.iter().any(|f| f.starts_with("--use-angle=")));
    }

    #[test]
    fn off_profile_stays_empty_under_every_backend() {
        // `off()` is documented as a truly stock launch and its doctest
        // asserts `build_flags().is_empty()`. A GPU backend must not break it.
        for backend in [GpuBackend::Disabled, GpuBackend::SwiftShader, GpuBackend::Native] {
            assert!(
                flags_for_profile(ProfileKind::Off, false, backend).is_empty(),
                "Off profile must stay stock under {backend:?}"
            );
        }
    }
```

Add to `mod tests` in `crates/zendriver-stealth/src/profile.rs`:

```rust
    #[test]
    fn stealth_profile_gpu_backend_defaults_to_disabled() {
        assert_eq!(
            StealthProfile::spoofed().build_flags(),
            StealthProfile::spoofed()
                .gpu_backend(crate::GpuBackend::Disabled)
                .build_flags(),
            "Disabled must be indistinguishable from not setting a backend"
        );
    }

    #[test]
    fn stealth_profile_gpu_backend_reaches_build_flags() {
        let flags = StealthProfile::spoofed()
            .gpu_backend(crate::GpuBackend::Native)
            .build_flags();
        assert!(flags.iter().any(|f| f.starts_with("--use-angle=")));
        assert!(!flags.iter().any(|f| f.contains("swiftshader")), "got: {flags:?}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p zendriver-stealth --lib 2>&1 | tail -20
```

Expected: FAIL — `this function takes 2 arguments but 3 arguments were supplied`, and `no method named gpu_backend`.

- [ ] **Step 3: Update `flags_for_profile`**

In `crates/zendriver-stealth/src/flags.rs`, change the signature and the `Spoofed` arm. Replace the whole `pub fn flags_for_profile` body with:

```rust
#[must_use]
pub fn flags_for_profile(
    kind: ProfileKind,
    native_isolation: bool,
    gpu_backend: GpuBackend,
) -> Vec<String> {
    match kind {
        // Off stays a truly stock launch under every backend — selecting a
        // GPU backend on an Off profile is a no-op by design.
        ProfileKind::Off => Vec::new(),
        ProfileKind::Native => {
            let mut v = shared_stealth_flags(native_isolation);
            v.extend(gpu_backend.angle_flags());
            v
        }
        ProfileKind::Spoofed => {
            let mut v = shared_stealth_flags(native_isolation);
            // A WebGL *context* must exist at all in headless — a null context
            // is itself a bot tell. Historically that was guaranteed by
            // unconditionally forcing SwiftShader here. That default is now
            // expressed as `GpuBackend::Disabled` keeping the SwiftShader
            // flags, while an explicit backend replaces them.
            match gpu_backend {
                GpuBackend::Disabled => {
                    v.extend(GpuBackend::SwiftShader.angle_flags());
                }
                explicit => v.extend(explicit.angle_flags()),
            }
            v
        }
    }
}
```

Update the existing doc comment above the function to describe the new third parameter.

- [ ] **Step 4: Update the four existing call sites in `flags.rs` tests**

The pre-existing tests at lines 98, 103, 113, 119, 125, 131, 139, 153, 167 and 178 call `flags_for_profile` with two arguments. Add `, GpuBackend::Disabled` to each so the snapshots stay anchored to the default path.

- [ ] **Step 5: Add the `StealthProfile` field and setter**

In `crates/zendriver-stealth/src/profile.rs`, add to the struct (after `native_webgl` at line 91):

```rust
    /// GPU backend for the launch flags. Defaults to
    /// [`GpuBackend::Disabled`] — today's behavior. `BrowserBuilder` overrides
    /// this at launch when its own `gpu_backend` was set.
    pub(crate) gpu_backend: crate::GpuBackend,
```

Add the setter next to the other opt-in setters:

```rust
    /// Select the GPU backend Chrome renders WebGL / WebGPU with.
    ///
    /// Defaults to [`GpuBackend::Disabled`](crate::GpuBackend::Disabled),
    /// which reproduces zendriver's historical flags exactly. See
    /// [`GpuBackend`](crate::GpuBackend) for what each variant costs.
    ///
    /// ```
    /// use zendriver_stealth::{GpuBackend, StealthProfile};
    /// let flags = StealthProfile::spoofed().gpu_backend(GpuBackend::Native).build_flags();
    /// assert!(flags.iter().any(|f| f.starts_with("--use-angle=")));
    /// ```
    #[must_use]
    pub fn gpu_backend(mut self, backend: crate::GpuBackend) -> Self {
        self.gpu_backend = backend;
        self
    }
```

Update `build_flags` at line 516:

```rust
        let mut flags =
            crate::flags::flags_for_profile(self.kind, self.native_isolation, self.gpu_backend);
```

If `StealthProfile` has manual constructors (`off()`, `native()`, `spoofed()`) that build the struct literally rather than via `..Default::default()`, add `gpu_backend: crate::GpuBackend::Disabled` to each.

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test -p zendriver-stealth 2>&1 | tail -20
```

Expected: PASS. Critically, the four `insta` flag snapshots must **not** change — if `cargo insta` reports pending snapshots, the default path regressed and the cause must be fixed rather than accepted.

- [ ] **Step 7: Confirm no snapshot drift**

```bash
cargo insta pending-snapshots --workspace 2>&1 | head
```

Expected: no pending snapshots. If any appear, revert and fix.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add crates/zendriver-stealth/src/flags.rs crates/zendriver-stealth/src/profile.rs
git commit -m "feat(stealth)!: thread GpuBackend through flags_for_profile and StealthProfile"
```

---

### Task 3: `BrowserBuilder::gpu_backend` and `--disable-gpu` suppression

`BrowserBuilder` is the single authority. `StealthProfile`'s own field exists so standalone `build_flags()` stays coherent; when both are set, the builder wins.

**Files:**
- Modify: `crates/zendriver/src/browser.rs:477-510` (struct field), `:1444-1447` (`--disable-gpu`), `:2916-2925` (launch propagation), and the setter next to `headless` at `:664`
- Modify: `crates/zendriver/src/lib.rs` (re-export `GpuBackend`)

**Interfaces:**
- Consumes: `GpuBackend`, `GpuBackend::allows_disable_gpu`, `GpuBackend::angle_flags` (Task 1); `StealthProfile::gpu_backend` (Task 2).
- Produces:
  - `pub fn BrowserBuilder::gpu_backend(self, backend: GpuBackend) -> Self`
  - `pub(crate) gpu_backend: Option<GpuBackend>` field
  - `pub use zendriver_stealth::GpuBackend;` from the `zendriver` crate root

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/zendriver/src/browser.rs` (near `build_flags_default_is_headless` at line 4997):

```rust
    #[test]
    fn build_flags_default_still_emits_disable_gpu() {
        let b = Browser::builder();
        let flags = b.build_flags(Path::new("/tmp/zd-test"));
        assert!(flags.contains(&"--disable-gpu".to_string()));
    }

    #[test]
    fn build_flags_native_gpu_backend_omits_disable_gpu() {
        let b = Browser::builder().gpu_backend(GpuBackend::Native);
        let flags = b.build_flags(Path::new("/tmp/zd-test"));
        assert!(
            !flags.contains(&"--disable-gpu".to_string()),
            "Native backend must not disable the GPU, got: {flags:?}"
        );
        assert!(
            flags.contains(&"--headless=new".to_string()),
            "headless must be unaffected by the GPU backend"
        );
    }

    #[test]
    fn build_flags_native_gpu_backend_emits_angle_flags() {
        let b = Browser::builder().gpu_backend(GpuBackend::Native);
        let flags = b.build_flags(Path::new("/tmp/zd-test"));
        assert!(flags.iter().any(|f| f == "--use-gl=angle"), "got: {flags:?}");
        assert!(flags.iter().any(|f| f.starts_with("--use-angle=")), "got: {flags:?}");
    }

    #[test]
    fn build_flags_swiftshader_backend_keeps_disable_gpu() {
        let b = Browser::builder().gpu_backend(GpuBackend::SwiftShader);
        let flags = b.build_flags(Path::new("/tmp/zd-test"));
        assert!(flags.contains(&"--disable-gpu".to_string()));
    }

    #[test]
    fn build_flags_headful_never_emits_disable_gpu_regardless_of_backend() {
        for backend in [GpuBackend::Disabled, GpuBackend::SwiftShader, GpuBackend::Native] {
            let b = Browser::builder().headless(false).gpu_backend(backend);
            let flags = b.build_flags(Path::new("/tmp/zd-test"));
            assert!(
                !flags.contains(&"--disable-gpu".to_string()),
                "headful must never disable the GPU, backend={backend:?}"
            );
        }
    }
```

Ensure `use zendriver_stealth::GpuBackend;` (or the crate-root re-export) is in scope for the test module.

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p zendriver --lib browser::tests::build_flags 2>&1 | tail -20
```

Expected: FAIL — `no method named gpu_backend found for struct BrowserBuilder`.

- [ ] **Step 3: Add the field and setter**

In `crates/zendriver/src/browser.rs`, add to `BrowserBuilder` (after `headless: Option<bool>` at line 478):

```rust
    pub(crate) gpu_backend: Option<GpuBackend>,
```

Add the setter immediately after the `headless` setter (which ends at line 667):

```rust
    /// Select the GPU backend Chrome renders WebGL / WebGPU with
    /// (default: [`GpuBackend::Disabled`], today's behavior).
    ///
    /// [`GpuBackend::Native`] drops `--disable-gpu` **and** names the
    /// platform's ANGLE backend together — doing only the former hangs
    /// headless Chrome. When a stealth profile also carries a backend, the
    /// value set here wins.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zendriver::GpuBackend;
    /// let builder = zendriver::Browser::builder().gpu_backend(GpuBackend::Native);
    /// ```
    #[must_use]
    pub fn gpu_backend(mut self, backend: GpuBackend) -> Self {
        self.gpu_backend = Some(backend);
        self
    }
```

- [ ] **Step 4: Update `build_flags`**

Replace the headless block at `crates/zendriver/src/browser.rs:1444-1447` with:

```rust
        let gpu_backend = self.gpu_backend.unwrap_or_default();
        if self.headless.unwrap_or(true) {
            v.push("--headless=new".to_string());
            // `--disable-gpu` and an explicit ANGLE backend are mutually
            // exclusive: keeping the flag suppresses the GPU that
            // `GpuBackend::Native` exists to select.
            if gpu_backend.allows_disable_gpu() {
                v.push("--disable-gpu".to_string());
            }
        }
        // ANGLE backend selection is independent of headless — a headful
        // launch honours it too. Emitted here rather than only in the stealth
        // flags so it applies under `StealthProfile::off()` as well.
        v.extend(gpu_backend.angle_flags());
```

- [ ] **Step 5: Propagate the builder's backend into the stealth profile at launch**

At `crates/zendriver/src/browser.rs:2916-2925`, the stealth profile is cloned before `build_flags()`. Change the `if let Some(ref profile) = self.stealth` block so the builder's backend overrides the profile's:

```rust
        let (stealth_obs, extra_flags): (Option<Arc<dyn TargetObserver>>, Vec<String>) =
            if let Some(ref profile) = self.stealth {
                // BrowserBuilder is the authority: when it set a backend, it
                // overrides whatever the profile carried, so the argv can
                // never contain two conflicting `--use-angle=` values.
                let profile = match self.gpu_backend {
                    Some(backend) => profile.clone().gpu_backend(backend),
                    None => profile.clone(),
                };
                let fp = profile.resolve_fingerprint(&exe)?;
                let obs: Arc<dyn TargetObserver> = Arc::new(StealthObserver::with_persona(
                    profile.clone(),
                    fp,
                    self.resolved_persona()?,
                ));
                let flags = profile.build_flags();
                (Some(obs), flags)
            } else {
                (None, Vec::new())
            };
```

Because `build_flags` (Step 4) already emits the ANGLE flags, the stealth profile would now emit them a second time. Chrome tolerates duplicate flags, but a duplicated `--use-angle=` is confusing in logs and in argv snapshots.

The append site is `crates/zendriver/src/browser.rs:2956`, currently:

```rust
        flags.extend(extra_flags);
```

Replace it with a de-duplicating extend:

```rust
        // The ANGLE backend flags are emitted by `build_flags` already, and
        // the stealth profile emits them too when it carries a backend.
        // Chrome would accept both, but two conflicting `--use-angle=` values
        // in the argv are a debugging trap.
        for f in extra_flags {
            if !flags.contains(&f) {
                flags.push(f);
            }
        }
```

- [ ] **Step 6: Re-export `GpuBackend` from the `zendriver` crate root**

In `crates/zendriver/src/lib.rs`, next to the existing `WebgpuSpec` re-export:

```rust
pub use zendriver_stealth::GpuBackend;
```

Also add it to the `zendriver::stealth` module re-export group alongside `WebgpuSpec`.

- [ ] **Step 7: Run tests to verify they pass**

```bash
cargo test -p zendriver --lib browser::tests 2>&1 | tail -20
```

Expected: PASS, including the five new tests and every pre-existing `build_flags_*` test.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add crates/zendriver/src/browser.rs crates/zendriver/src/lib.rs
git commit -m "feat(zendriver): add BrowserBuilder::gpu_backend with coupled --disable-gpu suppression"
```

---

### Task 4: Actionable error when a `Native` launch times out

A GPU process that fails to start surfaces today as a generic WebSocket-endpoint timeout. Under `GpuBackend::Native` that is the single most likely failure, and the message must say so — silent fallback is forbidden, so a clear error is the entire remedy.

`BrowserError` is `#[non_exhaustive]` (`crates/zendriver/src/error.rs:28`), and its doc comment states that new variants may be added in minor releases — so a dedicated typed variant is available here and is better than a string, because callers can match on it.

**Files:**
- Modify: `crates/zendriver/src/error.rs:352` (add a variant next to `WsTimeout`)
- Modify: `crates/zendriver/src/browser.rs:3059` (the `.map_err(|_| BrowserError::WsTimeout)??;` line)

**Interfaces:**
- Consumes: `BrowserBuilder::gpu_backend` field (Task 3).
- Produces: `BrowserError::GpuBackendUnavailable` — a new variant on a `#[non_exhaustive]` enum, so additive.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/zendriver/src/browser.rs`:

```rust
    #[test]
    fn native_backend_maps_ws_timeout_to_gpu_error() {
        let e = super::launch_timeout_error(GpuBackend::Native);
        assert!(
            matches!(e, BrowserError::GpuBackendUnavailable),
            "Native must report a GPU-specific failure, got: {e:?}"
        );
        // The message must name the knob and the escape hatch, without
        // implying zendriver performed a fallback itself.
        let msg = e.to_string();
        assert!(msg.contains("gpu_backend"), "got: {msg}");
        assert!(msg.contains("SwiftShader"), "got: {msg}");
    }

    #[test]
    fn non_native_backends_keep_the_plain_ws_timeout() {
        assert!(matches!(
            super::launch_timeout_error(GpuBackend::Disabled),
            BrowserError::WsTimeout
        ));
        assert!(matches!(
            super::launch_timeout_error(GpuBackend::SwiftShader),
            BrowserError::WsTimeout
        ));
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p zendriver --lib launch_timeout_error 2>&1 | tail -20
```

Expected: FAIL — `cannot find function launch_timeout_error`.

- [ ] **Step 3: Add the error variant**

In `crates/zendriver/src/error.rs`, immediately after the `WsTimeout` variant (line 352):

```rust
    /// Chrome never advertised its WS endpoint on a launch that requested
    /// [`GpuBackend::Native`](crate::GpuBackend::Native).
    ///
    /// Distinct from [`BrowserError::WsTimeout`] so callers can retry with a
    /// software backend programmatically. zendriver deliberately does **not**
    /// perform that fallback itself: silently switching to a software
    /// rasterizer would restore exactly the incoherent GPU fingerprint that
    /// selecting a backend was meant to avoid.
    #[error(
        "timed out waiting for chrome WS endpoint; this launch used \
         gpu_backend(GpuBackend::Native), which requires a usable GPU. \
         zendriver does not fall back automatically — pass \
         GpuBackend::SwiftShader for a software context, or \
         GpuBackend::Disabled for the historical default"
    )]
    GpuBackendUnavailable,
```

- [ ] **Step 4: Implement the selector**

Add near the other free functions in `crates/zendriver/src/browser.rs`:

```rust
/// Which timeout error a failed launch should report, given the GPU backend.
///
/// A `Native` launch on a host whose GPU process cannot start is by far the
/// most likely cause of an endpoint timeout, so it gets its own variant.
fn launch_timeout_error(backend: GpuBackend) -> BrowserError {
    match backend {
        GpuBackend::Native => BrowserError::GpuBackendUnavailable,
        GpuBackend::Disabled | GpuBackend::SwiftShader => BrowserError::WsTimeout,
    }
}
```

- [ ] **Step 5: Wire it into the timeout path**

At `crates/zendriver/src/browser.rs:3059`, replace:

```rust
        .map_err(|_| BrowserError::WsTimeout)??;
```

with:

```rust
        .map_err(|_| launch_timeout_error(self.gpu_backend.unwrap_or_default()))??;
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test -p zendriver --lib launch_timeout_error 2>&1 | tail -20
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/zendriver/src/browser.rs
git commit -m "feat(zendriver): name the GPU backend in launch-timeout errors"
```

---

### Task 5: Expose `gpu_backend` on the `browser_open` MCP tool

Required by the project's MCP coverage rule: a new `BrowserBuilder` option must be reachable from an MCP tool.

**Files:**
- Modify: `crates/zendriver-mcp/src/tools/lifecycle.rs:30-32` (input field), `:152` (builder wiring), `:125-126` and `:246` (output echo)
- Modify: `crates/zendriver-mcp/mcp-coverage-ledger.toml`
- Regenerate: `crates/zendriver-mcp/tests/snapshots/*.snap`

**Interfaces:**
- Consumes: `zendriver::GpuBackend` (Task 3).
- Produces: `gpu_backend` field on the `browser_open` input and output DTOs, serialized snake_case (`"disabled"`, `"swift_shader"`, `"native"`).

- [ ] **Step 1: Write the failing test**

Add to the test module in `crates/zendriver-mcp/src/tools/lifecycle.rs`:

```rust
    #[test]
    fn browser_open_input_defaults_gpu_backend_to_disabled() {
        let input: BrowserOpenInput = serde_json::from_str("{}").unwrap();
        assert_eq!(input.gpu_backend, zendriver::GpuBackend::Disabled);
    }

    #[test]
    fn browser_open_input_parses_native_gpu_backend() {
        let input: BrowserOpenInput =
            serde_json::from_str(r#"{"gpu_backend":"native"}"#).unwrap();
        assert_eq!(input.gpu_backend, zendriver::GpuBackend::Native);
    }

    #[test]
    fn browser_open_input_parses_swift_shader_gpu_backend() {
        let input: BrowserOpenInput =
            serde_json::from_str(r#"{"gpu_backend":"swift_shader"}"#).unwrap();
        assert_eq!(input.gpu_backend, zendriver::GpuBackend::SwiftShader);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p zendriver-mcp --lib lifecycle 2>&1 | tail -20
```

Expected: FAIL — `no field gpu_backend on type BrowserOpenInput`.

- [ ] **Step 3: Add the input field**

In `crates/zendriver-mcp/src/tools/lifecycle.rs`, next to the `headless` field at line 32:

```rust
    /// GPU backend Chrome renders WebGL / WebGPU with (default: `disabled`).
    ///
    /// `disabled` keeps zendriver's historical flags. `swift_shader` forces a
    /// software rasterizer. `native` uses the host GPU through the platform's
    /// ANGLE backend, giving a fully coherent GPU fingerprint — but it
    /// requires a real GPU and reports **the host's** device, not a spoofed
    /// one. There is no automatic fallback.
    #[serde(default)]
    pub gpu_backend: zendriver::GpuBackend,
```

Add the matching field to the output struct near line 126:

```rust
    /// Effective GPU backend for the launched browser.
    pub gpu_backend: zendriver::GpuBackend,
```

- [ ] **Step 4: Wire it through**

At line 152, extend the builder chain:

```rust
    let mut builder = Browser::builder()
        .headless(input.headless)
        .gpu_backend(input.gpu_backend)
        .stealth(stealth);
```

At line 246, echo it in the output:

```rust
        gpu_backend: input.gpu_backend,
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p zendriver-mcp --lib lifecycle 2>&1 | tail -20
```

Expected: PASS.

- [ ] **Step 6: Regenerate the schema snapshots**

```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo insta accept --all
```

Review the diff: it must add exactly the `gpu_backend` property to the `browser_open` input and output schemas, with an enum of `["disabled", "swift_shader", "native"]`. Any other change means something unintended moved.

- [ ] **Step 7: Add the ledger entries**

Append to `crates/zendriver-mcp/mcp-coverage-ledger.toml`:

```toml
# ── GPU backend selection (2026-07-24 GPU spoofing, phase 1) ────────────────
[[entry]]
api = "zendriver::browser::BrowserBuilder::gpu_backend"
covered = "browser_open.gpu_backend"

[[entry]]
api = "zendriver::GpuBackend"
covered = "browser_open.gpu_backend"

[[entry]]
api = "zendriver::stealth::GpuBackend"
covered = "browser_open.gpu_backend"
```

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add crates/zendriver-mcp/src/tools/lifecycle.rs \
        crates/zendriver-mcp/tests/snapshots \
        crates/zendriver-mcp/mcp-coverage-ledger.toml
git commit -m "feat(mcp): expose gpu_backend on browser_open"
```

---

### Task 6: GPU probe example

Produces the JSON that Plan 2's tier tables are built from, and lets any user capture their own device.

**Files:**
- Create: `crates/zendriver/examples/probe_gpu.rs`

**Interfaces:**
- Consumes: `Browser::builder`, `BrowserBuilder::gpu_backend` (Task 3).
- Produces: an executable example printing one JSON object to stdout. Top level: `gpuInNavigator` (bool), `adapter` (object or null, with `vendor` / `architecture` / `device` / `description` / `limits` / `features`), `deviceOk` (bool or a `"reject: <Name>"` string), `webgl1` and `webgl2`. Each context object carries `unmaskedVendor`, `unmaskedRenderer`, `extensions` (array), `params` (enum-name → value) and `precision` (`"<SHADER>/<PRECISION>"` → `[rangeMin, rangeMax, precision]`). Plan 2's tier tables consume exactly this shape.

- [ ] **Step 1: Write the example**

Create `crates/zendriver/examples/probe_gpu.rs`:

```rust
//! Dump this host's real GPU surface as JSON.
//!
//! Run against the host GPU (the useful case — captures real values):
//!
//! ```text
//! cargo run -p zendriver --example probe_gpu -- native
//! ```
//!
//! Or against the software rasterizer, which is what the ANGLE-drift canary
//! test compares against:
//!
//! ```text
//! cargo run -p zendriver --example probe_gpu -- swiftshader
//! ```
//!
//! The output is the input format for the tier tables: capture it on a real
//! device, then hand it to the profile dataset.

use zendriver::{Browser, GpuBackend};

/// Reads every value the tier tables need. Kept as one expression so it can be
/// evaluated in a single CDP round-trip.
const PROBE_JS: &str = r#"
(async () => {
  const out = {};
  out.gpuInNavigator = ('gpu' in navigator);
  try {
    const a = navigator.gpu ? await navigator.gpu.requestAdapter() : null;
    out.adapter = a ? {
      vendor: a.info ? a.info.vendor : null,
      architecture: a.info ? a.info.architecture : null,
      device: a.info ? a.info.device : null,
      description: a.info ? a.info.description : null,
      limits: a.limits ? Object.fromEntries(
        Object.keys(Object.getPrototypeOf(a.limits))
          .map(k => [k, a.limits[k]])
          .filter(([, v]) => typeof v === 'number')) : null,
      features: a.features ? Array.from(a.features) : null,
    } : null;
    if (a) {
      try { out.deviceOk = !!(await a.requestDevice()); }
      catch (e) { out.deviceOk = 'reject: ' + e.name; }
    }
  } catch (e) { out.adapterErr = String(e); }

  function readContext(kind) {
    const gl = document.createElement('canvas').getContext(kind);
    if (!gl) return null;
    const r = { extensions: gl.getSupportedExtensions(), params: {}, precision: {} };
    const dbg = gl.getExtension('WEBGL_debug_renderer_info');
    if (dbg) {
      r.unmaskedVendor = gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL);
      r.unmaskedRenderer = gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL);
    }
    // Every numeric GL enum the context recognises. Unknown enums throw or
    // return null, which is how non-parameters are filtered out.
    for (const name of Object.keys(Object.getPrototypeOf(gl))) {
      const val = gl[name];
      if (typeof val !== 'number') continue;
      try {
        const got = gl.getParameter(val);
        if (got === null || typeof got === 'object' && !ArrayBuffer.isView(got)) continue;
        r.params[name] = ArrayBuffer.isView(got) ? Array.from(got) : got;
      } catch (e) { /* not a gettable parameter */ }
    }
    for (const st of ['VERTEX_SHADER', 'FRAGMENT_SHADER']) {
      for (const pt of ['LOW_FLOAT','MEDIUM_FLOAT','HIGH_FLOAT','LOW_INT','MEDIUM_INT','HIGH_INT']) {
        const f = gl.getShaderPrecisionFormat(gl[st], gl[pt]);
        if (f) r.precision[st + '/' + pt] = [f.rangeMin, f.rangeMax, f.precision];
      }
    }
    return r;
  }

  out.webgl1 = readContext('webgl');
  out.webgl2 = readContext('webgl2');
  return JSON.stringify(out);
})()
"#;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary
async fn main() -> zendriver::Result<()> {
    let backend = match std::env::args().nth(1).as_deref() {
        Some("native") => GpuBackend::Native,
        Some("swiftshader") => GpuBackend::SwiftShader,
        Some("disabled") | None => GpuBackend::Disabled,
        Some(other) => {
            eprintln!("unknown backend {other:?}; expected native | swiftshader | disabled");
            return Ok(());
        }
    };

    let browser = Browser::builder().gpu_backend(backend).launch().await?;
    let tab = browser.main_tab();
    tab.goto("about:blank").await?;
    tab.wait_for_load().await?;
    // `Tab::evaluate` already sends `awaitPromise: true` (tab.rs:1096), so the
    // async IIFE above resolves before the value comes back.
    let json: String = tab.evaluate(PROBE_JS).await?;
    println!("{json}");
    browser.close().await?;
    Ok(())
}
```

This matches the repo idiom in `crates/zendriver/examples/persona_basic.rs:19-21` and `:33-41` — `Browser::builder()…launch()`, `browser.main_tab()`, `tab.goto()`, `tab.wait_for_load()`. There is no `new_tab` or `evaluate_async` on this API; do not invent one.

- [ ] **Step 2: Verify it compiles**

```bash
cargo build -p zendriver --example probe_gpu 2>&1 | tail -20
```

Expected: compiles with no errors.

- [ ] **Step 3: Run it against SwiftShader**

```bash
cargo run -p zendriver --example probe_gpu -- swiftshader 2>/dev/null | head -c 600
```

Expected: a JSON object whose `webgl2.unmaskedRenderer` contains `SwiftShader` and whose `webgl2.params.MAX_TEXTURE_SIZE` is `8192`.

- [ ] **Step 4: Run it against the host GPU**

```bash
cargo run -p zendriver --example probe_gpu -- native 2>/dev/null | head -c 600
```

Expected on a GPU-equipped host: `adapter` is non-null and `deviceOk` is `true`. On a GPU-less host this may hang or error — that is the documented `Native` failure mode, and Task 4's message should appear.

- [ ] **Step 5: Save the SwiftShader baseline for Task 7**

```bash
mkdir -p crates/zendriver/tests/fixtures
cargo run -p zendriver --example probe_gpu -- swiftshader 2>/dev/null \
  > crates/zendriver/tests/fixtures/swiftshader_probe.json
head -c 200 crates/zendriver/tests/fixtures/swiftshader_probe.json
```

Expected: a non-empty JSON file.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/zendriver/examples/probe_gpu.rs crates/zendriver/tests/fixtures/swiftshader_probe.json
git commit -m "feat(zendriver): add probe_gpu example dumping the GPU surface as JSON"
```

---

### Task 7: SwiftShader ANGLE-drift canary

The one tier verifiable on any host, including GPU-less CI. When Chrome's ANGLE constants move, this fails and tells Plan 2's tables to be re-derived.

**Files:**
- Create: `crates/zendriver/tests/gpu_backend.rs`

The canary asserts against constants inlined in the test, not against `fixtures/swiftshader_probe.json`. A fixture the test loads would drift silently with the code that writes it; a literal in the assertion has to be edited by a human who sees the failure. The fixture from Task 6 exists as Plan 2's dataset input, not as this test's oracle.

**Interfaces:**
- Consumes: `GpuBackend::SwiftShader`, the probe JS from Task 6.
- Produces: `swiftshader_tier_matches_recorded_baseline` — the canary later plans reference by name.

- [ ] **Step 1: Write the failing test**

Create `crates/zendriver/tests/gpu_backend.rs`:

```rust
//! Real-Chrome GPU backend tests. All `#[ignore]` — they launch a browser.
//!
//! Run with: `cargo test -p zendriver --test gpu_backend -- --ignored`

use zendriver::{Browser, GpuBackend};

/// Keep in sync with `examples/probe_gpu.rs`. Only the WebGL2 subset the
/// canary compares is needed here.
const CAPS_JS: &str = r#"
(() => {
  const gl = document.createElement('canvas').getContext('webgl2');
  if (!gl) return JSON.stringify({ error: 'no webgl2 context' });
  const dbg = gl.getExtension('WEBGL_debug_renderer_info');
  return JSON.stringify({
    unmaskedRenderer: dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
    maxTextureSize: gl.getParameter(gl.MAX_TEXTURE_SIZE),
    maxViewportDims: Array.from(gl.getParameter(gl.MAX_VIEWPORT_DIMS)),
    maxVertexUniformVectors: gl.getParameter(gl.MAX_VERTEX_UNIFORM_VECTORS),
    extensionCount: gl.getSupportedExtensions().length,
  });
})()
"#;

#[tokio::test]
#[ignore = "launches real Chrome"]
async fn swiftshader_tier_matches_recorded_baseline() {
    let browser = Browser::builder()
        .gpu_backend(GpuBackend::SwiftShader)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto("about:blank").await.expect("goto");
    tab.wait_for_load().await.expect("load");
    let raw: String = tab.evaluate(CAPS_JS).await.expect("evaluate");
    browser.close().await.ok();

    let got: serde_json::Value = serde_json::from_str(&raw).expect("probe json");

    assert!(
        got["unmaskedRenderer"]
            .as_str()
            .unwrap_or_default()
            .contains("SwiftShader"),
        "expected the SwiftShader backend, got: {got:#}"
    );

    // These are ANGLE's SwiftShader constants as measured on 2026-07-24.
    // A failure here does NOT mean this test is wrong — it means Chrome's
    // ANGLE constants moved, and every tier table derived from them must be
    // re-derived. Update these values together with the tables.
    assert_eq!(got["maxTextureSize"], 8192, "ANGLE drift: {got:#}");
    assert_eq!(
        got["maxViewportDims"],
        serde_json::json!([8192, 8192]),
        "ANGLE drift: {got:#}"
    );
    assert_eq!(got["maxVertexUniformVectors"], 4096, "ANGLE drift: {got:#}");
    assert_eq!(got["extensionCount"], 30, "ANGLE drift: {got:#}");
}
```

- [ ] **Step 2: Run it to verify it passes**

```bash
cargo test -p zendriver --test gpu_backend -- --ignored --nocapture 2>&1 | tail -25
```

Expected: PASS. If any assertion fails, the recorded constant is wrong for the local Chrome — update the constant **and** note the Chrome version in the comment, since Plan 2's tables inherit these numbers.

- [ ] **Step 3: Confirm it is excluded from the default run**

```bash
cargo test -p zendriver --test gpu_backend 2>&1 | tail -5
```

Expected: `0 passed; 0 failed; 1 ignored` (or similar) — the canary must not run in the normal suite.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add crates/zendriver/tests/gpu_backend.rs
git commit -m "test(zendriver): pin ANGLE SwiftShader constants as a drift canary"
```

---

### Task 8: Native-backend integration test

Proves the headline claim — `Native` yields a real adapter and a working `requestDevice()` — and skips cleanly where no GPU exists.

**Files:**
- Modify: `crates/zendriver/tests/gpu_backend.rs` (Task 7)

**Interfaces:**
- Consumes: `GpuBackend::Native` (Task 3).
- Produces: `native_backend_yields_a_real_adapter_and_device`.

- [ ] **Step 1: Write the test**

Append to `crates/zendriver/tests/gpu_backend.rs`:

```rust
const ADAPTER_JS: &str = r#"
(async () => {
  if (!('gpu' in navigator)) return JSON.stringify({ adapter: null, reason: 'no navigator.gpu' });
  const a = await navigator.gpu.requestAdapter();
  if (!a) return JSON.stringify({ adapter: null, reason: 'requestAdapter resolved null' });
  let deviceOk = false;
  try { deviceOk = !!(await a.requestDevice()); } catch (e) { deviceOk = false; }
  return JSON.stringify({
    adapter: { vendor: a.info ? a.info.vendor : null, architecture: a.info ? a.info.architecture : null },
    deviceOk,
  });
})()
"#;

#[tokio::test]
#[ignore = "launches real Chrome and requires a usable GPU"]
async fn native_backend_yields_a_real_adapter_and_device() {
    let browser = match Browser::builder()
        .gpu_backend(GpuBackend::Native)
        .launch()
        .await
    {
        Ok(b) => b,
        Err(e) => {
            // A GPU-less host is a legitimate skip, not a failure. `Native`
            // deliberately has no fallback, so the launch error IS the
            // expected outcome here — and Task 4 makes it
            // `BrowserError::GpuBackendUnavailable` specifically.
            eprintln!("skipping: Native backend unavailable on this host: {e}");
            return;
        }
    };
    let tab = browser.main_tab();
    tab.goto("about:blank").await.expect("goto");
    tab.wait_for_load().await.expect("load");
    let raw: String = tab.evaluate(ADAPTER_JS).await.expect("evaluate");
    browser.close().await.ok();

    let got: serde_json::Value = serde_json::from_str(&raw).expect("probe json");
    if got["adapter"].is_null() {
        eprintln!("skipping: no GPU adapter on this host ({})", got["reason"]);
        return;
    }

    assert!(
        got["adapter"]["vendor"].as_str().is_some_and(|v| !v.is_empty()),
        "a real adapter must report a vendor, got: {got:#}"
    );
    assert_eq!(
        got["deviceOk"], true,
        "the headline claim for GpuBackend::Native is a WORKING device, got: {got:#}"
    );
}
```

- [ ] **Step 2: Run it**

```bash
cargo test -p zendriver --test gpu_backend -- --ignored --nocapture 2>&1 | tail -25
```

Expected on a GPU-equipped host: PASS with `deviceOk == true`. On a GPU-less host: PASS with a `skipping:` line on stderr.

- [ ] **Step 3: Commit**

```bash
cargo fmt --all
git add crates/zendriver/tests/gpu_backend.rs
git commit -m "test(zendriver): verify Native backend yields a real adapter and device"
```

---

### Task 9: Re-confirm the stale `'gpu' in navigator` backlog note

The spec's Corrections section explicitly defers this to phase 2, with the caveat that the original probe used only the GPU-relevant flags rather than zendriver's complete launch set.

**Files:**
- Modify: `docs/superpowers/deferred-backlog.md:103`
- Modify: `docs/superpowers/specs/2026-07-24-gpu-spoofing-design.md` (Corrections section)

**Interfaces:**
- Consumes: `probe_gpu` example (Task 6).
- Produces: documentation only — no code.

- [ ] **Step 1: Measure under zendriver's real flag set**

```bash
cargo run -p zendriver --example probe_gpu -- disabled 2>/dev/null \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print('gpuInNavigator:', d['gpuInNavigator']); print('adapter:', d['adapter'])"
```

Record both values. This runs through `BrowserBuilder`, so it uses the complete launch argv, which the original ad-hoc probe did not.

- [ ] **Step 2: Update the backlog note**

Edit `docs/superpowers/deferred-backlog.md:103`. Replace the claim that `'gpu' in navigator` is `false` in both headless and headful with the measured result, dated 2026-07-24, and naming the Chrome version:

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --version
```

Keep the surrounding text about the fabrication path intact — only the factual claim changes.

- [ ] **Step 3: Update the spec's Corrections entry**

In `docs/superpowers/specs/2026-07-24-gpu-spoofing-design.md`, the `'gpu' in navigator` bullet currently says "Re-confirm against the full flag set in Phase 2 before treating the note as retired." Replace that sentence with the confirmed result and the Chrome version it was measured on.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/deferred-backlog.md docs/superpowers/specs/2026-07-24-gpu-spoofing-design.md
git commit -m "docs: re-confirm navigator.gpu presence under zendriver's full flag set"
```

---

### Task 10: Documentation, public-API baseline, and final gates

The project treats a shipped behavior change with stale docs as an incomplete PR. All three doc surfaces must move together.

**Files:**
- Modify: `README.md`, `crates/zendriver-mcp/README.md`
- Modify: `docs/book/src/fingerprint.md`
- Create: `docs/book/src/gpu-backend.md` (new chapter) and register it in `docs/book/src/SUMMARY.md`
- Regenerate: `crates/zendriver-mcp/public-api-baseline.txt`

**Interfaces:**
- Consumes: everything from Tasks 1–8.
- Produces: no code.

- [ ] **Step 1: Write the mdBook chapter**

Create `docs/book/src/gpu-backend.md` covering: what each `GpuBackend` variant does; that `Native` reports the host's GPU and gives no identity control; that there is no automatic fallback and why; the measured comparison table from the spec's Measurements section; and the `--disable-gpu`-alone hang. Include a runnable snippet:

````markdown
```rust,no_run
use zendriver::{Browser, GpuBackend};

# async fn demo() -> Result<(), Box<dyn std::error::Error>> {
let browser = Browser::builder()
    .gpu_backend(GpuBackend::Native)
    .build()
    .await?;
# Ok(())
# }
```
````

Register it in `docs/book/src/SUMMARY.md` next to the other fingerprint chapters.

- [ ] **Step 2: Cross-link from the fingerprint chapter**

In `docs/book/src/fingerprint.md`, add a short paragraph in the WebGPU section noting that `GpuBackend::Native` removes the need for `WebgpuSpec` fabrication entirely on a GPU-equipped host, linking to the new chapter.

- [ ] **Step 3: Update both READMEs**

Add `gpu_backend` to the feature matrix in `README.md` and to the `browser_open` option list in `crates/zendriver-mcp/README.md`. The MCP tool **count is unchanged** — no new tool was added, only an option. Verify no count needs editing:

```bash
grep -rn "MCP tool" README.md crates/zendriver-mcp/README.md | head
```

- [ ] **Step 4: Build the book**

```bash
mdbook build docs/book 2>&1 | tail -5
```

Expected: builds with no errors.

- [ ] **Step 5: Regenerate the public-API baseline**

```bash
cargo +nightly public-api -p zendriver --all-features > crates/zendriver-mcp/public-api-baseline.txt
git diff --stat crates/zendriver-mcp/public-api-baseline.txt
```

Expected diff: the new `GpuBackend` enum, its two methods, `BrowserBuilder::gpu_backend`, and the two re-exports. Anything else means an unintended API change.

- [ ] **Step 6: Verify the coverage check passes**

```bash
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked 2>&1 | tail -10
```

Expected: PASS. A failure names the API item missing from the ledger — add it to the entries from Task 5.

- [ ] **Step 7: Run the full gate**

Run these three in parallel; they are independent:

```bash
cargo fmt --all --check
```

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
```

```bash
cargo test --workspace --locked
```

Expected: all pass. Also run the feature-gated clippy pass, since `zendriver-mcp` gained a field:

```bash
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings
```

- [ ] **Step 8: Commit**

```bash
git add README.md crates/zendriver-mcp/README.md docs/book/src/gpu-backend.md \
        docs/book/src/SUMMARY.md docs/book/src/fingerprint.md \
        crates/zendriver-mcp/public-api-baseline.txt
git commit -m "docs: document GpuBackend across README, rustdoc, and the book"
```

---

## What this plan deliberately does not do

Recorded so a later reader does not mistake these for oversights:

- **No tier tables, `GpuProfile`, or resolver.** Plan 2. Those depend on probe output from Task 6 that does not exist until this plan lands — the spec defers the Linux tier set to "probes rather than guessed here."
- **No `webgl.js` rewrite.** Plan 2. The impossible `[32767,32767]` / `8192` pair survives this plan; `GpuBackend::Native` sidesteps it rather than fixing it.
- **No worker injection.** Plan 3. Independent of probe data but touches every patch, so it carries its own regression pass.
- **No L2 farbling or L3 MediaCapabilities.** Plan 4.
- **No change to the default fingerprint.** `GpuBackend::Disabled` is byte-for-byte today's behavior. The `feat(stealth)!` default change described in the spec belongs to Plan 2, where the complete profile replaces the six-parameter one.
