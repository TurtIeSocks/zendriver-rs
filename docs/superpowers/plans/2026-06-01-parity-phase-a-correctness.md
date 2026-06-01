# Parity Phase P-A — Correctness — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Full design detail (CDP calls, JS bodies, semantics) lives in `docs/superpowers/specs/2026-06-01-parity-phase-a-correctness-design.md` — read the referenced spec section before each task. Code-level bodies are intentionally NOT duplicated here; implement TDD from the spec + signatures below.

**Goal:** Close 4 correctness gaps where zendriver-rs diverges from nodriver/zendriver-py in ways users hit: cross-frame find, full keyboard/KeyEvents, React-controlled-input clearing, WebSocket message-size cap.

**Architecture:** All in the core `zendriver` crate except A4 (`zendriver-transport`). Additive API + two behavior improvements (press_with modifier wrapping, clear() native-setter). New dep `unicode-segmentation` for A2.

**Tech Stack:** Rust 2024, tokio, tokio-tungstenite, serde_json, CDP over `SessionHandle::call`. Tests use `MockConnection` (see existing `crates/zendriver/src/element/actions.rs` tests for the pattern).

**Execution order:** A4 → A3 → A1 → A2 (smallest/most-isolated first; A2 largest). Tasks touch disjoint files except A2/A1 both in `query`/`input` — serialize A1 before A2.

---

### Task A4: WebSocket max message/frame size

Spec §A4.

**Files:**
- Modify: `crates/zendriver-transport/src/connection.rs:~155` (the `connect_async(ws_url)` call site)

