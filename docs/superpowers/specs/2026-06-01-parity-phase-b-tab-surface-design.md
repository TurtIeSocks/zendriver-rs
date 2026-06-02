# Phase P-B — Tab/Page Convenience Surface (design)

Date: 2026-06-01
Status: design (delegate-mode brainstorm; awaiting user review)
Scope: page-level methods nodriver/zendriver-py expose that rs's `Tab` lacks. Mostly thin CDP wrappers; low risk; parallelizable. All land in core `zendriver` crate, no feature gate. Follows existing Tab conventions (`self.call(method, params)` session-routed; browser-scope calls like `activate()` for Browser-domain commands; chainable builders à la `screenshot_builder()`).

Items: B1 `content()` · B2 PDF + MHTML · B3 scroll · B4 `set_user_agent` · B5 tab-level raw mouse · B6 `reload` options · B7 window-state family · B8 misc (`bypass_insecure_connection_warning`, `bring_to_front`, `new_window`, `inspector_url`).

rs strengths to preserve: `screenshot_builder` (don't regress to a flat fn), `evaluate`/`evaluate_main` split, per-tab `InputController` (bezier), Frame/OOPIF model, isolated-world eval.

---

## B1 — `Tab::content()`

Page source HTML. rs has `Frame::content()` (documentElement.outerHTML) but no Tab convenience.

```rust
pub async fn content(&self) -> Result<String>
```
Impl: `DOM.getDocument { depth: 0 }` → root `nodeId` → `DOM.getOuterHTML { nodeId }`. Full serialization incl doctype (matches nodriver `get_content`). Name `content()` mirrors `Frame::content()`.

Tests: MockConnection — dispatches getDocument then getOuterHTML, returns the html string.

---

## B2 — PDF + MHTML export

rs has zero PDF/MHTML. Both are single CDP calls. Mirror the `screenshot_builder()` pattern for PDF (many knobs); MHTML is parameterless so a plain method.

**PDF** — chainable builder + shortcut:
```rust
pub fn pdf_builder(&self) -> PdfBuilder<'_>;          // landscape/print_background/scale/
                                                      // paper_size/margins/page_ranges/
                                                      // prefer_css_page_size → .bytes()/.save()
pub async fn print_to_pdf(&self, path) -> Result<()>; // shortcut: default A4 portrait, save
```
`PdfBuilder` maps 1:1 to `Page.printToPDF` params; terminal `.bytes() -> Vec<u8>` / `.save(path)`. `printToPDF` returns base64 (decode like screenshot) and can stream, but synchronous base64 is fine at our message cap (P-A A4 raises it to 256 MiB). New module `src/pdf/mod.rs` next to `screenshot/`.

**MHTML**:
```rust
pub async fn snapshot_mhtml(&self) -> Result<String>;     // Page.captureSnapshot { format: "mhtml" }
pub async fn save_snapshot(&self, path) -> Result<()>;    // write the mhtml to file
```
`save_snapshot` name matches nodriver/zendriver-py.

Tests: PDF builder dispatches `Page.printToPDF` with chosen knobs, decodes base64; `save_snapshot` dispatches `Page.captureSnapshot { format: mhtml }` and writes.

---

## B3 — `scroll_down` / `scroll_up`

rs only has `Element::scroll_into_view`. Add page-level scroll via `Input.synthesizeScrollGesture`.

```rust
pub async fn scroll_down(&self, pixels: f64) -> Result<()>;
pub async fn scroll_up(&self, pixels: f64) -> Result<()>;
pub async fn scroll_with(&self, opts: ScrollOptions) -> Result<()>; // { dx, dy, speed }
```
Anchor at viewport center (`Page.getLayoutMetrics` → center, or fixed 0.5/0.5 of `cssVisualViewport`). `scroll_down(px)` → `yDistance: -px` (CDP convention: negative scrolls content up/page down). `ScrollOptions.speed` (px/s) plumbs `synthesizeScrollGesture.speed`.

**Divergence from nodriver:** nodriver's `amount` is a % of viewport height; rs uses **pixels** (predictable, no viewport round-trip in the simple path). `scroll_with` carries the `speed` knob zendriver-py added.

Tests: `scroll_down(300.0)` dispatches `Input.synthesizeScrollGesture` with `yDistance: -300`; `scroll_with` forwards `speed`.

---

## B4 — `Tab::set_user_agent`

Runtime UA override (zendriver-py `set_user_agent(ua, accept_language, platform)`).

```rust
pub async fn set_user_agent(&self, user_agent: impl Into<String>) -> Result<()>;
pub async fn set_user_agent_with(&self, ovr: UserAgentOverride) -> Result<()>;
// UserAgentOverride { user_agent, accept_language: Option<String>, platform: Option<String> }
```
Maps `Emulation.setUserAgentOverride { userAgent, acceptLanguage?, platform? }`.

**⚠ Stealth interaction (documented, not enforced):** the Spoofed/Native stealth observer already issues `Emulation.setUserAgentOverride` *with* `userAgentMetadata` (UA-CH coherence). This runtime override is **last-write-wins** and sends NO `userAgentMetadata`, so calling it under the Spoofed profile drops Client-Hints coherence and can *increase* fingerprint detectability. rustdoc warns: prefer `StealthProfile`/builder UA for stealth; use this for non-stealth tabs or deliberate per-tab UA changes. (Not auto-merging metadata — keeps the method simple + matches zendriver-py's three-field surface.)

Tests: dispatches `Emulation.setUserAgentOverride` with ua + optional fields.

---

## B5 — Tab-level raw mouse

nodriver `tab.mouse_click(x,y)` / `mouse_move(x,y)` / `flash_point` for coordinate clicks (cf-checkbox style) + visual debug. rs has `Element` click/hover but no raw-coordinate Tab API. The `InputController` + `mouse::*` helpers already exist (`Element` actions use them) — just expose at Tab.

```rust
pub async fn mouse_move(&self, x: f64, y: f64) -> Result<()>;            // realistic bezier
pub async fn mouse_click(&self, x: f64, y: f64) -> Result<()>;          // left, realistic
pub async fn mouse_click_with(&self, x, y, opts: ClickOptions) -> Result<()>; // reuse P-A ClickOptions
pub async fn flash_point(&self, x: f64, y: f64) -> Result<()>;          // debug: inject a red dot
```
Reuse `self.input()` + `mouse::move_realistic`/`click_at` (already `pub(crate)`, same crate). `flash_point` injects a transient absolutely-positioned dot via `evaluate_main` (debug aid; cheap). Realistic-by-default consistent with `Element::click`; `_with` exposes raw/teleport via the existing `ClickOptions`.

Tests: `mouse_click(x,y)` emits `Input.dispatchMouseEvent` mousePressed/Released at (x,y); `flash_point` dispatches one `Runtime.evaluate`.

---

## B6 — `reload` options

rs `reload()` hardcodes `ignoreCache: false`. nodriver/zendriver-py default `ignore_cache=True` + accept a `script_to_evaluate_on_load`.

```rust
pub async fn reload(&self) -> Result<()>;                  // UNCHANGED: ignoreCache:false
pub async fn reload_with(&self, opts: ReloadOptions) -> Result<()>;
// ReloadOptions { ignore_cache: bool, script_to_evaluate_on_load: Option<String> }
```
Maps `Page.reload { ignoreCache, scriptToEvaluateOnLoad? }`.

**Decision:** keep `reload()`'s `false` default (no behavior change for existing rs users) and add `reload_with` for control — rather than flipping `reload()` to nodriver's `true` default. (Alternative — flip to `true` for nodriver parity — noted in Assumptions; pre-1.0 makes it possible but I bias to least-surprise.)

Tests: `reload_with({ignore_cache:true, script:Some(..)})` dispatches `Page.reload` with both fields.

---

## B7 — Window-state family

Wholly absent in rs. Browser-domain commands (dispatch at browser scope, like `activate()`'s `Target.activateTarget`).

```rust
pub async fn window_bounds(&self) -> Result<WindowBounds>;            // Browser.getWindowForTarget
pub async fn set_window_bounds(&self, b: WindowBounds) -> Result<()>; // Browser.setWindowBounds
pub async fn set_window_size(&self, w: i64, h: i64) -> Result<()>;    // convenience → setWindowBounds
pub async fn maximize(&self) -> Result<()>;
pub async fn minimize(&self) -> Result<()>;
pub async fn fullscreen(&self) -> Result<()>;
// WindowBounds { left, top, width, height: Option<i64>, state: WindowState }
// WindowState { Normal, Minimized, Maximized, Fullscreen }
```
Flow: `Browser.getWindowForTarget { targetId }` → `windowId` + bounds; `Browser.setWindowBounds { windowId, bounds }`. `maximize/minimize/fullscreen` set only `state` (CDP requires state-change be sent alone, no other bounds fields — encode that in `set_window_bounds`). New module `src/window.rs`.

Tests: `window_bounds()` dispatches getWindowForTarget w/ targetId, parses bounds; `maximize()` dispatches setWindowBounds w/ `{state:"maximized"}` only.

---

## B8 — Misc

- **`bypass_insecure_connection_warning()`** — `Tab::find().css("body").one()` → `type_text_fast("thisisunsafe")` (Chrome's interstitial bypass phrase; nodriver `select("body").send_keys("thisisunsafe")`). Trivial; leans on P-A typing fix.
- **`bring_to_front()`** — `Page.bringToFront` (session scope). DISTINCT from `activate()` (which is browser-scope `Target.activateTarget`). nodriver exposes both; rs only has `activate`. Additive.
- **`Browser::new_window()` / `new_window_at(url)`** — `Target.createTarget { url, newWindow: true }`. Browser-level (a new OS window, not a tab); reuses the `new_tab_at` registration path with the `newWindow` flag. nodriver's `get(new_window=True)`.
- **`Tab::inspector_url() -> String`** — compose the DevTools frontend URL: `http://{debug_host}:{debug_port}/devtools/inspector.html?ws={debug_host}:{debug_port}/devtools/page/{target_id}` from the browser's debug endpoint. Returns the URL only; **no auto-launch** (avoids an OS-`open` dependency). nodriver `inspector_url` + `open_external_inspector`; we ship the URL, caller opens it.

Tests: `bring_to_front` dispatches `Page.bringToFront`; `new_window_at` dispatches `Target.createTarget` w/ `newWindow:true`; `inspector_url` composes the expected string (needs the browser to expose `debug_host:port` — confirm it's stored from the stderr ws-url parse).

---

## Cross-cutting
- **Deps:** none new.
- **Feature gates:** none — all core.
- **New modules:** `src/pdf/mod.rs`, `src/window.rs` (keep `tab.rs` from growing; it's already ~2000 lines — sibling modules with `impl Tab` blocks, matching `screenshot/`).
- **SEMVER:** all additive except B6 (kept additive by not changing `reload()`); pre-1.0 regardless. CHANGELOG: Added.
- **Docs:** rustdoc per method; mdBook quickstart snippets for pdf, scroll, window-state.
- **Ordering:** independent; B1/B8 trivial first, B2/B7 (new modules) next, B3/B4/B5/B6 anytime. Fully parallelizable across implementers.

## Out of scope (deferred)
- `download_file()` JS-blob helper (low value).
- `get_all_urls` / `search_frame_resources` (niche; SKIP per audit).
- Auto-launching the external debugger (URL only).
- Element-level `flash`/`highlight` overlays (separate; B5 ships Tab `flash_point` only).

## Assumptions (delegate-mode checkpoint — correct any before writing-plans)
1. **PDF = builder (`pdf_builder`) + `print_to_pdf(path)` shortcut**, mirroring `screenshot_builder`/`screenshot`. New `src/pdf/` module.
2. **`save_snapshot(path)` + `snapshot_mhtml() -> String`** for MHTML.
3. **Scroll API uses pixels, not viewport-%** (+ `scroll_with` carrying zendriver-py's `speed`). Diverges from nodriver's %-amount.
4. **`reload()` keeps `ignoreCache:false`; add `reload_with`** — NOT flipping to nodriver's `true` default. (Reversible call.)
5. **`set_user_agent` is a runtime `Emulation.setUserAgentOverride`** with a documented "clobbers stealth UA-CH if called under Spoofed" warning; no metadata-merge.
6. **Full window-state family** (bounds/state/size/maximize/minimize/fullscreen) in `src/window.rs`, dispatched browser-scope.
7. **`new_window` is Browser-level** (`Browser::new_window`/`new_window_at`), not a `Tab` method.
8. **`inspector_url()` returns the URL only**; no auto-launch / no OS-open dep.
9. **Tab raw mouse reuses `InputController`** (realistic default + `_with` for raw); `flash_point` is a debug JS-dot helper.
10. **`bring_to_front` (Page.bringToFront) is additive alongside `activate` (Target.activateTarget)** — both kept, distinct semantics.
