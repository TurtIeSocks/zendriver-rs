# zendriver-mcp Coverage Expansion — Design

**Date:** 2026-06-02
**Status:** Proposed
**Predecessor:** [`2026-05-24-zendriver-rs-mcp-server-design.md`](./2026-05-24-zendriver-rs-mcp-server-design.md) (v0, the 49-tool server)

## Problem

MCP coverage of the `zendriver` public API is treated like test coverage: every
user-facing library operation should be reachable through a tool unless there is
a deliberate reason not to. Today `zendriver-mcp` ships **49 tools** covering
~60–65% of the MCP-worthy surface. Seven whole subsystems have **zero** tools,
and several shipped subsystems are detect-only or shallow.

This design closes the gap to the practical maximum: **all Tier 1/2/3 gaps**
identified in the coverage audit, leaving only the explicitly documented
non-goals uncovered.

## Coverage audit (method + result)

- **Denominator** — every `pub fn` / method on user-facing types across 8 crates
  (`Browser`, `Tab`, `Element`, `Frame`, query builders, `CookieJar`, `Storage`,
  `ScreenshotBuilder`, `PdfBuilder`, and feature crates `zendriver-stealth`,
  `-cloudflare`, `-imperva`, `-interception`, `-fetcher`). ~230 public items.
  Excluded: transport/CDP internals, `pub(crate)`, std-trait impls, opaque wire
  structs, pure config helpers.
- **Numerator** — the 49 `#[tool]` registrations in `crates/zendriver-mcp/src/server.rs`,
  each mapped to the `zendriver` call it wraps.

### Already covered (49 tools — unchanged)

Lifecycle/nav/tabs (open/close/status, goto/back/forward/reload/wait_for_idle,
5 tab tools); find/find_all (css/xpath/text_exact/text_regex/role(+name),
nth/visible_only/timeout/frame_id); element actions
(click/hover/type/press/set_value/clear/focus/scroll_into_view/upload);
element_state reads (visible/enabled/bbox/text/attrs/inner_html); html +
screenshot (png/jpeg/webp/full_page/quality/clip/save); evaluate/evaluate_main
(tab main-world); cookies (get/set/delete/clear/persist); storage local+session
(get/set/delete/clear); frame_list; set_stealth_profile (kind);
intercept add/remove/list/clear (block/redirect/respond/modify_request);
expect register/await/cancel (request/response/dialog/download — **detect-only**);
solve_turnstile; install_chrome (version/channel).

### Gaps to close (this design)

#### Tier 1 — whole subsystems, zero tools

| # | Gap | Unexposed `zendriver` API |
|---|-----|---------------------------|
| 1 | Imperva bypass | `Tab::imperva()` + entire `zendriver-imperva` crate — **not even a MCP feature flag** |
| 2 | Page scroll | `Tab::scroll_down` / `scroll_up` / `scroll_with(ScrollOptions)` |
| 3 | Window / viewport | `Tab::set_window_size` / `maximize` / `minimize` / `fullscreen` / `window_bounds` / `set_window_bounds` |
| 4 | PDF / MHTML export | `Tab::pdf_builder` / `print_to_pdf` / `snapshot_mhtml` / `save_snapshot` (full `PdfBuilder`) |
| 5 | Coordinate mouse | `Tab::mouse_move` / `mouse_click` / `mouse_click_with` / `mouse_drag`, `Element::mouse_drag` |
| 6 | Dialog drive | `MatchedDialog::accept(prompt)` / `dismiss` — currently detect, cannot answer |
| 7 | Frame-scoped eval | `Frame::evaluate` / `evaluate_main` — OOPIF JS unreachable |

#### Tier 2 — medium value

| # | Gap | Unexposed API |
|---|-----|---------------|
| 8  | Keyboard chords | `Element::press_with(KeyModifiers)`, `type_keys(KeySequence)` — no Ctrl+A / Cmd+C |
| 9  | Response body | `MatchedResponse::body()` |
| 10 | Download retrieval | `MatchedDownload::save_to` / `path` |
| 11 | Direct download | `Tab::download_file(url, dest)` |
| 12 | Runtime UA | `Tab::set_user_agent` / `set_user_agent_with(UserAgentOverride)` |
| 13 | Fine-grained stealth | `StealthProfile::platform` / `locale` / `timezone` / `memory_gb` / `cpu_count` / `chrome_version` / `user_agent` / `bypass_csp` |
| 14 | modify_response | `InterceptBuilder` response override (only `modify_request` exposed) |
| 15 | Frame navigation | `Frame::goto` / `wait_for_load` |
| 16 | Load waits | `Tab::wait_for_load` / `wait_for_ready_state(ReadyState)` |
| 17 | Hard reload | `Tab::reload_with(ReloadOptions)` (ignore-cache) |

