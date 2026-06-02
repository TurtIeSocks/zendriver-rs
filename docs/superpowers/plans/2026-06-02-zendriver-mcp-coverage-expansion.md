# zendriver-mcp Coverage Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Raise `zendriver-mcp` from 49 to 65 tools, closing every Tier 1/2/3 coverage gap in the audit so only documented non-goals remain uncovered.

**Architecture:** Each tool is a free async fn in `crates/zendriver-mcp/src/tools/<group>.rs` taking `Arc<Mutex<SessionState>>` + a typed `*Input`, returning a typed `*Output` (or `ErrorData`). A thin `#[tool]` wrapper in `server.rs` delegates to it. Feature-gated tools live in their own `#[tool_router]` block summed into `combined_tool_router()`. Every input/output type gets an `insta` schema snapshot; every tool gets a no-browser unit test + a gated real-Chrome integration test.

**Tech Stack:** Rust 2024, rmcp 1.7 (`#[tool]`/`#[tool_router]`), schemars 1.0, serde, tokio, insta. Wraps the `zendriver` crate + feature sub-crates (`-imperva`, `-interception`, `-stealth`, `-fetcher`).

---

## Shared recipe (every tool task follows this — do NOT re-paste boilerplate)

For a tool `browser_X` wrapping `zendriver` call(s) `C`:

1. **Module** — in `tools/<group>.rs`: define `#[derive(Debug, Deserialize, JsonSchema)] #[serde(deny_unknown_fields)] pub struct XInput { … }` and `#[derive(Debug, Serialize, JsonSchema)] pub struct XOutput { … }`. Write `pub async fn x(state: Arc<Mutex<SessionState>>, input: XInput) -> Result<XOutput, ErrorData>` that does `let s = state.lock().await; let tab = current_tab(&s).await?;` then calls `C`, mapping lib errors via `.map_err(|e| map_error(McpServerError::from(e)))?`.
2. **Register** — in `server.rs`, add a `#[tool(name = "browser_X", description = "…")] pub async fn browser_X(&self, Parameters(input): Parameters<group::XInput>) -> Result<Json<group::XOutput>, ErrorData> { group::x(self.state.clone(), input).await.map(Json) }` to the correct router impl block (base, or the feature-gated block).
3. **Snapshot** — in `tests/schema_snapshots.rs`, add `schema_snap!(group_x_in, tools::group::XInput);` + `schema_snap!(group_x_out, tools::group::XOutput);` (+ any new enums).
4. **Unit test** — in the module's `#[cfg(test)] mod tests`, add `x_with_no_browser_errors` asserting `err.message.contains("Browser not open")` (mirror `cloudflare.rs:151`). Skip for tools that don't need a tab.
5. **Integration test** — add/extend `tests/integration_<group>.rs` with a `#[tokio::test] #[ignore]` real-Chrome exercise gated by `feature = "integration-tests"`.

New module files also need: `pub mod <group>;` in `tools/mod.rs`; the trailing `current_tab` import + `use` lines mirroring `cloudflare.rs:28-39`.

**Snapshot regen after each phase:** `cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked` then `cargo insta accept --all`.

**Build gate after each task:** `cargo build -p zendriver-mcp --all-features` must pass. **Unit-test gate after each phase:** `cargo test -p zendriver-mcp --all-features --locked` (non-ignored) green.

---

## Phase A — Tier 1 (zero-tool subsystems)

### Task A1: `imperva` feature + module + `browser_solve_imperva`

**Files:**
- Modify: `crates/zendriver-mcp/Cargo.toml` (features)
- Create: `crates/zendriver-mcp/src/tools/imperva.rs`
- Modify: `crates/zendriver-mcp/src/tools/mod.rs`, `src/server.rs`
- Modify: `tests/schema_snapshots.rs`
- Create: `tests/integration_imperva.rs`

