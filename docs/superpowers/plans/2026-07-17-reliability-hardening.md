# Reliability & Evidence Hardening — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close a set of silent-data-loss and coherence gaps in the transport, monitor, expect, network-idle, and stealth surfaces, and add opt-in ergonomics — shipped as **one branch / one PR / one release wave**.

**Architecture:** Every change is either (a) additive+opt-in (new observable channel, new builder option) leaving the existing default reachable, or (b) a correctness fix to a documented-best-effort path that today silently misreports loss/completion. One breaking signature (`resolved_persona() -> Result`) is acceptable pre-release. No external actor or third-party fork is referenced anywhere; each defect is grounded in the current behavior of our own code.

**Tech Stack:** Rust 2024 (MSRV 1.85), Tokio, `tokio::sync::broadcast`, CDP over WebSocket, `insta` snapshot tests, `release-plz`, `cargo-public-api`.

## Global Constraints

- **Least-opinionated / everything-overridable:** never lock a value or remove an existing default. New strictness/coherence behavior lands as an opt-in knob or a new profile option; the current lenient default stays reachable.
- **Pre-release API churn is acceptable:** technically-stronger designs win over backward-compat. Mark breaking changes with a `!` conventional commit so `release-plz` `semver_check` passes.
- **MCP coverage (CI-enforced):** every new public item is either reachable via a `zendriver-mcp` tool or recorded in `crates/zendriver-mcp/mcp-coverage-ledger.toml` with `covered`/`excluded`. `Stream`-returning subscriptions are legitimately `excluded` (same class as `tab.monitor()`).
- **Doc sync (all three surfaces):** rustdoc on every new/changed public item (`no_run`-compilable); README feature matrix + MCP tool count where touched; mdBook chapter (`docs/book/src/`). `mdbook build docs/book` must pass.
- **Before every push:** `cargo fmt --all` → `cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged` → confirm `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings`. Clippy CI runs default features; also run `cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings` for feature-gated code.
- **Schema snapshots:** after any MCP I/O type change, `cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked` then `cargo insta accept --all`; commit the `.snap` diffs.
- **Public-API baseline:** on any intended `zendriver` public-API change, regenerate `cargo +nightly public-api -p zendriver --all-features > crates/zendriver-mcp/public-api-baseline.txt`.
- **Conventional commits, one per concern:** each task below = one commit. `release-plz` reads commits (not PR boundaries) to build the changelog and infer bumps.

---

## PR-Packaging Strategy (the crux of this plan)

**Decision: ONE feature branch → ONE PR → ONE `release-plz` release wave.**

Why this is the true minimum ceremony, not a compromise:

- `release-plz-pr.yml` accumulates every merge to `main` into a **single** pending "release PR." Ceremony cost (version bumps, changelog, `cargo publish` per bumped crate, GitHub Releases) is paid **once per release cycle**, regardless of how many feature PRs preceded it. So splitting into many PRs would **not** add publishing ceremony — but a single PR minimizes *review/merge* ceremony too.
- `release_always = false` + `semver_check = true`: bumps are inferred from conventional commits. Clean per-concern commits inside one PR yield a clean per-crate changelog **without** needing separate PRs.
- Everything here compiles as one unit; the dependency order (below) is satisfied by commit ordering within the branch, so no cross-PR sequencing is required.

**Crates that will bump + publish in the single wave:** `zendriver-transport`, `zendriver` (BREAKING → pre-1.0 minor bump), `zendriver-stealth`, `zendriver-mcp`. (`zendriver-datadome` is **not** touched — the cookie-scoping fix it would have carried is already in `main`.)

**Fallback (only if review size is a problem):** split at the Layer-2/Layer-3 boundary into two PRs (foundation+correctness, then ergonomics+docs). Both still land into the same one release wave. Default is not to split.

---

## Assumptions (judgement calls made on your behalf)