#### Tier 3 — convenience

| # | Gap | Unexposed API |
|---|-----|---------------|
| 18 | Link harvest | `Tab::get_all_urls(absolute)` / `get_all_linked_sources` |
| 19 | Element extras | `Element::outer_html` / `bounding_box_page` |
| 20 | Set text content | `Element::set_text` (≠ `set_value`) / `clear_by_deleting` |
| 21 | Download dir | `Tab::set_download_path` |
| 22 | Resource search | `Tab::search_frame_resources` |
| 23 | Page focus | `Tab::bring_to_front` |
| 24 | SSL interstitial | `Tab::bypass_insecure_connection_warning` |
| 25 | Inspector URL | `Tab::inspector_url` |

### Deliberate non-goals (NOT covered — documented, unchanged from v0)

- **Element ref/handle model.** Selectors remain the only handle. This is *why*
  relative DOM traversal (`Element::parent` / `children` / subtree `find`) is not
  a tool — it would need handle-passing. Covering it later means a
  selector-relative-query design, not a ref model. Out of scope here.
- **Stream-based interception escape hatch** (`InterceptBuilder::subscribe`) —
  needs server-side event buffering + backpressure.
- **Code-exec / JS sandbox tool** — `evaluate` already covers ad-hoc JS.
- **Workflow / recipe tools** — primitive coverage only; agents compose.
- **Fast non-stealth action variants** (`click_fast` / `hover_fast` /
  `type_text_fast`) — stealth-by-default philosophy; realistic variants ship.
- **Debug visualization** (`Element::flash` / `highlight_overlay`, `Tab::flash_point`)
  — operator-debug only, no agent value. (Mouse-flash debug folded out of
  `browser_mouse`.)
- **Raw transport** (`Connection` / `SessionHandle` / observers) — internal.

## Goals

1. Every Tier 1/2/3 API above reachable through a tool or a documented
   parameter extension to an existing tool.
2. Zero new wire-shape conventions — reuse the existing `Selector` arg,
   `return_snapshot` flag, `frame_id` modifier, and binary-output shape.
3. Each new tool ships with: integration test (real Chrome, gated), an `insta`
   JSON-schema snapshot, and an entry in the mdBook tool table.
4. The `imperva` subsystem becomes a first-class gated feature, mirroring
   `cloudflare`.

## Design

### Conventions reused (no new patterns)

- **`Selector` arg** — one-of `css|xpath|text|text_exact|text_regex|role(+name)`
  with `nth|visible_only|timeout_ms|frame_id`. All new element-targeted tools
  take it verbatim.
- **`return_snapshot: bool`** — state-changing tools optionally return a trimmed
  HTML snapshot for one-call act-and-observe.
- **`frame_id: Option<String>`** — the existing frame-retarget convention,
  extended to `evaluate` and the new load-wait tool.
- **Binary-output shape** — `{ byte_len, saved_path?, base64? }` (PNG screenshot
  is the precedent). `pdf` / `save_mhtml` / `download` reuse it: when `save_path`
  is set the bytes go to disk and `base64` is omitted; otherwise base64 is
  returned. Large blobs default to save-to-path with a size guard.

### New tools (one module per group, matching existing `tools/*.rs` layout)

**`tools/scroll.rs`**
- `browser_scroll` — `Tab::scroll_with(ScrollOptions)`. In:
  `{ dx?: f64, dy?: f64, direction?: up|down, amount_px?: f64, return_snapshot?: bool }`.
  Out: `{ scroll_x, scroll_y }`. `readOnly:false, idempotent:false, openWorld:true`.

**`tools/window.rs`**
- `browser_get_window` — `Tab::window_bounds`. Out: `WindowBounds { left, top, width, height, state }`. `readOnly:true`.
- `browser_set_window` — `set_window_size` / `set_window_bounds` / `maximize` / `minimize` / `fullscreen`.
  In: `{ mode: bounds|size|maximize|minimize|fullscreen, width?, height?, left?, top?, state? }`. `readOnly:false`.

**`tools/pdf.rs`**
- `browser_pdf` — full `PdfBuilder`. In:
  `{ landscape?, print_background?, scale?, paper_width?, paper_height?, margin_top?, margin_bottom?, margin_left?, margin_right?, page_ranges?, prefer_css_page_size?, save_path? }`.
  Out: binary-output shape. `readOnly:true` (no page mutation).
