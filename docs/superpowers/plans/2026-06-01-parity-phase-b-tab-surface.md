# Parity Phase P-B — Tab/Page Convenience Surface — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Checkbox steps. Full detail in `docs/superpowers/specs/2026-06-01-parity-phase-b-tab-surface-design.md` — read the referenced §section per task. Implement TDD from spec + signatures; no full bodies duplicated here.

**Goal:** Add page-level Tab methods nodriver/zendriver-py expose that rs lacks — content, PDF/MHTML, scroll, runtime UA, raw mouse, reload options, window-state, misc conveniences.

**Architecture:** Core `zendriver` crate. New sibling modules `src/pdf/mod.rs` + `src/window.rs` (keep `tab.rs` ~2000 lines from bloating; `impl Tab` blocks). Reuse `screenshot/` builder style, `InputController`/`mouse::*`, browser-scope dispatch (like `activate()`).

**Tech Stack:** Rust 2024, CDP via `self.call`/`self.session().call`, `MockConnection` tests.

**Order:** independent; suggested B1 → B6 → B3 → B4 → B5 → B8 (tab.rs methods, serialize to avoid edit collisions) then B2 (new pdf module) → B7 (new window module). NOTE: B1/B3/B4/B5/B6/B8 all edit `tab.rs` — implement sequentially, not parallel.

---

### Task B1: `Tab::content()` — Spec §B1
**Files:** Modify `crates/zendriver/src/tab.rs` (+ test).
- [ ] Failing test: `content()` dispatches `DOM.getDocument` then `DOM.getOuterHTML{nodeId}`, returns html string.
- [ ] Run → fail.
- [ ] Implement `pub async fn content(&self) -> Result<String>`: `DOM.getDocument{depth:0}` → root `nodeId` → `DOM.getOuterHTML{nodeId}` → `outerHTML`.
- [ ] Run → pass.
- [ ] Commit: `feat(tab): add content() page-source accessor`

### Task B6: `reload_with` options — Spec §B6
**Files:** Modify `crates/zendriver/src/tab.rs` (keep `reload()`; add `reload_with` + `ReloadOptions`).
- [ ] Failing test: `reload_with(ReloadOptions{ignore_cache:true, script_to_evaluate_on_load:Some("x")})` dispatches `Page.reload{ignoreCache:true, scriptToEvaluateOnLoad:"x"}`. Existing `reload()` test stays green (ignoreCache:false).
- [ ] Run → fail.
- [ ] Implement `struct ReloadOptions{ignore_cache:bool, script_to_evaluate_on_load:Option<String>}` + `pub async fn reload_with(&self, opts)`. `reload()` unchanged.
- [ ] Run → pass.
- [ ] Commit: `feat(tab): reload_with(ignore_cache, script_to_evaluate_on_load)`

### Task B3: `scroll_down`/`scroll_up`/`scroll_with` — Spec §B3
**Files:** Modify `crates/zendriver/src/tab.rs` (+ `ScrollOptions`).
- [ ] Failing test: `scroll_down(300.0)` dispatches `Input.synthesizeScrollGesture` with `yDistance:-300` at viewport center; `scroll_with(ScrollOptions{dx,dy,speed})` forwards `speed`.
- [ ] Run → fail.
- [ ] Implement: `scroll_down(px)`/`scroll_up(px)` (yDistance ∓px), `scroll_with(ScrollOptions{dx,dy,speed:Option<i64>})`. Anchor at viewport center (fixed x,y or via getLayoutMetrics). Pixels, not %.
- [ ] Run → pass.
- [ ] Commit: `feat(tab): page scroll_down/scroll_up/scroll_with`

### Task B4: `set_user_agent` — Spec §B4
**Files:** Modify `crates/zendriver/src/tab.rs` (+ `UserAgentOverride`).
- [ ] Failing test: `set_user_agent("UA")` dispatches `Emulation.setUserAgentOverride{userAgent:"UA"}`; `set_user_agent_with` adds `acceptLanguage`/`platform` when set.
- [ ] Run → fail.
- [ ] Implement `set_user_agent(ua)` + `set_user_agent_with(UserAgentOverride{user_agent, accept_language:Option, platform:Option})`. **rustdoc MUST warn**: clobbers stealth UA-CH under Spoofed (last-write-wins, no metadata).
- [ ] Run → pass.
- [ ] Commit: `feat(tab): runtime set_user_agent override (+stealth-clobber doc warning)`

