---
name: add-mcp-tool
description: >-
  Use when adding or extending a tool in the zendriver-mcp crate — exposing a
  zendriver `Tab`/`Browser`/`Frame`/`Element` API to MCP clients, creating a new
  `browser_*` tool, or changing a tool's input/output type. Triggers on edits
  under `crates/zendriver-mcp/src/tools/` or the `#[tool]` wrappers in
  `server.rs`. Keywords: rmcp, tool_router, JsonSchema, insta snapshot,
  coverage ledger, public_api, deny_unknown_fields, AckOutput.
---

# Add a zendriver-mcp tool

## Overview

A tool is two pieces: a **handler** (free async fn + typed I/O structs) in
`tools/<category>.rs`, and a thin **`#[tool]` wrapper** in `server.rs` that
delegates to it. Two gates fail *late* (not in stable PR CI) and are routinely
forgotten — the **schema snapshot** and the **coverage ledger**. This skill is
mostly about not skipping them.

Mirror the canonical examples instead of inventing shape:
- `tools/scroll.rs` — smallest complete end-to-end tool (I/O structs + handler + no-browser test).
- `tools/stealth.rs` — reuses the shared `actions::AckOutput` for fire-and-forget tools.
- `server.rs` `base_tool_router` block — the `#[tool]` wrapper pattern.

Categories (one file each under `tools/`): `actions cloudflare cookies download eval expect fetcher find fingerprints frames imperva intercept lifecycle monitor mouse navigation pdf reads request scroll snapshot stealth storage tabs window`. Add to the matching file; only make a new module for a genuinely new category.

## Procedure

1. **Handler + I/O** in `tools/<category>.rs` (use an existing category file; only add a new module for a genuinely new category).
2. **Wrapper** — a `#[tool]` method in `server.rs` in the right router block (see table).
3. **mod.rs** — `pub mod <x>;` only if you created a new module (`#[cfg(feature = "...")]` if gated).
4. **Snapshot** — register + regenerate + accept + commit the `.snap` (gate #1).
5. **Ledger** — add a `covered`/`excluded` entry for any new `zendriver` symbol exposed (gate #2).
6. **Verify** — fmt, clippy, snapshot test, no-browser unit test.

## Handler template (mirror `scroll.rs`)

```rust
use std::sync::Arc;
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::common::current_tab; // + page_snapshot if you support return_snapshot

/// Input for `browser_<name>`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)] // REQUIRED on every input struct
pub struct FooInput {
    /// Doc comments BECOME the JSON-schema `description` — write them for the agent.
    pub bar: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<i64>,
}

// Fire-and-forget? Reuse `crate::tools::actions::AckOutput { ok: bool }` — do NOT
// invent a new output type. Returning data? Define a `#[derive(Debug, Serialize, JsonSchema)]`
// output and use `#[serde(skip_serializing_if = "Option::is_none")]` on optional fields.

pub async fn foo(state: Arc<Mutex<SessionState>>, input: FooInput)
    -> Result<crate::tools::actions::AckOutput, ErrorData>
{
    let s = state.lock().await;
    let tab = current_tab(&s).await?; // surfaces BrowserNotOpen / NoCurrentTab
    tab.some_api(input.bar)
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?; // mandatory uniform error mapping
    Ok(crate::tools::actions::AckOutput { ok: true })
}

#[cfg(test)] // REQUIRED no-browser test — unit tests have no Chrome
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn foo_without_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = foo(state, FooInput { bar: "x".into(), speed: None })
            .await.expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }
}
```

## Wrapper template (in `server.rs`)

```rust
/// Doc comment should track the `description` string below.
#[tool(name = "browser_foo", description = "…agent-facing description…")]
pub async fn browser_foo(
    &self,
    Parameters(input): Parameters<foo::FooInput>, // rmcp `Parameters(..)` wrapper is mandatory
) -> Result<Json<actions::AckOutput>, ErrorData> {
    foo::foo(self.state.clone(), input).await.map(Json) // one-line delegate + `.map(Json)`
}
```

No-arg tool? Use the shared `crate::tools::common::EmptyInput` (a `pub struct EmptyInput {}`, constructed `EmptyInput {}`) — for both the handler param and `Parameters<EmptyInput>`. It is already schema-snapshotted (`common_empty_input`), so a no-arg tool registers only its **output** snapshot. Tool `name` must be **globally unique** across every router (a duplicate `#[tool(name=...)]` collides at runtime).