- [ ] **Step 1:** `Cargo.toml` — add `imperva = ["zendriver/imperva"]` under `[features]` and add `"imperva"` to the `default = [...]` array.
- [ ] **Step 2:** Create `tools/imperva.rs` modeled exactly on `tools/cloudflare.rs`. Gate with `#![cfg(feature = "imperva")]`. Wraps `tab.imperva()` → `ImpervaBypass`:
  - `SolveImpervaInput { #[serde(default = "default_timeout")] timeout_ms: u64 (default 30_000), #[serde(default, skip_serializing_if="Option::is_none")] poll_interval_ms: Option<u64>, #[serde(default)] with_interception: bool }`.
  - `Outcome` enum (snake_case): `TokenAcquired`, `ChallengeGone`, `AlreadyClear`, `Timeout`.
  - `SolveImpervaOutput { outcome: Outcome, #[serde(skip_serializing_if="Option::is_none")] reese84: Option<String> }`.
  - Body: `let mut bypass = tab.imperva(); bypass = bypass.timeout(Duration::from_millis(input.timeout_ms)); if let Some(p)=input.poll_interval_ms { bypass = bypass.poll_interval(Duration::from_millis(p)); } if input.with_interception { bypass = bypass.with_interception(); }` then match `bypass.wait_for_clearance().await`: `Ok(ClearanceOutcome::TokenAcquired{reese84,..}) => Solved+Some(reese84)`, `Ok(ChallengeGone)=>ChallengeGone`, `Ok(AlreadyClear)=>AlreadyClear`, `Err(ImpervaError::…Timeout)=>Timeout` (collapse like cloudflare), `Err(other)=>Err(map_error(McpServerError::from(ZendriverError::from(other))))`. Confirm the exact timeout variant name in `zendriver-imperva/src/error.rs` and the `ImpervaError→ZendriverError` `From` impl before finalizing.
  - Imports: `use zendriver::{ImpervaBypass?, …}` — imperva re-exports are surfaced via `zendriver` under `#[cfg(feature="imperva")]` (mirror how `ClearanceOutcome`/`CloudflareError` are imported in cloudflare.rs; the imperva `ClearanceOutcome` is a *different* type, so alias if both ever co-import).
- [ ] **Step 3:** `tools/mod.rs` — add `#[cfg(feature = "imperva")] pub mod imperva;` (alongside the cloudflare line).
- [ ] **Step 4:** `server.rs` — add a gated block mirroring the cloudflare one:
  ```
  #[cfg(feature = "imperva")]
  #[tool_router(router = imperva_tool_router, vis = "pub")]
  impl ZendriverServer { #[tool(name="browser_solve_imperva", description="Drive the Imperva/Incapsula clearance flow on the current tab. Polls until token_acquired (reese84 captured), challenge_gone, already_clear, or timeout (deadline elapsed — not an error). Errors only on structural failures (CDP/JS). `with_interception` enables the Fetch fast-path.")] pub async fn browser_solve_imperva(&self, Parameters(input): Parameters<imperva::SolveImpervaInput>) -> Result<Json<imperva::SolveImpervaOutput>, ErrorData> { imperva::solve_imperva(self.state.clone(), input).await.map(Json) } }
  ```
  Add `use crate::tools::imperva;` under `#[cfg(feature = "imperva")]`. In `combined_tool_router()` add `#[cfg(feature = "imperva")] let router = router + Self::imperva_tool_router();`.
- [ ] **Step 5:** Snapshots — `tests/schema_snapshots.rs`: new gated `mod imperva_snaps { schema_snap!(imperva_solve_in, tools::imperva::SolveImpervaInput); schema_snap!(imperva_outcome, tools::imperva::Outcome); schema_snap!(imperva_solve_out, tools::imperva::SolveImpervaOutput); }` under `#[cfg(feature = "imperva")]`.
- [ ] **Step 6:** Unit test `solve_imperva_with_no_browser_errors` (mirror cloudflare). Integration test `tests/integration_imperva.rs` — `#[ignore]`, gated `integration-tests + imperva`, hits a known Imperva-protected URL and asserts a non-error outcome.
- [ ] **Step 7:** `cargo build -p zendriver-mcp --all-features` → green. Commit `feat(mcp): browser_solve_imperva + imperva feature`.

### Task A2: `browser_scroll`

**Files:** Create `tools/scroll.rs`; modify `tools/mod.rs`, `server.rs`, `tests/schema_snapshots.rs`; extend `tests/integration_actions.rs`.