1. **Scope = all recommendations at their recommended treatment.** Correctness items land as fixes; the opinionated items (site-isolation/WebGL, input timing, idle strictness) land as **opt-in** knobs with current defaults preserved — because "adapt as opt-in, don't flip the default" *was* the recommendation for those.
2. **CSS-selector escaping for `AriaRole::Other` is dropped from scope.** `Other` takes `&'static str` (compile-time only), so there is no runtime-injection surface; escaping would be defense-in-depth against a non-existent vector. YAGNI. (Reinstate only if we ever widen `Other` to accept a runtime `String`.)
3. **The persona/`Accept-Language` coherence item is a spike, not a guaranteed change.** Our `StealthObserver` deliberately derives locale from `Fingerprint`, not a stored persona. Task 11 first proves whether a real divergence exists; it ships a fix only if it does.
4. **The docs-honesty reframe is in scope** (you said "all"), landing as a factual-accuracy edit to our own crate docs — no marketing claim we can't stand behind, no external reference.
5. **`BoundedBody` caps are configurable, not hardcoded.** A bounded default protects callers/agent context, but the bound and ceiling are caller-settable; the unbounded `body()` path stays.

---

## File Structure

| Area | Files | Responsibility |
|------|-------|----------------|
| Transport accounting | `crates/zendriver-transport/src/{connection.rs,actor.rs,frame.rs,lib.rs}` | New opt-in accounted event stream + generation counter |
| Observer policy | `crates/zendriver-transport/src/observer.rs` | Fail-closed-on-timeout as overridable policy |
| Core error | `crates/zendriver/src/error.rs`, `crates/zendriver/src/lib.rs` | `EventStreamIncomplete` variant + re-exports |
| Bounded body | `crates/zendriver/src/response_body.rs` (new), `crates/zendriver/src/lib.rs` | `BoundedBody` bounded capture + truncation flag |
| Network idle | `crates/zendriver/src/network_idle.rs` | responseReceived≠completion + `IdleOptions` strictness knob |
| Browser robustness | `crates/zendriver/src/browser.rs` | Observer-order ready barrier + atomic seed + `resolved_persona() -> Result` |
| Expect | `crates/zendriver/src/expect/*.rs` | Return `EventStreamIncomplete` instead of masking teardown as `Timeout` |
| Monitor | `crates/zendriver/src/monitor/mod.rs`, `crates/zendriver-mcp/src/tools/monitor.rs`, `crates/zendriver-mcp/src/state.rs` | `DeliveryBoundary` loss-accounting + bounded bodies over MCP |
| Stealth ergonomics | `crates/zendriver-stealth/src/{input_profile.rs,flags.rs,patches.rs,observer.rs}`, `crates/zendriver/src/browser.rs` | Opt-in input profile + opt-in native-isolation/real-WebGL profile |
| Docs | `crates/zendriver/src/lib.rs` docs, `README.md`, `crates/zendriver-mcp/README.md`, `docs/book/src/*`, crate `Cargo.toml` descriptions | Factual-accuracy reframe |
| Cross-cutting | `crates/zendriver-mcp/mcp-coverage-ledger.toml`, `crates/zendriver-mcp/public-api-baseline.txt`, `crates/zendriver-mcp/tests/snapshots/*` | Ledger + baseline + schema sync |

---

## LAYER 0 — Transport foundation (must land first)

### Task 1: Loss-accounted raw event stream

**Files:**
- Modify: `crates/zendriver-transport/src/connection.rs` (add `subscribe_raw_accounted`, `connection_generation`), `crates/zendriver-transport/src/actor.rs` (mirror events into the accounted bus **only when subscribers exist**), `crates/zendriver-transport/src/frame.rs` (new `AccountedRawEvent`), `crates/zendriver-transport/src/lib.rs` (export), `crates/zendriver/src/lib.rs` (re-export)
- Test: transport unit tests (sequencing, lag→`Lagged`, reconnect→`Reconnected`, disconnect→`Disconnected`)

