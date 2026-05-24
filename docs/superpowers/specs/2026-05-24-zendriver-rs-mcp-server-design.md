# zendriver-rs — MCP Server (`zendriver-mcp`)

**Date:** 2026-05-24
**Status:** Approved (delegate-mode brainstorming complete, ready for implementation plan)
**Depends on:** zendriver-rs v0.1.0 (P1–P6 all on `main`)
**Builds against:** rmcp v1.7+ (official Anthropic Rust MCP SDK)

## Summary

Ship `zendriver-mcp`, a new workspace crate that wraps zendriver-rs as a Model Context Protocol (MCP) server binary. Any MCP-compatible client (Claude Desktop, claude-code, custom agents) can drive a real, stealth-by-default Chrome browser through ~51 tools covering the full zendriver public API: navigation, find, actions, snapshots, eval, cookies, storage, frames, interception, expect, Cloudflare bypass, and Chrome-for-Testing fetch.

Two transports — stdio (default, local Claude Desktop / claude-code use case) and streamable HTTP (remote, multi-user use case). Single binary install via `cargo install zendriver-mcp`. Tools mirror zendriver's surface area closely so a power user familiar with the lib can predict the MCP tool name and shape.

Stealth-by-default is the strategic moat against playwright-mcp / chrome-devtools-mcp / browserbase: this server can scrape Cloudflare-protected and bot-detected sites out of the box.

Exit criterion: `zendriver-mcp` published to crates.io alongside the next release; mdBook chapter `mcp.md` with Claude Desktop config snippet; 10-question evaluation set passing against real Chrome; smoke tests over real stdio + HTTP transports green in CI.

## Goals

- **New workspace crate** `crates/zendriver-mcp/` (bin + thin lib). Published with the workspace at the next release cycle.
- **Two transports** via rmcp: stdio (default; `zendriver-mcp` with no args) + streamable HTTP (`zendriver-mcp --http <addr>`).
- **Session model:**
  - stdio → one `Browser` per process, single `current_tab` index in `SessionState`.
  - HTTP → one `Browser` per MCP session ID, dropped on session disconnect.
- **~41 core tools + ~10 gated tools**, all prefixed `browser_*`. Comprehensive zendriver API coverage; tools collapse where they'd otherwise be 1:1 wrappers (e.g. `browser_element_state` collapses 5+ read methods).
- **Selector-based handle model.** No persistent element refs. Every find / action tool takes the same `Selector` arg (one-of `css | xpath | text | text_exact | text_regex | role`, modifiers `nth | visible_only | timeout_ms | frame_id`). LLM uses selectors directly; server re-queries each call. Cheaper tokens than ref-numbered models; matches selector-driven Q5 decision.
- **Three snapshot modalities** for "let LLM see the page":
  - `browser_snapshot` — accessibility tree as plain text, structure-only, **no ref numbers** (selectors not refs).
  - `browser_html` — rendered HTML, optional `selector` subtree, optional `trim` (drop scripts/styles).
  - `browser_screenshot` — PNG/JPEG/WebP, returned as inline image content block; optional `save_path` for disk write.
- **JS eval exposed in two flavors**: `browser_evaluate` (isolated world, safer default) + `browser_evaluate_main` (main-world escape hatch, tool description warns about stealth breakage).
- **Auto-snapshot opt-in.** State-changing tools accept `return_snapshot: bool` (default `false`); when `true`, response bundles a fresh acc-tree snapshot to save a round trip.
- **Stealth-by-default.** Same as zendriver crate. `browser_open` accepts `stealth_profile` arg; `browser_set_stealth_profile` allows runtime swap.
- **Gated tools mirror crate features.** Same cargo feature flags as `zendriver`: `interception`, `expect`, `cloudflare`, `fetcher`. The published binary defaults to **all features on** (single-install convenience); lean local builds can opt out via `--no-default-features --features stealth`.
- **Actionable error messages.** `ZendriverError` mapped to MCP errors with `_metadata.suggested_next` hints (e.g. `ElementNotFound` → "Try `browser_snapshot` to inspect current page.").
- **Distribution:** `cargo install zendriver-mcp`, mdBook chapter `mcp.md` with Claude Desktop config snippet, README badge.
- **Testing:**
  - Unit per tool module via existing `MockConnection` from `zendriver-transport`.
  - `stdio_smoke.rs` / `http_smoke.rs` — full round-trip over real rmcp transport with mocked browser.
  - Real-Chrome integration tests behind `integration-tests` feature (mirrors lib pattern).
  - `insta` snapshot tests on tool JSON schemas (catch breaking-schema changes early).
  - 10-question evaluation XML per mcp-builder phase 4, stable read-only questions against real Chrome.

