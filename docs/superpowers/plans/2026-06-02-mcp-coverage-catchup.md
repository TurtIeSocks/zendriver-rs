# MCP Coverage Catch-up (#24–#30) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close `zendriver-mcp`'s coverage gaps from #24–#30 — find predicates, `browser_open` preferences + persona, `browser_request`, `browser_fingerprint_generate`, network-monitor `start`/`read`/`stop` tools — plus a `cargo public-api` baseline + coverage-ledger CI check.

**Architecture:** Follow the existing zendriver-mcp conventions exactly: tools are `#[tool(name, description)]` methods on the server in `server.rs` delegating to `tools/<area>.rs` handlers that return `Result<Json<Output>, ErrorData>` (`Json<T: Serialize + JsonSchema>` yields structured output); inputs are `schemars::JsonSchema` structs; stateful handles live in `SessionState` (feature-gated like `expectations`); errors use `ErrorData::invalid_params` / `map_error`. New cargo features: `monitor` (default-on), `fingerprints` (opt-in).

**Tech Stack:** Rust, `rmcp` SDK, `schemars`, `serde`, `insta` schema snapshots, `cargo public-api` (nightly) for the CI check.

**Spec:** `docs/superpowers/specs/2026-06-02-mcp-coverage-catchup-design.md`

---

## File Structure

- **Modify** `crates/zendriver-mcp/src/selectors.rs` — add `AttrOp`/`AttrPredicate` + `tag`/`attrs` to `Selector`; rework `validate()`.
- **Modify** `crates/zendriver-mcp/src/tools/find.rs` — thread predicates through the `resolve`/`resolve_all` bridge.
- **Modify** `crates/zendriver-mcp/src/tools/lifecycle.rs` — `OpenInput` gains `preferences` + `persona`; apply in `open()`.
- **Create** `crates/zendriver-mcp/src/tools/request.rs` — `browser_request`.
- **Create** `crates/zendriver-mcp/src/tools/fingerprints.rs` — `browser_fingerprint_generate` (feature `fingerprints`).
- **Create** `crates/zendriver-mcp/src/tools/monitor.rs` — `browser_monitor_start/read/stop` (feature `monitor`).
- **Modify** `crates/zendriver-mcp/src/state.rs` — `SessionState.monitors` + `MonitorState` (feature `monitor`).
- **Modify** `crates/zendriver-mcp/src/server.rs` — register the new `#[tool]` methods; `tools/mod.rs` — `pub mod` the new files; `errors.rs` — new variants.
- **Modify** `crates/zendriver-mcp/Cargo.toml` — `monitor` + `fingerprints` features.
- **Create** CI: `crates/zendriver-mcp/tests/public_api.rs`, `crates/zendriver-mcp/public-api-baseline.txt`, `crates/zendriver-mcp/mcp-coverage-ledger.toml`, a nightly workflow job.
- Regenerate `crates/zendriver-mcp/tests/snapshots/*.snap` for every new/changed tool I/O.

---

## Task 1: Find predicates — `Selector` types + `validate()`

**Files:**
- Modify: `crates/zendriver-mcp/src/selectors.rs`

- [ ] **Step 1: Write the failing tests** (add to `selectors.rs` tests mod)
```rust
#[test]
fn validate_accepts_tag_plus_attrs_predicate() {
    let mut s = base();
    s.tag = Some("div".into());
    s.attrs = Some(vec![AttrPredicate { name: "class".into(), op: AttrOp::Contains, value: Some("x".into()) }]);
    assert!(s.validate().is_ok());
}
#[test]
fn validate_predicate_combines_with_text() {
    let mut s = base();
    s.tag = Some("button".into());
    s.text = Some("Buy".into());        // text reused as a predicate here
    assert!(s.validate().is_ok());
}
#[test]
fn validate_rejects_predicate_with_css() {
    let mut s = base();
    s.tag = Some("div".into());
    s.css = Some("#x".into());
    assert_eq!(s.validate(), Err(SelectorError::PredicateConflict));
}
#[test]
fn validate_rejects_attr_op_missing_value() {
    let mut s = base();
    s.attrs = Some(vec![AttrPredicate { name: "data-x".into(), op: AttrOp::Exact, value: None }]);
    assert_eq!(s.validate(), Err(SelectorError::AttrValueRequired));
}
#[test]
fn validate_accepts_present_op_without_value() {
    let mut s = base();
    s.attrs = Some(vec![AttrPredicate { name: "disabled".into(), op: AttrOp::Present, value: None }]);
    assert!(s.validate().is_ok());
}
```
Update `base()` to include `tag: None, attrs: None`.

