# MCP Coverage Catch-up (#24–#30) — Design

- **Date:** 2026-06-02
- **Status:** Approved (brainstorming), pending implementation plan
- **Driver:** `zendriver-mcp` must track the `zendriver` surface (CLAUDE.md "MCP coverage" rule). Audit found the #24–#30 feature PRs added public APIs with no MCP tools.
- **Folds into:** the #30 branch (`claude/group-c-robustness`), which already carries the merged #24–#30 surface.

---

## 1. Context

`zendriver-mcp` currently exposes **65 tools** (`rmcp` SDK, `schemars::JsonSchema` inputs, `serde` outputs with `structured_content` for typed metadata, `insta` JSON-schema snapshots). An audit of the public APIs added in #24 (fingerprint), #28 (find/DOM), #29 (network), #30 (prefs) found these uncovered:

- **#24:** `BrowserBuilder::persona/persona_overlay/surface`, `Persona`/`Seed`/`Surface`/`Strategy`, the `zendriver-fingerprints` crate (pool/generative).
- **#28:** the predicate finders (`tag`/`attr*`/`has_attr`/`attr_regex`); `select`/`select_all`.
- **#29:** `tab.monitor()` (a `Stream`), `tab.request()`.
- **#30:** `BrowserBuilder::preference()`.

This spec closes those gaps (all except `select`/`select_all`), adds a CI check to prevent future drift, and follows the existing zendriver-mcp conventions throughout.

## 2. Decisions locked