## Non-goals

- **TypeScript / npm wrapper.** Single Rust binary distribution. No `npx zendriver-mcp` story for v0.
- **Stream-based interception escape hatch** (raw `Fetch.*` event stream from `zendriver-interception`). Too complex to model over MCP wire (would need server-side event buffering + backpressure). Library-only; users who need it consume the lib directly.
- **Code-execution / JS sandbox tool** (some MCP clients prefer code-exec composing primitives). Skipped — selector-based primitives are sufficient and a Rust binary can't expose a safe JS sandbox cheaply.
- **Persistent cross-session state in HTTP mode.** Each session gets its own `Browser`. No shared cookie jar, no shared cache, no resumable sessions across reconnects.
- **Auth / authorization layer for HTTP transport.** Bind localhost by default; security is the operator's job (reverse proxy, mTLS, network policy). Document this clearly.
- **Workflow / recipe tools** (e.g. "log into Google", "scrape product listings"). Comprehensive primitive coverage only; agents compose.
- **Element ref / handle model.** No persistent IDs returned. Selectors are the only handle.
- **Web UI / inspector for HTTP mode.** rmcp's stock JSON-RPC suffices for v0.
- **MCP resources / prompts.** v0 ships tools only. (May add curated resources — e.g. "current snapshot" as a Resource — in a follow-up.)
- **Auto-update of bundled Chrome.** `browser_install_chrome` is on-demand, not background.
- **Cross-publish to other registries** (only crates.io).

## Architecture

### File layout

```
crates/zendriver-mcp/
├── Cargo.toml
├── README.md                       # short — links to mdBook chapter
├── src/
│   ├── main.rs                     # CLI parse, transport selection, server bootstrap
│   ├── lib.rs                      # Re-exports for tests
│   ├── server.rs                   # rmcp Server setup + tool registration
│   ├── state.rs                    # SessionState: Browser, current_tab, expect/intercept registries
│   ├── transport/
│   │   ├── mod.rs
│   │   ├── stdio.rs                # rmcp stdio transport bootstrap
│   │   └── http.rs                 # rmcp streamable HTTP transport bootstrap
│   ├── selectors.rs                # Selector arg struct → zendriver FindBuilder builder closure
│   ├── errors.rs                   # ZendriverError → MCP error + actionable _metadata.suggested_next
│   ├── snapshot/
│   │   ├── mod.rs
│   │   ├── acc_tree.rs             # Accessibility tree builder (no refs)
│   │   └── html_trim.rs            # Trimmed-HTML builder
│   └── tools/
│       ├── mod.rs                  # Registration + cfg gating
│       ├── lifecycle.rs            # browser_open, browser_close, browser_status
│       ├── navigation.rs           # browser_goto, _back, _forward, _reload, _wait_for_idle
│       ├── tabs.rs                 # browser_tab_list, _new, _switch, _close, _activate
│       ├── snapshot.rs             # browser_snapshot, _html, _screenshot
│       ├── find.rs                 # browser_find, _find_all
│       ├── actions.rs              # browser_click, _hover, _type, _press, _set_value, _clear,
│       │                           #   _focus, _scroll_into_view, _upload
│       ├── reads.rs                # browser_element_state
│       ├── eval.rs                 # browser_evaluate, _evaluate_main
│       ├── cookies.rs              # browser_cookies_get, _set, _delete, _clear, _persist
│       ├── storage.rs              # browser_storage_get, _set, _delete, _clear
│       ├── frames.rs               # browser_frame_list
│       ├── stealth.rs              # browser_set_stealth_profile
│       ├── intercept.rs            # cfg(feature = "interception")
│       ├── expect.rs               # cfg(feature = "expect")
│       ├── cloudflare.rs           # cfg(feature = "cloudflare")
│       └── fetcher.rs              # cfg(feature = "fetcher")
└── tests/
    ├── stdio_smoke.rs              # End-to-end round-trip over real rmcp stdio
    ├── http_smoke.rs               # End-to-end round-trip over real rmcp HTTP
    ├── tools/
    │   ├── lifecycle.rs
    │   ├── navigation.rs
    │   ├── ... (one per tool module)
    │   └── eval.rs
    └── evaluations/
        ├── eval_set.xml            # 10-question evaluation set
        └── eval_runner.rs          # Optional: run evals against real Chrome
```