- [ ] **Step 1:** `tools/scroll.rs` (not gated). Wraps `Tab::scroll_with(ScrollOptions)` (fallback `scroll_down`/`scroll_up`).
  - `ScrollInput { #[serde(default)] dx: f64, #[serde(default)] dy: f64, #[serde(default)] return_snapshot: bool }` (positive `dy` = down). Map to `ScrollOptions`; check `tab.rs:112` for `ScrollOptions` field names — use them directly.
  - `ScrollOutput { scroll_x: f64, scroll_y: f64 }` (read post-scroll via `tab.evaluate_main("[scrollX,scrollY]")` or whatever `scroll_with` returns; if it returns `()`, read scroll offsets with a one-line eval). Reuse `navigation::snapshot_now` pattern if `return_snapshot`.
- [ ] **Step 2-5:** Register `browser_scroll` in base router; snapshot `scroll_in`/`scroll_out`; no-browser unit test; integration test scrolls a tall page and asserts `scroll_y > 0`. Build. Commit.

> **Naming note:** `tools::actions` already exports a `ScrollInput` (for `browser_scroll_into_view`, snapshot line `actions_scroll_in`). Name the new type `tools::scroll::PageScrollInput`/`PageScrollOutput` to avoid collision, and snapshot as `scroll_page_in`/`scroll_page_out`.

### Task A3: `browser_get_window` + `browser_set_window`

**Files:** Create `tools/window.rs`; modify `tools/mod.rs`, `server.rs`, snapshots; create `tests/integration_window.rs`.

- [ ] **Step 1:** `tools/window.rs`. Wraps `Tab::window_bounds` / `set_window_bounds` / `set_window_size` / `maximize` / `minimize` / `fullscreen` (window.rs:187-325).
  - `WindowBoundsDto { left: i64, top: i64, width: i64, height: i64, state: WindowStateDto }` where `WindowStateDto` (snake_case enum) mirrors `zendriver::WindowState` variants (read window.rs:42).
  - `browser_get_window` — `EmptyInput`-style (no args) → `WindowBoundsDto` from `tab.window_bounds()`. `readOnly`.
  - `SetWindowInput { mode: SetWindowMode, width?: i64, height?: i64, left?: i64, top?: i64, state?: WindowStateDto }`; `SetWindowMode` enum: `bounds|size|maximize|minimize|fullscreen`. Dispatch to the matching `Tab` method; for `bounds` build a `WindowBounds` (window.rs:83). Output: resulting `WindowBoundsDto`.
- [ ] **Step 2-7:** Register both in base router; snapshots for the DTOs + inputs/outputs; unit tests; integration test sets 800×600 then asserts `get_window` reports it. Build. Commit.

### Task A4: `browser_pdf` + `browser_save_mhtml`

**Files:** Create `tools/pdf.rs`; modify `tools/mod.rs`, `server.rs`, snapshots; add `binary_output` helper to `tools/common.rs`; create `tests/integration_pdf.rs`.

