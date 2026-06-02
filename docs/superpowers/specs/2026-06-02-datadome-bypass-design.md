# Group D — `zendriver-datadome` bypass crate + WebGPU coherence

**Status:** Approved (brainstorm, participate mode) — 2026-06-02
**Upstream:** [cdpdriver/zendriver#20](https://github.com/cdpdriver/zendriver/issues/20) — "DataDome is able to detect zendriver"
**PR2 group:** D (last remaining; A/B/C merged as #28/#29/#30)

## 1. Context & motivation

DataDome is an anti-bot vendor. zendriver's existing anti-bot crates
(`zendriver-cloudflare`, `zendriver-imperva`) are the template: an optional,
cargo-feature-gated crate with a thin `Tab::<name>()` entry point that
detects the vendor's challenge surface, polls for clearance, and exposes an
opt-in solver callback for the CAPTCHA step — **not** a bundled CAPTCHA solver.

### What upstream #20 actually reports

The #20 thread (~13 comments) is **entirely about fingerprint coherence, not
CAPTCHA solving**. Reporters never reach a visible challenge — they are
silently flagged by DataDome's *invisible device-check* because of GPU
fingerprint leaks:

- `webGLVendor` / `webGLRenderer` / `webGLCanvasHash` — containerized Chromium
  leaks renderer strings (`ANGLE (AMD Radeon 760M ... radeonsi LLVM 19 ...)`)
  that betray the container GPU.
- `navigator.gpu.requestAdapter()` (WebGPU) — the issue title's vector; GPU
  adapter info inconsistent with the WebGL renderer.

Maintainer framing: (1) some sites block Linux outright (unfixable here), (2)
container deployments lack GPU features (Vulkan/GLES) enabling detection.
Their plan is to fix `zendriver-docker` + a Windows image — out of scope for
this repo. The portion we *can* address in-library is fingerprint coherence.

**Critical design constraint** (Camoufox, quoted in-thread): *"Do NOT randomly
assign WebGL values. WAFs hash your fingerprint and compare against a dataset.
Random → detection as unknown device."* All spoofed fingerprint values must be
**coherent and dataset-sourced, never randomized**.

### Why this is two complementary halves

DataDome's dominant surface (the invisible device-check) has **no interactive
element to drive** — passing it is ~95% fingerprint coherence, ~5% "wait for
the `datadome` cookie." A pure imperva-style crate would therefore be hollow
on the headline surface and repeat imperva's documented "cleared-but-still-403"
trap. So Group D ships **both**:

1. The **`zendriver-datadome` crate** — surface detection, observe-and-wait
   clearance, opt-in CAPTCHA-solver callback, Block diagnosis.
2. An **8th `Surface::Webgpu`** farble surface in `zendriver-stealth` — closes
   the literal #20 leak so the device-check the crate waits on can actually
   pass.

PR #24 (fingerprint spoofing) already ships a `webgl` farble surface that
Value-spoofs vendor/renderer; **WebGPU is 100% uncovered** today
(`requestAdapter` / `navigator.gpu` / `GPUAdapter` appear nowhere in the
workspace).

### Guiding philosophy

Least-opinionated, everything-overridable, coherent defaults; never lock a
value (see memory `fingerprint-design-philosophy`). Pre-release: API churn is
acceptable — prefer the technically-stronger design over backward-compat.

## 2. Deliverables (one PR, 3 crates touched)

1. **New `zendriver-datadome` crate** — sibling to cloudflare/imperva.
2. **`Surface::Webgpu`** — 8th Persona farble surface in `zendriver-stealth`.
3. **Retrofit** imperva + cloudflare to a unified single-channel result model
   (folded into this PR, not deferred).

## 3. Crate layout (mirror imperva file-for-file)

```
crates/zendriver-datadome/
  Cargo.toml          # deps: zendriver-transport, zendriver-interception, tokio,
                      #       serde, serde_json, thiserror, tracing
  src/
    lib.rs            # re-exports, crate docs, the "clearance ≠ acceptance —
                      #   look upstream at WebGPU/stealth" limitations note
    bypass.rs         # DataDomeBypass driver, wait_for_clearance, ClearanceOutcome
    detection.rs      # detect.js + DataDomeSurface + detect_surface()
    captcha.rs        # DataDomeChallenge / DataDomeSolution / solver type +
                      #   cookie application
    interception.rs   # opt-in Fetch fast-path (with_interception)
    error.rs          # DataDomeError
    detect.js
```

Crate version: `0.1.0`.

## 4. Surface detection (`detection.rs` + `detect.js`)

One `Runtime.evaluate` round-trip against the page main world (mirror imperva's
bundled probe). `detect.js` reads, in one shot:

- the `datadome` cookie (presence + value, via `document.cookie`),
- the `window.dd` config object `{rt, cid, hsh, t, s, e, host}` — the challenge
  descriptor DataDome injects into the 403 challenge page,
- a `captcha-delivery.com` iframe (shadow-DOM-aware recursive walk, reuse
  cloudflare's `findBbox` approach),
- coarse body markers.

```rust
/// Which DataDome surface a tab is currently showing.
pub enum DataDomeSurface {
    /// window.dd.t == 'fe' device-check / interstitial — invisible JS
    /// interrogation; also covers the auto-resolving "please wait" page.
    DeviceCheck,
    /// captcha-delivery.com iframe present (slider / puzzle / press-hold).
    Captcha,
    /// window.dd.t == 'bv' — IP banned; unsolvable in-browser.
    Block,
    /// No DataDome surface detected; the datadome cookie may already be valid.
    None,
}

/// Surface-only convenience probe (mirror imperva's `detect_surface`).
pub fn detect_surface(session: &SessionHandle)
    -> Result<DataDomeSurface, DataDomeError>;
```

A `Captcha` surface is monolithic (no sub-kind) — unlike imperva's
hCaptcha/reCAPTCHA/native split, DataDome's CAPTCHA is one solver task type;
the crate just hands over the `captcha_url`.

## 5. Result model (unified single channel)

All flow-terminals are `Ok(Outcome)`; only genuine faults are `Err`.

```rust
pub enum ClearanceOutcome {
    /// datadome cookie acquired AND challenge markers gone.
    Cleared { datadome: String },
    /// Markers cleared but no datadome cookie observed (rare / legacy path).
    ChallengeGone,
    /// No DataDome surface present at call time. Fast path; no waiting.
    AlreadyClear,
    /// window.dd.t == 'bv' — IP banned. Nothing in-browser can clear this;
    /// the caller must change IP (e.g. residential proxy).
    Blocked,
    /// Deadline elapsed without reaching a terminal state.
    TimedOut { last_surface: Option<DataDomeSurface> },
}

pub enum DataDomeError {           // faults ONLY
    /// Captcha surface detected but no `on_captcha` solver registered.
    CaptchaRequired,
    /// Registered solver returned an error.
    CaptchaSolver(Box<dyn std::error::Error + Send + Sync>),
    /// with_interception hook failed at startup.
    Interception(#[from] InterceptionError),
    /// CDP transport / call error.
    Call(#[from] CallError),
    /// In-page evaluator raised or returned an unexpected shape.
    JsError(String),
}
```

`Block` is a successfully-*detected* terminal (we know exactly what's
happening), so it is an `Outcome`, not an `Error` — the caller branches on it
like `AlreadyClear`. `TimedOut` is likewise an outcome (a deadline in a
bot-management flow is a normal signal, not a malfunction).

## 6. Driver (`bypass.rs`)

`DataDomeBypass<'tab>`, constructed via `Tab::datadome()`. Same struct shape as
`ImpervaBypass` (`Arc<dyn solver>` + `interception_enabled: bool`, custom
`Debug` that elides the closure). Builder methods:

- `.timeout(Duration)` — overall deadline (default 30s).
- `.poll_interval(Duration)` — poll cadence (default 250ms).
- `.with_interception()` — enable the Fetch-domain fast-path (§8).
- `.on_captcha(solver)` — register the CAPTCHA solver callback (§7).

`wait_for_clearance(self) -> Result<ClearanceOutcome, DataDomeError>`:

1. Pre-probe the surface, **raced against the deadline** (imperva's
   `probe_with_deadline` — no probe, including the first, outlives the budget).
2. `None` → `AlreadyClear` (no surface to clear; any existing datadome
   cookie is left as-is).
3. `Block` → `Blocked` (immediate terminal).
4. `DeviceCheck` → poll loop: each tick re-detect; when the datadome cookie
   lands AND markers are gone → `Cleared`. *(This is the surface WebGPU
   coherence makes passable; without it the loop just `TimedOut`s.)*
5. `Captcha` → no solver ⇒ `Err(CaptchaRequired)`; with solver ⇒ extract
   `DataDomeChallenge` → call solver → apply `DataDomeSolution` → reload →
   resume poll.
6. Deadline reached ⇒ `TimedOut { last_surface }`.
7. With `.with_interception()`: a background oneshot signal (first
   `Set-Cookie: datadome=` or `captcha-delivery` 2xx) is raced against the
   poll; first to fire wins (mirror imperva's `interception.rs` +
   `InterceptionGuard` RAII cancel).

## 7. CAPTCHA escalation (`captcha.rs`) — the DataDome delta vs imperva

```rust
/// CAPTCHA escalation handed to a caller-supplied solver.
pub struct DataDomeChallenge {
    /// Built from window.dd + the current datadome cookie, e.g.
    /// https://geo.captcha-delivery.com/captcha/?initialCid=…&hash=…&cid=…&t=fe
    pub captcha_url: String,
    /// URL of the page presenting the CAPTCHA.
    pub site_url: String,
    /// Browser UA — MUST match the UA the page used (solver-service requirement).
    pub user_agent: String,
    pub cid: Option<String>,   // datadome cookie / dd.cid
    pub hash: Option<String>,  // dd.hsh
}

/// Token returned by a caller-supplied solver. For DataDome this is the
/// solved `datadome` COOKIE value — NOT a form-field token.
pub struct DataDomeSolution {
    pub datadome_cookie: String,
}
```

Solver type: `Fn(DataDomeChallenge) -> Future<Result<DataDomeSolution,
Box<dyn Error + Send + Sync>>>`, `Arc`-erased exactly like imperva's
`CaptchaSolver` + `arc_solver`.

**Applying the solution is the structural difference from imperva.** Imperva
injects a token into a form field; DataDome's solution *is* the cookie — apply
via `Network.setCookie { name: "datadome", value, domain, path: "/" }` then
`Page.reload`, then resume the poll. `t == 'bv'` ⇒ never call the solver
(a banned IP cannot be solved) ⇒ `Blocked`.

The solver callback is opt-in; without it a CAPTCHA surface yields
`Err(CaptchaRequired)`. We do **not** bundle a solver or attempt native slider
geometry in v1 (the most cat-and-mouse surface — DataDome rotates the puzzle
constantly). Native solving is a possible future feature behind its own flag.

## 8. Opt-in interception fast-path (`interception.rs`)

Mirror imperva: `spawn_signal(session)` subscribes (via
`InterceptBuilder::pattern(...).at_response().subscribe()`) to Fetch responses
matching `*captcha-delivery.com*` and any response carrying `Set-Cookie:
datadome=`, releasing every pause (`continue_()`) so the page keeps loading and
signalling a oneshot on the first clearance hit. An `InterceptionGuard`
(CancellationToken + JoinHandle) cooperatively cancels on drop. The poll loop
races this signal; first wins. Infallible setup (returns `-> _`, not
`Result`, to dodge `clippy::result_large_err`).

## 9. Feature wiring + `Tab` method

- Workspace: add `crates/zendriver-datadome` to `members`; add
  `zendriver-datadome = { path = "...", version = "0.1.0" }` to
  `[workspace.dependencies]`.
- `zendriver/Cargo.toml`:
  - `datadome = ["interception", "dep:zendriver-datadome"]`
  - `datadome-tests = ["datadome", "integration-tests"]`
  - add `"datadome"` to the `integration-tests` feature list.
  - `zendriver-datadome = { workspace = true, optional = true }`.
- `zendriver/src/tab.rs`:
  ```rust
  #[cfg(feature = "datadome")]
  impl Tab {
      #[must_use]
      pub fn datadome(&self) -> zendriver_datadome::DataDomeBypass<'_> {
          zendriver_datadome::DataDomeBypass::new(self.session())
      }
  }
  ```
- Re-export `DataDomeBypass`, `ClearanceOutcome` (as `DataDomeClearanceOutcome`
  at the zendriver-crate boundary, matching the `ImpervaClearanceOutcome`
  convention), `DataDomeError`, `DataDomeSurface`, `DataDomeChallenge`,
  `DataDomeSolution`, `detect_surface` through the zendriver crate behind the
  feature, following the imperva re-export set.
- `zendriver-mcp/Cargo.toml`: add `datadome = ["zendriver/datadome"]` to the
  `default` feature list.

`Tab::datadome()` also gives access to the standalone `detect_surface`
passthrough (re-exported) for a "which surface am I on" check without driving a
bypass.

## 10. WebGPU surface (`zendriver-stealth`)

- Add `Surface::Webgpu` to the `Surface` enum; classify as
  `SurfaceKind::Value` (coherent value-spoof, like `Webgl`).
- New `patches/webgpu.js`: override `navigator.gpu.requestAdapter()` to resolve
  a `GPUAdapter` whose `info` (vendor / architecture / device / description),
  `limits`, and `features` are **coherent with the already-spoofed WebGL
  `UNMASKED_RENDERER`**. Both the `webgl` and `webgpu` patches read the same
  persona-resolved renderer, so coherence is **by construction** (not
  runtime-sniffed). Vendor/architecture derived from the renderer string
  (NVIDIA / AMD / Intel / Apple) via a small dataset map; standard-tier
  `limits`. **Never randomized.**
- **Default strategy: coherent-mock** (B1). A real Chrome on the spoofed UA
  *has* WebGPU; deleting `navigator.gpu` can itself be a tell. `Block` (delete
  `navigator.gpu`) stays available per-persona via `.surface(Surface::Webgpu,
  Strategy::Block)`.
- Wire into the existing `bootstrap_script(persona, identity)` path; the
  per-surface override machinery (`.surface(...)`, persona overlay) already
  supports the new variant with no API change.
- The renderer→adapter mapping table is a plan-time implementation detail;
  v1 needs a small set covering the common vendors, defaulting to a plausible
  coherent adapter when the renderer is unrecognized.

## 11. Cross-crate retrofit (folded into this PR)

Unify all three anti-bot crates on the single-channel model (flow-terminals =
`Outcome`, faults = `Error`):

- **imperva**: move `Timeout` from `ImpervaError` into
  `ClearanceOutcome::TimedOut { last_surface }`. The MCP `solve_imperva` tool
  drops its `Err(ImpervaError::Timeout) =>` collapse special-case for a direct
  `Ok(TimedOut) =>` map. The MCP output enum already carries a `Timeout`
  variant, so the wire schema (and `*.snap`) is unchanged — regenerate to
  confirm.
- **cloudflare**: move `NoChallenge` + `ClearanceTimeout` from `CloudflareError`
  into `Outcome` variants; the MCP cloudflare tool drops its analogous collapse.
- Net result: `Error` in every anti-bot crate means a genuine fault (CDP / JS /
  solver error / captcha-without-solver) only.

This deletes code (the MCP error-collapse hacks) and removes a 3-crate
inconsistency. Pre-release, so the lib-level signature churn is acceptable;
the work is internal + test updates.

## 12. MCP tool (`browser_solve_datadome`)

Mirror `browser_solve_imperva`:

- Input: `{ timeout_ms (default 30_000), poll_interval_ms?, with_interception
  (default false) }`.
- Output: `{ outcome, datadome? }` where `outcome ∈ { cleared, challenge_gone,
  already_clear, blocked, timed_out }` and `datadome` is populated only on
  `cleared`.
- The solver callback is **not** wired over MCP (documented non-goal, exactly
  like imperva's `on_captcha`): a CAPTCHA surface without a registered solver
  surfaces as a `CaptchaRequired` error the agent handles out-of-band.
  `Blocked` is a distinct non-error outcome.

MCP coverage rule (repo CLAUDE.md): add the tool + a ledger entry in
`crates/zendriver-mcp/mcp-coverage-ledger.toml` for `Tab::datadome()` and the
new `Surface::Webgpu` variant (the latter is reachable via the existing
`browser_set_stealth_profile` / surface enum — regenerate schema snapshots).
Run the schema-snapshot step and `cargo insta accept --all`; commit the `.snap`
files.

## 13. Testing

- **Unit (MockConnection)** — deterministic, per-module, mirror the siblings:
  detection classification (mocked `detect.js` payloads → each surface),
  poll-loop terminals (mocked cookie/marker sequences → each `ClearanceOutcome`),
  captcha extraction + cookie application (mocked evals + `Network.setCookie`),
  interception signal.
- **Fixture integration** — a synthetic DataDome 403 page (static HTML: a
  `var dd = {...}` body + a fake `captcha-delivery` iframe + a clearance route
  that sets the `datadome` cookie). Exercises detect → poll → cookie-apply in
  normal CI with zero external dependency.
- **Nightly** (`datadome-tests` feature, gated `#[ignore]`) — a real
  DataDome-protected site + the deviceandbrowserinfo.com bot test, for drift
  signal; best-effort, never CI-blocking (the repo already has flaky
  network-timing tests; live anti-bot sites are worse).
- **WebGPU coherence** — unit-test the `webgpu.js` patch shape; assert
  `navigator.gpu.requestAdapter()` returns values coherent with the WebGL
  renderer in the existing stealth nightly.

## 14. Build sequencing

1. This spec → committed.
2. `writing-plans` → `docs/superpowers/plans/2026-06-02-datadome-bypass.md`.
3. Build on this branch (`claude/group-d-datadome`, off main @ 704c4c4) via
   subagent-driven TDD, per cohesive task.
4. Gates: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets
   --locked -- -D warnings`; `cargo clippy -p zendriver-mcp --all-features
   --all-targets -- -D warnings` (feature-gated code touched); schema snapshots;
   unit + fixture tests.
5. PR → `/founders-review` → merge → delete branch + worktree.

## 15. Explicit non-goals (v1)

- **Native slider / puzzle solving** (image-diff gap + Bézier drag) — the most
  cat-and-mouse surface; deferred to a possible future flagged feature. v1
  delegates to the opt-in solver callback.
- **Forging the device-check payload directly** (generating the `tags.js` POST
  out-of-browser to mint a cookie) — extremely brittle, rotates constantly;
  contrary to "run a real browser." Non-goal.
- **Fixing container GPU support** (Vulkan/GLES, Windows image) — belongs in
  `zendriver-docker`, not this repo (the upstream maintainer tracks it
  separately).
- **MCP solver callback** — same class as imperva's; agents handle
  `CaptchaRequired` out-of-band.