## Which router block?

| Tool is… | Put wrapper in | Also edit `combined_tool_router()`? |
|----------|----------------|-------------------------------------|
| always available | `base_tool_router` block | no |
| under an existing feature (`interception`/`expect`/`monitor`/`cloudflare`/`imperva`/`fetcher`/`fingerprints`) | that feature's `#[cfg(feature=…)] #[tool_router(router = <feat>_tool_router)]` block | no (already summed) |
| under a brand-new feature | a new `#[cfg(feature=…)] #[tool_router(router = <feat>_tool_router)]` block | **yes** — add `#[cfg] let router = router + Self::<feat>_tool_router();` |

The `tool_router` macro can't see through per-method `#[cfg]`, which is why feature tools live in separate impl blocks summed by `combined_tool_router()`.

## The two gates that fail late (don't skip)

**Gate 1 — schema snapshot.** Register each new I/O type in `tests/schema_snapshots.rs`, naming the snapshot `<category>_<thing>_in` / `_out` (e.g. `schema_snap!(scroll_page_out, tools::scroll::PageScrollOutput);`). Then:
```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo insta accept --all
```
Commit the generated `tests/snapshots/*.snap`. **CI's `cargo test --lib` does NOT run this** (it's an `--test`, default-features) — a skipped regen won't redden the PR; it surfaces as a wrong/pending wire shape in review. Reuse existing snapshots (e.g. `actions_ack_out`) instead of duplicating.

**Gate 2 — coverage ledger.** Every new public `zendriver` symbol the tool exposes — the method **and** any new public return/param type it surfaces — needs an entry in `crates/zendriver-mcp/mcp-coverage-ledger.toml`:
```toml
[[entry]]
api = "zendriver::tab::Tab::some_api"  # exact cargo-public-api path
covered = "browser_foo"                 # or: excluded = "<reason>"
```
Enforced **only by the nightly `mcp-coverage` job** (`tests/public_api.rs`, feature `public-api-check`) — green stable PR CI does not catch a missing entry. See CLAUDE.md "MCP coverage" for the local nightly command + how to regenerate `public-api-baseline.txt`.

## Verify (see CLAUDE.md "Before every push")

Run together: `cargo fmt --all`, then `cargo clippy --workspace --all-targets --locked -- -D warnings`, the snapshot test above, and `cargo test -p zendriver-mcp --lib`. Feature-gated tool → also `cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings`.

## Common mistakes

| Mistake | Fix |
|---------|-----|
| Skipped snapshot regen — "CI was green" | CI `--lib` never runs it. Run the `--all-features` snapshot test + commit `.snap`. |
| Skipped ledger entry | Nightly-only enforcement. Add `covered`/`excluded` for the exposed symbol now. |
| New output struct for a fire-and-forget tool | Reuse `actions::AckOutput { ok }`. |
| Forgot `#[serde(deny_unknown_fields)]` | Every input struct has it. |
| Dropped the `Parameters(..)` wrapper in the `#[tool]` method | Signature is `Parameters(input): Parameters<T>`. |
| New feature router not summed | Add the `#[cfg] router + Self::<feat>_tool_router()` line in `combined_tool_router()`. |
| Reused an existing tool `name` | Names are globally unique across routers. |
| No no-browser unit test | Add the `expect_err` / "Browser not open" test — unit tests have no Chrome. |