- [ ] **Step 1:** Add to `tools/common.rs`: a `BlobOutput { #[serde(skip_serializing_if="Option::is_none")] saved_path: Option<String>, byte_len: usize, #[serde(skip_serializing_if="Option::is_none")] base64: Option<String> }` struct + `pub fn blob_output(bytes: Vec<u8>, save_path: Option<String>) -> Result<BlobOutput, ErrorData>` that, when `save_path` set, writes bytes to disk (`std::fs::write`) and returns `saved_path`+`byte_len` (no base64); else base64-encodes when `bytes.len() <= MAX_INLINE` (e.g. 5 MiB) and otherwise errors with a "set save_path for large blobs" message. Pull in a base64 dep if not present (check screenshot's current handling first — reuse whatever it uses; `snapshot.rs` already builds an image content block, inspect it for the encoder).
- [ ] **Step 2:** `tools/pdf.rs`:
  - `PdfInput { landscape?: bool, print_background?: bool, scale?: f64, paper_width?: f64, paper_height?: f64, margin_top?: f64, margin_bottom?: f64, margin_left?: f64, margin_right?: f64, page_ranges?: String, prefer_css_page_size?: bool, save_path?: String }` — all optional, each maps to the matching `PdfBuilder` method (pdf/mod.rs:115-313) only when `Some`. Terminal `.bytes()`.
  - `browser_pdf` → `blob_output(bytes, input.save_path)`. `readOnly:true`.
  - `browser_save_mhtml` — `SaveMhtmlInput { save_path?: String }` → `tab.snapshot_mhtml()` (returns `String`) → `blob_output(s.into_bytes(), save_path)`. `readOnly:true`.
- [ ] **Step 3-7:** Register both (base router); snapshots; unit tests; integration test exports example.com → asserts `byte_len > 0` and PDF magic bytes when saved. Build. Commit.

### Task A5: `browser_mouse`

**Files:** Create `tools/mouse.rs`; modify `tools/mod.rs`, `server.rs`, snapshots; extend `tests/integration_actions.rs`.

- [ ] **Step 1:** `tools/mouse.rs`. Wraps `Tab::mouse_move`/`mouse_click_with`/`mouse_drag`.
  - `MouseInput { action: MouseAction, x: f64, y: f64, to_x?: f64, to_y?: f64, button?: MouseButtonArg, click_count?: u32, modifiers?: Vec<ModifierArg>, #[serde(default = "default_steps")] steps: usize, #[serde(default)] return_snapshot: bool }`. `MouseAction` enum: `move|click|drag`. Reuse `tools::actions::MouseButtonArg` (already snapshotted) for `button`; define `ModifierArg` (`alt|ctrl|meta|shift`) → `KeyModifiers` bitflags (keyboard.rs:164).
  - Dispatch: `move`→`mouse_move(x,y)`; `click`→`mouse_click_with(x,y,ClickOptions{button,click_count,modifiers,..})`; `drag`→`mouse_drag((x,y),(to_x,to_y),steps)` (require `to_x/to_y`, else `invalid_params`).
  - `MouseOutput` = nav-style snapshot option or `AckOutput`.
- [ ] **Step 2-7:** Register `browser_mouse` (base); snapshots (`MouseInput`, `MouseAction`, `ModifierArg`, output); unit test; integration test clicks a coordinate on a canvas/test page. Build. Commit.

### Task A6: Dialog-drive — extend `browser_expect_await`

**Files:** Modify `tools/expect.rs`, `server.rs` (description), snapshots; extend `tests/integration_expect.rs`.

- [ ] **Step 1:** Read `tools/expect.rs` await path (the `ExpectKind::Dialog` arm, ~line 198). Add to `AwaitInput`: `#[serde(default, skip_serializing_if="Option::is_none")] dialog_action: Option<DialogAction>` (enum `accept|dismiss`) + `dialog_prompt_text: Option<String>`; `fetch_body: bool` (Response); `save_to: Option<String>` (Download).
- [ ] **Step 2:** Currently the dialog/download/response handles are awaited inside a spawned task that serializes the matched event to JSON and drops the handle (state.rs:71 `ExpectationHandle`). Driving `accept`/`dismiss`/`body`/`save_to` requires acting on the live `Matched*` BEFORE drop. Change the **register** path so the spawned task, for `Dialog`, parks the `MatchedDialog` awaiting a oneshot command (`accept{text}`/`dismiss`) forwarded by `await`; for `Response` with `fetch_body`, call `.body()` and include base64 in the serialized JSON; for `Download` with `save_to`, call `.save_to(path)` and include the final path. Simplest correct shape: fold the action INTO the await call — register stays detect-only, and `await` re-resolves and drives. **Pick the inline-await approach:** move dialog/response-body/download-save handling so the `await` tool performs the drive using fields above, keeping register unchanged. Document the chosen mechanism in the module header.
- [ ] **Step 3-5:** Update `browser_expect_await` description in `server.rs`. Snapshot `AwaitInput` (re-accept), add `DialogAction` snapshot. Unit test for the new input fields' schema. Integration test: register dialog → trigger `alert()` via `browser_evaluate` → `await` with `dialog_action: accept` → assert resolved. Build. Commit.

### Task A7: Frame-scoped eval — extend `browser_evaluate` / `_evaluate_main`

**Files:** Modify `tools/eval.rs`, `server.rs` (descriptions), snapshots; extend `tests/integration_*` (frames).

- [ ] **Step 1:** Read `tools/eval.rs`. Add `#[serde(default, skip_serializing_if="Option::is_none")] frame_id: Option<String>` to `EvalInput`. When set, resolve the frame via `tab.frames().await` → `find(|f| f.id()==fid)` (mirror `find.rs:116 lookup_frame`; consider extracting that helper to `common.rs` and reusing in both) and call `frame.evaluate::<Value>` / `frame.evaluate_main::<Value>` instead of `tab.*`. Error `FrameNotFound` when no match.
- [ ] **Step 2-5:** Update both tool descriptions. Re-accept `EvalInput` snapshot. Unit test schema. Integration test: page with named iframe → `browser_evaluate` with that `frame_id` returns the frame's `location.href`. Build. Commit.

---

## Phase B — Tier 2

### Task B1: Keyboard chords — `browser_press` modifiers + `browser_key_sequence`

**Files:** Modify `tools/actions.rs`, `server.rs`, snapshots; extend `tests/integration_actions.rs`.

- [ ] **Step 1:** `actions.rs` — add `#[serde(default)] modifiers: Vec<ModifierArg>` to `PressInput`; when non-empty call `el.press_with(key, mods)` else `el.press(key)`. Define/reuse `ModifierArg`→`KeyModifiers` (share with `tools::mouse` — put `ModifierArg` in `actions.rs` and re-export, or in `common.rs`).
- [ ] **Step 2:** Add `browser_key_sequence` — `KeySequenceInput { selector: Selector, sequence: Vec<KeyStep> }`; `KeyStep` is an untagged/tagged enum: `{text: String}` | `{key: String, #[serde(default)] modifiers: Vec<ModifierArg>}`. Build `zendriver::KeySequence` (keyboard.rs:652) via `.text()`/`.key()`/`.chord()` then `el.type_keys(seq)`. Reuse `actions::parse_key` for key strings.
- [ ] **Step 3-7:** Register `browser_key_sequence` (base); snapshots (`PressInput` re-accept, `KeySequenceInput`, `KeyStep`, `ModifierArg`); unit tests; integration test: focus an input, send `Ctrl+A` then type — assert selection/replace. Build. Commit.

### Task B2: `browser_download` (direct fetch) + download-dir

**Files:** Create `tools/download.rs`; modify mod/server/snapshots; create `tests/integration_download.rs`.

- [ ] **Step 1:** `tools/download.rs`. `DownloadInput { url: String, save_path?: String }` → `tab.download_file(&url, save_path.map(PathBuf::from)).await` (tab.rs:2362). Output `BlobOutput`-style `{ saved_path, byte_len }` (read the saved file len; `download_file` returns the path or `()` — inspect signature and adapt).
- [ ] **Step 2:** `browser_set_download_path` — `SetDownloadPathInput { path: String }` → `tab.set_download_path(PathBuf)` → `AckOutput`. (Tier 3 but cheap; co-locate here.)
- [ ] **Step 3-7:** Register both (base); snapshots; unit tests; integration test downloads a small static asset → asserts byte_len. Build. Commit.

### Task B3: Runtime UA + fine-grained stealth

**Files:** Modify `tools/stealth.rs`, `server.rs`, snapshots; extend `tests/integration_*`.

- [ ] **Step 1:** `browser_set_user_agent` (new, in `stealth.rs` or `navigation.rs`) — `SetUserAgentInput { user_agent: String, accept_language?: String, platform?: String }` → `tab.set_user_agent_with(UserAgentOverride{..})` (tab.rs:157,1745). Output `AckOutput`.
- [ ] **Step 2:** Extend `SetStealthProfileInput` with optional overrides `{ platform?, locale?, timezone?, memory_gb?: u32, cpu_count?: u32, chrome_version?: u32, user_agent?, bypass_csp?: bool }`. These are applied at the next `browser_open`; store them on `SessionState` (extend `stealth_profile_choice` into a struct OR add a `stealth_overrides: StealthOverrides` field). Lifecycle handler layers them onto the resolved `StealthProfile` builder (profile.rs:187-248). Keep wire enum `StealthProfileChoice` stable; add a sibling `StealthOverrides` struct in `state.rs`.
- [ ] **Step 3-7:** Register `browser_set_user_agent`; re-accept `SetStealthProfileInput` snapshot + add `StealthOverrides`; unit tests; integration test sets UA then `browser_evaluate("navigator.userAgent")` asserts match. Build. Commit.

### Task B4: Interception `modify_response`

**Files:** Modify `tools/intercept.rs`, snapshots; extend `tests/integration_interception.rs`.

- [ ] **Step 1:** Read `tools/intercept.rs` `InterceptAction` enum (~line 60). Add variant `ModifyResponse { #[serde(default, skip_serializing_if="Option::is_none")] status: Option<u16>, #[serde(default)] headers: BTreeMap<String,String> }`. In the add-rule dispatch, call the builder's `modify_response(pattern, closure)` (interception `InterceptBuilder`) returning a `ResponseOverrides`. Confirm the lib exposes `modify_response` on `InterceptBuilder`; if only `PausedRequest::continue_response` exists, document and wire via the response-stage rule path.
- [ ] **Step 2-5:** Re-accept `InterceptAction` + `AddRuleInput` snapshots; unit test schema; integration test rewrites a response status. Build. Commit.

### Task B5: Frame navigation + load waits + hard reload

**Files:** Modify `tools/frames.rs`, `tools/navigation.rs`, `server.rs`, snapshots.

- [ ] **Step 1:** `browser_frame_goto` (frames.rs) — `FrameGotoInput { frame_id: String, url: String, timeout_ms?: u64 }` → lookup frame → `frame.goto(url)` then `frame.wait_for_load()` (frame/mod.rs:373,416). Output `AckOutput`.
- [ ] **Step 2:** `browser_wait_for_load` (navigation.rs) — `WaitForLoadInput { ready_state?: ReadyStateArg (interactive|complete), frame_id?: String, timeout_ms?: u64 }` → `tab.wait_for_ready_state(ReadyState)` or `tab.wait_for_load()` (tab.rs:833,904); frame variant uses `frame.wait_for_load()`. `readOnly`.
- [ ] **Step 3:** Extend `HistoryInput` (reload path only) — add `#[serde(default)] ignore_cache: bool`; reload fn: when true call `tab.reload_with(ReloadOptions{ ignore_cache: true, ..})` (tab.rs:74,1437) else `tab.reload()`. (Confirm `ReloadOptions` field name.)
- [ ] **Step 4-7:** Register `browser_frame_goto`, `browser_wait_for_load`; snapshots; unit tests; integration tests. Build. Commit.

---

## Phase C — Tier 3

### Task C1: Link harvest + element extras + set-text + resource search

**Files:** Modify `tools/reads.rs`, `tools/snapshot.rs`, `tools/actions.rs`, `server.rs`, snapshots.

- [ ] **Step 1:** `browser_get_links` (reads.rs) — `GetLinksInput { #[serde(default)] absolute: bool, #[serde(default)] include_sources: bool }` → `tab.get_all_urls(absolute)` + (when `include_sources`) `tab.get_all_linked_sources()` (tab.rs:2224,2270). Output `{ urls: Vec<String>, #[serde(skip_serializing_if="Option::is_none")] sources: Option<Vec<String>> }`. `readOnly`.
- [ ] **Step 2:** `browser_element_state` — extend `ReadFieldsPreset`/`ElementState` with `outer_html: Option<String>` (`el.outer_html()`) + `bounding_box_page: Option<…>` (`el.bounding_box_page()`, reads.rs:267). Re-accept snapshots.
- [ ] **Step 3:** `browser_html` (snapshot.rs) — add `#[serde(default)] outer: bool`; selector mode → `el.outer_html()` when `outer` else `el.inner_html()`.
- [ ] **Step 4:** `browser_set_value` — add `#[serde(default)] mode: SetValueMode` (`value|text`); `text` → `el.set_text(v)` (actions.rs:500) else `el.set_value(v)`. `browser_clear` — add `#[serde(default)] mode: ClearMode` (`value|backspace`); `backspace` → `el.clear_by_deleting()` (actions.rs:545).
- [ ] **Step 5:** `browser_search_resources` (reads.rs) — `SearchResourcesInput { query: String }` → `tab.search_frame_resources(&query)` (tab.rs:2003). Output list of `{ frame_id, url, .. }` per `FrameResourceMatch` (tab.rs:246). `readOnly`.
- [ ] **Step 6-8:** Register new tools; re-accept all touched snapshots + add new ones; unit tests; integration tests. Build. Commit.

### Task C2: `browser_bypass_insecure_warning` + folds

**Files:** Modify `tools/navigation.rs` (or `lifecycle.rs`), `tools/lifecycle.rs` (status), `tools/tabs.rs`, `server.rs`, snapshots.

- [ ] **Step 1:** `browser_bypass_insecure_warning` — no-arg → `tab.bypass_insecure_connection_warning()` (tab.rs:2099) → `AckOutput`. Register (base).
- [ ] **Step 2:** Fold `inspector_url` — add `inspector_url: Option<String>` to `lifecycle::StatusOutput`, populated from `tab.inspector_url()` (tab.rs:2131). Re-accept `lifecycle_status_out` snapshot.
- [ ] **Step 3:** Fold `bring_to_front` — `browser_tab_activate` handler additionally calls `tab.bring_to_front()` after `activate()`; update description. (No new tool.)
- [ ] **Step 4-6:** Snapshots; unit tests; integration smoke. Build. Commit.

---

## Phase D — Docs + final verification

### Task D1: mdBook + counts

**Files:** Modify `docs/book/src/mcp.md`, `crates/zendriver-mcp/README.md`.

- [ ] **Step 1:** Update `mcp.md` tool table: add Scroll/Window/PDF/Mouse/Imperva/Download rows; bump counts 49 → 65; mark Imperva gated (note: in `default`). Update the prose "49 MCP tools" line.
- [ ] **Step 2:** Update the `Selector`-modifier and `return_snapshot` notes if any new modifiers were added.
- [ ] **Step 3:** Commit `docs: mcp tool surface 49 → 65`.

### Task D2: Full verification + evals

- [ ] **Step 1:** `cargo build -p zendriver-mcp --all-features --locked` → green.
- [ ] **Step 2:** `cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings` → clean (repo uses `clippy.toml`).
- [ ] **Step 3:** `cargo test -p zendriver-mcp --all-features --locked` (non-ignored) → green; `cargo insta accept --all` for new schema snapshots; re-run to confirm no `.snap.new` remain.
- [ ] **Step 4:** `cargo test -p zendriver-mcp --no-default-features --locked` → green (lean build still compiles; gated tools cleanly absent).
- [ ] **Step 5:** Write `crates/zendriver-mcp/tests/evals/coverage-expansion.xml` — 10 stable read-only real-Chrome eval questions per mcp-builder Phase 4, exercising scroll/window/pdf/mouse/frame-eval/get_links/etc.
- [ ] **Step 6:** Final commit.

---

## Self-Review

- **Spec coverage:** Every Tier 1 (A1-A7), Tier 2 (B1-B5), Tier 3 (C1-C2) item from the design maps to a task. Imperva feature-wiring (A1) covers the design's biggest gap. Docs/feature/test cross-cutting concerns → D1/D2. ✅
- **Non-goals respected:** No task touches the ref/handle model, interception stream, JS sandbox, workflow tools, fast variants, or debug viz. ✅
- **Naming consistency:** `ModifierArg` shared across mouse (A5) + keyboard (B1) — defined once in `actions.rs`, reused. `BlobOutput`/`blob_output` defined once in `common.rs` (A4), reused by download (B2). `ScrollInput` collision flagged → new type is `PageScrollInput` (A2). `lookup_frame` helper reused by eval (A7) + frames (B5) — extract to `common.rs`. ✅
- **Open confirmations flagged inline (resolve while implementing, not placeholders):** exact `ImpervaError` timeout variant + `From` impl (A1); `ScrollOptions` field names (A2); `WindowState` variants (A3); base64 encoder already in tree (A4); `download_file` return type (B2); `InterceptBuilder::modify_response` existence (B4); `ReloadOptions` field name (B5). Each names the source file/line to check.