### `Cargo.toml`

```toml
[package]
name = "zendriver-mcp"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
description = "MCP server exposing zendriver-rs's stealth browser automation to MCP clients."
keywords = ["mcp", "browser", "automation", "cdp", "stealth"]
categories = ["command-line-utilities", "web-programming::http-server"]

[[bin]]
name = "zendriver-mcp"
path = "src/main.rs"

[lib]
path = "src/lib.rs"

[features]
default = ["stealth", "interception", "expect", "cloudflare", "fetcher"]
stealth      = ["zendriver/stealth"]
interception = ["zendriver/interception"]
expect       = ["zendriver/expect"]
cloudflare   = ["zendriver/cloudflare"]
fetcher      = ["zendriver/fetcher"]
integration-tests = []

[dependencies]
zendriver  = { path = "../zendriver", version = "0.1.0", default-features = false }
rmcp       = { version = "1.7", features = ["server", "transport-io", "transport-streamable-http-server", "macros"] }
schemars   = { version = "0.8" }
tokio      = { workspace = true }
tracing    = { workspace = true }
tracing-subscriber = { workspace = true }
serde      = { workspace = true }
serde_json = { workspace = true }
thiserror  = { workspace = true }
clap       = { version = "4", features = ["derive"] }
url        = { workspace = true }
async-trait = { workspace = true }

[dev-dependencies]
insta             = { workspace = true }
serial_test       = { workspace = true }
tokio-test        = { workspace = true }
wiremock          = { workspace = true }
```

### Components

#### `SessionState`

Holds one MCP session's mutable state. Lives behind `Arc<Mutex<_>>` (or per-tool fine-grained locking) in the rmcp handler context.

```rust
pub struct SessionState {
    pub browser: Option<Browser>,
    pub current_tab_id: Option<String>,           // zendriver target_id; None when no tab
    pub stealth_profile: StealthProfile,
    #[cfg(feature = "expect")]
    pub expectations: HashMap<ExpectationId, ExpectationHandle>,
    #[cfg(feature = "interception")]
    pub rules: HashMap<RuleId, InterceptRuleHandle>,
}
```

Tab IDs are strings on the wire (zendriver `target_id`) — `current_tab_id` keeps wire and state aligned.

- **stdio mode:** one global `Arc<Mutex<SessionState>>` shared by all handlers.
- **HTTP mode:** rmcp's per-session context holds the `Arc<Mutex<SessionState>>`; dropped when the session terminates.

#### `Selector`