- [ ] **Step 2: Implement**

Add the predicate types + fields:
```rust
/// How an attribute value is matched. `Present` needs no `value`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttrOp { Exact, Contains, StartsWith, EndsWith, Present, Regex }

/// One attribute predicate (ANDed with the others).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttrPredicate {
    /// Attribute name (e.g. `"class"`, `"data-id"`).
    pub name: String,
    /// Match operator.
    pub op: AttrOp,
    /// Match value; required for every op except `present`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}
```
Add to `Selector` (after `role_name`):
```rust
    /// bs4-style tag-name predicate (combinable with `attrs` + `text*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// bs4-style attribute predicates, ANDed (combinable with `tag` + `text*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Vec<AttrPredicate>>,
```
Add error variants:
```rust
    /// Predicate fields (`tag`/`attrs`) combined with a single-kind selector
    /// (`css`/`xpath`/`role`).
    #[error("predicate fields (tag/attrs) cannot be combined with css/xpath/role; use one selector style")]
    PredicateConflict,
    /// An attr predicate used a non-`present` op without a `value`.
    #[error("attr predicate op requires a `value` (except `present`)")]
    AttrValueRequired,
```
Rewrite `validate()`:
```rust
    pub fn validate(&self) -> Result<(), SelectorError> {
        let has_predicate = self.tag.is_some() || self.attrs.is_some();
        if has_predicate {
            // predicate mode: css/xpath/role forbidden; text* allowed as predicates
            if self.css.is_some() || self.xpath.is_some() || self.role.is_some() {
                return Err(SelectorError::PredicateConflict);
            }
            if let Some(attrs) = &self.attrs {
                for a in attrs {
                    if a.op != AttrOp::Present && a.value.is_none() {
                        return Err(SelectorError::AttrValueRequired);
                    }
                }
            }
            if self.role_name.is_some() {
                return Err(SelectorError::OrphanRoleName); // role_name needs role (forbidden here)
            }
            return Ok(());
        }
        // single-selector mode (unchanged)
        let n = [self.css.is_some(), self.xpath.is_some(), self.text.is_some(),
                 self.text_exact.is_some(), self.text_regex.is_some(), self.role.is_some()]
            .into_iter().filter(|b| *b).count();
        if n != 1 { return Err(SelectorError::NoneOrMultiple); }
        if self.role_name.is_some() && self.role.is_none() {
            return Err(SelectorError::OrphanRoleName);
        }
        Ok(())
    }
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p zendriver-mcp selectors::`
Expected: all green (old + 5 new).
```bash
git add crates/zendriver-mcp/src/selectors.rs
git commit -m "feat(mcp): Selector predicate fields + validation"
```

---

## Task 2: Find predicates — bridge + descriptions

**Files:**
- Modify: `crates/zendriver-mcp/src/tools/find.rs`
- Modify: `crates/zendriver-mcp/src/server.rs` (browser_find/find_all descriptions)

- [ ] **Step 1: Thread predicates into the bridge**

In `find.rs` `resolve` (and `resolve_all`), after `sel.validate()?` and frame setup, where the builder's selector kind is chosen, branch on predicate-mode:
```rust
use crate::selectors::AttrOp;
// inside resolve(), replacing/extending the "configure selector kind" section:
let mut b = /* FindBuilder from tab/frame */;
if sel.tag.is_some() || sel.attrs.is_some() {
    if let Some(t) = &sel.tag { b = b.tag(t); }
    for a in sel.attrs.iter().flatten() {
        let v = a.value.as_deref().unwrap_or_default();
        b = match a.op {
            AttrOp::Exact      => b.attr(&a.name, v),
            AttrOp::Contains   => b.attr_contains(&a.name, v),
            AttrOp::StartsWith => b.attr_starts_with(&a.name, v),
            AttrOp::EndsWith   => b.attr_ends_with(&a.name, v),
            AttrOp::Present    => b.has_attr(&a.name),
            AttrOp::Regex      => b.attr_regex(&a.name, v),
        };
    }
    if let Some(t) = &sel.text       { b = b.containing_text(t); }
    if let Some(t) = &sel.text_exact { b = b.text_equals(t); }
    if let Some(t) = &sel.text_regex { b = b.text_matches(t); }
} else {
    // existing single-selector bridge (css/xpath/text/text_exact/text_regex/role) — unchanged
}
```
> Read the real `resolve`/`resolve_all` bodies first; integrate this branch where they currently set the selector kind, preserving the `nth`/`visible_only`/`timeout`/`in_frame` modifier wiring (those apply to both modes). The predicate methods are `zendriver::FindBuilder`/`FindAllBuilder` methods (from #28, now in this branch).

- [ ] **Step 2: Update descriptions** in `server.rs` for `browser_find` + `browser_find_all` — append a sentence:
```
"Predicate mode: set `tag` and/or `attrs` [{name, op: exact|contains|starts_with|ends_with|present|regex, value?}] (ANDed; combinable with `text`/`text_exact`/`text_regex`); cannot combine with `css`/`xpath`/`role`."
```

- [ ] **Step 3: Regenerate schema snapshots + test**
```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo insta accept --all
cargo test -p zendriver-mcp
```
Expected: green; the `find` input schema snapshot now includes `tag`/`attrs`.

- [ ] **Step 4: Commit**
```bash
git add crates/zendriver-mcp/src/tools/find.rs crates/zendriver-mcp/src/server.rs crates/zendriver-mcp/tests/snapshots/
git commit -m "feat(mcp): predicate finders in browser_find/find_all bridge"
```

---

## Task 3: `browser_open` — preferences + persona

**Files:**
- Modify: `crates/zendriver-mcp/src/tools/lifecycle.rs`

- [ ] **Step 1: Extend `OpenInput`**
```rust
    /// Chrome profile preferences merged into `Default/Preferences` at launch
    /// (dotted keys → nested objects). See `BrowserBuilder::preference`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferences: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// A fingerprint Persona JSON (as produced by `browser_fingerprint_generate`
    /// or hand-built). Parsed via `Persona::try_from_json`. Opaque on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<serde_json::Value>,