**Interfaces — Produces:**
```rust
// frame.rs
pub enum AccountedRawEvent {
    Event { generation: u64, sequence: u64, event: RawEvent },
    Lagged { generation: u64, missed: u64 },
    Reconnected { previous: u64, generation: u64 },
    Disconnected { generation: u64 },
}
// connection.rs
impl Connection {
    pub fn subscribe_raw_accounted(&self) -> impl Stream<Item = AccountedRawEvent> + Send + Unpin;
    pub fn connection_generation(&self) -> u64;
}
```

**Rationale (our current defect):** `Connection::subscribe_raw` (`connection.rs:272`) drops broadcast-lagged frames silently (`res.ok()`), and a reconnect is invisible to subscribers. Any capture/replay/monitor consumer is silently misled. This is a real correctness gap in our transport.

**Critical implementation constraint:** the actor must **gate** the per-event clone + second-bus `send` behind `accounted_event_tx.receiver_count() > 0`. Never clone+push a `RawEvent` (method `String` + params `Value`, potentially large Network.* JSON) on connections with zero accounted subscribers. `subscribe_raw` stays byte-for-byte unchanged.

**Test intent:** subscribe_raw_accounted; force broadcast overflow → assert a `Lagged{missed:n}` with correct count and that `sequence` resumes monotonically after; trigger reconnect → assert `Reconnected{previous,generation}` with a bumped generation and per-generation sequence reset to 1; drop the ws → assert exactly one `Disconnected`.

**Doc/ledger:** rustdoc on all new items; add `excluded = "Stream-returning subscription; not a request/response tool"` ledger entries for `subscribe_raw_accounted`, `connection_generation`, `AccountedRawEvent`. Regenerate `public-api-baseline.txt`.

**Commit:** `feat(transport): add loss-accounted raw event stream (opt-in)`

### Task 2: Fail-closed observer timeout policy (overridable)

**Files:**
- Modify: `crates/zendriver-transport/src/observer.rs`
- Test: observer unit test `required_observer_timeout_detaches_target`

**Interfaces — Produces:**
```rust
pub enum ObserverFailurePolicy { Required, BestEffort }
impl TargetObserver { fn failure_policy(&self) -> ObserverFailurePolicy { ObserverFailurePolicy::Required } }
```

**Rationale (our current defect):** today (`observer.rs:13`) an observer that times out is *skipped and the actor releases the debugger* — the page then runs **without** the observer's setup (e.g. stealth patches never applied) = a silent bypass on the stable surface. The error/panic branches already `Target.detachFromTarget`; only the timeout branch fails open. Unify it: a `Required` observer that times out detaches (fail closed); `BestEffort` restores today's continue-the-chain behavior.

**Least-opinionated note:** this is the one behavior-default change. It is (a) overridable per-observer via `failure_policy() -> BestEffort`, and (b) closes a security-relevant silent bypass. Record it explicitly in the commit body as a shipped behavior change.

**Test intent:** a `Required` observer that never completes within the timeout → assert `Target.detachFromTarget` sent and the target is not handed out; a `BestEffort` observer → assert the debugger is released and the chain continues.

**Commit:** `feat(transport)!: observer timeout fails closed by default (BestEffort opt-out)`

### Task 3: `EventStreamIncomplete` error variant

**Files:**
- Modify: `crates/zendriver/src/error.rs` (add variant next to `Disconnected` at `error.rs:52`), `crates/zendriver/src/lib.rs` (ensure exported)
- Test: covered by Tasks 7/8 consumers

**Interfaces — Produces:**
```rust
// ZendriverError
#[error("event stream ended before the awaited condition was observed")]
EventStreamIncomplete,
```

**Rationale:** consumers (Task 7 expect, Task 8 monitor) need to distinguish "the condition did not occur" (`Timeout`) from "we lost the ability to observe it" (teardown/reconnect/dropped subscriber). Additive.

**Commit:** `feat(error): add EventStreamIncomplete to distinguish lost-observation from timeout`

