# Parity Phase P-E — Convenience Sweep (full method-for-method parity)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Checkbox TDD steps. Each item names the upstream nodriver `file:line` to READ for faithful semantics; implement TDD from there. All additive, core `zendriver` crate.

**Goal:** Close the remaining low-value convenience methods so zendriver-rs is at literal method-for-method parity with nodriver/zendriver-py (on top of the P-A…P-D superset). After this, "full parity" is accurate.

**Architecture:** Additive methods on `Tab` (`tab.rs`) + `Element` (`element/*.rs`) + `BoundingBox`/Position. No new deps, no gates. Reuse existing patterns (`self.call`, `evaluate_main`, `find_all`, `InputController`/`mouse::*`, `with_refresh`).

**Reference:** upstream at `/Users/rin/GitHub/nodriver/nodriver/core/{tab,element}.py`. Read the cited line before porting.

**Order:** Dispatch 1 (Tab batch) → Dispatch 2 (Element batch). Both edit `tab.rs`/`element` — serialize.

---

## Dispatch 1 — Tab conveniences (`crates/zendriver/src/tab.rs`)

### E1 `js_dumps` — nodriver tab.py:911
`pub async fn js_dumps(&self, obj_name: &str) -> Result<serde_json::Value>` — evaluate the named JS object/expression and return it deep-serialized. Impl: `Runtime.evaluate { expression: obj_name, returnByValue: true, ... }` → `result.value` as `serde_json::Value`. (rs typed `evaluate<T>` already exists; this is the untyped dump.) TDD: dispatches `Runtime.evaluate` with `returnByValue:true`, returns the Value. Commit `feat(tab): js_dumps untyped object dump`.

### E2 `get_all_urls` + `get_all_linked_sources` — tab.py:1531 / 1522
`get_all_urls(&self, absolute: bool) -> Result<Vec<String>>` — JS collect every `[href]`/`[src]` URL; absolute=resolve against document. `get_all_linked_sources(&self) -> Result<Vec<Element>>` — `find_all` over `[src],[href]` → elements. TDD: `get_all_urls` evaluates the collector JS (assert it reads href+src); `get_all_linked_sources` routes through find_all. Commit `feat(tab): get_all_urls + get_all_linked_sources`.

### E3 `wait_for_ready_state` — (zendriver-py parity)
`pub enum ReadyState { Loading, Interactive, Complete }` + `pub async fn wait_for_ready_state(&self, until: ReadyState) -> Result<()>` — poll `document.readyState` until it reaches `until` (or timeout). TDD: polls `Runtime.evaluate document.readyState`, returns when reached. Commit `feat(tab): wait_for_ready_state`.

### E4 `download_file` — tab.py:1374
`pub async fn download_file(&self, url: impl Into<String>, filename: Option<PathBuf>) -> Result<()>` — port nodriver's approach (read the body): set download behavior + trigger the download for `url` (JS fetch→blob→anchor click, or navigate). TDD: dispatches the download trigger (assert the JS/anchor or setDownloadBehavior path nodriver uses). Commit `feat(tab): download_file convenience`.

### E5 `mouse_drag` (tab) — tab.py:1922
`pub async fn mouse_drag(&self, from: (f64, f64), to: (f64, f64), steps: usize) -> Result<()>` — `Input.dispatchMouseEvent` mousePressed at `from`, `steps` mouseMoved toward `to`, mouseReleased at `to`. Reuse `self.input()`/`mouse::*` if a drag primitive fits; else dispatch directly. TDD: emits pressed → moves → released in order. Commit `feat(tab): mouse_drag(from,to,steps)`.

### E6 `search_frame_resources` — tab.py:1671
`pub async fn search_frame_resources(&self, query: &str) -> Result<Vec<FrameResourceMatch>>` — `Page.getResourceTree` → for each resource `Page.searchInResource`/`getResourceContent` matching `query`. Niche; port the shape nodriver uses. TDD: dispatches getResourceTree + a search call, returns matches. If the CDP search method is awkward, return `(url, content)` matches via getResourceContent + substring. Commit `feat(tab): search_frame_resources`.

After batch: `cargo test -p zendriver --lib tab` + `cargo clippy -p zendriver --all-targets -- -D warnings`. Re-export any new pub enums (`ReadyState`) from lib.rs.

---

## Dispatch 2 — Element conveniences (`crates/zendriver/src/element/*.rs`, `query` BoundingBox)

### E7 `set_text` — element.py:771
`pub async fn set_text(&self, value: impl AsRef<str>) -> Result<()>` — set the element's text content (nodriver sets the text-node value via `DOM.setNodeValue`; a `textContent=` + bubbled `input` is the simpler faithful equivalent — match nodriver's observable result). TDD: dispatches the set. Commit `feat(element): set_text`.

### E8 `flash` + `highlight_overlay` — element.py:913 / 1001
`flash(&self, duration: Duration) -> Result<()>` — inject a transient colored overlay at the element's bbox (mirror Tab `flash_point`, element-scoped). `highlight_overlay(&self) -> Result<()>` — `Overlay.highlightNode { backendNodeId }` (devtools highlight). TDD: `flash` dispatches `Runtime.evaluate`/`callFunctionOn`; `highlight_overlay` dispatches `Overlay.highlightNode`. Commit `feat(element): flash + highlight_overlay (debug viz)`.

### E9 `mouse_drag` (element) — element.py:596
`pub async fn mouse_drag(&self, to: (f64, f64), steps: usize) -> Result<()>` (or to another Element) — scroll into view, drag from this element's bbox center to `to`. Reuse the Tab `mouse_drag` (E5) once it exists, or `mouse::*`. TDD: emits pressed-at-center → moves → released. Commit `feat(element): mouse_drag`.

### E10 Position absolute coords + `to_viewport` — element.py:504 / 1165 / 1190
Extend the bounding-box surface: add absolute page coords (`abs_x`/`abs_y` = bbox + scroll offset) and a `to_viewport(scale)` equivalent. Either add fields/methods to `BoundingBox` (in `query/`) or a new `Element::bounding_box_page() -> Result<...>` returning page-absolute coords (read `window.scrollX/Y` + bbox). TDD: page coords = viewport bbox + scroll offset. Commit `feat(element): absolute page-coordinate bounding box`.

After batch: `cargo test -p zendriver --lib` + `cargo clippy -p zendriver --all-targets -- -D warnings`.

---

## Phase verification
Parallel: `cargo clippy --workspace --all-targets --all-features -- -D warnings` + `cargo test --workspace --lib --all-features`. Then update top-level CHANGELOG ("convenience sweep — full parity") + memory.

## Self-review
Covers the documented "low-value conveniences not ported" tail from the P-D-complete report: js_dumps, get_all_urls/get_all_linked_sources, search_frame_resources, element flash/highlight_overlay, download_file, wait_for_ready_state, set_text, mouse_drag (tab+element), Position abs/to_viewport. Deliberate SKIPs (opencv cf_verify, pickle cookies, SOCKS5 forwarder, __getattr__/__repr__, record_video, breathe/sleep, UC-import) remain documented non-goals — rs has better equivalents or they're Python-isms.
