# zendriver-mcp Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a new workspace crate `zendriver-mcp` exposing zendriver-rs through ~51 MCP tools over stdio + streamable HTTP, so any MCP client (Claude Desktop, claude-code, custom agents) can drive a stealth-by-default Chrome browser.

**Architecture:** New `crates/zendriver-mcp/` workspace member. Bin + thin lib. Tools registered against an rmcp `Server`. Single `Arc<Mutex<SessionState>>` per MCP session holds `Browser` + `current_tab_id` + expect/intercept registries. Selector-based handle model (no element refs). Gated cargo features mirror the `zendriver` crate.

**Tech Stack:** `rmcp` v1.7 (official Anthropic Rust MCP SDK), `schemars` 0.8 for JSON schema generation, `clap` 4 for CLI, `tokio` for async runtime, existing `zendriver-transport::MockConnection` for unit tests.

**Spec:** [docs/superpowers/specs/2026-05-24-zendriver-rs-mcp-server-design.md](../specs/2026-05-24-zendriver-rs-mcp-server-design.md)

---

## API Reality (supersedes task code where conflict)

Recon against live source (rmcp v1.7.0 at `/Users/rin/GitHub/rust-sdk`, zendriver at `crates/zendriver/src/`) found gaps between plan code blocks and actual APIs. **When task code conflicts with anything below, the truth below wins** — adapt the task code to match.

### rmcp v1.7.0 — server pattern is MACRO-based, not method-call

The plan's `server.tool(name, desc, handler)` pattern is wrong. Real rmcp uses `#[tool_router]` on an impl block, `#[tool]` on methods, and emits `#[tool_handler] impl ServerHandler` for you.

**Cargo features:** `["server", "macros", "schemars", "transport-io", "transport-streamable-http-server"]`. `schemars` **1.0+** (not 0.8 as plan said). `transport-io` is required for `rmcp::transport::stdio()` — earlier draft incorrectly said server pulled it in automatically.

**Zendriver feature note:** the `zendriver` crate has **no `stealth` cargo feature** — stealth is always-on in the lib. `zendriver-mcp`'s `Cargo.toml` should NOT declare `stealth = ["zendriver/stealth"]`. Default-features for the MCP binary: `["interception", "expect", "cloudflare", "fetcher"]`.

**rmcp client API:** use `CallToolRequestParams` (plural) — the singular `CallToolRequestParam` is a deprecated alias that warns under `-D warnings`. Construction: `CallToolRequestParams::new(name).with_arguments(json_map)` where `json_map` is `serde_json::Map<String, Value>`. Result's `content` is `Vec<Content>`, not `Option<Vec<Content>>`.

**`#[tool_router(server_handler)]` shortcut:** does NOT require a `tool_router` field on the struct (that pattern is for the explicit `#[tool_handler(router = self.tool_router)]` form). With the shortcut, the macro references a static `Self::tool_router()` from the auto-emitted `ServerHandler` impl — your struct only needs the application state fields.

**`Tab::wait_for_idle_with` arg order is `(timeout, quiet_window)`, NOT `(quiet_window, timeout)`** — earlier draft was reversed. Verified at `crates/zendriver/src/tab.rs:1157`.

**`Tab` has NO `content()` / `page_source()` method** — only `Frame::content()` (at `crates/zendriver/src/frame/mod.rs:346`). To get a tab's HTML: `tab.main_frame().await?.content().await?` OR `tab.evaluate_main::<String>("document.documentElement.outerHTML").await?`.

**`Platform` enum variants are `Win32 / MacIntel / LinuxX86_64`** (at `crates/zendriver-stealth/src/profile.rs:24-30`), NOT `Mac / Linux / Windows`. Stealth profile mapping should use these names.

**rmcp `CallToolResult.structured_content` is `Option<Value>`** (separate field from `content: Vec<Content>`). Clients reading structured output should prefer `result.structured_content` and fall back to parsing `content`.

**Tab is `Clone`** (Arc-backed at `crates/zendriver/src/tab.rs:50-51`), so `current_tab` helper can return owned `Tab` cheaply — no borrow lifetime issues.

**`AriaRole` lives at `zendriver::AriaRole`** (re-exported from `query::role`) with **21 explicit variants** + `Other(&'static str)` escape hatch: `Button, Link, Textbox, Combobox, Checkbox, Radio, Tab, Menu, Menuitem, Dialog, Heading, Banner, Navigation, Main, Article, List, Listitem, Row, Cell, Columnheader, Rowheader`. **No `FromStr` impl** — write a `parse_role(&str) -> Result<AriaRole, ErrorData>` mapping lowercase names; reject unknowns with a list-bearing error.

**`zendriver::BoundingBox` lacks `Serialize` / `JsonSchema` derives** — MCP layer needs its own wire-shape struct + `From<zendriver::BoundingBox>` impl. Same for any other zendriver type that flows over the wire (screenshot clip rects, etc.).

**`Tab::find() / find_all()` borrow `&self`** — returned `FindBuilder<'a>` / `FindAllBuilder<'a>` is lifetime-tied. Construct builder + apply selectors + await terminal within one `tab` borrow. `in_frame(&Frame)` likewise wants a stable borrow — look up Frame into owned handle first, then pass `&f`.

**`FindBuilder::visible_only(bool)` is currently a NO-OP in zendriver lib** (see `crates/zendriver/src/query/mod.rs:502` comment "TODO(T16) — depends on actionability::check_visible"). MCP layer still passes the bool through so the feature lights up automatically when the lib lands the implementation. `Element::is_visible() / is_enabled()` work today via a separate `actionability::check_*` code path — so `browser_element_state.include = visible_enabled` returns real data.

**`ZendriverError::FrameNotFound` is a TUPLE variant `FrameNotFound(String)`**, NOT struct `FrameNotFound { id: String }`. Construct with `ZendriverError::FrameNotFound(id.to_string())`.

**`Element::evaluate(js)` wraps the expression** as `function(el) { return (<js>); }` internally. Pass an expression like `"el.tagName.toLowerCase()"`, NOT an arrow `"el => el.tagName.toLowerCase()"` — the latter yields a JS Function object instead of the value.