---

## LAYER 1 — Core correctness

### Task 4: `wait_for_idle` — responseReceived ≠ completion, plus opt-in strictness

**Files:**
- Modify: `crates/zendriver/src/network_idle.rs`
- Test: `network_idle.rs` unit tests

**Interfaces — Produces (extend existing `IdleOptions`):**
```rust
pub enum IdleLossPolicy { Lenient, Strict } // Lenient = today's best-effort default
pub struct IdleOptions { /* existing fields */ pub loss_policy: IdleLossPolicy /* default Lenient */ }
```

**Rationale (our current defect):** `network_idle.rs:50-51,154-156` treats `Network.responseReceived` as a terminal event. `responseReceived` fires when **headers** arrive, not when the body finishes — so `wait_for_idle` can report idle while response bodies are still streaming. Fix: only `loadingFinished`/`loadingFailed` clear the in-flight set (`max_inflight_age` already covers stuck requests). Separately, add `IdleLossPolicy`: `Strict` (opt-in) surfaces `EventStreamIncomplete` on a delivery gap via the Task-1 accounted stream; `Lenient` (default) preserves today's best-effort semantics.

**Test intent:** simulate `responseReceived` without `loadingFinished` → assert still-in-flight (not idle); add `loadingFinished` → idle. With `Strict`, inject a `Lagged` boundary → `EventStreamIncomplete`; with `Lenient`, same injection → still resolves best-effort.

**Doc/ledger:** rustdoc; if `IdleOptions` is reachable via an MCP tool, extend it + snapshot; else ledger note. Book: `docs/book/src/` idle/monitor chapter.

**Commit:** `fix(network-idle): treat responseReceived as headers-only; add opt-in strict loss policy`

### Task 5: `BoundedBody` bounded body capture

**Files:**
- Create: `crates/zendriver/src/response_body.rs`
- Modify: `crates/zendriver/src/lib.rs` (`mod` + `pub use`)
- Test: `response_body.rs` unit tests

**Interfaces — Produces:**
```rust
pub struct BoundedBody { pub bytes: Vec<u8>, pub truncated: bool, pub encoded_len: u64 }
impl BoundedBody { pub fn capture(full: &[u8], max_bytes: usize) -> Self; } // max_bytes == 0 => reject/unbounded per taste
```

**Rationale:** unbounded body capture risks OOM / blown agent context. Provide bounded capture with an explicit `truncated` flag so a short body is never silently mistaken for a complete one. The existing unbounded `NetworkExchange::body()` path stays; bounding is caller-chosen (`max_bytes`), not a locked ceiling.

**Watch-out:** compute `truncated` against the **decoded** length, not a padded/base64 length, so a fully-captured small body is never reported as truncated.

**Test intent:** body < bound → `truncated=false`, bytes intact; body > bound → `truncated=true`, `bytes.len()==bound`, `encoded_len` = full length.

**Doc/ledger:** rustdoc; ledger `covered`/`excluded` as appropriate once Task 8 wires it to MCP. Regenerate baseline.

**Commit:** `feat(response-body): add BoundedBody bounded capture with explicit truncation`

### Task 6: Browser robustness — ready barrier + atomic seed + `resolved_persona() -> Result`

**Files:**
- Modify: `crates/zendriver/src/browser.rs`, `crates/zendriver/src/error.rs` (a `Seed`/persona error variant if not present)
- Test: `browser.rs` unit tests

**Interfaces — Produces (breaking):**
```rust
// was: fn resolved_persona(&self) -> Persona
fn resolved_persona(&self) -> Result<Persona, ZendriverError>;
```

**Rationale (our current defects):** (a) a `Tab` can be handed out before the stealth/user/shadow-root observers finish → a fingerprint-coherence race; fix by registering the tab-registrar observer **last** as a ready barrier. (b) The persisted persona seed is written non-atomically and a corrupt/truncated seed silently yields a fresh identity for a *credentialed* profile; fix with `create_new(true)` + `sync_all()` + an `AlreadyExists` concurrent-create re-read, and make a malformed seed a hard error (hence `resolved_persona -> Result`).