- **Persona on the wire = JSON currency + a generate tool.** `browser_open` accepts a `Persona` JSON; `browser_fingerprint_generate` produces one from pool/generative.
- **Monitor = `start`/`read`/`stop`** handle tools with observe-time body capture (a `Stream` can't be one request/response tool).
- **CI = `cargo public-api` baseline + a forward coverage ledger.**
- **Skip `select`/`select_all`** MCP tools (redundant with `browser_find` `css`) — recorded `excluded` in the ledger.
- **mcp-builder refinements:** typed `structured_content` outputs for request/monitor/fingerprint; persona input stays an opaque `serde_json::Value` (documented exception — `Persona` is too large to mirror as a DTO); actionable `invalid_params` errors; **no `ToolAnnotations`** (the existing 65 tools don't set them — consistency over retrofitting).

## 3. Find predicates (extend the `Selector` bridge)

`crates/zendriver-mcp/src/selectors.rs` — extend the `Selector` struct with a combinable predicate group (mirrors the Rust 1A model: predicates combine with each other + the text predicates, but NOT with `css`/`xpath`/`role`):

```rust
// added to Selector
pub tag: Option<String>,
pub attrs: Option<Vec<AttrPredicate>>,   // ANDed

#[derive(Deserialize, JsonSchema)]
pub struct AttrPredicate {
    pub name: String,
    pub op: AttrOp,             // exact|contains|starts_with|ends_with|present|regex
    pub value: Option<String>, // required for all ops except `present`
}
```

- `Selector::validate()` gains: the predicate group (`tag`/`attrs` + the existing `text`/`text_exact`/`text_regex` reused as the text predicates) is mutually exclusive with `css`/`xpath`/`role`; combining them → `invalid_params("predicate fields (tag/attrs/text*) cannot combine with css/xpath/role")` (maps the Rust `ConflictingSelectors`). `present` op without a `value` is fine; other ops require `value` → else `invalid_params`.
- `find.rs` `resolve`/`resolve_all` thread the predicate fields into `FindBuilder.tag()/.attr()/.attr_contains()/.attr_starts_with()/.attr_ends_with()/.has_attr()/.attr_regex()` and the text predicates into `.containing_text()/.text_equals()/.text_matches()`.
- Unconditional (core `find`). Update the `browser_find`/`browser_find_all` descriptions to mention predicates.

## 4. `preference` (browser_open)

`tools/lifecycle.rs` `OpenInput` gains:
```rust
pub preferences: Option<std::collections::HashMap<String, serde_json::Value>>,
```
Looped into `BrowserBuilder::preference(k, v)` during launch. Unconditional. Description notes owned-vs-supplied behavior (defaults only auto-written for port-owned temp profiles).

## 5. `browser_request` tool (`tools/request.rs`, new)

```rust
#[derive(Deserialize, JsonSchema)]
pub struct RequestInput {
    pub tab_id: Option<String>,
    pub method: HttpMethod,                 // GET|POST|PUT|DELETE|HEAD|PATCH (enum)
    pub url: String,
    pub headers: Option<HashMap<String, String>>,
    pub body: Option<String>,
    pub json: Option<serde_json::Value>,    // sets body + Content-Type
    pub bypass_cors: Option<bool>,
}
#[derive(Serialize, JsonSchema)]            // structured_content
pub struct RequestOutput {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,                       // utf8-lossy (common JSON/text case)
    pub body_base64: String,                // full-fidelity bytes
}
```
Maps to `tab.request().<method>(url).header(..)/.json(..)/.body(..).bypass_cors().send()`. A non-2xx status is a **normal result** (carried in `status`), not an error; only transport/fetch failure → `map_error`. `json` + `body` both set → `invalid_params`. Unconditional (`tab.request` is core).

## 6. Persona (`browser_open` + `browser_fingerprint_generate`)

- **`browser_open`** `OpenInput` gains `pub persona: Option<serde_json::Value>` — parsed via `Persona::try_from_json`; a parse error → `invalid_params` with the serde message. Opaque `Value` (documented: it already encodes identity + per-surface strategies + seed; agents get it from the generate tool or hand-edit). Applied via `BrowserBuilder::persona(p)`.
- **`browser_fingerprint_generate`** (`tools/fingerprints.rs`, new):
  ```rust
  #[derive(Deserialize, JsonSchema)]
  pub struct GenerateInput { pub source: FpSource, pub seed: Option<u64> } // FpSource = pool|generative
  #[derive(Serialize, JsonSchema)]
  pub struct GenerateOutput { pub persona: serde_json::Value }  // structured_content
  ```
  `pool` → `zendriver_fingerprints::pool::PoolSet::load_or_download(..).sample(seed)`; `generative` → `Generator::embedded().generate(seed)`. Returns the persona JSON. Seed omitted → `Seed::random()`.
- **Gating:** `browser_fingerprint_generate` needs the optional `zendriver-fingerprints` crate → new mcp feature **`fingerprints`** (`= ["dep:zendriver-fingerprints"]` with its `pool`+`generative`), **opt-in** (drags the generative BN + pool deps). Persona-JSON-on-open is unconditional. The tool is `#[cfg(feature = "fingerprints")]`.

## 7. Monitor tools (`tools/monitor.rs`, new)

```rust
// browser_monitor_start
struct StartInput { tab_id: Option<String>, url_pattern: Option<String>, capture_bodies: Option<bool> }
struct StartOutput { handle: String }                 // structured_content
// browser_monitor_read
struct ReadInput  { handle: String, max: Option<usize> }
struct ReadOutput { events: Vec<MonitorEvent>, dropped: usize }   // structured_content
// browser_monitor_stop
struct StopInput  { handle: String }
struct StopOutput { stopped: bool }
```

- **State:** `SessionState` gains `monitors: Mutex<HashMap<String, MonitorState>>` where `MonitorState` holds a bounded `VecDeque<MonitorEvent>`, a `dropped` counter, the `CancellationToken`, and the drain `JoinHandle`.
- **`start`:** open `tab.monitor().url_pattern(..)?.start()`, spawn a task draining the `Stream`: for each `NetworkEvent`, convert to a `MonitorEvent` (serializable mirror); for `Http` exchanges when `capture_bodies`, call `exchange.body().await` **inside the drain task** (observe-time — before Chrome evicts) and attach it; WS/ES payloads are inline. Push to the deque; on cap overflow drop oldest + bump `dropped`. Returns a `handle` (uuid/short id).
- **`read`:** drain up to `max` events from the deque (clearing them) + return the current `dropped` then reset it. Unknown handle → `invalid_params`.
- **`stop`:** cancel the token, drop the `MonitorState` (RAII stops the monitor). Idempotent-ish: unknown handle → `{ stopped: false }` (don't error on double-stop).
- `MonitorEvent` = a serde mirror of `NetworkEvent` (Http{url,method,status,headers,error,body?,body_base64?}, WebSocketOpen/Frame/Close, EventSourceMessage).
- **Gating:** `tab.monitor()` is behind zendriver's `monitor` feature → new mcp feature **`monitor`** (`= ["zendriver/monitor"]`), **default-on** (light). Tools `#[cfg(feature = "monitor")]`.

## 8. CI: public-api baseline + coverage ledger

- **Files:** `crates/zendriver-mcp/tests/public_api.rs` (the check), `crates/zendriver-mcp/public-api-baseline.txt` (checked-in `cargo public-api -p zendriver` output), `crates/zendriver-mcp/mcp-coverage-ledger.toml`.
- **Ledger shape:**
  ```toml
  [[entry]]
  api = "zendriver::BrowserBuilder::preference"
  covered = "browser_open.preferences"
  [[entry]]
  api = "zendriver::Tab::select"
  excluded = "redundant with browser_find css"
  ```
- **Check:** the test runs `cargo public-api -p zendriver`, diffs against the baseline; every **new** public item (present now, absent from baseline) must have a ledger `entry` (`covered` or `excluded`) — else fail with the offending API + "add an MCP tool or record an exclusion reason in mcp-coverage-ledger.toml." Removed items → instruct to update the baseline.
- **Backfill now:** ledger entries for all #24–#30 new items (the tools added here = `covered`; `select`/`select_all` = `excluded`; the `zendriver-fingerprints` *crate* public items = `covered` by `browser_fingerprint_generate` where exposed, else `excluded`).
- **Caveat — nightly:** `cargo public-api` needs nightly rustdoc JSON. Add a dedicated **nightly CI job** running this test (the other jobs stay on stable). The test is `#[cfg_attr(not(feature = "public-api-check"), ignore)]` (or behind an env gate) so it only runs in that job. Pin a `cargo-public-api` version.

## 9. Features, conventions, testing

- **New mcp `Cargo.toml` features:** `monitor` (default-on, `zendriver/monitor`), `fingerprints` (opt-in, `dep:zendriver-fingerprints` + its `pool`/`generative`). Add `monitor` to the default set; document `fingerprints` as opt-in.
- **Conventions (match existing):** rich per-tool `description` strings in `server.rs`; `structured_content` for all new typed outputs; actionable `ErrorData::invalid_params` (list accepted variants / the conflict); **no `ToolAnnotations`** (project doesn't use them). Register every new tool in `server.rs` + `tools/mod.rs`.
- **Schema snapshots:** regenerate + accept `insta` JSON-schema snapshots for every new tool I/O (`cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked` + `cargo insta accept --all`); commit the `.snap` files.
- **Tests:** unit — `Selector` predicate validation + bridge; monitor buffer drain/cap/dropped; request input mapping. Integration (gated) — `browser_request` GET/POST against a fixture; monitor captures a fetch + WS; `browser_fingerprint_generate` returns a parseable persona that `browser_open` accepts.
- **CLAUDE.md feature note:** add `monitor` to the `cargo clippy -p zendriver-mcp --all-features` reminder line's relevance (already `--all-features` covers it).

## 10. Out of scope

- `select`/`select_all` MCP tools (redundant → ledger `excluded`).
- A typed `Persona` DTO (opaque `Value` for v1; revisit if agents need field-level schemas).
- The mcp-builder **evaluation suite** (10 eval Q&A) — valuable but a separate follow-up.
- Exposing every `zendriver-fingerprints` internal (only the generate tool surface).

## 11. Open questions for the plan stage

- Exact `cargo public-api` invocation + version pin + how the baseline is regenerated (a `just`/script target).
- `MonitorEvent` body fields when `capture_bodies=false` (omit vs null).
- Whether `browser_fingerprint_generate` `pool` source should download-on-first-use in an MCP context (network at tool-call time) or require a pre-seeded set — lean download-on-first-use with a clear error if offline.