- [ ] **Step 1 — failing test:** in `connection.rs` tests, add `ws_config_uses_256mib_cap()` asserting a `const WS_MAX_BYTES: usize = 256 << 20;` and that the `WebSocketConfig` built for the connect uses `max_message_size == Some(WS_MAX_BYTES)` and `max_frame_size == Some(WS_MAX_BYTES)`. (Factor the config into a small `fn ws_config() -> WebSocketConfig` so it's unit-testable without a live socket.)
- [ ] **Step 2 — run, expect fail** (`ws_config` undefined): `cargo test -p zendriver-transport ws_config_uses_256mib_cap`
- [ ] **Step 3 — implement:** add `const WS_MAX_BYTES: usize = 256 << 20;` + `fn ws_config() -> WebSocketConfig { WebSocketConfig { max_message_size: Some(WS_MAX_BYTES), max_frame_size: Some(WS_MAX_BYTES), ..Default::default() } }`. Replace `connect_async(ws_url)` with `connect_async_with_config(ws_url, Some(ws_config()), false)`. Import `tokio_tungstenite::{connect_async_with_config, tungstenite::protocol::WebSocketConfig}`.
- [ ] **Step 4 — run, expect pass.** Also `cargo build -p zendriver-transport`.
- [ ] **Step 5 — commit:** `feat(transport): raise WebSocket message/frame cap to 256 MiB`

**Acceptance:** large CDP messages (full-page screenshots, big response bodies) no longer silently drop the socket. Config is unit-tested.

---

### Task A3: React-controlled-input clearing

Spec §A3.

**Files:**
- Modify: `crates/zendriver/src/element/actions.rs` (`set_value`, `clear`; add `clear_by_deleting`)
- Test: same file's `#[cfg(test)] mod tests`

- [ ] **Step 1 — failing tests:** (a) `set_value` dispatched JS contains `getOwnPropertyDescriptor` + `.set.call` + both `'input'` and `'change'` events + the `HTMLTextAreaElement`/`HTMLInputElement` proto branch; (b) `clear` routes through the same native-setter JS with `''`; (c) `clear_by_deleting` focuses then emits a select-all chord (`Meta`/`Ctrl`+A) followed by `Backspace` dispatch(es). Use `MockConnection` + assert `Runtime.callFunctionOn` `functionDeclaration` substrings / `Input.dispatchKeyEvent` sequence.
- [ ] **Step 2 — run, expect fail.**
- [ ] **Step 3 — implement** per spec: rewrite `set_value`/`clear` JS to use the native prototype setter (textarea vs input proto, fallback to `this.value=v`), fire bubbled `input`+`change`. Add `pub async fn clear_by_deleting(&self)` (focus → select-all chord → Backspace-from-end loop bounded by `value.length + slack`; NO forward-Delete). Reuse `with_refresh`.
- [ ] **Step 4 — run, expect pass.** `cargo test -p zendriver element::actions`
- [ ] **Step 5 — commit:** `fix(element): defeat React _valueTracker in set_value/clear; add clear_by_deleting`

**Acceptance:** controlled React inputs accept programmatic value/clear; `clear_by_deleting` handles inputs that ignore value-set.

---

### Task A1: cross-frame find + best_match

Spec §A1. **Do before A2.**

**Files:**
- Modify: `crates/zendriver/src/query/mod.rs` (`FindBuilder`/`FindAllBuilder`: add fields `include_frames: bool`, `best_match: bool` + chainable methods; thread into `one()`/`many()` dispatch)
- Modify: `crates/zendriver/src/query/selectors.rs` (distance-sorted text/regex collectors; a `text_len_of(scope, &RemoteRef)` helper for cross-scope scoring)
- Test: both files' test modules

- [ ] **Step 1 — failing tests:** (a) `include_frames().one()` falls through to a frame when main scope returns empty (mock main eval → null, then a `frames()` Frame session → node); (b) `best_match().one()` on text returns the closest-text-length candidate (mock collector returns multiple; assert the dispatched JS sorts ascending by `abs(len-needleLen)` and `.one()` takes `[0]`); (c) css/xpath + `.best_match()` is a no-op (same result as without) and logs.
- [ ] **Step 2 — run, expect fail** (methods undefined).
- [ ] **Step 3 — implement** per spec: add `.include_frames()` / `.best_match()` to both builders. In `one()`: if `include_frames` && !`best_match`, try main scope then iterate `tab.frames()` scopes, first hit wins; if `best_match`, gather candidates across enabled scopes + pick global min distance (`text_len_of` for cross-scope). `many()`: gather across scopes main-first. Add distance-sort variants in `selectors.rs` (extend the substring/regex collector JS to sort by `abs(innerTextLen - needleLen)` when best_match). best_match on non-text selector → `tracing::debug!` + ignore.
- [ ] **Step 4 — run, expect pass.** `cargo test -p zendriver query`
- [ ] **Step 5 — commit:** `feat(query): cross-frame find via include_frames + best_match closest-length`

**Acceptance:** `tab.find().text("…").include_frames().one()` reaches into iframes; `.best_match()` mirrors nodriver's closest-length pick. Both opt-in; defaults unchanged.

---

### Task A2: KeyEvents / keyboard port

Spec §A2. Largest task — split into A2a (event model) then A2b (public API). **Add dep first.**

**Files:**
- Modify: `crates/zendriver/Cargo.toml` (+ `unicode-segmentation`) and `Cargo.toml` workspace deps if pinned centrally
- Modify: `crates/zendriver/src/input/keyboard.rs` (char descriptor table, `KeyPress` enum, `key_events()` builder, dispatch)
- Modify: `crates/zendriver/src/element/input.rs` (`type_text`/`type_text_fast`/`press`/`press_with` route through builder; add `type_keys` + `KeySequence`)

- [ ] **Step 1 (A2a) — failing tests:** `char_descriptor('A')==(KeyA,65,shift=true)`, `('!')==(Digit1,49,shift)`, `(':')==(Semicolon,186,shift)`, `('a')` no-shift, `('1')==Digit1`, emoji `'🚀'`→`None`. `key_events(Char('A'),mods,DownAndUp)` yields Shift-down, A-down, A-up, Shift-up (conventional order) with correct `code`/`windowsVirtualKeyCode`. Emoji → single `{type:"char", text}` payload.
- [ ] **Step 2 — run, expect fail.**
- [ ] **Step 3 (A2a) — implement** per spec: `fn char_descriptor(c)->Option<CharDescriptor{code,windows_vk,shift,base}>` covering a-z/A-Z/digits/shifted-digits/`SPECIAL_CHAR_MAP`/`SPECIAL_CHAR_SHIFT_MAP`; `enum KeyPress{Char,DownAndUp}`; `fn key_events(target,mods,kind)->Vec<KeyEventPayload>` (char-event path when descriptor None or kind=Char; else modifier-wrap down/up sequence: mod-downs → main keyDown(code/key/vk/text) → main keyUp → mod-ups). `dispatch_key_events(tab,&[..])`.
- [ ] **Step 4 — run A2a tests, expect pass.** Commit: `feat(input): char-descriptor table + key-event builder (shift/modifier/char-event)`
- [ ] **Step 5 (A2b) — failing tests:** `type_text` of `"Aa!🚀"` produces the expected payload sequence (uppercase via Shift, `!` via Shift+Digit1, emoji via char-event); `press_with(Char('a'),CTRL)` emits Control-down + a-down/up + Control-up; `type_keys(KeySequence::new().text("hi").key(Enter).chord(Char('a'),CTRL))` flattens in order. Segment text via `unicode_segmentation::UnicodeSegmentation::graphemes`.
- [ ] **Step 6 — run, expect fail.**
- [ ] **Step 7 (A2b) — implement:** route `type_text`/`type_text_fast` per-grapheme through `key_events` (realistic path keeps timing/typo wrapper); `press`/`press_with` through DownAndUp; add `KeySequence` builder (`text`/`key`/`chord`) + `type_keys`. Keep `dispatch_char`/`dispatch_special` as thin wrappers or replace.
- [ ] **Step 8 — run, expect pass.** `cargo test -p zendriver input`
- [ ] **Step 9 — commit:** `feat(input): full keyboard parity — non-ASCII/emoji, shift synthesis, modifier chords, type_keys`

**Acceptance:** non-ASCII/emoji/CJK typeable; uppercase/symbols emit correct codes + synthesized Shift; `Ctrl+A` emits real modifier events; mixed input via `type_keys`.

---

## Phase verification (after all 4 tasks)
Run in parallel (one Bash batch): `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test -p zendriver -p zendriver-transport`. Background if >5s. Fix any regressions before phase commit.

## Self-review notes
- Spec coverage: A1✓ A2✓ A3✓ A4✓ (all spec sections mapped).
- A2 modifier order = conventional (spec Assumption 2). Emoji detection = descriptor-None (Assumption 3).
- Signature consistency: `key_events`, `char_descriptor`, `KeyPress`, `KeySequence`, `clear_by_deleting`, `include_frames`, `best_match`, `ws_config`/`WS_MAX_BYTES` used consistently.