- `browser_save_mhtml` — `Tab::snapshot_mhtml` / `save_snapshot`. In: `{ save_path? }`. Out: binary-output shape. `readOnly:true`.

**`tools/mouse.rs`**
- `browser_mouse` — `Tab::mouse_move` / `mouse_click_with` / `mouse_drag`. In:
  `{ action: move|click|drag, x: f64, y: f64, to_x?, to_y?, button?, click_count?, modifiers?, steps?, return_snapshot? }`.
  Out: `{}` or snapshot. `readOnly:false`.

**`tools/imperva.rs`** (gated `feature = "imperva"`)
- `browser_solve_imperva` — `Tab::imperva()` → `ImpervaBypass::wait_for_clearance`.
  In: `{ timeout_ms?, poll_interval_ms?, with_interception? }`. Out:
  `{ outcome: token_acquired|challenge_gone|already_clear, reese84?, surface? }`.
  Mirrors `cloudflare.rs`. `readOnly:false, openWorld:true`. (CAPTCHA-solver
  callback `on_captcha` is **not** wired over MCP — same rationale as the
  interception stream non-goal; documented.)

**`tools/download.rs`**
- `browser_download` — `Tab::download_file(url, dest)`. In: `{ url, save_path? }`. Out: binary-output shape. `readOnly:false`.

**Extend `tools/navigation.rs`**
- `browser_wait_for_load` (new) — `Tab::wait_for_load` / `wait_for_ready_state`.
  In: `{ ready_state?: interactive|complete, frame_id?, timeout_ms? }`. `readOnly:true`.
- `browser_reload` (extend) — add `{ ignore_cache?: bool }` → `reload_with(ReloadOptions)`.

**Extend `tools/frames.rs`**
- `browser_frame_goto` (new) — `Frame::goto` + `wait_for_load`. In: `{ frame_id, url, timeout_ms? }`. `readOnly:false`.

**Extend `tools/eval.rs`**
- `browser_evaluate` / `browser_evaluate_main` — add `frame_id?: Option<String>`
  → `Frame::evaluate` / `evaluate_main` when set, else `Tab::*`.

**Extend `tools/actions.rs`**
- `browser_press` — add `modifiers?: [alt,ctrl,meta,shift]` → `press_with`.
- `browser_key_sequence` (new) — `Element::type_keys(KeySequence)`. In:
  `{ selector, sequence: [ {text} | {key} | {key, modifiers} ] }`. `readOnly:false`.
- `browser_set_value` — add `mode?: value|text` → `set_text` when `text`.
- `browser_clear` — add `mode?: value|backspace` → `clear_by_deleting` when `backspace`.

**Extend `tools/expect.rs`** (completes detect-only → drive)
- await for `Dialog` — add `{ dialog_action?: accept|dismiss, prompt_text? }`,
  driven inline before return (fits no-handle model: register → trigger → await-and-respond in one call).
- await for `Response` — add `{ fetch_body?: bool }` → `MatchedResponse::body()` (base64 in output).
- await for `Download` — add `{ save_to?: string }` → `MatchedDownload::save_to`, return final path.

**Extend `tools/stealth.rs`**
- `browser_set_stealth_profile` — add optional overrides
  `{ platform?, locale?, timezone?, memory_gb?, cpu_count?, chrome_version?, user_agent?, bypass_csp? }`
  layered on the chosen `kind` (`StealthProfile` builder chain).
- `browser_set_user_agent` (new) — `Tab::set_user_agent_with(UserAgentOverride)`.
  In: `{ user_agent, accept_language?, platform? }`. `readOnly:false`.

**Extend `tools/intercept.rs`** (gated `interception`)
- `browser_intercept_add_rule` — add `InterceptAction::ModifyResponse { status?, headers? }` → `modify_response`.

**Extend `tools/reads.rs` + `tools/snapshot.rs` (Tier 3)**
- `browser_element_state` — `include` preset gains `outer_html` + `bounding_box_page` fields (`Element::outer_html` / `bounding_box_page`).
- `browser_html` — add `outer?: bool` (selector mode → `Element::outer_html` vs `inner_html`).
- `browser_get_links` (new) — `Tab::get_all_urls(absolute)` / `get_all_linked_sources`. In: `{ absolute?, include_sources? }`. Out: `{ urls: [], sources?: [] }`. `readOnly:true`.
- `browser_search_resources` (new) — `Tab::search_frame_resources`. In: `{ query }`. Out: `{ matches: [{ frame_id, url, ... }] }`. `readOnly:true`.