**`Element::tag_name` workaround:** `el.evaluate::<String>("el.tagName.toLowerCase()").await.ok()`. Use `.ok()` to make tag discovery best-effort (so describe doesn't fail wholesale if eval rejects).

**`Tab::new_for_test` is `pub(crate)` + `#[cfg(test)]`** — not reachable from `zendriver-mcp` tests. Either expose a test factory in the lib or rely on the `integration-tests`-gated end-to-end shape (slower but real).

**Canonical pattern (use as template for all tool modules):**

```rust
use rmcp::{handler::server::wrapper::Parameters, tool_router, schemars, ServiceExt, transport::stdio, ErrorData};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct OpenInput { headless: bool }

#[derive(Clone)]
pub struct ZendriverServer {
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::state::SessionState>>,
}

#[tool_router(server_handler)]
impl ZendriverServer {
    #[tool(description = "Launch Chrome with stealth defaults.")]
    async fn browser_open(&self, Parameters(input): Parameters<OpenInput>) -> Result<String, ErrorData> {
        let mut s = self.state.lock().await;
        // ... call zendriver, populate s.browser ...
        Ok("opened".into())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let handler = ZendriverServer { state: std::sync::Arc::new(tokio::sync::Mutex::new(crate::state::SessionState::new())) };
    let service = handler.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
```

**Implications:**
- Drop the plan's per-module `pub fn register(server, state)` pattern. Instead, every tool is a `#[tool]` method on `ZendriverServer` (one big struct).
- Tools that mutate state lock `self.state` (already an `Arc<Mutex<_>>`).
- For 51 tools in one impl block: it works (rmcp's calculator example has many) but split via `impl` blocks per category if it grows unwieldy. **Multiple `#[tool_router]` impls on the same struct are NOT supported** — keep tools in one impl OR use sub-modules each with their own state-wrapper struct (overkill for v0).
- Recommended: keep tools as standalone async fns in `tools/*.rs` for organization, and have the canonical `#[tool_router]` impl block in `server.rs` call into them. The impl block delegates:
  ```rust
  #[tool_router(server_handler)]
  impl ZendriverServer {
      #[tool(description = "Launch Chrome with stealth defaults.")]
      async fn browser_open(&self, p: Parameters<lifecycle::OpenInput>) -> Result<Json<lifecycle::OpenOutput>, ErrorData> {
          crate::tools::lifecycle::open(self.state.clone(), p.0).await.map(Json)
      }
      // ... 50 more delegators ...
  }
  ```

**Structured output:** wrap returns in `rmcp::Json<T>` (T: Serialize + JsonSchema). Plain strings auto-wrap as text content. For images use `Content::image(base64_data, "image/png")`.

**Error type:** `rmcp::ErrorData` (re-exported from `rmcp::model::ErrorData`). Constructors `ErrorData::invalid_request(msg, Some(json))`, `invalid_params`, `internal_error`. The `data: Option<Value>` slot carries our `_meta.suggested_next` payload.

**stdio:** `handler.serve(rmcp::transport::stdio()).await?.waiting().await?`. `stdio()` returns `(Stdin, Stdout)`.

**HTTP server:** `rmcp::transport::streamable_http_server::StreamableHttpService::new(factory, LocalSessionManager::default().into(), StreamableHttpServerConfig::default())`. Factory is `Fn() -> Result<H, _>` — called once per session, returns a fresh `ZendriverServer { state: fresh Arc<Mutex<SessionState>> }`. Mount under axum: `axum::Router::new().nest_service("/mcp", service)`.

**Server identity / capabilities:** override `get_info()` in a separate `impl ServerHandler` (or rely on `#[tool_handler]` defaults that auto-pick name+version from `CARGO_PKG_*` env). Capabilities are auto-emitted by `#[tool_handler]` for tools.

**Client (for tests):** `().serve(TokioChildProcess::new(Command::new(bin)))` over stdio; `().serve(StreamableHttpClientTransport::new(uri)?)` over HTTP. Then `client.peer().list_tools(Default::default()).await` and `client.peer().call_tool(CallToolRequestParams::new(name).with_arguments(json))`.

### zendriver API corrections

| Plan said | Reality |
|---|---|
| `Browser::version() / Browser::headless()` accessors | **do not exist** — drop from `browser_status` output |
| `b.tabs()` sync | `b.tabs(&self).await -> Vec<Tab>` async |
| `b.new_tab(url)` | `b.new_tab()` no-arg; `b.new_tab_at(url)` |
| `Tab::url() -> String` sync | `tab.url(&self).await -> Result<url::Url>` async; `.to_string()` to coerce |
| `Tab::title() -> String` sync | `tab.title(&self).await -> Result<String>` async |
| `Tab::frames() -> Vec<Frame>` sync | `tab.frames(&self).await -> Result<Vec<Frame>>` async |
| `Tab::frame(id)` lookup-by-id | only `frame_by_url(substr)` and `frame_by_name(name)`; loop `tab.frames().await` + match `f.id() == id` for ID lookup |
| `Tab::accessibility_tree()` | **does not exist** — use `tab.evaluate_main::<serde_json::Value>("...")` against the `chrome.runtime` AX bridge OR call raw CDP `Accessibility.getFullAXTree` via `tab.session()`; simplest for v0 is to derive a tree from `document.body.innerHTML` + a JS walker, then format |
| `Tab::wait_for_idle(Duration)` | `wait_for_idle()` uses defaults; `wait_for_idle_with(idle_time, timeout)` for custom |
| `Element::text()` | `element.inner_text(&self).await -> Result<String>` |
| `Element::tag_name()` | **does not exist** — derive via `element.attr("nodeName").await` or eval `el => el.tagName` |
| `Storage::all()` | `storage.get_all(&self).await -> Result<HashMap<String, String>>` |
| `Storage::delete(key)` | `storage.remove(&self, key).await -> Result<()>` |
| `CookieJar::set_all(&[CookieParams])` | `cookies.set_many(Vec<Cookie>).await -> Result<()>`; `set(Cookie)` for single |
| `CookieJar::for_url(url)` takes `&str` | takes `&url::Url` — parse with `url::Url::parse(s)?` |
| `CookieJar::delete(name, url)` | `cookies.delete(&self, name, domain: Option<&str>, path: Option<&str>).await` |
| `CookieJar::save_to_file / load_from_file` | **not in lib** — implement at MCP layer: `cookies.all().await?` → `serde_json::to_writer`; load = `serde_json::from_reader` → `cookies.set_many(...)` |
| `CookieParams` separate from `Cookie` | one `Cookie` struct — fields `name, value, domain, path, expires, http_only, secure, same_site, url` |
| `ZendriverError::NoTab / BrowserNotOpen` | **don't exist** — actual variants: `Browser, Transport, Cdp, ElementNotFound, Timeout, Navigation, JsException, ElementStale, NotRefreshable, NotActionable, FrameNotFound, TabNotFound, Cookie, Storage, HistoryNavigation, Serde, Io, Stealth, Interception(feat), Cloudflare(feat), Fetcher(feat)`. Use `TabNotFound` for missing tab; add a custom **MCP-layer** error enum for "browser not open" + "expectation not found" + "feature disabled". Don't try to extend `ZendriverError`. |
| `FindBuilder::role(impl Into<String>)` | takes typed `zendriver::AriaRole` enum. Selector arg's `role: String` must map to `AriaRole` via `AriaRole::from_str`. |
| `FindBuilder::role_named(role: String, name)` | `(AriaRole, name)`. Same enum mapping. |
| `FindBuilder::text_regex(String)` | takes parsed `regex::Regex`. Compile from string at handler entry: `regex::Regex::new(&pattern).map_err(...)` |
| `FindBuilder::visible_only()` unary | `visible_only(bool)` — pass `sel.visible_only` directly |
| `FindBuilder::one_or_none()` | exists — use this instead of catching ElementNotFound to return `found: false` |
| `FindAllBuilder::many_or_empty()` | exists — for `browser_find_all` (avoids ElementNotFound on empty result) |
| `Element::press(impl Into<String>)` | takes typed `zendriver::Key` enum. Map `key: String` ("Enter", "Tab", etc.) via `Key::from_str` or match. |
| `Element::click_with(ClickOptions)` | `ClickOptions { button: MouseButton, click_count: u32, ... }` — confirm exact fields in `crates/zendriver/src/element/actions.rs` |
| `Element::upload_files(&[paths])` | takes `&[impl AsRef<Path>]` — `&[&str]` works |
| `ScreenshotBuilder::capture()` / `clip_element(&el)` | `.bytes() -> Result<Vec<u8>>`; clipping is `.clip(BoundingBox)` — fetch el's bbox first via `element.bounding_box().await?` |
| `Tab::screenshot()` builder via method | use `tab.screenshot_builder()` for the builder; `tab.screenshot()` is a convenience returning bytes directly with defaults |
| `Frame::frame_id() / is_oopif()` | `frame.id() -> &str`; **no `is_oopif`** — drop from `FrameSummary`. Use `frame.is_main()` if needed. `frame.url()` returns plain `String`, not `Result`. |
| `Tab::expect_*` register/await pattern | already in lib: `tab.expect_request(matcher).timeout(d).matched().await -> Result<MatchedRequest>` (one-call await). For MCP we still need the 3-tool split (register → await later) but the lib's matcher type is `impl Into<UrlMatcher>`. Inside `browser_expect_register`, spawn a tokio task holding the `RequestExpectation::matched()` future and pipe result through a `tokio::sync::oneshot` channel keyed by expectation_id. `browser_expect_await` consumes the receiver. |
| Cloudflare API | `tab.cloudflare() -> CloudflareBypass<'_>`, then `.wait_for_clearance(timeout).await -> Result<ClearanceOutcome, CloudflareError>`. **`ClearanceOutcome` variants: `TokenAcquired(String)`, `ChallengeGone`** — no `Solved`, no `Timeout` (timeout is `Err(CloudflareError)`). Map MCP `outcome` enum accordingly: timeout-error → `Outcome::Timeout`, `TokenAcquired(_)` → `Outcome::Solved`, `ChallengeGone` → `Outcome::ChallengeGone`. |
| Interception API | `tab.intercept() -> InterceptBuilder` with chained `.pattern("...").at_request().block()? / .redirect(pat, repl) / .respond(pat, ResponseInfo) / .modify_request(pat, f)` then `.start() -> InterceptHandle`. **There's no per-rule remove** — handle drops to stop interception. MCP layer must wrap: store the `InterceptHandle` keyed by a generated `rule_id` UUID; `browser_intercept_remove_rule` drops the handle from the map. **One handle per "rule" simplification** — if user wants multiple rules they get multiple handles (each `tab.intercept()...start()`). |
| Fetcher API | `Fetcher::new()` not `::builder()`. Methods: `.cache_dir(p) / .version(VersionSpec) / .platform(p) / .on_progress(cb) / .manifest_url(url) / .expected_sha256(sha) / .ensure_chrome().await -> Result<PathBuf>`. **`ensure_chrome` returns `PathBuf` only, no version metadata** — derive version by inspecting the path or by storing `VersionSpec` separately. **No `cache::list` for listing installed Chromes** — drop `browser_list_installed_chromes` or implement by reading the cache dir directly. |
| `StealthProfile` enum `Auto/Native/Spoofed` | actually `StealthProfile::off() / native() / spoofed()` constructors with builder modifiers (`.fingerprint() / .memory_gb() / .cpu_count() / .chrome_version() / .platform() / .locale() / .timezone() / .user_agent() / .bypass_csp() / .arg() / .args()`). For MCP, the `stealth_profile_choice` enum stays as `Auto/Native/SpoofMacos/SpoofLinux/SpoofWindows` at the wire level, but the handler maps to `StealthProfile`: `Auto → StealthProfile::native()` (sysinfo auto-detect happens inside `native()`), `SpoofMacos → StealthProfile::spoofed().platform(Platform::Mac)`, etc. |

### Net plan-level deltas

1. **Server architecture:** one `ZendriverServer` struct in `server.rs` carrying `Arc<Mutex<SessionState>>`. One `#[tool_router(server_handler)] impl ZendriverServer { ... }` block delegating to per-module async fns in `tools/*.rs`. The `tools/` module structure stays; the `register()` fn pattern goes.
2. **Custom error layer:** add `errors::McpServerError` (or just have `map_error` return `ErrorData` and map MCP-specific cases inline before reaching `ZendriverError`).
3. **`Tab::accessibility_tree()` is fake** — for `browser_snapshot`, derive AX-equivalent via `tab.evaluate_main::<serde_json::Value>("(() => buildAxTree(document.body))()")` with a JS walker bundled into the snapshot module, OR drop the AX-tree story for v0 and ship only `browser_html` + `browser_screenshot`. **Recommendation:** drop AX tree for v0; ship `browser_html` (trimmed) as the "see structure" tool. Smaller surface, real backing.
4. **Cookie persist:** implement at MCP layer (`serde_json` round-trip of `Vec<Cookie>` to disk).
5. **Storage method names:** `get_all`/`remove`, not `all`/`delete`.
6. **Frame model:** drop `is_oopif` from `FrameSummary`. Use `is_main` + `parent_id` instead.
7. **Selectors role:** map MCP `role: String` to `zendriver::AriaRole` enum (and surface a clean error if unknown role string).
8. **Selectors regex:** parse `text_regex: String` to `regex::Regex` in `resolve()`.
9. **Key enum:** map MCP `key: String` for `browser_press` to `zendriver::Key` enum.
10. **Url type:** `Tab::url()` returns `url::Url` — `.as_str().to_string()` to get a `String` for output.
11. **Async tabs():** `Browser::tabs()` is async — handlers needing the tab list must `.await`.
12. **Element discovery:** no `Element::tag_name()`. For `ElementDescriptor.tag`, do `let tag: String = el.evaluate("el => el.tagName.toLowerCase()").await?;` or skip the `tag` field for v0 (use attrs instead).
13. **Interception simplification:** v0 supports one-rule-per-handle. No multi-rule per InterceptBuilder. `browser_intercept_add_rule` creates one handle per call. `_clear_rules` drops all handles in `s.rules`.
14. **Fetcher list tool:** drop `browser_list_installed_chromes` (no lib support); add later if useful. Drops total tool count from 51 → 50.

### Updated total tool count

50 (was 51) after dropping `browser_list_installed_chromes`. Also reconsider `browser_snapshot` (drop AX tree → -1 = 49) but keep `browser_html` + `browser_screenshot` for "see the page" — net 49 tools for v0.

---

## File structure

See spec section "File layout" for the full tree. New paths:

- `crates/zendriver-mcp/{Cargo.toml, README.md}`
- `crates/zendriver-mcp/src/{main.rs, lib.rs, server.rs, state.rs, selectors.rs, errors.rs}`
- `crates/zendriver-mcp/src/transport/{mod.rs, stdio.rs, http.rs}`
- `crates/zendriver-mcp/src/snapshot/{mod.rs, acc_tree.rs, html_trim.rs}`
- `crates/zendriver-mcp/src/tools/{mod.rs, lifecycle.rs, navigation.rs, tabs.rs, snapshot.rs, find.rs, actions.rs, reads.rs, eval.rs, cookies.rs, storage.rs, frames.rs, stealth.rs, intercept.rs, expect.rs, cloudflare.rs, fetcher.rs}`
- `crates/zendriver-mcp/tests/{stdio_smoke.rs, http_smoke.rs, schema_snapshots.rs, tools/*.rs, evaluations/eval_set.xml}`
- `docs/book/src/mcp.md` (+ SUMMARY.md entry)
- Root `Cargo.toml` workspace member + workspace dep entry

## Task list

| # | Title | Files |
|---|---|---|
| 0 | Workspace bootstrap + crate skeleton | root Cargo.toml, crates/zendriver-mcp/{Cargo.toml, src/lib.rs, src/main.rs} |
| 1 | `Selector` arg type | src/selectors.rs + test |
| 2 | Error mapping | src/errors.rs + test |
| 3 | `SessionState` | src/state.rs + test |
| 4 | Accessibility-tree + HTML-trim builders | src/snapshot/{mod,acc_tree,html_trim}.rs + tests |
| 5 | Server bootstrap + stdio transport | src/{server.rs, transport/{mod,stdio}.rs}, main.rs |
| 6 | Streamable HTTP transport | src/transport/http.rs, main.rs |
| 7 | Lifecycle tools (3) | src/tools/lifecycle.rs + tests |
| 8 | Navigation tools (5) | src/tools/navigation.rs + tests |
| 9 | Tab tools (5) | src/tools/tabs.rs + tests |
| 10 | Snapshot tools (3) | src/tools/snapshot.rs + tests |
| 11 | Find tools (2) | src/tools/find.rs + tests |
| 12 | Action tools (9) | src/tools/actions.rs + tests |
| 13 | Read + eval + stealth + frame tools (5) | src/tools/{reads,eval,stealth,frames}.rs + tests |
| 14 | Cookie tools (5) | src/tools/cookies.rs + tests |
| 15 | Storage tools (4) | src/tools/storage.rs + tests |
| 16 | Gated: interception tools (4) | src/tools/intercept.rs + tests |
| 17 | Gated: expect tools (3) | src/tools/expect.rs + tests |
| 18 | Gated: cloudflare tool (1) | src/tools/cloudflare.rs + tests |
| 19 | Gated: fetcher tools (2) | src/tools/fetcher.rs + tests |
| 20 | Stdio + HTTP end-to-end smoke tests | tests/{stdio_smoke,http_smoke}.rs |
| 21 | Tool schema snapshot tests | tests/schema_snapshots.rs |
| 22 | Real-Chrome integration tests + evaluation XML | tests/integration/*.rs + tests/evaluations/eval_set.xml |
| 23 | mdBook chapter + README badges | docs/book/src/{SUMMARY.md, mcp.md}, README.md |
| 24 | Workspace publish prep | crates/zendriver-mcp/Cargo.toml metadata, scripts/publish.sh order |

---

## Task 0: Workspace bootstrap + crate skeleton

**Files:**
- Modify: `Cargo.toml` (root)
- Create: `crates/zendriver-mcp/Cargo.toml`
- Create: `crates/zendriver-mcp/src/lib.rs`
- Create: `crates/zendriver-mcp/src/main.rs`
- Create: `crates/zendriver-mcp/README.md` (one paragraph)

- [ ] **Step 1: Add the new member to the workspace**

Modify `Cargo.toml` — append to `[workspace] members`:

```toml
[workspace]
resolver = "3"
members = [
    "crates/zendriver",
    "crates/zendriver-transport",
    "crates/zendriver-stealth",
    "crates/zendriver-cloudflare",
    "crates/zendriver-interception",
    "crates/zendriver-fetcher",
    "crates/zendriver-mcp",
]
```

And to `[workspace.dependencies]`:

```toml
zendriver-mcp = { path = "crates/zendriver-mcp", version = "0.1.0" }
rmcp         = { version = "1.7", default-features = false }
schemars     = { version = "0.8" }
clap         = { version = "4", features = ["derive"] }
```

- [ ] **Step 2: Create `crates/zendriver-mcp/Cargo.toml`**

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
zendriver           = { workspace = true, default-features = false }
rmcp                = { workspace = true, features = ["server", "transport-io", "transport-streamable-http-server", "macros"] }
schemars            = { workspace = true }
tokio               = { workspace = true }
tracing             = { workspace = true }
tracing-subscriber  = { workspace = true }
serde               = { workspace = true }
serde_json          = { workspace = true }
thiserror           = { workspace = true }
clap                = { workspace = true }
url                 = { workspace = true }
async-trait         = { workspace = true }

[dev-dependencies]
insta               = { workspace = true }
serial_test         = { workspace = true }
tokio-test          = { workspace = true }
wiremock            = { workspace = true }
```

- [ ] **Step 3: Create `src/lib.rs`**

```rust
//! Internal library for the `zendriver-mcp` binary.
//!
//! Exposed primarily so integration tests can construct the server stack
//! directly without spawning the binary.

pub mod errors;
pub mod selectors;
pub mod server;
pub mod snapshot;
pub mod state;
pub mod tools;
pub mod transport;
```

- [ ] **Step 4: Create `src/main.rs` stub**

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
```

(Real main lands in Task 5 + 6.)

- [ ] **Step 5: Create empty module files to satisfy `lib.rs`**

```bash
mkdir -p crates/zendriver-mcp/src/transport
mkdir -p crates/zendriver-mcp/src/snapshot
mkdir -p crates/zendriver-mcp/src/tools
touch crates/zendriver-mcp/src/{errors,selectors,server,state}.rs
touch crates/zendriver-mcp/src/transport/mod.rs
touch crates/zendriver-mcp/src/snapshot/mod.rs
touch crates/zendriver-mcp/src/tools/mod.rs
```

Each file's first line should be `//! Stub.` so `cargo build` is happy with an empty module.

```bash
for f in crates/zendriver-mcp/src/{errors,selectors,server,state}.rs \
         crates/zendriver-mcp/src/transport/mod.rs \
         crates/zendriver-mcp/src/snapshot/mod.rs \
         crates/zendriver-mcp/src/tools/mod.rs; do
  echo '//! Stub.' > "$f"
done
```

- [ ] **Step 6: Create `crates/zendriver-mcp/README.md`**

```markdown
# zendriver-mcp

MCP server exposing [zendriver-rs](https://crates.io/crates/zendriver) — a stealth-by-default browser automation library — to any Model Context Protocol client.

See the [mdBook chapter](https://turtiesocks.github.io/zendriver-rs/mcp.html) for the full tool reference and Claude Desktop config snippet.
```

- [ ] **Step 7: Build + verify**

```bash
cargo build -p zendriver-mcp --locked
cargo build --workspace --locked
```

Expected: both succeed; `zendriver-mcp` produces an empty binary.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(mcp): bootstrap zendriver-mcp crate skeleton

New workspace member crates/zendriver-mcp wrapping zendriver-rs as an
MCP server. Stub binary + lib + module tree only; tools land in
subsequent commits.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 1: `Selector` arg type

**Files:**
- Create: `crates/zendriver-mcp/src/selectors.rs`
- Test: inline `#[cfg(test)] mod tests` in same file

The `Selector` struct is shared by every find / action tool. Validates exactly-one-of selector kinds + maps to a `FindBuilder` configurator closure.

- [ ] **Step 1: Write failing tests**

Replace `crates/zendriver-mcp/src/selectors.rs` with:

```rust
//! Selector argument type shared by find / action tools.
//!
//! Validates exactly-one-of selector kinds at deserialize-time
//! application and exposes an [`apply`](Selector::apply) helper
//! that configures a zendriver `FindBuilder`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xpath: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_exact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_regex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth: Option<usize>,
    #[serde(default = "default_visible_only")]
    pub visible_only: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
}

fn default_visible_only() -> bool { true }
fn default_timeout_ms() -> u64 { 5000 }

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SelectorError {
    #[error("Selector requires exactly one of: css, xpath, text, text_exact, text_regex, role")]
    NoneOrMultiple,
    #[error("`role_name` requires `role` to also be set")]
    OrphanRoleName,
}

impl Selector {
    pub fn validate(&self) -> Result<(), SelectorError> {
        let n = [
            self.css.is_some(),
            self.xpath.is_some(),
            self.text.is_some(),
            self.text_exact.is_some(),
            self.text_regex.is_some(),
            self.role.is_some(),
        ]
        .into_iter()
        .filter(|b| *b)
        .count();
        if n != 1 {
            return Err(SelectorError::NoneOrMultiple);
        }
        if self.role_name.is_some() && self.role.is_none() {
            return Err(SelectorError::OrphanRoleName);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Selector {
        Selector {
            css: None,
            xpath: None,
            text: None,
            text_exact: None,
            text_regex: None,
            role: None,
            role_name: None,
            nth: None,
            visible_only: true,
            timeout_ms: 5000,
            frame_id: None,
        }
    }

    #[test]
    fn validate_rejects_zero_selectors() {
        let s = base();
        assert_eq!(s.validate(), Err(SelectorError::NoneOrMultiple));
    }

    #[test]
    fn validate_rejects_two_selectors() {
        let mut s = base();
        s.css = Some("#x".into());
        s.text = Some("hi".into());
        assert_eq!(s.validate(), Err(SelectorError::NoneOrMultiple));
    }

    #[test]
    fn validate_accepts_single_css() {
        let mut s = base();
        s.css = Some("#x".into());
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_rejects_role_name_without_role() {
        let mut s = base();
        s.text = Some("hi".into());
        s.role_name = Some("Submit".into());
        assert_eq!(s.validate(), Err(SelectorError::OrphanRoleName));
    }
}
```

- [ ] **Step 2: Run + verify pass**

```bash
cargo test -p zendriver-mcp --lib selectors -- --nocapture
```

Expected: 4 tests pass.

- [ ] **Step 3: Wire `apply` to `FindBuilder` (deferred until Task 11)**

Leave a `// TODO(task-11): impl Selector::apply` marker at the end of `impl Selector` — task 11 lands the FindBuilder bridge once `find.rs` exists.

```rust
// NOTE: Selector::apply(&self, find: FindBuilder<'_>) -> FindBuilder<'_>
// lives in tools/find.rs because it depends on zendriver's FindBuilder type.
```

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-mcp/src/selectors.rs
git commit -m "feat(mcp): Selector arg type with one-of validation

Selector is the shared arg struct on every find / action tool.
Validates exactly-one-of {css, xpath, text, text_exact, text_regex, role}
and forbids role_name without role. apply() bridge to zendriver
FindBuilder lands with the find tools in a later commit.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: Error mapping

**Files:**
- Create: `crates/zendriver-mcp/src/errors.rs`
- Test: inline `#[cfg(test)] mod tests`

Maps `zendriver::ZendriverError` to MCP errors with `_metadata.suggested_next` hints.

- [ ] **Step 1: Write failing tests**

Replace `crates/zendriver-mcp/src/errors.rs` with:

```rust
//! ZendriverError → rmcp error mapping with actionable next-step hints.

use rmcp::model::ErrorData as McpError;
use serde_json::json;
use zendriver::ZendriverError;

pub fn map_error(err: ZendriverError) -> McpError {
    let (msg, suggested_next) = match &err {
        ZendriverError::NoTab => (
            "No tab open. Call `browser_open` first.".to_string(),
            Some("browser_open"),
        ),
        ZendriverError::ElementNotFound { selector } => (
            format!("No element matched `{selector}`. Try `browser_snapshot` to inspect current page."),
            Some("browser_snapshot"),
        ),
        ZendriverError::Timeout { op, ms } => (
            format!("`{op}` timed out after {ms}ms. Retry with larger `timeout_ms` or inspect with `browser_snapshot`."),
            Some("browser_snapshot"),
        ),
        ZendriverError::BrowserNotOpen => (
            "Browser not open. Call `browser_open`.".into(),
            Some("browser_open"),
        ),
        ZendriverError::FrameNotFound { id } => (
            format!("No frame with id `{id}`. Try `browser_frame_list`."),
            Some("browser_frame_list"),
        ),
        _ => (err.to_string(), None),
    };
    let meta = match suggested_next {
        Some(next) => json!({ "suggested_next": next }),
        None => json!({}),
    };
    McpError::invalid_request(msg, Some(meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tab_suggests_browser_open() {
        let e = map_error(ZendriverError::NoTab);
        assert!(e.message.contains("browser_open"));
        assert_eq!(e.data.as_ref().unwrap()["suggested_next"], "browser_open");
    }

    #[test]
    fn element_not_found_echoes_selector() {
        let e = map_error(ZendriverError::ElementNotFound { selector: "#x".into() });
        assert!(e.message.contains("`#x`"));
        assert_eq!(e.data.as_ref().unwrap()["suggested_next"], "browser_snapshot");
    }
}
```

> **Note:** the exact `ZendriverError` variant names need to match what zendriver actually ships. Before running tests, grep the current variants:

```bash
grep -E "^\s*[A-Z][A-Za-z]+\s*\{?" crates/zendriver/src/error.rs | head -30
```

Adjust the match arms to use the real variant names. If a variant is missing, leave a `// TODO` and map only what exists.

- [ ] **Step 2: Run + verify**

```bash
cargo test -p zendriver-mcp --lib errors -- --nocapture
```

Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-mcp/src/errors.rs
git commit -m "feat(mcp): ZendriverError to MCP error mapping with next-step hints

Each error category includes _metadata.suggested_next pointing at the
tool the agent should likely call next (e.g. ElementNotFound suggests
browser_snapshot).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: `SessionState`

**Files:**
- Create: `crates/zendriver-mcp/src/state.rs`

- [ ] **Step 1: Write failing test**

Replace `crates/zendriver-mcp/src/state.rs` with:

```rust
//! Per-MCP-session mutable state.
//!
//! Wrapped in `Arc<tokio::sync::Mutex<_>>` and shared across tool handlers.
//! In stdio mode, one global instance. In HTTP mode, one per session ID.

use std::collections::HashMap;
use zendriver::Browser;

#[cfg(feature = "expect")]
pub type ExpectationId = String;

#[cfg(feature = "interception")]
pub type RuleId = String;

pub struct SessionState {
    pub browser: Option<Browser>,
    pub current_tab_id: Option<String>,
    pub stealth_profile_choice: StealthProfileChoice,

    #[cfg(feature = "expect")]
    pub expectations: HashMap<ExpectationId, ExpectationHandle>,

    #[cfg(feature = "interception")]
    pub rules: HashMap<RuleId, InterceptRuleHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StealthProfileChoice {
    Auto,
    Native,
    SpoofMacos,
    SpoofLinux,
    SpoofWindows,
}

impl Default for StealthProfileChoice {
    fn default() -> Self { Self::Auto }
}

#[cfg(feature = "expect")]
pub struct ExpectationHandle {
    pub kind: String,
    // Buffer of matched events between registration and await.
    pub buffer: tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
}

#[cfg(feature = "interception")]
pub struct InterceptRuleHandle {
    pub pattern: String,
    pub action_kind: String,
    // Real handle into zendriver-interception lives here.
    // pub _drop_guard: zendriver_interception::RuleGuard,
}

impl SessionState {
    pub fn new() -> Self {
        Self {
            browser: None,
            current_tab_id: None,
            stealth_profile_choice: StealthProfileChoice::default(),
            #[cfg(feature = "expect")]
            expectations: HashMap::new(),
            #[cfg(feature = "interception")]
            rules: HashMap::new(),
        }
    }
}

impl Default for SessionState {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_empty() {
        let s = SessionState::new();
        assert!(s.browser.is_none());
        assert!(s.current_tab_id.is_none());
        assert_eq!(s.stealth_profile_choice, StealthProfileChoice::Auto);
    }
}
```

- [ ] **Step 2: Run + verify**

```bash
cargo test -p zendriver-mcp --lib state -- --nocapture
```

Expected: 1 test passes; also builds with `--no-default-features --features stealth` to confirm cfg gating compiles.

```bash
cargo build -p zendriver-mcp --no-default-features --features stealth --locked
```

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-mcp/src/state.rs
git commit -m "feat(mcp): SessionState with cfg-gated registries

Holds Browser + current_tab_id + stealth profile choice. expect
registry and interception registry are cfg-gated to match crate
features.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: Accessibility-tree + HTML-trim builders

**Files:**
- Create: `crates/zendriver-mcp/src/snapshot/mod.rs`
- Create: `crates/zendriver-mcp/src/snapshot/acc_tree.rs`
- Create: `crates/zendriver-mcp/src/snapshot/html_trim.rs`

- [ ] **Step 1: Write failing tests**

`crates/zendriver-mcp/src/snapshot/mod.rs`:

```rust
//! Snapshot formatters consumed by the snapshot tools.
//!
//! - `acc_tree` turns CDP Accessibility.getFullAXTree into a compact text tree.
//! - `html_trim` trims rendered HTML (drops scripts/styles, collapses whitespace).

pub mod acc_tree;
pub mod html_trim;
```

`crates/zendriver-mcp/src/snapshot/acc_tree.rs`:

```rust
//! Accessibility-tree → plain-text formatter.
//!
//! Selector-based handle model (no ref numbers in output).

use serde_json::Value;

pub fn format(ax_tree: &Value) -> String {
    let mut out = String::new();
    if let Some(nodes) = ax_tree.get("nodes").and_then(Value::as_array) {
        for node in nodes {
            format_node(node, 0, &mut out);
        }
    }
    out
}

fn format_node(node: &Value, depth: usize, out: &mut String) {
    let role = node
        .get("role")
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let name = node
        .get("name")
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if role.is_empty() && name.is_empty() {
        return;
    }
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(&format!("[{role}]"));
    if !name.is_empty() {
        out.push_str(&format!(" \"{name}\""));
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_single_node() {
        let tree = json!({
            "nodes": [
                {
                    "role": { "value": "button" },
                    "name": { "value": "Submit" }
                }
            ]
        });
        let out = format(&tree);
        assert_eq!(out, "[button] \"Submit\"\n");
    }

    #[test]
    fn skips_empty_nodes() {
        let tree = json!({ "nodes": [{}] });
        assert_eq!(format(&tree), "");
    }
}
```

`crates/zendriver-mcp/src/snapshot/html_trim.rs`:

```rust
//! HTML trimmer — drops `<script>`/`<style>` blocks + collapses whitespace.

pub fn trim(html: &str) -> String {
    let no_scripts = strip_block(html, "script");
    let no_styles = strip_block(&no_scripts, "style");
    collapse_ws(&no_styles)
}

fn strip_block(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(&open) {
        out.push_str(&rest[..start]);
        match rest[start..].find(&close) {
            Some(end_off) => {
                rest = &rest[start + end_off + close.len()..];
            }
            None => { rest = ""; break; }
        }
    }
    out.push_str(rest);
    out
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scripts() {
        let html = "<p>hi</p><script>alert(1)</script><p>bye</p>";
        assert_eq!(trim(html), "<p>hi</p><p>bye</p>");
    }

    #[test]
    fn strips_styles() {
        let html = "<style>p{color:red}</style><p>hi</p>";
        assert_eq!(trim(html), "<p>hi</p>");
    }

    #[test]
    fn collapses_whitespace() {
        let html = "<p>hi\n\n  there</p>";
        assert_eq!(trim(html), "<p>hi there</p>");
    }
}
```

- [ ] **Step 2: Wire module into `lib.rs`**

`crates/zendriver-mcp/src/lib.rs` already has `pub mod snapshot;` from Task 0.

- [ ] **Step 3: Run + verify**

```bash
cargo test -p zendriver-mcp --lib snapshot -- --nocapture
```

Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-mcp/src/snapshot
git commit -m "feat(mcp): acc-tree + html-trim snapshot builders

Pure functions consumed by browser_snapshot and browser_html. Acc tree
strips ref numbers (selector-based handle model). HTML trim drops
scripts/styles + collapses whitespace.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: Server bootstrap + stdio transport

**Files:**
- Replace: `crates/zendriver-mcp/src/server.rs`
- Replace: `crates/zendriver-mcp/src/transport/mod.rs`
- Create: `crates/zendriver-mcp/src/transport/stdio.rs`
- Replace: `crates/zendriver-mcp/src/main.rs`

- [ ] **Step 1: Set up CLI + main**

`src/main.rs`:

```rust
use clap::Parser;
use zendriver_mcp::server;
use zendriver_mcp::state::{SessionState, StealthProfileChoice};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Parser)]
#[command(name = "zendriver-mcp", version)]
struct Cli {
    /// Run streamable HTTP transport on this address (e.g. 127.0.0.1:8765).
    /// Default: stdio.
    #[arg(long)]
    http: Option<String>,

    /// Default stealth profile.
    #[arg(long, value_enum, default_value_t = StealthProfileArg::Auto)]
    stealth_profile: StealthProfileArg,

    /// Default headless.
    #[arg(long, default_value_t = true)]
    headless: bool,

    /// Tracing log level.
    #[arg(long, default_value = "info")]
    log: String,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum StealthProfileArg {
    Auto, Native, SpoofMacos, SpoofLinux, SpoofWindows,
}

impl From<StealthProfileArg> for StealthProfileChoice {
    fn from(v: StealthProfileArg) -> Self {
        match v {
            StealthProfileArg::Auto         => StealthProfileChoice::Auto,
            StealthProfileArg::Native       => StealthProfileChoice::Native,
            StealthProfileArg::SpoofMacos   => StealthProfileChoice::SpoofMacos,
            StealthProfileArg::SpoofLinux   => StealthProfileChoice::SpoofLinux,
            StealthProfileArg::SpoofWindows => StealthProfileChoice::SpoofWindows,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&cli.log))
        .with_writer(std::io::stderr) // never stdout in stdio mode
        .init();

    let state = Arc::new(Mutex::new(SessionState {
        stealth_profile_choice: cli.stealth_profile.into(),
        ..SessionState::new()
    }));

    match cli.http {
        Some(_addr) => unimplemented!("HTTP transport — Task 6"),
        None        => server::run_stdio(state).await?,
    }
    Ok(())
}
```

- [ ] **Step 2: Write `src/server.rs`**

```rust
//! rmcp Server setup + tool registration entry point.

use std::sync::Arc;
use tokio::sync::Mutex;
use crate::state::SessionState;

pub async fn run_stdio(state: Arc<Mutex<SessionState>>) -> Result<(), Box<dyn std::error::Error>> {
    crate::transport::stdio::serve(build_server(state)).await
}

fn build_server(state: Arc<Mutex<SessionState>>) -> rmcp::Server {
    let mut server = rmcp::ServerBuilder::new("zendriver-mcp", env!("CARGO_PKG_VERSION"))
        .with_capabilities(rmcp::ServerCapabilities::default().with_tools())
        .build();
    crate::tools::register_all(&mut server, state);
    server
}
```

> **Note:** rmcp's exact builder API may differ — check `rmcp` v1.7 docs (`cargo doc -p rmcp --open`) before locking exact names. If `ServerBuilder` is named differently, adapt and document the actual API in a comment.

- [ ] **Step 3: Write `src/transport/mod.rs`** (replace stub)

```rust
//! Transport bootstraps for rmcp.

pub mod stdio;
#[cfg(feature = "default")]
pub mod http;
```

Actually drop the cfg gating on http (it's a feature of rmcp, not our crate):

```rust
pub mod stdio;
pub mod http;
```

- [ ] **Step 4: Write `src/transport/stdio.rs`**

```rust
//! Stdio transport — JSON-RPC over stdin/stdout.

pub async fn serve(server: rmcp::Server) -> Result<(), Box<dyn std::error::Error>> {
    let transport = rmcp::transport::stdio();
    server.serve(transport).await?;
    Ok(())
}
```

> Again, rmcp's exact transport-construction API may differ — confirm by reading rmcp v1.7 README before writing.

- [ ] **Step 5: Stub `tools::register_all`**

`src/tools/mod.rs`:

```rust
//! Tool registration entry point.

use std::sync::Arc;
use tokio::sync::Mutex;
use crate::state::SessionState;

pub fn register_all(_server: &mut rmcp::Server, _state: Arc<Mutex<SessionState>>) {
    // Tools registered in subsequent tasks.
}
```

- [ ] **Step 6: Build + smoke**

```bash
cargo build -p zendriver-mcp --locked
cargo build --workspace --locked
```

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(mcp): server bootstrap + stdio transport scaffolding

clap CLI parses --http / --stealth-profile / --headless / --log.
server::build_server constructs an rmcp Server and calls
tools::register_all (stub). Stdio path is wired end to end; tools
land in later commits.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: Streamable HTTP transport

**Files:**
- Create: `crates/zendriver-mcp/src/transport/http.rs`
- Modify: `crates/zendriver-mcp/src/main.rs`, `src/server.rs`

- [ ] **Step 1: Write `src/transport/http.rs`**

```rust
//! Streamable HTTP transport — stateless JSON per MCP best practices.

use std::net::SocketAddr;

pub async fn serve(
    addr: SocketAddr,
    build_server: impl Fn() -> rmcp::Server + Send + Sync + 'static,
) -> Result<(), Box<dyn std::error::Error>> {
    let transport = rmcp::transport::streamable_http_server(addr);
    transport.serve_with(move |_session_id| Ok(build_server())).await?;
    Ok(())
}
```

> rmcp's per-session-server pattern: each new MCP session calls the closure to build a fresh `Server`. This is how each HTTP session gets its own `Arc<Mutex<SessionState>>`.

- [ ] **Step 2: Update `src/server.rs`**

```rust
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::state::SessionState;

pub async fn run_stdio(state: Arc<Mutex<SessionState>>) -> Result<(), Box<dyn std::error::Error>> {
    crate::transport::stdio::serve(build_server(state)).await
}

pub async fn run_http(addr: std::net::SocketAddr, default_profile: crate::state::StealthProfileChoice)
    -> Result<(), Box<dyn std::error::Error>>
{
    crate::transport::http::serve(addr, move || {
        let s = Arc::new(Mutex::new(SessionState {
            stealth_profile_choice: default_profile,
            ..SessionState::new()
        }));
        build_server(s)
    }).await
}

fn build_server(state: Arc<Mutex<SessionState>>) -> rmcp::Server {
    let mut server = rmcp::ServerBuilder::new("zendriver-mcp", env!("CARGO_PKG_VERSION"))
        .with_capabilities(rmcp::ServerCapabilities::default().with_tools())
        .build();
    crate::tools::register_all(&mut server, state);
    server
}
```

- [ ] **Step 3: Update `main.rs` match arm**

Replace the `Some(_addr) => unimplemented!(...)` with:

```rust
Some(addr) => {
    let parsed: std::net::SocketAddr = addr.parse()?;
    server::run_http(parsed, cli.stealth_profile.into()).await?
}
```

- [ ] **Step 4: Build + verify**

```bash
cargo build -p zendriver-mcp --locked
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(mcp): streamable HTTP transport

Per-session SessionState in HTTP mode: each new MCP session gets its
own Browser and registries, dropped on disconnect. CLI flag --http
selects HTTP over default stdio.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Tool tasks (7–19) — shared pattern

Each tool task follows the same TDD micro-cycle:

1. Define input + output structs with `#[derive(Debug, Deserialize, Serialize, JsonSchema)]`.
2. Write handler async fn that takes `Arc<Mutex<SessionState>>` + input → `Result<Output, McpError>`.
3. Write a `MockConnection`-based unit test for success + an error path.
4. Register the tool in the module's `register()` fn.
5. Build + test.
6. Commit each module-task as one logical commit.

For tools that mutate state, accept a `return_snapshot: bool` arg (default false). When true, the handler grabs a fresh acc-tree snapshot after the action and bundles it into output.

The first sub-step of every tool task shows full code for **one** representative tool from the group. Sibling tools in the same task have abbreviated entries — signature + handler body delta + test case — but you still write the full code in the file.

### Shared helper module — `state/helpers.rs`

To avoid 10+ copies of `current_tab(&s)` and `EmptyInput`, extract them into a private helper module that every tool module imports. Add this in Task 5 (right after `state.rs` is in place):

```rust
// crates/zendriver-mcp/src/state/helpers.rs
use rmcp::model::ErrorData as McpError;
use serde::Deserialize;
use schemars::JsonSchema;
use zendriver::{Tab, ZendriverError};
use super::SessionState;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyInput {}

pub async fn current_tab<'a>(s: &'a SessionState) -> Result<&'a Tab, McpError> {
    let b = s.browser.as_ref().ok_or_else(|| crate::errors::map_error(ZendriverError::BrowserNotOpen))?;
    let id = s.current_tab_id.as_ref().ok_or_else(|| crate::errors::map_error(ZendriverError::NoTab))?;
    b.tabs().into_iter().find(|t| t.target_id() == id)
        .ok_or_else(|| crate::errors::map_error(ZendriverError::NoTab))
}
```

Then `state.rs` adds `pub mod helpers;` and every tool module does `use crate::state::helpers::{current_tab, EmptyInput};` instead of redefining locally. The tool snippets in Tasks 8–19 reference these by name; treat the local re-definitions shown in those snippets as illustrative — actually `use` the shared versions.

---

## Task 7: Lifecycle tools (3) — `browser_open`, `browser_close`, `browser_status`

**Files:**
- Create: `crates/zendriver-mcp/src/tools/lifecycle.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write `tools/lifecycle.rs` — `browser_open`**

```rust
//! Browser lifecycle tools.

use std::sync::Arc;
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use rmcp::model::ErrorData as McpError;
use zendriver::Browser;
use crate::state::{SessionState, StealthProfileChoice};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenInput {
    #[serde(default = "default_true")]
    pub headless: bool,
    #[serde(default)]
    pub stealth_profile: Option<StealthProfileChoice>,
}

fn default_true() -> bool { true }

#[derive(Debug, Serialize, JsonSchema)]
pub struct OpenOutput {
    pub chrome_version: String,
    pub headless: bool,
    pub profile: StealthProfileChoice,
}

pub async fn open(state: Arc<Mutex<SessionState>>, input: OpenInput) -> Result<OpenOutput, McpError> {
    let mut s = state.lock().await;
    if s.browser.is_some() {
        return Err(McpError::invalid_request("Browser already open. Call `browser_close` first.", None));
    }
    let profile = input.stealth_profile.unwrap_or(s.stealth_profile_choice);
    let browser = Browser::builder()
        .headless(input.headless)
        .launch()
        .await
        .map_err(crate::errors::map_error)?;
    let chrome_version = browser.version().await.unwrap_or_default();
    let tabs = browser.tabs();
    s.current_tab_id = tabs.first().map(|t| t.target_id().to_string());
    s.browser = Some(browser);
    s.stealth_profile_choice = profile;
    Ok(OpenOutput { chrome_version, headless: input.headless, profile })
}
```

> If `Browser::version()` doesn't exist on the current zendriver API, return `String::new()` for `chrome_version`. The intent is observability; the exact source can flex.

- [ ] **Step 2: Write `browser_close` + `browser_status`**

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyInput {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CloseOutput { pub ok: bool }

pub async fn close(state: Arc<Mutex<SessionState>>, _: EmptyInput) -> Result<CloseOutput, McpError> {
    let mut s = state.lock().await;
    if let Some(b) = s.browser.take() {
        b.close().await.map_err(crate::errors::map_error)?;
    }
    s.current_tab_id = None;
    Ok(CloseOutput { ok: true })
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StatusOutput {
    pub open: bool,
    pub tab_count: usize,
    pub current_tab: Option<TabSummary>,
    pub headless: Option<bool>,
    pub profile: StealthProfileChoice,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TabSummary {
    pub id: String,
    pub url: String,
    pub title: String,
}

pub async fn status(state: Arc<Mutex<SessionState>>, _: EmptyInput) -> Result<StatusOutput, McpError> {
    let s = state.lock().await;
    let Some(b) = s.browser.as_ref() else {
        return Ok(StatusOutput {
            open: false, tab_count: 0, current_tab: None, headless: None,
            profile: s.stealth_profile_choice,
        });
    };
    let tabs = b.tabs();
    let current_tab = match &s.current_tab_id {
        Some(id) => tabs.iter().find(|t| t.target_id() == id).map(|t| TabSummary {
            id: t.target_id().to_string(),
            url: t.url().unwrap_or_default(),
            title: t.title().unwrap_or_default(),
        }),
        None => None,
    };
    Ok(StatusOutput {
        open: true,
        tab_count: tabs.len(),
        current_tab,
        headless: b.headless(),
        profile: s.stealth_profile_choice,
    })
}
```

- [ ] **Step 3: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn close_with_no_browser_is_noop() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let out = close(state, EmptyInput {}).await.unwrap();
        assert!(out.ok);
    }

    #[tokio::test]
    async fn status_with_no_browser_reports_closed() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let out = status(state, EmptyInput {}).await.unwrap();
        assert!(!out.open);
        assert_eq!(out.tab_count, 0);
        assert!(out.current_tab.is_none());
    }
}
```

> `open` itself launches a real Chrome — tested in the integration suite (Task 22), not unit tests.

- [ ] **Step 4: Register in `tools/mod.rs`**

Replace the stub `register_all`:

```rust
pub fn register_all(server: &mut rmcp::Server, state: Arc<Mutex<SessionState>>) {
    crate::tools::lifecycle::register(server, state.clone());
}
```

Add `pub mod lifecycle;` at the top.

In `tools/lifecycle.rs`, add at the bottom:

```rust
pub fn register(server: &mut rmcp::Server, state: Arc<Mutex<SessionState>>) {
    let s1 = state.clone(); let s2 = state.clone(); let s3 = state.clone();
    server.tool("browser_open",   "Launch Chrome with stealth defaults.", move |input| { let s = s1.clone(); async move { open(s, input).await } });
    server.tool("browser_close",  "Close the open browser.",              move |input| { let s = s2.clone(); async move { close(s, input).await } });
    server.tool("browser_status", "Report open browser + current tab.",   move |input| { let s = s3.clone(); async move { status(s, input).await } });
}
```

> The exact `server.tool(...)` signature comes from rmcp — confirm against v1.7 docs. The macro `#[rmcp::tool]` may be more idiomatic; adapt accordingly. The shape (name + description + async fn taking input) is universal.

- [ ] **Step 5: Build + test**

```bash
cargo test -p zendriver-mcp --lib lifecycle -- --nocapture
cargo build -p zendriver-mcp --locked
```

Expected: 2 unit tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(mcp): lifecycle tools — browser_open / _close / _status

browser_open launches Chrome with stealth defaults and records the
initial tab. browser_close drops the Browser. browser_status reports
open/headless/current_tab + tab count.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8: Navigation tools (5) — `browser_goto / _back / _forward / _reload / _wait_for_idle`

**Files:**
- Create: `crates/zendriver-mcp/src/tools/navigation.rs`

- [ ] **Step 1: Write `tools/navigation.rs` — `browser_goto`**

```rust
//! Navigation tools.

use std::sync::Arc;
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use rmcp::model::ErrorData as McpError;
use zendriver::ZendriverError;
use crate::state::SessionState;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GotoInput {
    pub url: String,
    #[serde(default = "default_wait")]
    pub wait_for: WaitFor,
    #[serde(default)]
    pub return_snapshot: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitFor { Load, Idle, None }
fn default_wait() -> WaitFor { WaitFor::Load }

#[derive(Debug, Serialize, JsonSchema)]
pub struct NavOutput {
    pub url: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

async fn current_tab<'a>(s: &'a SessionState) -> Result<&'a zendriver::Tab, McpError> {
    let b = s.browser.as_ref().ok_or_else(|| crate::errors::map_error(ZendriverError::BrowserNotOpen))?;
    let id = s.current_tab_id.as_ref().ok_or_else(|| crate::errors::map_error(ZendriverError::NoTab))?;
    b.tabs().into_iter().find(|t| t.target_id() == id)
        .ok_or_else(|| crate::errors::map_error(ZendriverError::NoTab))
}

pub async fn goto(state: Arc<Mutex<SessionState>>, input: GotoInput) -> Result<NavOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.goto(&input.url).await.map_err(crate::errors::map_error)?;
    match input.wait_for {
        WaitFor::Load => tab.wait_for_load().await.map_err(crate::errors::map_error)?,
        WaitFor::Idle => tab.wait_for_idle(std::time::Duration::from_millis(5000)).await.map_err(crate::errors::map_error)?,
        WaitFor::None => {}
    }
    let snapshot = if input.return_snapshot { Some(snapshot_now(tab).await?) } else { None };
    Ok(NavOutput {
        url: tab.url().unwrap_or_default(),
        title: tab.title().unwrap_or_default(),
        snapshot,
    })
}

async fn snapshot_now(tab: &zendriver::Tab) -> Result<String, McpError> {
    let raw = tab.accessibility_tree().await.map_err(crate::errors::map_error)?;
    Ok(crate::snapshot::acc_tree::format(&raw))
}
```

> `tab.accessibility_tree()` — confirm the exact zendriver method name (could be `tab.ax_tree()` etc.). Adapt.

- [ ] **Step 2: Add `browser_back / _forward / _reload / _wait_for_idle`**

Each follows the same `current_tab(&s)` pattern. Bodies:

```rust
pub async fn back(state: Arc<Mutex<SessionState>>, _: EmptyInput) -> Result<NavOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.back().await.map_err(crate::errors::map_error)?;
    Ok(NavOutput { url: tab.url().unwrap_or_default(), title: tab.title().unwrap_or_default(), snapshot: None })
}

pub async fn forward(state: Arc<Mutex<SessionState>>, _: EmptyInput) -> Result<NavOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.forward().await.map_err(crate::errors::map_error)?;
    Ok(NavOutput { url: tab.url().unwrap_or_default(), title: tab.title().unwrap_or_default(), snapshot: None })
}

pub async fn reload(state: Arc<Mutex<SessionState>>, _: EmptyInput) -> Result<NavOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.reload().await.map_err(crate::errors::map_error)?;
    Ok(NavOutput { url: tab.url().unwrap_or_default(), title: tab.title().unwrap_or_default(), snapshot: None })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdleInput {
    #[serde(default = "default_idle_timeout")]
    pub timeout_ms: u64,
}
fn default_idle_timeout() -> u64 { 5000 }

#[derive(Debug, Serialize, JsonSchema)]
pub struct IdleOutput { pub idle: bool }

pub async fn wait_for_idle(state: Arc<Mutex<SessionState>>, input: IdleInput) -> Result<IdleOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.wait_for_idle(std::time::Duration::from_millis(input.timeout_ms)).await.map_err(crate::errors::map_error)?;
    Ok(IdleOutput { idle: true })
}

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct EmptyInput {}
```

- [ ] **Step 3: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn goto_with_no_browser_returns_actionable_error() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = goto(state, GotoInput { url: "https://example.com".into(), wait_for: WaitFor::Load, return_snapshot: false }).await.unwrap_err();
        assert!(err.message.contains("browser_open"));
    }
}
```

- [ ] **Step 4: Register**

In `tools/mod.rs` add `pub mod navigation;` and call `navigation::register(server, state.clone())`. Write `register()` in `navigation.rs` registering all 5 with one-line descriptions.

- [ ] **Step 5: Build + test**

```bash
cargo test -p zendriver-mcp --lib navigation -- --nocapture
cargo build -p zendriver-mcp --locked
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(mcp): navigation tools — goto/back/forward/reload/wait_for_idle

Five-tool group sharing a current_tab(&s) helper and a NavOutput
struct (url + title + optional snapshot). browser_goto honors a
wait_for arg (load/idle/none) and an optional return_snapshot bool.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9: Tab tools (5) — `browser_tab_list / _new / _switch / _close / _activate`

**Files:**
- Create: `crates/zendriver-mcp/src/tools/tabs.rs`

- [ ] **Step 1: Write tool handlers**

Pattern: each handler takes `Arc<Mutex<SessionState>>` + typed input, returns a typed output. Shared helpers: `browser(&s) -> Result<&Browser, McpError>` and `target_to_summary(t)`.

Outputs:
- `tab_list` → `Vec<TabSummary { id, url, title, is_current }>`
- `tab_new { url: Option<String>, activate: bool = true }` → `TabSummary`
- `tab_switch { tab_id }` → `TabSummary`
- `tab_close { tab_id: Option<String> }` (default current) → `{ closed_id, current_tab_id: Option<String> }`
- `tab_activate { tab_id }` → `{ id }`

Sample handler:

```rust
pub async fn tab_new(state: Arc<Mutex<SessionState>>, input: TabNewInput) -> Result<TabSummary, McpError> {
    let mut s = state.lock().await;
    let b = s.browser.as_ref().ok_or_else(|| crate::errors::map_error(ZendriverError::BrowserNotOpen))?;
    let tab = match input.url {
        Some(u) => b.new_tab(&u).await,
        None    => b.new_tab("about:blank").await,
    }.map_err(crate::errors::map_error)?;
    let id = tab.target_id().to_string();
    let summary = TabSummary {
        id: id.clone(),
        url: tab.url().unwrap_or_default(),
        title: tab.title().unwrap_or_default(),
        is_current: input.activate,
    };
    if input.activate { s.current_tab_id = Some(id); }
    Ok(summary)
}
```

Repeat for the other four. `tab_close` clears `s.current_tab_id` if closing the current tab, then picks the first remaining tab as the new current.

- [ ] **Step 2: Tests (mock-based, no real Chrome)**

```rust
#[tokio::test]
async fn list_with_no_browser_errors() {
    let state = Arc::new(Mutex::new(SessionState::new()));
    let err = tab_list(state, EmptyInput {}).await.unwrap_err();
    assert!(err.message.contains("browser_open"));
}
```

- [ ] **Step 3: Register + commit**

Same pattern as Task 7/8. Commit message: "feat(mcp): tab tools — list/new/switch/close/activate".

---

## Task 10: Snapshot tools (3) — `browser_snapshot / _html / _screenshot`

**Files:**
- Create: `crates/zendriver-mcp/src/tools/snapshot.rs`

- [ ] **Step 1: Write tools**

```rust
use base64::Engine;

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct SnapshotInput {
    #[serde(default)]
    pub frame_id: Option<String>,
}

pub async fn snapshot(state: Arc<Mutex<SessionState>>, input: SnapshotInput) -> Result<String, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let raw = match input.frame_id {
        Some(id) => tab.frame(&id).map_err(crate::errors::map_error)?.accessibility_tree().await,
        None     => tab.accessibility_tree().await,
    }.map_err(crate::errors::map_error)?;
    Ok(crate::snapshot::acc_tree::format(&raw))
}

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct HtmlInput {
    #[serde(default)] pub selector: Option<crate::selectors::Selector>,
    #[serde(default = "default_true")] pub trim: bool,
    #[serde(default)] pub frame_id: Option<String>,
}
fn default_true() -> bool { true }

pub async fn html(state: Arc<Mutex<SessionState>>, input: HtmlInput) -> Result<String, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let raw = match (&input.selector, &input.frame_id) {
        (None, None)         => tab.content().await,
        (None, Some(fid))    => tab.frame(fid).map_err(crate::errors::map_error)?.content().await,
        (Some(sel), _)       => {
            sel.validate().map_err(|e| McpError::invalid_request(e.to_string(), None))?;
            let el = crate::tools::find::resolve(tab, sel).await?;
            el.inner_html().await
        }
    }.map_err(crate::errors::map_error)?;
    Ok(if input.trim { crate::snapshot::html_trim::trim(&raw) } else { raw })
}

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct ScreenshotInput {
    #[serde(default = "default_format")] pub format: ImgFormat,
    #[serde(default)] pub full_page: bool,
    #[serde(default)] pub selector: Option<crate::selectors::Selector>,
    #[serde(default)] pub omit_background: bool,
    #[serde(default)] pub save_path: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImgFormat { Png, Jpeg, Webp }
fn default_format() -> ImgFormat { ImgFormat::Png }

#[derive(Debug, Serialize, JsonSchema)]
pub struct ScreenshotOutput {
    pub bytes_base64: String,
    pub format: ImgFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_path: Option<String>,
}

pub async fn screenshot(state: Arc<Mutex<SessionState>>, input: ScreenshotInput) -> Result<ScreenshotOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let bytes = {
        let mut builder = tab.screenshot();
        builder = match input.format { ImgFormat::Png => builder.png(), ImgFormat::Jpeg => builder.jpeg(), ImgFormat::Webp => builder.webp() };
        if input.full_page { builder = builder.full_page(); }
        if input.omit_background { builder = builder.omit_background(); }
        if let Some(sel) = input.selector.as_ref() {
            sel.validate().map_err(|e| McpError::invalid_request(e.to_string(), None))?;
            let el = crate::tools::find::resolve(tab, sel).await?;
            builder = builder.clip_element(&el);
        }
        builder.capture().await.map_err(crate::errors::map_error)?
    };
    let saved_path = if let Some(p) = input.save_path {
        tokio::fs::write(&p, &bytes).await.map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Some(p)
    } else { None };
    Ok(ScreenshotOutput {
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        format: input.format,
        saved_path,
    })
}
```

> The screenshot output uses `bytes_base64` in the JSON payload. For MCP clients that consume `image` content blocks, the registration helper wraps `bytes_base64` into an `ImageContent` block per rmcp's API — confirm the exact wrapper signature and adapt.

- [ ] **Step 2: Add `base64` to Cargo deps**

```toml
base64 = { workspace = true }
```

- [ ] **Step 3: Tests**

```rust
#[tokio::test]
async fn snapshot_with_no_browser_errors() {
    let state = Arc::new(Mutex::new(SessionState::new()));
    let err = snapshot(state, SnapshotInput { frame_id: None }).await.unwrap_err();
    assert!(err.message.contains("browser_open"));
}

#[test]
fn img_format_serde_roundtrip() {
    let s = serde_json::to_string(&ImgFormat::Png).unwrap();
    assert_eq!(s, r#""png""#);
}
```

- [ ] **Step 4: Register + build + commit**

```bash
git add -A
git commit -m "feat(mcp): snapshot tools — snapshot/html/screenshot

browser_snapshot returns the trimmed acc tree (no refs). browser_html
returns rendered HTML (subtree if selector given, trimmed by default).
browser_screenshot returns base64-encoded bytes + optional disk
write via save_path.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 11: Find tools (2) — `browser_find / _find_all`

**Files:**
- Create: `crates/zendriver-mcp/src/tools/find.rs`

- [ ] **Step 1: Implement `Selector::apply` + `resolve`**

```rust
//! Find tools + the Selector → FindBuilder bridge used by every action tool.

use std::sync::Arc;
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use rmcp::model::ErrorData as McpError;
use zendriver::{Tab, Element, ZendriverError};
use crate::state::SessionState;
use crate::selectors::Selector;

pub async fn resolve(tab: &Tab, sel: &Selector) -> Result<Element, McpError> {
    sel.validate().map_err(|e| McpError::invalid_request(e.to_string(), None))?;
    let mut b = tab.find();
    if let Some(s) = &sel.css        { b = b.css(s); }
    else if let Some(s) = &sel.xpath { b = b.xpath(s); }
    else if let Some(s) = &sel.text  { b = b.text(s); }
    else if let Some(s) = &sel.text_exact { b = b.text_exact(s); }
    else if let Some(s) = &sel.text_regex { b = b.text_regex(s); }
    else if let Some(s) = &sel.role  {
        match &sel.role_name {
            Some(name) => { b = b.role_named(s, name); }
            None       => { b = b.role(s); }
        }
    }
    if let Some(n) = sel.nth         { b = b.nth(n); }
    if sel.visible_only              { b = b.visible_only(); }
    b = b.timeout(std::time::Duration::from_millis(sel.timeout_ms));
    if let Some(fid) = &sel.frame_id {
        let frame = tab.frame(fid).map_err(crate::errors::map_error)?;
        b = b.in_frame(&frame);
    }
    b.one().await.map_err(|e| crate::errors::map_error(e))
}

pub async fn resolve_all(tab: &Tab, sel: &Selector, limit: usize) -> Result<Vec<Element>, McpError> {
    sel.validate().map_err(|e| McpError::invalid_request(e.to_string(), None))?;
    let mut b = tab.find_all();
    if let Some(s) = &sel.css        { b = b.css(s); }
    else if let Some(s) = &sel.xpath { b = b.xpath(s); }
    else if let Some(s) = &sel.text  { b = b.text(s); }
    else if let Some(s) = &sel.text_exact { b = b.text_exact(s); }
    else if let Some(s) = &sel.text_regex { b = b.text_regex(s); }
    else if let Some(s) = &sel.role  {
        match &sel.role_name {
            Some(name) => { b = b.role_named(s, name); }
            None       => { b = b.role(s); }
        }
    }
    if sel.visible_only { b = b.visible_only(); }
    b = b.timeout(std::time::Duration::from_millis(sel.timeout_ms));
    if let Some(fid) = &sel.frame_id {
        let frame = tab.frame(fid).map_err(crate::errors::map_error)?;
        b = b.in_frame(&frame);
    }
    let many = b.many_or_empty().await.map_err(crate::errors::map_error)?;
    Ok(many.into_iter().take(limit).collect())
}
```

Replace the `todo!` body with the duplicated configuration block. Confirm zendriver's actual `FindAllBuilder` method names (`many`, `many_or_empty`, `limit`).

- [ ] **Step 2: Write tool handlers**

```rust
#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct FindInput { #[serde(flatten)] pub selector: Selector }

#[derive(Debug, Serialize, JsonSchema)]
pub struct ElementDescriptor {
    pub tag: String,
    pub text_snippet: String,
    pub attrs: std::collections::BTreeMap<String, String>,
    pub visible: bool,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounding_box: Option<BoundingBox>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BoundingBox { pub x: f64, pub y: f64, pub width: f64, pub height: f64 }

#[derive(Debug, Serialize, JsonSchema)]
pub struct FindOutput { pub found: bool, #[serde(skip_serializing_if = "Option::is_none")] pub element: Option<ElementDescriptor> }

pub async fn find(state: Arc<Mutex<SessionState>>, input: FindInput) -> Result<FindOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    match resolve(tab, &input.selector).await {
        Ok(el)  => Ok(FindOutput { found: true,  element: Some(describe(&el).await?) }),
        Err(e)  => {
            if e.message.contains("No element matched") { Ok(FindOutput { found: false, element: None }) }
            else { Err(e) }
        }
    }
}

async fn describe(el: &Element) -> Result<ElementDescriptor, McpError> {
    let tag = el.tag_name().to_string();
    let text = el.text().await.unwrap_or_default();
    let attrs = el.attrs().await.map_err(crate::errors::map_error)?;
    let visible = el.is_visible().await.map_err(crate::errors::map_error)?;
    let enabled = el.is_enabled().await.map_err(crate::errors::map_error)?;
    let bb = el.bounding_box().await.ok();
    Ok(ElementDescriptor {
        tag,
        text_snippet: text.chars().take(200).collect(),
        attrs: attrs.into_iter().collect(),
        visible, enabled,
        bounding_box: bb.map(|b| BoundingBox { x: b.x, y: b.y, width: b.width, height: b.height }),
    })
}

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct FindAllInput { #[serde(flatten)] pub selector: Selector, #[serde(default = "default_limit")] pub limit: usize }
fn default_limit() -> usize { 50 }

pub async fn find_all(state: Arc<Mutex<SessionState>>, input: FindAllInput) -> Result<Vec<ElementDescriptor>, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let els = resolve_all(tab, &input.selector, input.limit).await?;
    let mut out = Vec::with_capacity(els.len());
    for el in els { out.push(describe(&el).await?); }
    Ok(out)
}
```

- [ ] **Step 3: Tests + register + commit**

Test the no-browser error path. Register both tools. Commit:

```bash
git commit -m "feat(mcp): find tools + Selector→FindBuilder bridge

resolve() and resolve_all() are the shared bridge from MCP Selector
to zendriver FindBuilder; every action tool calls resolve() internally.
browser_find returns one ElementDescriptor or found=false; browser_find_all
returns up to limit elements.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 12: Action tools (9)

**Files:**
- Create: `crates/zendriver-mcp/src/tools/actions.rs`

Nine tools sharing the `resolve(tab, &input.selector)` pattern + `return_snapshot: bool` arg:

`browser_click / _hover / _type / _press / _set_value / _clear / _focus / _scroll_into_view / _upload`

- [ ] **Step 1: Write each handler**

Pattern (showing `click` as the template):

```rust
use crate::tools::find::resolve;

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct ClickInput {
    #[serde(flatten)] pub selector: Selector,
    #[serde(default)] pub button: Option<MouseButtonArg>,
    #[serde(default)] pub click_count: Option<u32>,
    #[serde(default)] pub return_snapshot: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MouseButtonArg { Left, Middle, Right }

#[derive(Debug, Serialize, JsonSchema)]
pub struct ActionOutput { pub ok: bool, #[serde(skip_serializing_if = "Option::is_none")] pub snapshot: Option<String> }

pub async fn click(state: Arc<Mutex<SessionState>>, input: ClickInput) -> Result<ActionOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = resolve(tab, &input.selector).await?;
    let mut opts = zendriver::ClickOptions::default();
    if let Some(b) = input.button { opts.button = match b { MouseButtonArg::Left => zendriver::MouseButton::Left, MouseButtonArg::Middle => zendriver::MouseButton::Middle, MouseButtonArg::Right => zendriver::MouseButton::Right }; }
    if let Some(n) = input.click_count { opts.click_count = n; }
    el.click_with(opts).await.map_err(crate::errors::map_error)?;
    let snapshot = if input.return_snapshot { Some(snapshot_now(tab).await?) } else { None };
    Ok(ActionOutput { ok: true, snapshot })
}
```

The other eight follow the same skeleton:

- `hover { selector, return_snapshot }` → `el.hover().await`
- `type_ { selector, text, clear_first, return_snapshot }` → optional `el.clear().await` then `el.type_text(&text).await`
- `press { selector, key, return_snapshot }` → `el.press(&key).await`
- `set_value { selector, value, return_snapshot }` → `el.set_value(&value).await`
- `clear { selector, return_snapshot }` → `el.clear().await`
- `focus { selector }` → `el.focus().await` (no snapshot — non-visual)
- `scroll_into_view { selector }` → `el.scroll_into_view().await`
- `upload { selector, paths: Vec<String> }` → `el.upload_files(&paths).await`

- [ ] **Step 2: Tests, register, commit**

Test no-browser error for one tool (others are identical). Register all 9 in `register()`. Commit:

```bash
git commit -m "feat(mcp): action tools (9) — click/hover/type/press/set_value/clear/focus/scroll_into_view/upload

All share the Selector arg + resolve() pattern. State-changers accept
return_snapshot: bool (default false) for one-call action+observe.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 13: Read + eval + stealth + frame tools (5)

**Files:**
- Create: `crates/zendriver-mcp/src/tools/{reads,eval,stealth,frames}.rs`

Five tools, one module each.

- [ ] **Step 1: `reads.rs` — `browser_element_state`**

```rust
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadFieldsPreset { All, ExistsOnly, VisibleEnabled, Geometry, TextAttrs }

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct ElementStateInput {
    #[serde(flatten)] pub selector: Selector,
    #[serde(default = "default_preset")] pub include: ReadFieldsPreset,
}
fn default_preset() -> ReadFieldsPreset { ReadFieldsPreset::All }

#[derive(Debug, Serialize, JsonSchema)]
pub struct ElementStateOutput {
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")] pub visible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub in_viewport: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub bounding_box: Option<BoundingBox>,
    #[serde(skip_serializing_if = "Option::is_none")] pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub attrs: Option<std::collections::BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub inner_html: Option<String>,
}
```

Handler resolves the selector; if not found, returns `exists: false` only. If found, populates fields per preset:

| preset | populates |
|--------|----------|
| `all` | every field |
| `exists_only` | none beyond `exists` |
| `visible_enabled` | visible + enabled |
| `geometry` | bounding_box + in_viewport |
| `text_attrs` | text + attrs + inner_html |

- [ ] **Step 2: `eval.rs` — `browser_evaluate / _evaluate_main`**

```rust
#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct EvalInput {
    pub expression: String,
    #[serde(default = "default_await")] pub await_promise: bool,
    #[serde(default)] pub frame_id: Option<String>,
}
fn default_await() -> bool { true }

#[derive(Debug, Serialize, JsonSchema)]
pub struct EvalOutput { pub value: serde_json::Value }

pub async fn evaluate(state: Arc<Mutex<SessionState>>, input: EvalInput) -> Result<EvalOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let value: serde_json::Value = match input.frame_id {
        Some(fid) => tab.frame(&fid).map_err(crate::errors::map_error)?.evaluate(&input.expression).await,
        None      => tab.evaluate(&input.expression).await,
    }.map_err(crate::errors::map_error)?;
    Ok(EvalOutput { value })
}

pub async fn evaluate_main(state: Arc<Mutex<SessionState>>, input: EvalInput) -> Result<EvalOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let value: serde_json::Value = match input.frame_id {
        Some(fid) => tab.frame(&fid).map_err(crate::errors::map_error)?.evaluate_main(&input.expression).await,
        None      => tab.evaluate_main(&input.expression).await,
    }.map_err(crate::errors::map_error)?;
    Ok(EvalOutput { value })
}
// Register browser_evaluate_main with description: "Runs in the page's main world —
// breaks stealth isolation if used carelessly. Prefer browser_evaluate."
```

- [ ] **Step 3: `stealth.rs` — `browser_set_stealth_profile`**

```rust
#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct SetStealthInput { pub profile: StealthProfileChoice }

#[derive(Debug, Serialize, JsonSchema)]
pub struct SetStealthOutput { pub active_profile: StealthProfileChoice }

pub async fn set_stealth_profile(state: Arc<Mutex<SessionState>>, input: SetStealthInput) -> Result<SetStealthOutput, McpError> {
    let mut s = state.lock().await;
    s.stealth_profile_choice = input.profile;
    // Applying live to an open browser may require a fresh tab — documented in tool description.
    Ok(SetStealthOutput { active_profile: input.profile })
}
```

- [ ] **Step 4: `frames.rs` — `browser_frame_list`**

```rust
#[derive(Debug, Serialize, JsonSchema)]
pub struct FrameSummary {
    pub id: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub is_oopif: bool,
}

pub async fn frame_list(state: Arc<Mutex<SessionState>>, _: EmptyInput) -> Result<Vec<FrameSummary>, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let frames = tab.frames();
    Ok(frames.iter().map(|f| FrameSummary {
        id: f.frame_id().to_string(),
        url: f.url().unwrap_or_default(),
        parent_id: f.parent_id().map(|s| s.to_string()),
        is_oopif: f.is_oopif(),
    }).collect())
}
```

- [ ] **Step 5: Tests, register, commit**

One no-browser-error test per module. Register each. Commit:

```bash
git commit -m "feat(mcp): reads/eval/stealth/frames tools (5)

element_state collapses 5+ reads into one tool with a ReadFieldsPreset
arg. evaluate + evaluate_main expose isolated + main-world eval (main
warns about stealth). set_stealth_profile swaps the active profile.
frame_list returns frame summaries (frame-scoped find/eval go via the
frame_id arg on existing tools).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 14: Cookie tools (5) — `cookies_get / _set / _delete / _clear / _persist`

**Files:**
- Create: `crates/zendriver-mcp/src/tools/cookies.rs`

- [ ] **Step 1: Implement tools**

```rust
#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct CookiesGetInput {
    #[serde(default)] pub url: Option<String>,
    #[serde(default)] pub name: Option<String>,
}

pub async fn cookies_get(state: Arc<Mutex<SessionState>>, input: CookiesGetInput) -> Result<Vec<zendriver::Cookie>, McpError> {
    let s = state.lock().await;
    let b = s.browser.as_ref().ok_or_else(|| crate::errors::map_error(ZendriverError::BrowserNotOpen))?;
    let jar = b.cookies();
    let cs = match input.url {
        Some(u) => jar.for_url(&u).await,
        None    => jar.all().await,
    }.map_err(crate::errors::map_error)?;
    Ok(match input.name {
        Some(n) => cs.into_iter().filter(|c| c.name == n).collect(),
        None    => cs,
    })
}

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct CookiesSetInput { pub cookies: Vec<zendriver::CookieParams> }

#[derive(Debug, Serialize, JsonSchema)] pub struct CookiesSetOutput { pub added: usize }

pub async fn cookies_set(state: Arc<Mutex<SessionState>>, input: CookiesSetInput) -> Result<CookiesSetOutput, McpError> {
    let s = state.lock().await;
    let b = s.browser.as_ref().ok_or_else(|| crate::errors::map_error(ZendriverError::BrowserNotOpen))?;
    b.cookies().set_all(&input.cookies).await.map_err(crate::errors::map_error)?;
    Ok(CookiesSetOutput { added: input.cookies.len() })
}

// browser_cookies_delete { name, url } → b.cookies().delete(&name, url.as_deref()).await
// browser_cookies_clear           → b.cookies().clear().await
// browser_cookies_persist { direction, path }
//   "save" → b.cookies().save_to_file(&path).await
//   "load" → b.cookies().load_from_file(&path).await
```

- [ ] **Step 2: Tests, register, commit**

```bash
git commit -m "feat(mcp): cookie tools — get/set/delete/clear/persist

cookies_persist takes direction=\"save\"|\"load\" + path so the LLM
can checkpoint and restore auth state across sessions.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 15: Storage tools (4) — `storage_get / _set / _delete / _clear`

**Files:**
- Create: `crates/zendriver-mcp/src/tools/storage.rs`

- [ ] **Step 1: Implement tools**

```rust
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind { Local, Session }

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct StorageGetInput { pub kind: StorageKind, #[serde(default)] pub key: Option<String> }

#[derive(Debug, Serialize, JsonSchema)]
pub struct StorageGetOutput { pub values: std::collections::BTreeMap<String, String> }

pub async fn storage_get(state: Arc<Mutex<SessionState>>, input: StorageGetInput) -> Result<StorageGetOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let store = match input.kind { StorageKind::Local => tab.local_storage(), StorageKind::Session => tab.session_storage() };
    let values = match input.key {
        Some(k) => {
            let v = store.get(&k).await.map_err(crate::errors::map_error)?;
            v.map(|v| std::iter::once((k, v)).collect()).unwrap_or_default()
        }
        None => store.all().await.map_err(crate::errors::map_error)?.into_iter().collect(),
    };
    Ok(StorageGetOutput { values })
}

// browser_storage_set     { kind, key, value }
// browser_storage_delete  { kind, key }
// browser_storage_clear   { kind }
```

- [ ] **Step 2: Tests, register, commit**

```bash
git commit -m "feat(mcp): storage tools — local/session get/set/delete/clear

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 16: Gated interception tools (4) — `cfg(feature = "interception")`

**Files:**
- Create: `crates/zendriver-mcp/src/tools/intercept.rs`
- Modify: `crates/zendriver-mcp/src/tools/mod.rs`

- [ ] **Step 1: Implement**

```rust
//! Interception tools — gated behind `interception` feature.

#![cfg(feature = "interception")]

use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use crate::state::{SessionState, RuleId, InterceptRuleHandle};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum InterceptAction {
    Block,
    Redirect { to: String },
    Respond { status: u16, body: String, #[serde(default)] headers: std::collections::BTreeMap<String, String> },
    ModifyRequest { #[serde(default)] headers: std::collections::BTreeMap<String, String> },
}

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct AddRuleInput { pub pattern: String, pub action: InterceptAction }

#[derive(Debug, Serialize, JsonSchema)] pub struct AddRuleOutput { pub rule_id: RuleId }

pub async fn add_rule(state: Arc<Mutex<SessionState>>, input: AddRuleInput) -> Result<AddRuleOutput, rmcp::model::ErrorData> {
    let mut s = state.lock().await;
    let b = s.browser.as_ref().ok_or_else(|| crate::errors::map_error(zendriver::ZendriverError::BrowserNotOpen))?;
    let kind = match &input.action {
        InterceptAction::Block             => "block",
        InterceptAction::Redirect { .. }   => "redirect",
        InterceptAction::Respond { .. }    => "respond",
        InterceptAction::ModifyRequest { ..} => "modify_request",
    }.to_string();
    // Real zendriver-interception install:
    // let guard = b.interception().add_rule(&input.pattern, input.action.into_zendriver()).await?;
    let id: RuleId = Uuid::new_v4().to_string();
    s.rules.insert(id.clone(), InterceptRuleHandle { pattern: input.pattern, action_kind: kind });
    Ok(AddRuleOutput { rule_id: id })
}

// browser_intercept_remove_rule  { rule_id } → s.rules.remove + drop guard
// browser_intercept_list_rules                → enumerate s.rules
// browser_intercept_clear_rules                → s.rules.clear()
```

Confirm the zendriver-interception API surface (`InterceptAction`, `RuleGuard`, etc.) and adapt the `// Real zendriver-interception install:` line.

- [ ] **Step 2: Add `uuid` dep**

```toml
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 3: cfg-gate registration in `tools/mod.rs`**

```rust
pub fn register_all(server: &mut rmcp::Server, state: Arc<Mutex<SessionState>>) {
    crate::tools::lifecycle::register(server, state.clone());
    // ... navigation, tabs, snapshot, find, actions, reads, eval, stealth, frames, cookies, storage ...
    #[cfg(feature = "interception")] crate::tools::intercept::register(server, state.clone());
    #[cfg(feature = "expect")]       crate::tools::expect::register(server, state.clone());
    #[cfg(feature = "cloudflare")]   crate::tools::cloudflare::register(server, state.clone());
    #[cfg(feature = "fetcher")]      crate::tools::fetcher::register(server, state.clone());
}
```

- [ ] **Step 4: Test + commit**

```bash
cargo test -p zendriver-mcp --lib intercept --features interception -- --nocapture
cargo build -p zendriver-mcp --no-default-features --features "stealth interception" --locked
git add -A
git commit -m "feat(mcp): interception tools (gated) — add/remove/list/clear rules

Behind cargo feature 'interception'. Stream-based escape hatch is
deliberately not exposed (out of scope per spec). Registered only when
feature is enabled.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 17: Gated expect tools (3) — `cfg(feature = "expect")`

**Files:**
- Create: `crates/zendriver-mcp/src/tools/expect.rs`

- [ ] **Step 1: Implement register / await / cancel**

```rust
#![cfg(feature = "expect")]

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpectKind { Request, Response, Dialog, Download }

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpectMatcher {
    #[serde(default)] pub url_substr: Option<String>,
    #[serde(default)] pub url_regex: Option<String>,
    #[serde(default)] pub status_min: Option<u16>,
    #[serde(default)] pub status_max: Option<u16>,
}

pub async fn register_expectation(state: Arc<Mutex<SessionState>>, input: RegisterInput) -> Result<RegisterOutput, McpError> {
    let mut s = state.lock().await;
    let b = s.browser.as_ref().ok_or_else(|| crate::errors::map_error(ZendriverError::BrowserNotOpen))?;
    let tab = current_tab_local(&s)?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    // Spawn a task that listens to tab events matching input.matcher and forwards to tx.
    // Exact zendriver subscription API: tab.events().on::<RequestEvent>(...) etc.
    let id = uuid::Uuid::new_v4().to_string();
    s.expectations.insert(id.clone(), ExpectationHandle { kind: format!("{:?}", input.kind), buffer: rx });
    Ok(RegisterOutput { expectation_id: id })
}

pub async fn await_expectation(state: Arc<Mutex<SessionState>>, input: AwaitInput) -> Result<AwaitOutput, McpError> {
    let mut s = state.lock().await;
    let handle = s.expectations.remove(&input.expectation_id)
        .ok_or_else(|| McpError::invalid_request(format!("No expectation `{}`", input.expectation_id), None))?;
    // Need to await on handle.buffer while holding state lock — split into a non-locked block instead.
    drop(s);
    let mut rx = handle.buffer;
    let event = tokio::time::timeout(std::time::Duration::from_millis(input.timeout_ms), rx.recv()).await
        .map_err(|_| McpError::invalid_request("expectation timed out".into(), None))?
        .ok_or_else(|| McpError::invalid_request("expectation channel closed".into(), None))?;
    Ok(AwaitOutput { event })
}

// browser_expect_cancel { expectation_id } → s.expectations.remove(&id)
```

Buffering details (when to start, how to handle drop) come from zendriver's expect implementation — reuse the same dispatcher and adapt for MCP's request/response model. This task is the most novel; expect to spend extra time on the event-channel plumbing.

- [ ] **Step 2: Test + commit**

```bash
git commit -m "feat(mcp): expect tools (gated) — register/await/cancel pattern

MCP request/response can't model the lib's await-with-guard pattern in
one call, so we split into three tools. Server buffers matching events
from registration onward so 'register → do action → await' doesn't race.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 18: Gated cloudflare tool (1) — `cfg(feature = "cloudflare")`

**Files:**
- Create: `crates/zendriver-mcp/src/tools/cloudflare.rs`

- [ ] **Step 1: Implement**

```rust
#![cfg(feature = "cloudflare")]

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct SolveInput { #[serde(default = "default_timeout")] pub timeout_ms: u64 }
fn default_timeout() -> u64 { 30_000 }

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome { Solved, ChallengeGone, Timeout }

#[derive(Debug, Serialize, JsonSchema)]
pub struct SolveOutput { pub outcome: Outcome }

pub async fn solve_turnstile(state: Arc<Mutex<SessionState>>, input: SolveInput) -> Result<SolveOutput, McpError> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let outcome = zendriver_cloudflare::solve_turnstile(tab, std::time::Duration::from_millis(input.timeout_ms)).await
        .map_err(crate::errors::map_error)?;
    Ok(SolveOutput { outcome: match outcome {
        zendriver_cloudflare::ClearanceOutcome::Solved        => Outcome::Solved,
        zendriver_cloudflare::ClearanceOutcome::ChallengeGone => Outcome::ChallengeGone,
        zendriver_cloudflare::ClearanceOutcome::Timeout       => Outcome::Timeout,
    }})
}
```

- [ ] **Step 2: Test + commit**

```bash
git commit -m "feat(mcp): cloudflare tool (gated) — solve_turnstile

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 19: Gated fetcher tools (2) — `cfg(feature = "fetcher")`

**Files:**
- Create: `crates/zendriver-mcp/src/tools/fetcher.rs`

- [ ] **Step 1: Implement install + list**

```rust
#![cfg(feature = "fetcher")]

#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
pub struct InstallInput {
    #[serde(default)] pub version: Option<String>,
    #[serde(default)] pub channel: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InstallOutput { pub path: String, pub version: String }

pub async fn install_chrome(_state: Arc<Mutex<SessionState>>, input: InstallInput) -> Result<InstallOutput, McpError> {
    let mut f = zendriver_fetcher::Fetcher::builder();
    if let Some(v) = input.version { f = f.version(&v); }
    if let Some(c) = input.channel { f = f.channel(&c); }
    let res = f.build().ensure_chrome().await.map_err(crate::errors::map_error)?;
    Ok(InstallOutput { path: res.path.display().to_string(), version: res.version })
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InstalledEntry { pub path: String, pub version: String, pub channel: Option<String> }

pub async fn list_installed(_: Arc<Mutex<SessionState>>, _: EmptyInput) -> Result<Vec<InstalledEntry>, McpError> {
    let entries = zendriver_fetcher::cache::list().await.map_err(crate::errors::map_error)?;
    Ok(entries.into_iter().map(|e| InstalledEntry { path: e.path.display().to_string(), version: e.version, channel: e.channel }).collect())
}
```

- [ ] **Step 2: Test + commit**

```bash
git commit -m "feat(mcp): fetcher tools (gated) — install + list

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 20: Stdio + HTTP end-to-end smoke tests

**Files:**
- Create: `crates/zendriver-mcp/tests/stdio_smoke.rs`
- Create: `crates/zendriver-mcp/tests/http_smoke.rs`

- [ ] **Step 1: stdio smoke test**

`tests/stdio_smoke.rs`:

```rust
//! Spin up the server binary over stdio with a real rmcp client and round-trip
//! a tools/list + browser_status call. Browser is never opened so no real Chrome.

use std::process::Stdio;

#[tokio::test]
async fn list_tools_and_call_status() {
    let bin = env!("CARGO_BIN_EXE_zendriver-mcp");
    let mut child = tokio::process::Command::new(bin)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    let mut client = rmcp::client::Client::connect_stdio(&mut child).await.unwrap();
    let tools = client.list_tools().await.unwrap();
    assert!(tools.iter().any(|t| t.name == "browser_status"));
    let status = client.call_tool("browser_status", serde_json::json!({})).await.unwrap();
    assert_eq!(status.structured["open"], false);
    child.kill().await.unwrap();
}
```

Adapt exact rmcp client API as needed. The shape (list_tools → call_tool → assert) is universal.

- [ ] **Step 2: HTTP smoke test**

`tests/http_smoke.rs`: same skeleton, but spawn with `--http 127.0.0.1:0` (bind random port; read it from stderr "listening on..." log line) and use rmcp's HTTP client.

- [ ] **Step 3: Run + commit**

```bash
cargo test -p zendriver-mcp --test stdio_smoke -- --nocapture
cargo test -p zendriver-mcp --test http_smoke  -- --nocapture
git add -A
git commit -m "test(mcp): stdio + HTTP smoke tests

Round-trip tools/list + browser_status (no real Chrome) over both
transports. Confirms transport wiring, registration, and basic
request/response flow.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 21: Tool schema snapshot tests

**Files:**
- Create: `crates/zendriver-mcp/tests/schema_snapshots.rs`

- [ ] **Step 1: Snapshot every tool's input + output JSON schema**

```rust
use schemars::schema_for;
use insta::assert_yaml_snapshot;

#[test]
fn snapshot_browser_open_input() {
    assert_yaml_snapshot!("browser_open_input", schema_for!(zendriver_mcp::tools::lifecycle::OpenInput));
}

// One pair (input + output) per tool. ~50 input + 50 output = ~100 snapshot tests.
// Generate them by iterating the tool list.
```

For ergonomics, factor a macro:

```rust
macro_rules! schema_snap {
    ($name:literal, $ty:ty) => {
        #[test]
        fn $name() {
            assert_yaml_snapshot!($name, schemars::schema_for!($ty));
        }
    }
}
```

Then list every input/output type. First run creates pending snapshots; review and accept with `cargo insta accept`.

- [ ] **Step 2: First run + accept**

```bash
cargo test -p zendriver-mcp --test schema_snapshots -- --nocapture
cargo insta accept --all
```

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test(mcp): schema snapshots for every tool input/output

Captures the JSON schema for every Deserialize/Serialize input and
output type. Future schema breakage will fail this test; reviewer
either accepts the diff with cargo insta accept or fixes the regression.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 22: Real-Chrome integration tests + evaluation XML

**Files:**
- Create: `crates/zendriver-mcp/tests/integration/{lifecycle,navigation,find,screenshot}.rs`
- Create: `crates/zendriver-mcp/tests/evaluations/eval_set.xml`

- [ ] **Step 1: Integration test — full lifecycle against real Chrome**

`tests/integration/lifecycle.rs`:

```rust
#![cfg(feature = "integration-tests")]

#[tokio::test]
async fn open_goto_status_close() {
    let state = std::sync::Arc::new(tokio::sync::Mutex::new(zendriver_mcp::state::SessionState::new()));
    let out = zendriver_mcp::tools::lifecycle::open(state.clone(), zendriver_mcp::tools::lifecycle::OpenInput { headless: true, stealth_profile: None }).await.unwrap();
    assert!(!out.chrome_version.is_empty());
    let _nav = zendriver_mcp::tools::navigation::goto(state.clone(), zendriver_mcp::tools::navigation::GotoInput { url: "https://example.com".into(), wait_for: zendriver_mcp::tools::navigation::WaitFor::Load, return_snapshot: false }).await.unwrap();
    let status = zendriver_mcp::tools::lifecycle::status(state.clone(), zendriver_mcp::tools::lifecycle::EmptyInput {}).await.unwrap();
    assert_eq!(status.current_tab.unwrap().title, "Example Domain");
    let _close = zendriver_mcp::tools::lifecycle::close(state, zendriver_mcp::tools::lifecycle::EmptyInput {}).await.unwrap();
}
```

Add one similar test per category (navigation/find/screenshot/cookies).

- [ ] **Step 2: Evaluation XML**

`tests/evaluations/eval_set.xml`:

```xml
<evaluation>
  <qa_pair>
    <question>Open example.com and report the page title.</question>
    <answer>Example Domain</answer>
  </qa_pair>
  <qa_pair>
    <question>On https://www.rust-lang.org/, what is the text of the first H1?</question>
    <answer>Rust</answer>
  </qa_pair>
  <qa_pair>
    <question>On example.com, count the number of &lt;a&gt; elements via browser_find_all with css=a.</question>
    <answer>1</answer>
  </qa_pair>
  <qa_pair>
    <question>On https://httpbin.org/forms/post, what ARIA role does the &quot;Customer name&quot; input have?</question>
    <answer>textbox</answer>
  </qa_pair>
  <qa_pair>
    <question>On https://httpbin.org/cookies/set/foo/bar, after the request completes, list the cookie names with browser_cookies_get.</question>
    <answer>foo</answer>
  </qa_pair>
  <qa_pair>
    <question>After calling browser_storage_set local k v on example.com, what value does browser_storage_get local k return?</question>
    <answer>v</answer>
  </qa_pair>
  <qa_pair>
    <question>Navigate to example.com then to https://www.iana.org/, then call browser_back — what URL is current?</question>
    <answer>https://example.com/</answer>
  </qa_pair>
  <qa_pair>
    <question>On example.com, what is the computed lang attribute on the &lt;html&gt; element (via browser_evaluate &quot;document.documentElement.lang&quot;)?</question>
    <answer></answer>
  </qa_pair>
  <qa_pair>
    <question>Take a PNG screenshot of example.com with browser_screenshot. Is the returned bytes_base64 non-empty?</question>
    <answer>true</answer>
  </qa_pair>
  <qa_pair>
    <question>On https://example.com, what is the value of browser_element_state with css=h1 and include=text_attrs for the text field?</question>
    <answer>Example Domain</answer>
  </qa_pair>
</evaluation>
```

- [ ] **Step 3: Run + commit**

```bash
cargo test -p zendriver-mcp --features integration-tests --test integration -- --nocapture
git add -A
git commit -m "test(mcp): real-Chrome integration tests + evaluation set

Behind feature flag integration-tests (mirrors lib pattern). Eval XML
holds 10 stable read-only questions for the mcp-builder evaluation
process.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 23: mdBook chapter + README badges

**Files:**
- Modify: `docs/book/src/SUMMARY.md`
- Create: `docs/book/src/mcp.md`
- Modify: `README.md`

- [ ] **Step 1: Add chapter to SUMMARY**

In `docs/book/src/SUMMARY.md`, append under the appropriate section:

```markdown
- [MCP server](./mcp.md)
```

- [ ] **Step 2: Write `mcp.md`**

```markdown
# MCP server (`zendriver-mcp`)

`zendriver-mcp` is a Model Context Protocol server that wraps
zendriver-rs. Any MCP-compatible client (Claude Desktop, claude-code,
custom agents) can drive a real Chrome browser through ~51 tools.

## Install

```bash
cargo install zendriver-mcp
```

## Claude Desktop

```json
{
  "mcpServers": {
    "zendriver": {
      "command": "zendriver-mcp"
    }
  }
}
```

## HTTP mode

```bash
zendriver-mcp --http 127.0.0.1:8765
```

Bind localhost only by default; expose via reverse proxy + mTLS / network
policy for remote access.

## Tool reference

[Full table — see spec for now; regenerate from `schema_snapshots.rs` for the
docs build.]

## Stealth

`--stealth-profile` selects the default profile (auto / native /
spoof_macos / spoof_linux / spoof_windows). `browser_set_stealth_profile`
swaps live.

## Troubleshooting

- `tracing` logs go to stderr (never stdout — stdin/stdout are reserved for
  the MCP protocol on stdio).
- Common errors include actionable `suggested_next` hints in `_metadata`.
- Use `--log debug` for verbose CDP-call logging.
```

- [ ] **Step 3: README badge**

In `README.md`, add to the badge row:

```markdown
[![MCP](https://img.shields.io/badge/MCP-server-blue)](https://turtiesocks.github.io/zendriver-rs/mcp.html)
```

And add a "MCP server" row in the feature matrix table referencing the chapter.

- [ ] **Step 4: Build mdBook + verify links**

```bash
(cd docs/book && mdbook build)
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs(mcp): mdBook chapter + README badge for zendriver-mcp

Install snippet, Claude Desktop config, HTTP mode notes, stealth
overview, troubleshooting.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 24: Workspace publish prep

**Files:**
- Modify: `crates/zendriver-mcp/Cargo.toml`
- Modify: `scripts/publish.sh` (or whatever the actual publish workflow is — `.github/workflows/release.yml` per recent commit `f20a7b8`)
- Modify: CHANGELOG.md

- [ ] **Step 1: Confirm metadata**

Ensure `crates/zendriver-mcp/Cargo.toml` has:

- `description`, `keywords`, `categories`, `repository`, `homepage`, `documentation`
- `rust-version.workspace = true`
- No `publish = false`

- [ ] **Step 2: Add to publish order**

`zendriver-mcp` depends on `zendriver`. Topological order in the publish workflow:

```
zendriver-transport
zendriver-stealth
zendriver-interception
zendriver-fetcher
zendriver-cloudflare
zendriver
zendriver-mcp        # NEW — publish last
```

If the publish workflow is `.github/workflows/release.yml`, update its crate-list step.

- [ ] **Step 3: CHANGELOG entry**

Under the next-release heading, add:

```markdown
### Added

- `zendriver-mcp` — Model Context Protocol server exposing zendriver-rs as
  ~51 MCP tools over stdio + streamable HTTP. See the [MCP chapter](https://turtiesocks.github.io/zendriver-rs/mcp.html).
```

- [ ] **Step 4: Final sanity check**

```bash
cargo build --workspace --all-features --locked
cargo test  --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
cargo doc --workspace --all-features --no-deps
```

All green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore(mcp): publish-workflow + CHANGELOG entries for zendriver-mcp

Added to topological publish order after zendriver. CHANGELOG calls
out the new crate under Added.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Post-plan checks

- All 51 tools registered and callable via `tools/list` (verified by Task 20 smoke test).
- All gated tools wired only when feature is enabled (verified by `cargo build --no-default-features --features stealth` succeeding without any gated symbols).
- 10-question evaluation XML present at `tests/evaluations/eval_set.xml` (Task 22).
- README badge + mdBook chapter live (Task 23).
- Workflow publishes `zendriver-mcp` after `zendriver` (Task 24).

After Task 24 merges, the next workspace release cuts `zendriver-mcp` 0.1.0 to crates.io.