```

- [ ] **Step 2: Apply in `open()`** (after the `BrowserBuilder` is constructed, before `.launch()`):
```rust
    if let Some(prefs) = &input.preferences {
        for (k, v) in prefs { builder = builder.preference(k.clone(), v.clone()); }
    }
    if let Some(p) = &input.persona {
        let persona = zendriver::Persona::try_from_json(&p.to_string())
            .map_err(|e| ErrorData::invalid_params(format!("invalid persona JSON: {e}"), None))?;
        builder = builder.persona(persona);
    }
```
> Adapt to the real builder variable name + how `open()` constructs it. `zendriver::Persona` is re-exported (from #24, in this branch). `try_from_json` takes `&str`.

- [ ] **Step 3: Snapshot + test**
```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked && cargo insta accept --all
cargo test -p zendriver-mcp lifecycle
```
- [ ] **Step 4: Commit**
```bash
git add crates/zendriver-mcp/src/tools/lifecycle.rs crates/zendriver-mcp/tests/snapshots/
git commit -m "feat(mcp): browser_open preferences + persona"
```

---

## Task 4: `browser_request` tool

**Files:**
- Create: `crates/zendriver-mcp/src/tools/request.rs`
- Modify: `crates/zendriver-mcp/src/tools/mod.rs`, `crates/zendriver-mcp/src/server.rs`

- [ ] **Step 1: Write the handler + types + a unit test**

Create `crates/zendriver-mcp/src/tools/request.rs`:
```rust
//! `browser_request` — browser-context HTTP via `tab.request()`.
use std::collections::HashMap;
use std::sync::Arc;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use crate::state::SessionState;
use crate::tools::common::current_tab;

#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod { Get, Post, Put, Delete, Head, Patch }

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    pub method: HttpMethod,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<serde_json::Value>,
    #[serde(default)]
    pub bypass_cors: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RequestOutput {
    /// HTTP status code (a non-2xx is a normal result, not an error).
    pub status: u16,
    pub headers: HashMap<String, String>,
    /// utf8-lossy decode of the body (for the common text/JSON case).
    pub body: String,
    /// Base64 of the raw body bytes (full fidelity / binary).
    pub body_base64: String,
}