**Test intent:** concurrent seed create → both observers converge on one identity (no duplicate write, no rotation); truncated seed file → `resolved_persona()` returns `Err`, not a silent new identity; tab handed out only after observers complete.

**Doc/ledger:** rustdoc; regenerate baseline (breaking signature). Mark commit BREAKING.

**Commit:** `fix(browser)!: ready-barrier tab handoff, atomic seed, fail-closed on corrupt identity`

### Task 7: Expect — surface `EventStreamIncomplete` instead of masking teardown as `Timeout`

**Files:**
- Modify: `crates/zendriver/src/expect/{mod.rs,dialog.rs,download.rs,request.rs,response.rs}`
- Test: expect unit tests

**Interfaces — Consumes:** Task 1 `subscribe_raw_accounted` / `AccountedRawEvent`, Task 3 `EventStreamIncomplete`.

**Rationale:** today a dropped subscriber or transport teardown during an `expect_*` wait surfaces as `Timeout` — indistinguishable from "the awaited event genuinely never happened." Subscribe via the accounted stream and return `EventStreamIncomplete` on `Disconnected`/`Reconnected`/decode-failure. Also relabel the dialog `expect` default doc from "default/auto-accept" to "observe" — `MatchedDialog` has no `Drop` auto-accept, so the current doc is inaccurate.

**Test intent:** await an event, then tear down the stream before it arrives → `EventStreamIncomplete` (not `Timeout`); genuine no-show within deadline → still `Timeout`.

**Doc/ledger:** rustdoc; MCP `expect` tool schema unchanged unless error surface is exposed — re-run schema snapshots to confirm no diff.

**Commit:** `fix(expect): report EventStreamIncomplete on transport teardown instead of Timeout`

---

## LAYER 2 — Observability consumer

### Task 8: Monitor `DeliveryBoundary` loss-accounting + bounded bodies over MCP

**Files:**
- Modify: `crates/zendriver/src/monitor/mod.rs` (new `NetworkEvent::DeliveryBoundary`, `NetworkDeliveryBoundary`, subscribe via accounted stream, `partial.clear()` on gap), `crates/zendriver-mcp/src/tools/monitor.rs` + `crates/zendriver-mcp/src/state.rs` (surface boundaries + bounded bodies + truncation fields), `crates/zendriver-mcp/tests/snapshots/*monitor*`
- Test: monitor unit tests + schema snapshots

**Interfaces — Produces:**
```rust
pub enum NetworkDeliveryBoundary { Lagged{missed:u64,generation:u64}, Reconnected{previous:u64,generation:u64}, Disconnected{generation:u64}, CorrelationEvicted{url:String}, DecodeFailed, Unknown }
// NetworkEvent (currently: Http, WebSocketOpen, WebSocketFrame, WebSocketClose, EventSourceMessage)
NetworkEvent::DeliveryBoundary(NetworkDeliveryBoundary),
```
**Consumes:** Task 1 accounted stream, Task 5 `BoundedBody`.

**Rationale (our current defects):** the monitor assembles exchanges from `subscribe_raw` and (a) silently stitches across dropped windows (a delivery gap can emit a partial exchange as "complete"), (b) silently `evict_one`s past the 10k correlation cap, (c) silently `continue`s on decode failure, (d) silently degrades body-fetch errors to `(None,None)` indistinguishable from empty. Replace each silent path with an explicit `DeliveryBoundary` event and `partial.clear()`/`urls.clear()` on gap. Expose bounded bodies + `body_truncated`/`body_encoded_bytes` + `body_capture_error` over the MCP tool.

**Non-exhaustive note:** `NetworkEvent` is a plain enum; adding a variant is a compile-break for exhaustive external matches (fine pre-release; update the in-repo `examples/network_monitor.rs` match).

