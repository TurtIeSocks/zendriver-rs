# Phase P-A — Correctness Parity (design)

Date: 2026-06-01
Status: design (delegate-mode brainstorm; awaiting user review)
Scope: 4 correctness gaps where zendriver-rs diverges from nodriver / zendriver-py in ways users hit in real automation. NOT convenience surface (that is P-B+).

Items:
- **A1** — cross-frame `find()` + `best_match` (closest-text-length) — *parity target: nodriver*
- **A2** — KeyEvents/keyboard port (non-ASCII/emoji, shift synthesis, modifier wrapping, mixed input) — *parity target: zendriver-py*
- **A3** — React controlled-input clearing (native prototype setter + delete-fallback) — *parity target: zendriver-py*
- **A4** — WebSocket `max_size` config — *parity target: both Python libs*

Guiding constraints: preserve every rs strength (isolated-world eval, actionability, bezier/typing realism, Frame/OOPIF model, single-socket flat transport). Pre-1.0 so additive + small behavior changes are acceptable (per repo SEMVER + `api-churn-acceptable-pre-release`). All four land in the core `zendriver` crate, no new feature gate.

---

## A1 — cross-frame `find()` + `best_match`

### Problem
`Tab::find()` searches only the tab's main document. nodriver's `find(text)` auto-walks every connectable iframe (incl `content_document`), which is how `find("verify you are human")` reaches a Cloudflare challenge checkbox inside an iframe. rs has the frames (`Tab::frames()`, first-class `Frame` w/ own session) but `FindBuilder` never fans out across them. Separately, rs text matching returns the first narrowest match in document order; nodriver picks the candidate whose text length is *closest* to the needle (so `find("accept all")` returns the button, not a `<script>` whose text merely contains the phrase).

### Design
Two new `FindBuilder` / `FindAllBuilder` modifiers, both opt-in:

```rust
tab.find().text("verify you are human").include_frames().one().await?;
tab.find().text("accept all").best_match().one().await?;
```

- **`.include_frames()`** — fan the resolved selector out across the main document **and** every `Frame` in `Tab::frames()`. Recurses same-origin + OOPIF frames (each `Frame` already dispatches on its own session via `QueryScope::Frame`). Default off.
  - `.one()` without `best_match`: resolve main scope first, return first hit; only descend frames if main yields nothing; frames searched in registry order, first hit wins. (Early-return keeps the common case cheap.)
  - `.one()` with `best_match`: must gather candidates from all scopes, then pick global best (see below).
  - `.many()`: always gather across all scopes, concatenated main-first then per-frame.
- **`.best_match()`** — among candidates, pick the one minimizing `abs(elementTextLen - needleLen)`. Applies to `.text()` / `.text_exact()` / `.text_regex()` queries (no-op + debug-log warning on css/xpath/role, where "text length" is meaningless). Default off.
  - Single-scope: scoring done JS-side — the substring/regex collector returns matches already sorted ascending by `abs(len(innerText)-len(needle))`; `.one()` takes `[0]`.
  - Cross-scope (`include_frames` + `best_match`): each scope returns its locally-sorted list; Rust reads the top candidate's text length per scope (1 extra `Runtime.callFunctionOn`/scope) and picks the global min. Ties → earliest scope, earliest document order (deterministic).

### Why opt-in (alternatives considered)
- *Default-on cross-frame (nodriver-faithful):* every `find` pays an N-frame fan-out; surprising perf cost on frame-heavy pages, and the headline use case (CF/Imperva challenge clicks) is already handled inside those crates. Rejected as default; available via `.include_frames()`.
- *Default-on best_match (nodriver-faithful):* changes `.text().one()` results silently for existing users. rs's current "first narrowest in doc order" is a reasonable, cheaper default. Rejected as default; available via `.best_match()`.
- Net: rs keeps its explicit/cheap defaults; nodriver's two behaviors become one method call each. (See Assumptions — this is the main reversible call.)