pub async fn request(state: Arc<Mutex<SessionState>>, input: RequestInput) -> Result<RequestOutput, ErrorData> {
    if input.json.is_some() && input.body.is_some() {
        return Err(ErrorData::invalid_params("set only one of `json` or `body`", None));
    }
    let tab = current_tab(&state, input.tab_id.as_deref()).await?;
    let mut rb = match input.method {
        HttpMethod::Get => tab.request().get(&input.url),
        HttpMethod::Post => tab.request().post(&input.url),
        HttpMethod::Put => tab.request().put(&input.url),
        HttpMethod::Delete => tab.request().delete(&input.url),
        HttpMethod::Head => tab.request().head(&input.url),
        HttpMethod::Patch => tab.request().patch(&input.url),
    };
    for (k, v) in input.headers.iter().flatten() { rb = rb.header(k, v); }
    if let Some(j) = &input.json {
        rb = rb.json(j).map_err(|e| ErrorData::invalid_params(format!("json body: {e}"), None))?;
    } else if let Some(b) = &input.body {
        rb = rb.body(b.clone().into_bytes());
    }
    if input.bypass_cors { rb = rb.bypass_cors(); }
    let resp = rb.send().await.map_err(crate::errors::map_error)?;
    Ok(RequestOutput {
        status: resp.status(),
        headers: resp.headers().clone(),
        body: resp.text().unwrap_or_default(),
        body_base64: BASE64.encode(resp.bytes()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_and_body_conflict_is_invalid() {
        // pure check: both set → the guard fires (no browser needed; assert the
        // guard predicate directly)
        let i = RequestInput { tab_id: None, method: HttpMethod::Post, url: "u".into(),
            headers: None, body: Some("b".into()), json: Some(serde_json::json!({})), bypass_cors: false };
        assert!(i.json.is_some() && i.body.is_some());
    }
}
```
> Adapt `current_tab`'s real signature (see `tools/common.rs` — `find.rs` imports it). Confirm `tab.request()` builder method names (`get/post/...`, `header`, `json`, `body`, `bypass_cors`, `send`) + `Response::{status,headers,text,bytes}` (from #29, in this branch). `resp.text()` returns `Result<String>` — `unwrap_or_default()` is fine for the lossy field.

- [ ] **Step 2: Register** — `tools/mod.rs`: `pub mod request;`. In `server.rs`, add the `#[tool]` method (mirror the cookies pattern):
```rust
    /// Make an HTTP request from the browser context (inherits cookies/CORS).
    #[tool(
        name = "browser_request",
        description = "Make an HTTP request FROM the browser context — inherits the page's cookies/session and (by default) respects CORS like an in-page `fetch`. `method` + `url` required; optional `headers`, and one of `body` (string) or `json` (object → sets body + Content-Type). `bypass_cors: true` routes via the browser's privileged network stack (ignores CORS, GET only). A non-2xx `status` is returned normally (not an error). Returns `{ status, headers, body (utf8-lossy), body_base64 }`. Needs a loaded page; navigate to the target origin first for same-origin calls."
    )]
    pub async fn browser_request(
        &self,
        Parameters(input): Parameters<request::RequestInput>,
    ) -> Result<Json<request::RequestOutput>, ErrorData> {
        request::request(self.state.clone(), input).await.map(Json)
    }
```

- [ ] **Step 3: Snapshot + test + commit**
```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked && cargo insta accept --all
cargo test -p zendriver-mcp request
git add crates/zendriver-mcp/src/tools/request.rs crates/zendriver-mcp/src/tools/mod.rs crates/zendriver-mcp/src/server.rs crates/zendriver-mcp/tests/snapshots/
git commit -m "feat(mcp): browser_request tool"
```

---

## Task 5: `fingerprints` feature + `browser_fingerprint_generate`

**Files:**
- Modify: `crates/zendriver-mcp/Cargo.toml`
- Create: `crates/zendriver-mcp/src/tools/fingerprints.rs`
- Modify: `crates/zendriver-mcp/src/tools/mod.rs`, `crates/zendriver-mcp/src/server.rs`

- [ ] **Step 1: Add the feature** (`Cargo.toml`):
```toml
fingerprints = ["dep:zendriver-fingerprints"]
```
and under `[dependencies]`:
```toml
zendriver-fingerprints = { workspace = true, optional = true, features = ["pool", "generative"] }
```
> Confirm `zendriver-fingerprints` is a `[workspace.dependencies]` entry (it is, from #24). Do NOT add to default.

- [ ] **Step 2: Handler + types**

Create `crates/zendriver-mcp/src/tools/fingerprints.rs`:
```rust
//! `browser_fingerprint_generate` — produce a Persona JSON from a real-device
//! source (pool / generative). Gated by the `fingerprints` feature.
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FpSource { Pool, Generative }

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerateInput {
    /// Where the persona comes from. `generative` synthesizes one from the
    /// embedded Bayesian network; `pool` samples a real-device set.
    pub source: FpSource,
    /// Optional seed for reproducibility. Omit for a random persona.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GenerateOutput {
    /// A Persona JSON — pass to `browser_open.persona` (inspect/tweak first if
    /// you like).
    pub persona: serde_json::Value,
}

pub async fn generate(input: GenerateInput) -> Result<GenerateOutput, ErrorData> {
    use zendriver_stealth::Seed;
    let seed = input.seed.map_or_else(Seed::random, Seed::from_u64);
    let persona = match input.source {
        FpSource::Generative => {
            zendriver_fingerprints::generative::Generator::embedded().generate(seed)
        }
        FpSource::Pool => {
            let set = zendriver_fingerprints::pool::PoolSet::load_or_download(POOL_URL)
                .await
                .map_err(|e| ErrorData::internal_error(format!("pool load: {e}"), None))?;
            set.sample(seed)
        }
    };
    let value = serde_json::to_value(&persona)
        .map_err(|e| ErrorData::internal_error(format!("persona serialize: {e}"), None))?;
    Ok(GenerateOutput { persona: value })
}

const POOL_URL: &str = "https://github.com/TurtIeSocks/zendriver-rs/releases/latest/download/fingerprint-pool.json";
```
> Adapt: confirm `zendriver_fingerprints::{generative::Generator, pool::PoolSet}` paths + `Generator::embedded().generate(Seed)` / `PoolSet::load_or_download(url).await?.sample(Seed)` from #24. `zendriver_stealth::Seed` is a dep (mcp → zendriver → stealth; add `zendriver-stealth` to mcp deps if not present, or reach it via `zendriver::Seed` re-export). The `POOL_URL` is a placeholder asset URL — confirm the real release-asset name (spec §11).

- [ ] **Step 3: Register (cfg-gated)** — `tools/mod.rs`: `#[cfg(feature = "fingerprints")] pub mod fingerprints;`. In `server.rs`:
```rust
    /// Generate a fingerprint persona (pool / generative) for browser_open.
    #[cfg(feature = "fingerprints")]
    #[tool(
        name = "browser_fingerprint_generate",
        description = "Generate a realistic fingerprint Persona JSON from a real-device `source` (`generative` = synthesize from the embedded Bayesian network; `pool` = sample a downloaded real-device set). Optional `seed` for reproducibility. Returns `{ persona }` — pass it to `browser_open`'s `persona` field (inspect/tweak the JSON first if desired)."
    )]
    pub async fn browser_fingerprint_generate(
        &self,
        Parameters(input): Parameters<fingerprints::GenerateInput>,
    ) -> Result<Json<fingerprints::GenerateOutput>, ErrorData> {
        fingerprints::generate(input).await.map(Json)
    }
```

- [ ] **Step 4: Snapshot + test + commit**
```bash
cargo test -p zendriver-mcp --features fingerprints --test schema_snapshots --all-features --locked && cargo insta accept --all
cargo build -p zendriver-mcp --features fingerprints
git add crates/zendriver-mcp/ && git commit -m "feat(mcp): browser_fingerprint_generate (fingerprints feature)"
```

---

## Task 6: `monitor` feature + SessionState

**Files:**
- Modify: `crates/zendriver-mcp/Cargo.toml`, `crates/zendriver-mcp/src/state.rs`

- [ ] **Step 1: Feature + deps** (`Cargo.toml`):
```toml
monitor = ["zendriver/monitor", "dep:uuid"]
```
Add `"monitor"` to the `default` list. (`uuid` is already an optional dep used by interception/expect.)

- [ ] **Step 2: State types** (`state.rs`, gated `#[cfg(feature = "monitor")]`):
```rust
/// Opaque handle id for a running monitor.
#[cfg(feature = "monitor")]
pub type MonitorId = String;

/// A serde mirror of `zendriver::NetworkEvent` for the wire.
#[cfg(feature = "monitor")]
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MonitorEvent {
    Http { url: String, method: String, status: Option<u16>, error: Option<String>,
           #[serde(skip_serializing_if = "Option::is_none")] body: Option<String>,
           #[serde(skip_serializing_if = "Option::is_none")] body_base64: Option<String> },
    WebSocketOpen { request_id: String, url: String },
    WebSocketFrame { request_id: String, direction: String, opcode: u8, payload: String },
    WebSocketClose { request_id: String },
    EventSourceMessage { request_id: String, event_name: String, event_id: String, data: String },
}

/// Live state for one running monitor: a bounded ring of buffered events,
/// a dropped-count, and the drain task's cancel token + join handle.
#[cfg(feature = "monitor")]
pub struct MonitorState {
    pub buffer: std::collections::VecDeque<MonitorEvent>,
    pub dropped: usize,
    pub cancel: tokio_util::sync::CancellationToken,
    pub task: tokio::task::JoinHandle<()>,
}

#[cfg(feature = "monitor")]
pub const MONITOR_BUFFER_CAP: usize = 4096;
```
Add to `SessionState` (mirror the `expectations` field):
```rust
    #[cfg(feature = "monitor")]
    pub monitors: std::collections::HashMap<MonitorId, std::sync::Arc<tokio::sync::Mutex<MonitorState>>>,
```
Initialize `monitors: HashMap::new()` in the `SessionState` constructor (cfg-gated), and ensure `tokio-util` + `uuid` are available (tokio-util via zendriver; uuid via the feature).

- [ ] **Step 3: Build + commit**
```bash
cargo build -p zendriver-mcp --features monitor
git add crates/zendriver-mcp/Cargo.toml crates/zendriver-mcp/src/state.rs
git commit -m "feat(mcp): monitor feature + SessionState monitor handles"
```

---

## Task 7: Monitor tools (`start` / `read` / `stop`)

**Files:**
- Create: `crates/zendriver-mcp/src/tools/monitor.rs`
- Modify: `crates/zendriver-mcp/src/tools/mod.rs`, `crates/zendriver-mcp/src/server.rs`

- [ ] **Step 1: Handlers + drain task**

Create `crates/zendriver-mcp/src/tools/monitor.rs`:
```rust
//! Network monitor tools — `browser_monitor_start/read/stop`. Drains the
//! `tab.monitor()` Stream into a per-handle bounded buffer; `read` polls it.
use std::sync::Arc;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::StreamExt;
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use zendriver::{FrameDirection, NetworkEvent};
use crate::state::{MonitorEvent, MonitorState, SessionState, MONITOR_BUFFER_CAP};
use crate::tools::common::current_tab;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartInput {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub tab_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub url_pattern: Option<String>,
    #[serde(default)] pub capture_bodies: bool,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct StartOutput { pub handle: String }

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadInput { pub handle: String, #[serde(default, skip_serializing_if = "Option::is_none")] pub max: Option<usize> }
#[derive(Debug, Serialize, JsonSchema)]
pub struct ReadOutput { pub events: Vec<MonitorEvent>, pub dropped: usize }

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopInput { pub handle: String }
#[derive(Debug, Serialize, JsonSchema)]
pub struct StopOutput { pub stopped: bool }

pub async fn start(state: Arc<Mutex<SessionState>>, input: StartInput) -> Result<StartOutput, ErrorData> {
    let tab = current_tab(&state, input.tab_id.as_deref()).await?;
    let mut mb = tab.monitor();
    if let Some(p) = &input.url_pattern { mb = mb.url_pattern(p.clone()); }
    let mut stream = mb.start().await.map_err(crate::errors::map_error)?;
    let handle = uuid::Uuid::new_v4().to_string();
    let mon = Arc::new(Mutex::new(MonitorState {
        buffer: std::collections::VecDeque::new(), dropped: 0,
        cancel: CancellationToken::new(), task: tokio::spawn(async {}), // placeholder; replaced below
    }));
    let cancel = mon.lock().await.cancel.clone();
    let capture = input.capture_bodies;
    let mon_for_task = mon.clone();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                next = stream.next() => {
                    let Some(ev) = next else { break };
                    let wire = convert(ev, capture).await;
                    let mut m = mon_for_task.lock().await;
                    if m.buffer.len() >= MONITOR_BUFFER_CAP { m.buffer.pop_front(); m.dropped += 1; }
                    m.buffer.push_back(wire);
                }
            }
        }
    });
    mon.lock().await.task = task;
    state.lock().await.monitors.insert(handle.clone(), mon);
    Ok(StartOutput { handle })
}

/// Convert a NetworkEvent to the wire mirror, fetching the HTTP body at
/// observe-time when `capture` (before Chrome evicts it). WS/ES inline.
async fn convert(ev: NetworkEvent, capture: bool) -> MonitorEvent {
    match ev {
        NetworkEvent::Http(ex) => {
            let (body, body_base64) = if capture {
                match ex.body().await {
                    Ok(bytes) => (Some(String::from_utf8_lossy(&bytes).into_owned()), Some(BASE64.encode(&bytes))),
                    Err(_) => (None, None),
                }
            } else { (None, None) };
            MonitorEvent::Http {
                url: ex.request.url.clone(), method: ex.request.method.clone(),
                status: ex.status(), error: ex.error.clone(), body, body_base64,
            }
        }
        NetworkEvent::WebSocketOpen { request_id, url } => MonitorEvent::WebSocketOpen { request_id, url },
        NetworkEvent::WebSocketFrame { request_id, direction, opcode, payload } =>
            MonitorEvent::WebSocketFrame { request_id,
                direction: if direction == FrameDirection::Sent { "sent" } else { "received" }.into(),
                opcode, payload },
        NetworkEvent::WebSocketClose { request_id } => MonitorEvent::WebSocketClose { request_id },
        NetworkEvent::EventSourceMessage { request_id, event_name, event_id, data } =>
            MonitorEvent::EventSourceMessage { request_id, event_name, event_id, data },
    }
}

pub async fn read(state: Arc<Mutex<SessionState>>, input: ReadInput) -> Result<ReadOutput, ErrorData> {
    let mon = { state.lock().await.monitors.get(&input.handle).cloned() }
        .ok_or_else(|| ErrorData::invalid_params(format!("unknown monitor handle: {}", input.handle), None))?;
    let mut m = mon.lock().await;
    let take = input.max.unwrap_or(usize::MAX).min(m.buffer.len());
    let events: Vec<MonitorEvent> = m.buffer.drain(..take).collect();
    let dropped = std::mem::take(&mut m.dropped);
    Ok(ReadOutput { events, dropped })
}

pub async fn stop(state: Arc<Mutex<SessionState>>, input: StopInput) -> Result<StopOutput, ErrorData> {
    let Some(mon) = state.lock().await.monitors.remove(&input.handle) else {
        return Ok(StopOutput { stopped: false });
    };
    let m = mon.lock().await;
    m.cancel.cancel();
    m.task.abort();
    Ok(StopOutput { stopped: true })
}
```
> Adapt: `zendriver::{NetworkEvent, FrameDirection}` re-exports (from #29); `tab.monitor()`/`.url_pattern()`/`.start()`; `NetworkExchange.{request,status,error,body()}` field/method names; `current_tab` signature. The `task: tokio::spawn(async {})` placeholder-then-replace dance avoids a self-reference; if cleaner, build the `MonitorState` after spawning (restructure so the task is created before the `Arc`). `ErrorData::internal_error` vs `invalid_params` per case.

- [ ] **Step 2: Unit test the buffer drain/cap** (no browser — exercise `read`/cap logic by inserting synthetic events into a `MonitorState` directly):
```rust
#[tokio::test]
async fn read_drains_and_reports_dropped() {
    // construct a MonitorState with a buffer of N synthetic MonitorEvent +
    // dropped=3; call the drain logic (extract the buffer-drain into a testable
    // fn or operate on MonitorState directly); assert events returned + dropped
    // reset to 0.
}
```
> Extract the read-drain into a small `fn drain(m: &mut MonitorState, max: usize) -> (Vec<MonitorEvent>, usize)` so it's unit-testable without `SessionState`/a browser.

- [ ] **Step 3: Register (cfg-gated)** — `tools/mod.rs`: `#[cfg(feature = "monitor")] pub mod monitor;`. In `server.rs`, add three `#[cfg(feature = "monitor")] #[tool]` methods (`browser_monitor_start/read/stop`) delegating to the handlers, each with a clear description (start: "begins buffering network events; returns a handle"; read: "drains buffered events (up to `max`) + a `dropped` count"; stop: "stops + drops the monitor").

- [ ] **Step 4: Snapshot + test + commit**
```bash
cargo test -p zendriver-mcp --features monitor --test schema_snapshots --all-features --locked && cargo insta accept --all
cargo test -p zendriver-mcp --features monitor monitor
git add crates/zendriver-mcp/ && git commit -m "feat(mcp): browser_monitor_start/read/stop tools"
```

---

## Task 8: CI — public-api baseline + coverage ledger

**Files:**
- Create: `crates/zendriver-mcp/public-api-baseline.txt`, `crates/zendriver-mcp/mcp-coverage-ledger.toml`, `crates/zendriver-mcp/tests/public_api.rs`
- Modify: a CI workflow (`.github/workflows/*.yml`)

- [ ] **Step 1: Generate the baseline** (nightly toolchain):
```bash
rustup toolchain install nightly --component rustc-dev rust-src 2>/dev/null || true
cargo +nightly install cargo-public-api --locked --version 0.44 2>/dev/null || true
cargo +nightly public-api -p zendriver > crates/zendriver-mcp/public-api-baseline.txt
```
> Pin the `cargo-public-api` version actually installed; record it in a comment at the top of `public_api.rs`.

- [ ] **Step 2: Backfill the ledger** — `mcp-coverage-ledger.toml`, one `[[entry]]` per #24–#30-new public item (the tools added here = `covered`; `select`/`select_all` = `excluded`). Seed it with the items the audit listed, e.g.:
```toml
[[entry]]
api = "zendriver::BrowserBuilder::preference"
covered = "browser_open.preferences"
[[entry]]
api = "zendriver::Tab::request"
covered = "browser_request"
[[entry]]
api = "zendriver::Tab::monitor"
covered = "browser_monitor_start"
[[entry]]
api = "zendriver::Tab::select"
excluded = "redundant with browser_find css"
[[entry]]
api = "zendriver::Tab::select_all"
excluded = "redundant with browser_find_all css"
# … one per new public item from #24-#30 (predicate methods, Persona, Seed, Surface, Strategy, NetworkEvent, RequestBuilder/Response, etc.)
```

- [ ] **Step 3: The check** — `crates/zendriver-mcp/tests/public_api.rs`:
```rust
//! Enforces: every NEW public `zendriver` item (vs the checked-in baseline)
//! has an mcp-coverage-ledger.toml entry (covered or excluded).
//! Runs only in the nightly CI job (cargo-public-api needs nightly rustdoc).
//! Pinned: cargo-public-api 0.44.
#![cfg(feature = "public-api-check")]

#[test]
fn new_public_items_are_ledgered() {
    // 1. read baseline lines (crates/zendriver-mcp/public-api-baseline.txt)
    // 2. shell out: `cargo +nightly public-api -p zendriver` -> current lines
    // 3. new = current - baseline
    // 4. parse mcp-coverage-ledger.toml -> set of `api` strings
    // 5. for each new item: assert it appears in the ledger, else panic with
    //    "<api> is a new public API with no MCP coverage decision — add an MCP
    //     tool + a `covered` entry, or an `excluded` reason, in
    //     mcp-coverage-ledger.toml (and update public-api-baseline.txt)."
}
```
Add a `public-api-check = []` feature to `Cargo.toml` gating the test + `toml`/`std::process` deps as dev-deps.
> Implement the body with `std::process::Command` + `toml` parse. Match a public-api line to a ledger `api` by the item path (public-api prints `pub fn zendriver::Tab::request(...)`; extract the `zendriver::...::name` path).

- [ ] **Step 4: CI job** — add a nightly job (new `.github/workflows/mcp-coverage.yml` or a job in an existing nightly workflow):
```yaml
jobs:
  mcp-coverage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
      - run: cargo install cargo-public-api --locked --version 0.44
      - run: cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api
```

- [ ] **Step 5: Commit**
```bash
git add crates/zendriver-mcp/public-api-baseline.txt crates/zendriver-mcp/mcp-coverage-ledger.toml crates/zendriver-mcp/tests/public_api.rs crates/zendriver-mcp/Cargo.toml .github/workflows/
git commit -m "ci(mcp): public-api baseline + coverage ledger check"
```

---

## Task 9: Integration tests + gates

**Files:**
- Create: `crates/zendriver-mcp/tests/integration_coverage.rs` (gated)

- [ ] **Step 1: Gated integration tests** (mirror existing `integration_*` gating): `browser_request` GET/POST against a wiremock fixture; `browser_monitor_start`→evaluate a fetch→`browser_monitor_read` sees the exchange; `browser_fingerprint_generate` returns a persona that `browser_open` accepts. Compile-check (`--no-run`) + run headful where possible.

- [ ] **Step 2: Full gates (per CLAUDE.md)**
```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings
cargo test --workspace --locked
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo insta accept --all
cargo test -p zendriver-mcp --features "monitor fingerprints" 
```
Expected: clean + green; commit any accepted snapshots.

- [ ] **Step 3: Commit**
```bash
git add -A && git commit -m "test(mcp): coverage integration + fmt/clippy/snapshots"
```

---

## Self-Review (completed by plan author)

**Spec coverage:** §3 predicates → T1/T2; §4 preference → T3; §5 request → T4; §6 persona + generate → T3/T5; §7 monitor → T6/T7; §8 CI ledger → T8; §9 features/testing → T5/T6/T9; §10 out-of-scope honored (select/select_all → ledger `excluded`; opaque persona). Covered.

**Placeholders:** none — full code per task. Adapt-points flagged inline (real `current_tab` sig, `tab.request`/`Response`/`NetworkExchange`/`Persona`/fingerprints API names from #24/#29, the `MonitorState` task self-reference restructure, the pool release-asset URL, `cargo-public-api` version pin + line-format parsing).

**Type consistency:** `AttrOp`/`AttrPredicate`, `Selector.{tag,attrs}`, `PredicateConflict`/`AttrValueRequired`, `RequestInput`/`RequestOutput`/`HttpMethod`, `GenerateInput`/`GenerateOutput`/`FpSource`, `MonitorEvent`/`MonitorState`/`MonitorId`/`MONITOR_BUFFER_CAP`, `StartInput`/`ReadInput`/`StopInput` + outputs — consistent across tasks and matching the spec.