### Task B5: Tab raw mouse + flash_point — Spec §B5
**Files:** Modify `crates/zendriver/src/tab.rs` (reuse `input::mouse`).
- [ ] Failing test: `mouse_click(x,y)` emits `Input.dispatchMouseEvent` mousePressed+mouseReleased at (x,y); `mouse_move(x,y)` emits mouseMoved; `flash_point(x,y)` dispatches one `Runtime.evaluate`.
- [ ] Run → fail.
- [ ] Implement `mouse_move(x,y)` (realistic), `mouse_click(x,y)` (left realistic), `mouse_click_with(x,y,ClickOptions)` (reuse P-A `ClickOptions`), `flash_point(x,y)` (inject transient red dot via `evaluate_main`). Use `self.input()` + `mouse::move_realistic`/`click_at`.
- [ ] Run → pass.
- [ ] Commit: `feat(tab): coordinate mouse_move/mouse_click(_with) + flash_point`

### Task B8: misc (bypass warning, bring_to_front, new_window, inspector_url) — Spec §B8
**Files:** Modify `crates/zendriver/src/tab.rs` + `crates/zendriver/src/browser.rs` (new_window).
- [ ] Failing tests: `bring_to_front()`→`Page.bringToFront`; `Browser::new_window_at(url)`→`Target.createTarget{url,newWindow:true}`; `inspector_url()` composes `…/devtools/page/<target_id>` string; `bypass_insecure_connection_warning()` focuses body + types `thisisunsafe`.
- [ ] Run → fail.
- [ ] Implement: `Tab::bring_to_front()` (Page.bringToFront), `Tab::bypass_insecure_connection_warning()` (find css body → type_text_fast), `Tab::inspector_url()->String` (needs browser debug host:port — confirm stored from ws-url parse; thread it onto `Browser`/`TabInner`), `Browser::new_window()`/`new_window_at(url)` (createTarget+newWindow:true, reuse new_tab_at registration).
- [ ] Run → pass.
- [ ] Commit: `feat(tab,browser): bring_to_front, bypass_insecure_warning, inspector_url, new_window`

### Task B2: PDF + MHTML — Spec §B2
**Files:** Create `crates/zendriver/src/pdf/mod.rs`; Modify `crates/zendriver/src/lib.rs` (mod + re-exports), `tab.rs` (`pdf_builder`/`print_to_pdf`/`snapshot_mhtml`/`save_snapshot`).
- [ ] Failing tests: `pdf_builder().landscape(true).save(...)` dispatches `Page.printToPDF` with `landscape:true`, decodes base64; `save_snapshot(path)` dispatches `Page.captureSnapshot{format:"mhtml"}` and writes file.
- [ ] Run → fail.
- [ ] Implement `PdfBuilder<'tab>` mirroring `ScreenshotBuilder` (landscape/print_background/scale/paper_size/margins/page_ranges/prefer_css_page_size → `.bytes()`/`.save()`), `Tab::pdf_builder()` + `print_to_pdf(path)` shortcut; `Tab::snapshot_mhtml()->String` + `save_snapshot(path)`.
- [ ] Run → pass.
- [ ] Commit: `feat(tab): PDF export (Page.printToPDF builder) + MHTML save_snapshot`

### Task B7: window-state family — Spec §B7
**Files:** Create `crates/zendriver/src/window.rs`; Modify `lib.rs`, `tab.rs`.
- [ ] Failing tests: `window_bounds()` dispatches `Browser.getWindowForTarget{targetId}` and parses bounds; `maximize()` dispatches `Browser.setWindowBounds` with `{windowState:"maximized"}` only.
- [ ] Run → fail.
- [ ] Implement `WindowBounds{left,top,width,height:Option, state:WindowState}` + `WindowState{Normal,Minimized,Maximized,Fullscreen}`; `Tab::window_bounds()`, `set_window_bounds()`, `set_window_size(w,h)`, `maximize()`/`minimize()`/`fullscreen()`. Browser-scope dispatch (cache windowId via getWindowForTarget). State-change sent alone (no other bounds fields).
- [ ] Run → pass.
- [ ] Commit: `feat(tab): window-state (bounds/size/maximize/minimize/fullscreen)`

---
## Phase verification
Parallel batch: `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test -p zendriver`. Background if >5s.
## Self-review
Spec coverage B1-B8 ✓. New modules `pdf`, `window` re-exported in `lib.rs`. `inspector_url` depends on browser exposing debug host:port — verify/thread during B8. Scroll = pixels (Assumption 3). `reload()` default unchanged (Assumption 4).