Shared arg struct on every find / action tool. Maps to a zendriver `FindBuilder` configuration closure.

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    // One-of (validated server-side, returns clear error if 0 or 2+):
    pub css:        Option<String>,
    pub xpath:      Option<String>,
    pub text:       Option<String>,        // substring text match
    pub text_exact: Option<String>,
    pub text_regex: Option<String>,
    pub role:       Option<AriaRole>,
    pub role_named: Option<RoleNamed>,     // {role, name}

    // Modifiers (all optional, sensible defaults):
    #[serde(default)]
    pub nth:          Option<usize>,
    #[serde(default = "default_true")]
    pub visible_only: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms:   u64,                 // 5000
    #[serde(default)]
    pub frame_id:     Option<String>,      // None = current tab's main frame
}
```

Validation runs in a helper `selector.apply(tab) -> FindBuilder` that returns a clear error if exactly-one-of-selector-kind isn't satisfied.

#### Tool registration pattern

Each tool module exposes `pub fn register(server: &mut Server, state: Arc<Mutex<SessionState>>)`. `server.rs` calls each module's `register`. Per-tool macro from `rmcp-macros` (or hand-rolled `register_tool` helper) handles JSON-schema generation via `schemars::JsonSchema` derive on the input + output structs.

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClickInput {
    #[serde(flatten)]
    pub selector: Selector,
    #[serde(default)]
    pub button: Option<MouseButton>,
    #[serde(default)]
    pub modifiers: Option<KeyModifiers>,
    #[serde(default = "default_click_count")]
    pub click_count: u32,
    #[serde(default)]
    pub return_snapshot: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ClickOutput {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}
```

#### Error mapping

```rust
pub fn map_error(err: ZendriverError) -> McpError {
    match err {
        ZendriverError::NoTab => McpError::invalid_request(
            "No tab open. Call `browser_open` first.",
            json!({ "suggested_next": "browser_open" }),
        ),
        ZendriverError::ElementNotFound { selector } => McpError::invalid_request(
            format!("No element matched `{selector}`."),
            json!({ "suggested_next": "browser_snapshot", "selector": selector }),
        ),
        ZendriverError::Timeout { op, ms } => McpError::invalid_request(
            format!("`{op}` timed out after {ms}ms. Retry with larger `timeout_ms` or inspect with `browser_snapshot`."),
            json!({ "suggested_next": "browser_snapshot", "timeout_ms": ms }),
        ),
        // ...
    }
}
```

### Data flow (single tool call)

1. Client sends MCP tool-call request over stdio / HTTP.
2. `rmcp` parses transport frame; routes to registered tool by name.
3. `schemars`-generated schema deserializes input.
4. Tool handler acquires `Arc<Mutex<SessionState>>` lock.
5. Handler validates selector (if applicable) and calls zendriver API.
6. Result wrapped into `ToolResult`:
   - Text content with primary structured result (JSON-stringified output struct).
   - Optional `image` content block for screenshots.
   - Optional `structuredContent` field per modern rmcp SDK.
7. Errors → `map_error()` → MCP error response.
8. State lock released.

## Tool reference

> The full table below describes every tool in v0. Numbers in parens are tool counts per category.

### Lifecycle (3)

| Tool | Args | Output |
|------|------|--------|
| `browser_open` | `headless: bool = true`, `stealth_profile: StealthProfileChoice = "auto"` | `{ chrome_version, headless, profile }` |
| `browser_close` | — | `{ ok }` |
| `browser_status` | — | `{ open, tab_count, current_tab: { id, url, title } \| null, headless, profile }` |

### Navigation (5)

| Tool | Args | Output |
|------|------|--------|
| `browser_goto` | `url`, `wait_for: WaitFor = "load"` (`load \| idle \| none`), `return_snapshot: bool = false` | `{ url, title, snapshot? }` |
| `browser_back` | `return_snapshot: bool = false` | `{ url, snapshot? }` |
| `browser_forward` | `return_snapshot: bool = false` | `{ url, snapshot? }` |
| `browser_reload` | `return_snapshot: bool = false` | `{ url, snapshot? }` |
| `browser_wait_for_idle` | `timeout_ms: u64 = 5000` | `{ idle, in_flight }` |