**Small Tier-3 standalone tools** (kept minimal; trivial reads folded to cut surface noise)
- `browser_set_download_path` (new) — `Tab::set_download_path`. In: `{ path }`. `readOnly:false`.
- `browser_bypass_insecure_warning` (new) — `Tab::bypass_insecure_connection_warning`. `readOnly:false`.
- `inspector_url` — **folded** into `browser_status` output (read state; no new tool).
- `bring_to_front` — **folded** into `browser_tab_activate` (`activate` already foregrounds; add note, no new tool).

### Tool-count delta

49 → **65** registered tools (16 new tools + ~10 in-place parameter extensions).
Two folds (`inspector_url`, `bring_to_front`) avoid trivial single-call tools.

New tools: `browser_scroll`, `_get_window`, `_set_window`, `_pdf`, `_save_mhtml`,
`_mouse`, `_solve_imperva`, `_download`, `_wait_for_load`, `_frame_goto`,
`_key_sequence`, `_set_user_agent`, `_get_links`, `_search_resources`,
`_set_download_path`, `_bypass_insecure_warning`.

### Architecture impact

- **New modules:** `tools/scroll.rs`, `tools/window.rs`, `tools/pdf.rs`,
  `tools/mouse.rs`, `tools/imperva.rs`, `tools/download.rs`. Registered as
  one-liner wrappers in `server.rs`.
- **New feature flag.** `crates/zendriver-mcp/Cargo.toml`: add
  `imperva = ["zendriver/imperva"]` and include it in `default`. New
  `#[cfg(feature = "imperva")] #[tool_router(router = imperva_tool_router)]`
  block in `server.rs`, summed in `combined_tool_router()` exactly like
  `cloudflare_tool_router`.
- **Binary-output guard.** A shared `binary_output(bytes, save_path)` helper in
  `tools/common.rs` (size threshold → force save-to-path, omit base64). Reused by
  pdf / mhtml / download / response-body.
- **Docs.** `docs/book/src/mcp.md` tool table + counts updated (49 → 66; add
  Scroll/Window/PDF/Mouse/Imperva/Download rows; mark Imperva gated). 49 → 65.

### Testing strategy

- **Integration tests** (`tests/integration_*.rs`, `feature = "integration-tests"`,
  `#[ignore]`, real Chrome) — one new file per new module: `integration_scroll`,
  `_window`, `_pdf`, `_mouse`, `_imperva`, `_download`; extend existing
  `integration_actions` / `_expect` / `_lifecycle` for the parameter extensions.
- **Schema snapshots** (`tests/schema_snapshots.rs` + `tests/snapshots/`, `insta`)
  — every new tool and every extended input/output schema gets a snapshot; CI
  diff requires `cargo insta accept`. This is the wire-shape review gate.
- **Evaluations** (mcp-builder Phase 4) — 10 stable, read-only, real-Chrome
  questions exercising the new surface (e.g. "scroll to the page footer and
  return the copyright year", "export this page to PDF and report the byte
  length", "evaluate `navigator.userAgent` inside the OOPIF named `checkout`").

### Phasing (for writing-plans)

- **Phase A — Tier 1** (highest value): imperva (+feature wiring), scroll, window,
  pdf + mhtml, mouse, dialog-drive, frame-eval.
- **Phase B — Tier 2**: key chords/modifiers, response-body, download-save,
  download_file, runtime-UA, fine-grained stealth, modify_response, frame-goto,
  load-waits, hard-reload.
- **Phase C — Tier 3**: links, element outer_html/bbox_page, set_text/clear-by-backspace,
  set_download_path, search_resources, bypass_insecure_warning, + the two folds.

Each phase ends with: `cargo build`, gated integration run, `insta` accept,
mdBook table update.

## Assumptions

1. **"API" = user-facing library operations**, not transport/CDP internals or
   pure config structs.
2. **Scope = everything (all Tier 1/2/3)** per user selection; only the listed
   non-goals stay uncovered.
3. **Imperva is promoted to a default MCP feature** (mirrors cloudflare). If a
   leaner default is wanted, it can be gated-but-off-by-default instead.
4. **Binary blobs default to save-to-path** above a size threshold rather than
   always base64-inlining (token-cost guard).
5. **Dialog drive folds into the await call** (no separate handle tool),
   consistent with the no-ref-model non-goal.
6. **`inspector_url` and `bring_to_front` are folded** into existing tools to
   avoid trivial-tool surface bloat; everything else gets a dedicated tool or a
   named parameter.
7. **Imperva CAPTCHA-solver callback (`on_captcha`) is not wired over MCP** —
   same class as the interception-stream non-goal.