### Touch points
`query/mod.rs` (add `include_frames: bool`, `best_match: bool` to both builders + the two methods; thread into `one()`/`many()` dispatch), `query/selectors.rs` (add distance-sort variants of the text/regex collectors; a `text_len_of(scope, ref)` helper for cross-scope scoring), `tab.rs` (`frames()` already exists). No change to `QueryScope`.

### Tests (MockConnection unit + 1 nightly)
- `include_frames().one()` falls through to a frame when main misses (mock main → empty, frame session → node).
- `best_match().one()` picks the closest-length candidate (mock collector returns 3 lengths; assert the dispatched JS sorts, `.one()` takes the nearest).
- cross-scope best_match tie → earliest scope.
- css/xpath + `.best_match()` logs + no-ops (returns same as without).
- nightly: real page w/ same-origin iframe, `find(text).include_frames()` resolves the in-frame element.

---

## A2 — KeyEvents / keyboard port

### Problem
rs `dispatch_char` sends `keyDown`/`keyUp` with `text`+`key`=char, no `code`/`windowsVirtualKeyCode`, no shift synthesis, no modifier wrapper events. Consequences:
- **Non-ASCII / emoji / CJK can't be typed reliably** — no `char`-event path (Chrome's text-insertion event type).
- **Shifted chars** (`A`, `!`, `:`) don't emit a real Shift keydown or correct `code`/`keyCode`; keystroke-sensitive sites (and some validators) misbehave.
- **Modifier chords** (`press_with(Ctrl+a)`) set the CDP `modifiers` bitfield but emit no Ctrl keydown/keyup — sites listening for the modifier keydown miss it.
- No mixed-input builder (`from_text` / `from_mixed_input`).

zendriver-py solved all of this in `core/keys.py` (the 0.11 rewrite). Port its semantics into idiomatic Rust, building on rs's existing `Key` / `SpecialKey` / `KeyModifiers`.

### Design
A pure event-builder layer + a dispatch layer; existing public methods route through it.

**Printable-char descriptor table** (`input/keyboard.rs`): `fn char_descriptor(c: char) -> Option<CharDescriptor>` returning `{ code: &'static str, windows_vk: i32, shift: bool, base: char }` — `None` ⇒ route to `char`-event. Coverage (mirrors keys.py):
- `a–z` → `Key{UPPER}`, vk = ASCII upper, shift=false
- `A–Z` → `Key{UPPER}`, vk = ASCII upper, shift=true
- `0–9` → `Digit{n}`, vk = ASCII digit, shift=false
- shifted digits `)!@#$%^&*(` (index 0–9) → `Digit{index}`, shift=true
- `SPECIAL_CHAR_MAP`: `;=,-./` `` ` `` `[\]'` → (code, vk) from the keys.py table, shift=false
- `SPECIAL_CHAR_SHIFT_MAP`: `:+<_>?~{|}"` → base char's (code, vk), shift=true
- space/tab/enter → handled as `SpecialKey::{Space,Tab,Enter}`
- everything else (non-ASCII, accents, emoji, multi-codepoint) → `None` ⇒ `char`-event