### Tabs (5)

| Tool | Args | Output |
|------|------|--------|
| `browser_tab_list` | — | `[{ id, url, title, is_current }]` |
| `browser_tab_new` | `url: Option<String>`, `activate: bool = true` | `{ id, url, title }` |
| `browser_tab_switch` | `tab_id` | `{ id, url, title }` |
| `browser_tab_close` | `tab_id: Option<String>` (default current) | `{ closed_id, current_tab_id: Option<String> }` |
| `browser_tab_activate` | `tab_id` | `{ id }` |

### Snapshots (3)

| Tool | Args | Output |
|------|------|--------|
| `browser_snapshot` | `frame_id: Option<String>` | text content: trimmed accessibility tree |
| `browser_html` | `selector: Option<Selector>`, `trim: bool = true`, `frame_id: Option<String>` | text content: HTML string |
| `browser_screenshot` | `format: ImgFormat = "png"`, `full_page: bool = false`, `selector: Option<Selector>`, `omit_background: bool = false`, `save_path: Option<String>` | image content (inline) + `{ saved_path? }` |

### Find + actions (11)

All take a flattened `Selector` arg, all state-changers also take `return_snapshot: bool = false`.

| Tool | Extra args | Output |
|------|------------|--------|
| `browser_find` | — | `{ found, element?: ElementDescriptor }` |
| `browser_find_all` | `limit: usize = 50` | `[ElementDescriptor]` |
| `browser_click` | `button: MouseButton = "left"`, `modifiers: KeyModifiers = {}`, `click_count: u32 = 1` | `{ ok, snapshot? }` |
| `browser_hover` | — | `{ ok, snapshot? }` |
| `browser_type` | `text: String`, `clear_first: bool = false` | `{ ok, snapshot? }` |
| `browser_press` | `key: String` (e.g. `"Enter"`, `"Tab"`) | `{ ok, snapshot? }` |
| `browser_set_value` | `value: String` | `{ ok, snapshot? }` |
| `browser_clear` | — | `{ ok, snapshot? }` |
| `browser_focus` | — | `{ ok }` |
| `browser_scroll_into_view` | — | `{ ok }` |
| `browser_upload` | `paths: Vec<String>` | `{ ok }` |

`ElementDescriptor`:

```rust
pub struct ElementDescriptor {
    pub tag: String,
    pub text_snippet: String,        // first 200 chars
    pub attrs: BTreeMap<String, String>,
    pub bounding_box: Option<BoundingBox>,
    pub visible: bool,
    pub enabled: bool,
}
```

### Reads (1)

| Tool | Args | Output |
|------|------|--------|
| `browser_element_state` | `Selector`, `include: ReadFieldsPreset = "all"` | `{ exists, visible, enabled, in_viewport, bounding_box?, text?, attrs?, inner_html? }` |

`ReadFieldsPreset` is an enum of fixed bundles — pick one: `all` / `exists_only` / `visible_enabled` / `geometry` / `text_attrs`. Single-pick keeps the schema simple; bitflag combination isn't worth the extra arg-shape complexity for an LLM consumer.

### Eval (2)

| Tool | Args | Output |
|------|------|--------|
| `browser_evaluate` | `expression: String`, `await_promise: bool = true`, `frame_id: Option<String>` | `{ value: serde_json::Value }` |
| `browser_evaluate_main` | same as above | same; tool description warns: "Runs in the page's main world — breaks stealth isolation if used carelessly." |

### Cookies (5)

| Tool | Args | Output |
|------|------|--------|
| `browser_cookies_get` | `url: Option<String>`, `name: Option<String>` | `[Cookie]` |
| `browser_cookies_set` | `cookies: Vec<CookieParams>` | `{ added }` |
| `browser_cookies_delete` | `name`, `url: Option<String>` | `{ deleted }` |
| `browser_cookies_clear` | — | `{ ok }` |
| `browser_cookies_persist` | `direction: "save" \| "load"`, `path: String` | `{ count }` |