**Test intent:** inject a `Lagged` mid-exchange → assert the partial is cleared and a `DeliveryBoundary::Lagged` is emitted rather than a bogus complete `Http`; saturate correlation map past cap → `CorrelationEvicted`; malformed payload → `DecodeFailed` (raw payload omitted). Schema snapshot: new `delivery_boundary` oneOf variant + truncation fields present.

**Doc/ledger:** ledger entries for `NetworkDeliveryBoundary` (+ `BoundedBody` `covered` via monitor tool); regenerate schema snapshots (`cargo insta accept --all`) + baseline; README MCP tool matrix if the monitor output shape is described there; book `mcp.md`/monitor chapter.

**Commit:** `feat(monitor): surface delivery-loss boundaries and bounded bodies instead of silent gaps`

---

## LAYER 3 — Stealth opt-in ergonomics (independent of Layers 0–2)

### Task 9: Opt-in `InputProfile` selection (native timing stays default)

**Files:**
- Modify: `crates/zendriver-stealth/src/input_profile.rs` (add `InputProfile::coherent()` + `PartialEq`), `crates/zendriver/src/browser.rs` (`BrowserBuilder::input_profile(InputProfile)` + `resolved_input_profile()` decoupled from stealth selection)
- Test: input-profile unit test + a browser-builder test

**Interfaces — Produces:**
```rust
impl InputProfile { pub fn coherent() -> Self; }
impl BrowserBuilder { pub fn input_profile(self, p: InputProfile) -> Self; }
```

**Rationale:** input timing is currently coupled to stealth selection, so turning stealth off can silently turn actions into zero-delay mechanical input. Decouple it: let callers pick a non-mechanical `coherent()` preset **explicitly**, independent of stealth. Default is unchanged (native timing) — we do **not** add latency to plain browsers by default.

**Test intent:** `input_profile(InputProfile::coherent())` on a stealth-off browser → non-mechanical timing applied; no call → today's native default unchanged.

**Doc/ledger:** rustdoc; add MCP field or `excluded` ledger entry for `InputProfile::coherent` + `BrowserBuilder::input_profile`; regenerate baseline (two new items). Book stealth/input chapter.

**Commit:** `feat(stealth): opt-in coherent input profile decoupled from stealth selection`

### Task 10: Opt-in native site-isolation + real-WebGL profile (no default flip)

**Files:**
- Modify: `crates/zendriver-stealth/src/flags.rs`, `crates/zendriver-stealth/src/patches.rs`, associated `insta` flag snapshots
- Test: flag snapshot tests

**Rationale:** offer a profile that keeps Chrome's real site isolation (`IsolateOrigins`/`site-per-process` **not** disabled) and the real WebGL renderer (no vendor/renderer patch), for callers who want maximum coherence/security over spoofing. **This is a trade-off, not a strict win** — dropping the WebGL patch removes an anti-WAF coherence defense — so it ships as an **explicit profile option**, and the current default profile's flags/patches are **unchanged**. Update snapshots only for the new option's expected output.

**Test intent:** the new profile's flag set omits the isolation-disable and its patch set omits the WebGL block; the existing default profile's snapshots are byte-identical to today.

**Doc/ledger:** rustdoc; book stealth chapter documents the coherence-vs-anti-WAF trade-off honestly; baseline if a new public profile constructor is added.

**Commit:** `feat(stealth): add opt-in native-isolation/real-WebGL profile (default unchanged)`

### Task 11 (SPIKE → conditional): persona `Accept-Language` coherence

**Files:** investigate `crates/zendriver-stealth/src/observer.rs` (`observer.rs:110` uses `Persona::default()` for `resolve_languages`, while locale is documented as carried on `Fingerprint`).