**Event model**: `enum KeyPress { Char, DownAndUp }` and
`fn key_events(target: KeyTarget, mods: KeyModifiers, kind: KeyPress) -> Vec<KeyEventPayload>` where `KeyTarget = Char(char) | Special(SpecialKey)`. Logic:
- `Char` kind, or a target whose `char_descriptor` is `None` ⇒ emit one `{ type:"char", text, key:text }` payload.
- `DownAndUp` kind on a known printable char ⇒ resolve shift (descriptor.shift OR caller's Shift bit), then emit, in **conventional order**: modifier keydowns (accumulating bits) → main keyDown (code/key/vk/text) → main keyUp → modifier keyups (reverse). *(Divergence from keys.py, which releases modifiers before the main key-up; conventional order is more correct and matches Playwright.)*
- `DownAndUp` on a `SpecialKey` ⇒ same wrapper, main = `rawKeyDown`/`keyUp` with the SpecialKey's `to_cdp()` triple, no `text`.

**Dispatch**: `dispatch_key_events(tab, &[KeyEventPayload])` issues each `Input.dispatchKeyEvent`. `dispatch_char`/`dispatch_special` become thin wrappers (or are replaced).

**Public API** (`element/input.rs`):
- `type_text` / `type_text_fast` — segment `text` into grapheme clusters (`unicode-segmentation`); per cluster: single ASCII char with descriptor → `DownAndUp`; else → `char`-event. Realistic path keeps per-char delay / typo / thinking-pause around the new builder. **Fixes uppercase/symbol/emoji/CJK typing.**
- `press(Key)` / `press_with(Key, KeyModifiers)` — route through `DownAndUp` so modifier chords emit real wrapper events. **Fixes `Ctrl+A` etc.**
- **new** `type_keys(seq: KeySequence)` — `from_mixed_input` parity. `KeySequence` builder:
  ```rust
  KeySequence::new()
      .text("Hello ")
      .key(SpecialKey::Enter)
      .chord(Key::Char('a'), KeyModifiers::CTRL)
  ```
  items: `Text(String) | Key(SpecialKey) | Chord(Key, KeyModifiers)`. Flattens to one payload `Vec`, dispatched in order.

### New dependency
`unicode-segmentation` (grapheme clustering) — lightweight, ubiquitous, no transitive weight. Avoids an emoji-detection crate: the `char_descriptor` = `None` rule already routes emoji/non-ASCII to the `char` path, matching keys.py's effective `keyCode is None → CHAR` behavior.

### Tests
- `char_descriptor` table: `'A'`→(KeyA,65,shift), `'!'`→(Digit1,49,shift), `':'`→(Semicolon,186,shift), `'a'`→no-shift, `'1'`→Digit1, emoji→None.
- `key_events('A', DownAndUp)` emits Shift-down, A-down, A-up, Shift-up in order with correct vk.
- `press_with(Char('a'), CTRL)` emits Control keydown + a-down/up + Control keyup.
- emoji / CJK char → single `char`-type payload with full cluster text.
- `type_keys` mixed sequence → flattened payload order.
- nightly: type `"Héllo World! 🚀"` into an input, read back `.value` == sent.

---

## A3 — React controlled-input clearing

### Problem
rs `clear()` does `el.value=''; dispatch('input')` and `set_value()` does `el.value=v; dispatch('input'+'change')`. React's controlled inputs install a `_valueTracker` on the element; assigning `.value` directly is reverted on the next render because React thinks the value didn't change. zendriver-py defeats this with the **native prototype setter** and adds a keystroke-based `clear_input_by_deleting()` fallback for inputs with custom delete behavior.

### Design
- **Fix `set_value()` + `clear()` in place** (no API change) to use the native setter:
  ```js
  function(v){
    const proto = this instanceof HTMLTextAreaElement
        ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const desc = Object.getOwnPropertyDescriptor(proto, 'value');
    (desc && desc.set ? desc.set.call(this, v) : (this.value = v));
    this.dispatchEvent(new Event('input',  {bubbles:true}));
    this.dispatchEvent(new Event('change', {bubbles:true}));
  }
  ```
  `clear()` passes `''`. (`clear()` gains the `change` event too — harmless, matches zendriver-py; the old comment about omitting `change` is removed.)
- **New `clear_by_deleting()`** — keystroke fallback for inputs that ignore programmatic value-set (some web-components / masked inputs): `focus()` → select-all (`Meta+A` on macOS targets, else `Ctrl+A`) → `Backspace`; if value still non-empty, loop `Backspace` from end up to a bounded cap (`value.length + small slack`). Uses Backspace-from-end (NOT forward-Delete) to avoid the VK_DELETE infinite-loop zendriver hit on some VMs.

### Why fix-in-place vs new method names
zendriver-py has `clear_input` + `clear_input_by_deleting` as separate methods (history: its `clear_input` predates the React fix). rs has only `clear()`. Making `clear()`/`set_value()` correct-by-default is strictly better and needs no rename; `clear_by_deleting()` is the additive escape hatch. (Naming `clear_by_deleting` over `clear_input_by_deleting` to match rs's `clear()` not `clear_input()`.)

### Tests
- `set_value` dispatched JS contains `getOwnPropertyDescriptor` + `.set.call` + both events; textarea branch present.
- `clear` passes `''` through the same path.
- `clear_by_deleting` emits focus → select-all chord → Backspace sequence (MockConnection).
- nightly: a React controlled `<input>` (tiny inline page) — `set_value` then read `.value` survives a re-render tick.

---

## A4 — WebSocket max_size

### Problem
`connection.rs:156` calls bare `connect_async(ws_url)`. tokio-tungstenite's default caps (~16 MiB message / 64 MiB frame) silently drop the socket when a single CDP message exceeds them — full-page screenshots, large `getResponseBody`, big DOM/MHTML dumps. Both Python libs set `2**28` (256 MiB). Silent-failure-mode bug.

### Design
Replace with `connect_async_with_config`:
```rust
use tokio_tungstenite::{connect_async_with_config, tungstenite::protocol::WebSocketConfig};

const WS_MAX_BYTES: usize = 256 << 20; // 256 MiB, matches zendriver-py 2**28

let config = WebSocketConfig {
    max_message_size: Some(WS_MAX_BYTES),
    max_frame_size:   Some(WS_MAX_BYTES),
    ..Default::default()
};
let (ws, _resp) = connect_async_with_config(ws_url, Some(config), false).await?;
```
`disable_nagle=false` (keep default). No keepalive/ping change needed — tokio-tungstenite doesn't auto-ping, so rs never closes on a missing pong (the reason Python sets `ping_timeout=900`; N/A here).

### Tests
- Unit: construct the config, assert both fields = `WS_MAX_BYTES` (guards against a future refactor dropping the cap).
- Manual/nightly: capture a full-page screenshot of a tall page (> 16 MiB PNG) over the live socket without disconnect. (Document as a manual check if no nightly harness slot.)

---

## Cross-cutting

- **Deps:** +`unicode-segmentation` (A2). No other new deps. A4 uses tokio-tungstenite APIs already in-tree.
- **Feature gates:** none — all core `zendriver`.
- **SEMVER:** A2 `press_with` now emits modifier wrapper events + `type_text` fixes uppercase/symbol output = behavior changes (improvements); A3 `clear()` adds a `change` event. All acceptable pre-1.0; note in CHANGELOG under Changed.
- **Docs:** rustdoc on new methods; mdBook quickstart gains a cross-frame-find note + a "typing unicode / chords" snippet.
- **Ordering within P-A:** A4 (smallest, isolated) → A3 (small) → A1 → A2 (largest). Independent; can parallelize A4/A3 vs A1/A2.

## Out of scope (deferred)
- Persistent `add_handler(event, cb)` registry (P-D design note — rs Stream model likely kept).
- Tab-level `mouse_click(x,y)` / raw coordinate clicks (P-C).
- `best_match` / `include_frames` as defaults (revisit post-feedback).
- Element `flash`/`highlight` debug overlays (P-C, low value).

## Assumptions (delegate-mode checkpoint — correct any before writing-plans)
1. **`include_frames` + `best_match` are opt-in modifiers, not new defaults.** Keeps rs's cheap/explicit defaults; nodriver-faithful default-on is the alternative. (Main reversible call.)
2. **A2 modifier order = conventional** (mod-down → key-down → key-up → mod-up), diverging from keys.py's release-mod-before-key-up. More correct; unlikely to matter to any site.
3. **Emoji/non-ASCII detection = "no printable-char descriptor ⇒ `char` event"**, not a dedicated emoji crate. Matches keys.py's effective behavior; avoids a dep.
4. **`clear()`/`set_value()` fixed in place** (gain native-setter + `change` event) rather than adding parallel `clear_input` names. New `clear_by_deleting()` is additive.
5. **A4 cap = 256 MiB** both message + frame (matches zendriver-py `2**28`).
6. **`unicode-segmentation` is an acceptable new dependency.**
7. **`type_keys(KeySequence)` is included now** (from_mixed_input parity) rather than deferred.
8. Per-phase deliverable = this design spec; `writing-plans`/implementation is a later follow-on after all phase brainstorms, not invoked per-phase.