### Storage (4)

| Tool | Args | Output |
|------|------|--------|
| `browser_storage_get` | `kind: "local" \| "session"`, `key: Option<String>` | `{ values: BTreeMap<String, String> }` |
| `browser_storage_set` | `kind`, `key`, `value` | `{ ok }` |
| `browser_storage_delete` | `kind`, `key` | `{ deleted }` |
| `browser_storage_clear` | `kind` | `{ ok }` |

### Frames (1)

| Tool | Args | Output |
|------|------|--------|
| `browser_frame_list` | — | `[{ id, url, parent_id, is_oopif }]` |

All other frame-scoped ops (find / eval / snapshot / html) take an optional `frame_id` arg.

### Stealth (1)

| Tool | Args | Output |
|------|------|--------|
| `browser_set_stealth_profile` | `profile: StealthProfileChoice` (`auto` / `native` / `spoof_macos` / `spoof_linux` / `spoof_windows`) | `{ active_profile }` |

Some profile changes require a fresh tab; tool description documents the constraint.

### Gated: interception (4) — `cfg(feature = "interception")`

| Tool | Args | Output |
|------|------|--------|
| `browser_intercept_add_rule` | `pattern: String` (URL pattern), `action: InterceptAction` (`block` / `redirect{to}` / `respond{status,body,headers}` / `modify_request{...}`) | `{ rule_id }` |
| `browser_intercept_remove_rule` | `rule_id` | `{ removed }` |
| `browser_intercept_list_rules` | — | `[{ rule_id, pattern, action_kind }]` |
| `browser_intercept_clear_rules` | — | `{ cleared }` |

### Gated: expect (3) — `cfg(feature = "expect")`