**Steps:**
- [ ] Determine whether a persona-provided `Accept-Language` can diverge from the `Fingerprint`-derived language list on any code path (does any caller set persona locale without it reaching `fingerprint`?).
- [ ] If a real divergence exists → route both the JS-surface language and the `Emulation.setUserAgentOverride.acceptLanguage` from one source; add a unit test asserting header==JS locale. Commit `fix(stealth): single-source Accept-Language and JS locale`.
- [ ] If no divergence exists → record the finding in the PR description and **skip** (no code change). No commit.

**Rationale:** avoid porting a "fix" for a bug that may not exist in our design; prove the gap first.

---

## LAYER 4 — Docs honesty (independent, lands last)

### Task 12: Factual-accuracy reframe of anti-detection claims

**Files:**
- Modify: `crates/zendriver/src/lib.rs` crate-doc header, `README.md`, `crates/zendriver-mcp/README.md`, `docs/book/src/{introduction.md,stealth.md,faq.md}`, `crates/zendriver/Cargo.toml` (+ any crate) `description`
- Test: `mdbook build docs/book`

**Rationale:** our docs currently state "undetectable by default" / imply guaranteed invisibility. No automation stack can guarantee that; the claim is unsupportable and undercuts trust. Reframe to what we actually deliver: coherent identity + explicit anti-detection controls, with an honest "no stack guarantees invisibility to a determined site" caveat. Keep the feature matrix and MCP tool count accurate.

**Test intent:** `mdbook build docs/book` passes; grep shows no remaining "undetectable"/"guaranteed" absolute claims; tool count matches default-feature build.

**Commit:** `docs: reframe anti-detection claims to accurate, non-absolute language`

---

## Finalization (part of the single PR, before opening it)

### Task 13: Gates green + coverage/baseline/snapshots synced

- [ ] `cargo fmt --all` ; `cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged`
- [ ] Confirm: `cargo fmt --all --check` ; `cargo clippy --workspace --all-targets --locked -- -D warnings` ; `cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings`
- [ ] `cargo test --workspace --locked` (run in background; > 5s)
- [ ] `cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked` ; `cargo insta accept --all` ; commit `.snap` diffs
- [ ] `cargo +nightly public-api -p zendriver --all-features > crates/zendriver-mcp/public-api-baseline.txt` ; then `cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked` (needs nightly + `cargo-public-api` 0.52.0)
- [ ] Confirm every new public item has a `covered`/`excluded` entry in `mcp-coverage-ledger.toml`
- [ ] `mdbook build docs/book`
- [ ] Open ONE PR; let `release-plz-pr.yml` build the single release PR; merge feature PR → review release PR → merge release PR once to publish the wave.

**Commit:** fold into the tasks above (no standalone commit — this is the gate, not a change).

---

## Self-Review

**Spec coverage** — every recommendation mapped: transport loss-accounting (T1), fail-closed observer (T2), EventStreamIncomplete (T3), network_idle responseReceived + opt-in strictness (T4), BoundedBody (T5), browser ready-barrier/atomic-seed/resolved_persona (T6), expect teardown reporting (T7), monitor DeliveryBoundary + bounded bodies (T8), opt-in input profile (T9), opt-in native-isolation/WebGL (T10), persona locale spike (T11), docs honesty (T12). Drift casualties (cookie `psl` scoping, `visible_only` wiring) intentionally excluded — already in `main`. CSS-escape intentionally excluded — no runtime injection surface (Assumption 2).

**Type consistency** — `AccountedRawEvent`/`subscribe_raw_accounted` (T1) consumed by T7/T8; `EventStreamIncomplete` (T3) consumed by T4/T7/T8; `BoundedBody` (T5) consumed by T8; `NetworkDeliveryBoundary` variants mirror `AccountedRawEvent` variants. Names consistent across tasks.

**Placeholder scan** — no TBD/TODO; each task carries files, signatures, rationale grounded in a cited current-code line, test intent, and commit message.

**Ordering** — Layer 0 → 1 → 2 strictly ordered by the `AccountedRawEvent`/`EventStreamIncomplete` dependency; Layers 3–4 independent and may interleave. All within one branch; commit order satisfies compile order.