Register / await / cancel pattern (MCP request/response is synchronous, so the lib's `expect_*().await` guard pattern is split into three calls).

| Tool | Args | Output |
|------|------|--------|
| `browser_expect_register` | `kind: "request" \| "response" \| "dialog" \| "download"`, `matcher: ExpectMatcher` (URL pattern, status range, etc.) | `{ expectation_id }` |
| `browser_expect_await` | `expectation_id`, `timeout_ms: u64 = 30000` | `{ event: ExpectEvent }`, auto-disposes |
| `browser_expect_cancel` | `expectation_id` | `{ cancelled }` |

Server buffers events from registration onward, so `register` → `do action` → `await` doesn't race.

### Gated: cloudflare (1) — `cfg(feature = "cloudflare")`

| Tool | Args | Output |
|------|------|--------|
| `browser_solve_turnstile` | `timeout_ms: u64 = 30000` | `{ outcome: "solved" \| "challenge_gone" \| "timeout" }` |

### Gated: fetcher (2) — `cfg(feature = "fetcher")`

| Tool | Args | Output |
|------|------|--------|
| `browser_install_chrome` | `version: Option<String>` (default latest stable), `channel: Option<Channel>` | `{ path, version }` |
| `browser_list_installed_chromes` | — | `[{ path, version, channel }]` |

### Total

- Core: 3 + 5 + 5 + 3 + 11 + 1 + 2 + 5 + 4 + 1 + 1 = **41**
- Gated full: + 4 + 3 + 1 + 2 = **10 more → 51 total**

## CLI

```
zendriver-mcp [OPTIONS]

OPTIONS:
    --http <ADDR>             Run streamable HTTP transport on ADDR (e.g. 127.0.0.1:8765)
                              Default: stdio
    --stealth-profile <KIND>  Default stealth profile [auto|native|spoof_macos|spoof_linux|spoof_windows]
                              Default: auto
    --headless <BOOL>         Default headless [true|false]
                              Default: true
    --chrome <PATH>           Custom Chrome binary
    --log <LEVEL>             Tracing log level [trace|debug|info|warn|error]
                              Default: info
    -h, --help
    -V, --version
```

CLI defaults seed `SessionState`'s initial values; `browser_open` args still override per-call.

## Error handling

`ZendriverError` variants mapped to MCP errors with `_metadata.suggested_next`:

| Variant | Message | Suggested next |
|---------|---------|----------------|
| `NoTab` | "No tab open. Call `browser_open` first." | `browser_open` |
| `ElementNotFound { selector }` | "No element matched `{selector}`." | `browser_snapshot` |
| `Timeout { op, ms }` | "`{op}` timed out after {ms}ms. Retry with larger `timeout_ms` or inspect with `browser_snapshot`." | `browser_snapshot` |
| `Cdp(_)` | Wrap CDP message + hint about Chrome state. | — |
| `BrowserNotOpen` | "Browser not open. Call `browser_open`." | `browser_open` |
| `FrameNotFound { id }` | "No frame with id `{id}`." | `browser_frame_list` |
| `ExpectationNotFound { id }` | "No expectation with id `{id}` (already awaited / cancelled?)." | `browser_expect_register` |
| `RuleNotFound { id }` | "No interception rule with id `{id}`." | `browser_intercept_list_rules` |
| `FeatureDisabled { feature }` | "Tool requires cargo feature `{feature}`. Reinstall with `cargo install zendriver-mcp --features {feature}`." | — |

## Testing

- **Unit tests per tool module.** Existing `MockConnection` from `zendriver-transport` lets us drive zendriver without a real Chrome. Tools that map 1:1 to lib calls get one-success-one-error test each.
- **Schema snapshots.** `insta` snapshots of every tool's input + output JSON schema (`tests/schema_snapshots.rs`). Catches accidental breaking changes; reviewed manually on intentional breaks.
- **`stdio_smoke.rs`** — spawn the server binary, talk to it over stdio with rmcp's client SDK, run `browser_open` + `browser_status` + `browser_close` against a mock connection. Validates full transport wiring.
- **`http_smoke.rs`** — same, over streamable HTTP, with the rmcp client SDK + a bound localhost port.
- **`integration-tests` feature** — opt-in suite that runs against real Chrome (mirrors zendriver's pattern). Run via `cargo test --features integration-tests`. Includes a "100-step compound flow" test exercising open/goto/find/click/screenshot/cookies/close.
- **Evaluation set** — `tests/evaluations/eval_set.xml` with 10 read-only stable-answer questions:
  1. "Page title of example.com" → "Example Domain"
  2. "Count of `<h2>` on https://en.wikipedia.org/wiki/Rust_(programming_language)" → exact count
  3. "ARIA role of the first form input on https://httpbin.org/forms/post" → "textbox"
  4. "All links on example.com's main page (count)" → exact count
  5. "Computed `lang` attribute of `<html>` on developer.mozilla.org" → "en-US"
  6. "First H1 text on https://www.rust-lang.org/" → exact text
  7. "Cookie names set by https://httpbin.org/cookies/set/foo/bar after the call" → ["foo"]
  8. "localStorage key count after `browser_storage_set local k v`" → 1
  9. "After navigating example.com, `browser_back` returns blank/about:blank" → true
  10. "Screenshot of example.com is non-zero bytes" → true

  Eval runner is optional (behind a feature flag) — XML is the deliverable.

## Distribution & docs

- **Install:** `cargo install zendriver-mcp`.
- **mdBook chapter `mcp.md`** under `docs/book/src/mcp.md`. Contents:
  - Install + Claude Desktop config snippet.
  - Full tool reference (rendered from this spec).
  - HTTP mode operator notes (bind localhost, reverse-proxy if exposing).
  - Stealth profile guidance.
  - Troubleshooting (`tracing` log routing, common error mappings).
- **Claude Desktop snippet:**
  ```json
  {
    "mcpServers": {
      "zendriver": {
        "command": "zendriver-mcp"
      }
    }
  }
  ```
- **README badge:** "MCP server: zendriver-mcp" linking to mdBook chapter.

## Decisions (delegate-mode picks)

These were judgement calls I made on the user's behalf during brainstorming. Each is load-bearing; revisit before implementation if any feels wrong.

1. **Eval exposure**: both `browser_evaluate` (isolated) + `browser_evaluate_main` (main-world, with stealth-break warning in description). Power-user tool; LLM agents commonly need it for detection bypass and DOM-bound JS.
2. **Tool prefix `browser_*`** (not `zendriver_*`). Matches playwright-mcp / chrome-devtools-mcp conventions; familiar to LLM clients.
3. **Find / action tools take `Selector` directly** — no two-step `find` → store-ref → `act`. Saves a round-trip on every action.
4. **Crate placement `crates/zendriver-mcp/`** in workspace. Publishes alongside lib. Alternative (separate repo) loses workspace test/build infra.
5. **Stealth on by default** (matches `zendriver` crate).
6. **Auto-snapshot off by default**, opt-in via `return_snapshot: bool` arg on state-changers. Q7 trade-off chose configurable; default false saves tokens on the common case.
7. **Cookie + storage writes fully exposed** — auth-state persistence is the real value-add for these tools.
8. **Expect = 3-tool register / await / cancel pattern** — MCP request/response can't model the lib's "register-then-act-then-await" guard pattern in a single call.
9. **Interception Stream escape hatch NOT exposed** — would require server-side event buffering with backpressure; out of scope for v0.
10. **Screenshot returns inline image content** by default; `save_path` is an opt-in side effect.
11. **Snapshot acc tree strips ref numbers** — selector model (Q5) makes refs dead weight.
12. **HTTP session = one Browser per MCP session**, dropped on disconnect. No persistent cross-session state.
13. **`browser_element_state` collapses 5+ reads** into one tool with a `include: ReadFields` bitflag arg.
14. **`browser_cookies_persist` collapses save + load** into one tool with `direction: "save" | "load"`.
15. **rmcp v1.7+** as the SDK (official Anthropic Rust MCP). Latest stable as of 2026-05-13.
16. **Published binary defaults to ALL cargo features ON** (one-install convenience). This tweaks the Q6 answer slightly — features still exist (so lean local builds opt in), but the published `cargo install zendriver-mcp` does not require feature flags.
17. **Frame-scoped ops via `frame_id` arg on existing tools** — only `browser_frame_list` is a dedicated frame tool. Avoids duplicating every find/eval/snapshot per frame.
18. **No code-execution / dynamic JS sandbox tool** — selector-based primitives suffice; Rust binary can't expose a safe JS sandbox cheaply.
19. **Tool count ~51** (41 core + 10 gated). Consistent with playwright-mcp scale.
20. **10-question read-only stable-answer evaluation set** per mcp-builder phase 4.

## Implementation phases

(Detailed task breakdown lands in the implementation plan; this is a high-level sketch.)

1. **Bootstrap.** New crate, `Cargo.toml`, `main.rs` with rmcp stdio + HTTP transport wiring, `SessionState` skeleton, error mapping, `Selector` arg type + apply helper.
2. **Lifecycle + navigation + tabs (13 tools).** First end-to-end working slice.
3. **Snapshots (3 tools).** Acc-tree builder + HTML trimmer + screenshot wrapper.
4. **Find + actions + reads + eval (14 tools).** Bulk of core surface.
5. **Cookies + storage + frames + stealth (11 tools).** Round out core.
6. **Gated tools.** Per-feature: interception (4), expect (3), cloudflare (1), fetcher (2).
7. **Testing.** Schema snapshots, smoke tests, integration suite, evaluation XML.
8. **Docs + release.** mdBook chapter, README badge, publish.

Plan will land at `docs/superpowers/plans/2026-05-24-zendriver-rs-mcp-server.md`.
