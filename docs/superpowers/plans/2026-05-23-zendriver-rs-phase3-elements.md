# zendriver-rs Phase 3 (Elements + Input + Actionability) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the `Element` + `FindBuilder` surface to Python-zendriver parity: xpath/text/role selectors, traversal, full reads + actions, realistic Bezier mouse + per-key typing input with actionability gates, true isolated-world `Element::evaluate`, screenshots.

**Architecture:** `element.rs` + `query.rs` split into module directories. New `input/` module owns Bezier mouse + keyboard dispatch + `InputController` (per-Browser state). `InputProfile` in `zendriver-stealth` ties realism tunables to active StealthProfile. Auto-refresh on stale handles via `ElementOrigin` memoization. Playwright-style actionability checks before every action.

**Tech Stack:** Rust + `regex` (text_regex selector) + `bitflags` (KeyModifiers) + `rand` `SmallRng` (Bezier jitter + typing pauses, seedable for tests) + CDP `Input.dispatchMouseEvent` / `dispatchKeyEvent` / `DOM.resolveNode` / `Page.captureScreenshot` / `Accessibility.getPartialAXTree`.

**Spec:** [docs/superpowers/specs/2026-05-23-zendriver-rs-phase3-elements-design.md](../specs/2026-05-23-zendriver-rs-phase3-elements-design.md)

---

## File structure

### Workspace root (modify)
- `Cargo.toml` — add `regex`, `bitflags`, `rand` to `[workspace.dependencies]`

### `crates/zendriver/` (extensive modify)
- `Cargo.toml` — add `regex`, `bitflags`, `rand` deps; add `rand` to dev-deps with `small_rng` feature
- `src/lib.rs` — add `pub mod input;` + re-exports of new types
- `src/browser.rs` — `BrowserInner` gains `input: Arc<InputController>`; `Browser::input()` accessor
- `src/error.rs` — `ZendriverError` gains `ElementStale`, `NotRefreshable`, `NotActionable(Duration, String)`
- `src/tab.rs` — `Tab::screenshot()` (full page); `tab.browser()` accessor (used by Element to reach InputController)
- `src/element.rs` → DELETED (replaced by `src/element/mod.rs` + 8 sibling files)
- `src/query.rs` → DELETED (replaced by `src/query/mod.rs` + 5 sibling files)

### `crates/zendriver/src/element/` (NEW directory)
- `mod.rs` — struct + `Arc<Inner>` + `ElementOrigin` + module re-exports
- `reads.rs` — `attr`, `attrs`, `inner_text`, `inner_html`, `outer_html`, `bounding_box`, `is_visible`, `is_enabled`
- `actions.rs` — `click` (upgraded), `click_raw`, `click_with(ClickOptions)`, `hover`, `focus`, `scroll_into_view`, `set_value`, `clear`, `upload_files`
- `input.rs` — `type_text`, `type_text_raw`, `press`, `press_with`
- `traversal.rs` — `parent`, `children`, `find()`, `find_all()`
- `isolated_eval.rs` — `Element::evaluate` upgraded to true isolated world via `DOM.resolveNode` + `Runtime.callFunctionOn` + `Runtime.releaseObject`
- `refresh.rs` — `with_refresh` wrapper + `refresh()` impl + `is_stale_node_error` helper
- `screenshot.rs` — `Element::screenshot()` via `Page.captureScreenshot { clip }`

### `crates/zendriver/src/query/` (NEW directory)
- `mod.rs` — `FindBuilder`, `FindAllBuilder`, `QueryScope`
- `selectors.rs` — `SelectorKind` enum + per-kind `resolve_one` / `resolve_many`
- `modifiers.rs` — builder methods for `nth`, `visible_only`, `in_frame`, `timeout`
- `actionability.rs` — `check_visible`, `check_stable`, `check_enabled`, `check_receives_pointer`, `wait_actionable`, `ActionabilityCheck`
- `role.rs` — `AriaRole` enum (20 common + `Other(&'static str)`) + role-to-CSS compilation

### `crates/zendriver/src/input/` (NEW directory)
- `mod.rs` — `InputController` struct + `InputState` + factory
- `bezier.rs` — `BezierPath` + `cubic_bezier` helper
- `mouse.rs` — `move_realistic`, `move_raw`, `click_at`, `MouseButton`, `MouseButtonSet`
- `keyboard.rs` — `Key`, `SpecialKey`, `KeyModifiers`, `type_text_realistic`, `type_text_raw`, `dispatch_char`, `dispatch_special`, `neighbor_key` lookup
- `pointer_state.rs` — `MouseButtonSet` bitflags

### `crates/zendriver-stealth/src/` (modify)
- `input_profile.rs` (NEW) — `InputProfile` struct + `native()` + `spoofed()` constructors
- `lib.rs` — re-export `InputProfile`
- `profile.rs` — `StealthProfile::input_profile()` method returning the appropriate InputProfile

### `crates/zendriver/examples/` (new files in T29)
- 5 Rust ports of Python examples

### `crates/zendriver/tests/` (modify + new)
- `integration_phase3.rs` (NEW, gated `integration-tests`) — selectors, actions, auto-refresh, actionability

---

## Task list (overview)

| # | Title | Files (scope) |
|---|---|---|
| 0 | Module split refactor (element.rs → element/, query.rs → query/) | crates/zendriver/src |
| 1 | Deps + ZendriverError variants | Cargo.toml × 2, error.rs |
| 2 | InputProfile in zendriver-stealth | crates/zendriver-stealth/src/{input_profile.rs, lib.rs, profile.rs} |
| 3 | Key + SpecialKey + KeyModifiers + neighbor_key | crates/zendriver/src/input/keyboard.rs (types only) |
| 4 | BezierPath + cubic_bezier helper | crates/zendriver/src/input/bezier.rs |
| 5 | InputController + per-Browser wiring | crates/zendriver/src/input/mod.rs, browser.rs |
| 6 | MouseButton + MouseButtonSet + Input.dispatchMouseEvent helpers | crates/zendriver/src/input/{mouse.rs, pointer_state.rs} |
| 7 | Keyboard dispatch (dispatch_char, dispatch_special, type_text_realistic, type_text_raw) | crates/zendriver/src/input/keyboard.rs |
| 8 | AriaRole enum + role-to-CSS | crates/zendriver/src/query/role.rs |
| 9 | SelectorKind enum + CSS + XPath resolve | crates/zendriver/src/query/selectors.rs |
| 10 | SelectorKind: Text + TextRegex resolve | crates/zendriver/src/query/selectors.rs |
| 11 | SelectorKind: Role + role_named via Accessibility.getPartialAXTree | crates/zendriver/src/query/selectors.rs |
| 12 | FindBuilder extension (all new selectors + modifiers) | crates/zendriver/src/query/{mod.rs, modifiers.rs} |
| 13 | FindAllBuilder + many/many_or_empty | crates/zendriver/src/query/mod.rs |
| 14 | Actionability checks (visible/stable/enabled/receives_pointer) | crates/zendriver/src/query/actionability.rs |
| 15 | wait_actionable poll loop + NotActionable | crates/zendriver/src/query/actionability.rs |
| 16 | ElementOrigin + Element inner refactor | crates/zendriver/src/element/mod.rs |
| 17 | with_refresh + refresh() impl | crates/zendriver/src/element/refresh.rs |
| 18 | Element reads (attr/attrs/inner_html/bounding_box/is_visible/is_enabled) | crates/zendriver/src/element/reads.rs |
| 19 | Element actions: hover + focus + scroll_into_view | crates/zendriver/src/element/actions.rs |
| 20 | Element actions: click upgrade with ClickOptions | crates/zendriver/src/element/actions.rs |
| 21 | Element actions: set_value + clear + upload_files | crates/zendriver/src/element/actions.rs |
| 22 | Element input: type_text + type_text_raw + press + press_with | crates/zendriver/src/element/input.rs |
| 23 | Element traversal: parent + children | crates/zendriver/src/element/traversal.rs |
| 24 | Element-scoped find + find_all (FindBuilder::new_for_element) | crates/zendriver/src/element/traversal.rs, query/mod.rs |
| 25 | Element::evaluate true isolated world | crates/zendriver/src/element/isolated_eval.rs |
| 26 | Tab::screenshot + Element::screenshot | crates/zendriver/src/{tab.rs, element/screenshot.rs} |
| 27 | P3 integration tests (selectors, actions, auto-refresh, actionability, isolated eval) | crates/zendriver/tests/integration_phase3.rs |
| 28 | Port 5 Python examples to Rust | crates/zendriver/examples/*.rs |
| 29 | Snapshot regen + README touch-up | various |

---

## Task 0: Module split refactor

**Files:**
- Delete: `crates/zendriver/src/element.rs`
- Delete: `crates/zendriver/src/query.rs`
- Create: `crates/zendriver/src/element/mod.rs`
- Create: `crates/zendriver/src/element/{reads,actions,input,traversal,isolated_eval,refresh,screenshot}.rs` (all stubs except mod.rs)
- Create: `crates/zendriver/src/query/mod.rs`
- Create: `crates/zendriver/src/query/{selectors,modifiers,actionability,role}.rs` (all stubs)
- Modify: `crates/zendriver/src/lib.rs` (module declarations unchanged — `pub mod element;` and `pub mod query;` already work for directories)

This is a structural-only refactor. Move existing P2 code into the new layout without changing public API or test behavior.

- [ ] **Step 1: Create new `element/mod.rs` with the P2 content**

Move the entire contents of `crates/zendriver/src/element.rs` into `crates/zendriver/src/element/mod.rs`. No code changes. Verify:

```bash
mkdir -p crates/zendriver/src/element
git mv crates/zendriver/src/element.rs crates/zendriver/src/element/mod.rs
```

- [ ] **Step 2: Create sibling stub files**

Each gets a single line:

```rust
//! Populated in Phase 3.
```

Apply to: `reads.rs`, `actions.rs`, `input.rs`, `traversal.rs`, `isolated_eval.rs`, `refresh.rs`, `screenshot.rs` (all in `crates/zendriver/src/element/`).

- [ ] **Step 3: Add `pub mod` declarations to `element/mod.rs`**

At the top of `crates/zendriver/src/element/mod.rs`, after the existing `//!` doc block:

```rust
pub mod actions;
pub mod input;
pub mod isolated_eval;
pub mod reads;
pub mod refresh;
pub mod screenshot;
pub mod traversal;
```

- [ ] **Step 4: Same procedure for query/**

```bash
mkdir -p crates/zendriver/src/query
git mv crates/zendriver/src/query.rs crates/zendriver/src/query/mod.rs
```

Then create stubs: `selectors.rs`, `modifiers.rs`, `actionability.rs`, `role.rs` (each `//! Populated in Phase 3.`).

Add to top of `crates/zendriver/src/query/mod.rs`:

```rust
pub mod actionability;
pub mod modifiers;
pub mod role;
pub mod selectors;
```

- [ ] **Step 5: Verify nothing broke**

```bash
cargo build --workspace --locked
cargo test --workspace --lib --locked   # all P1+P2 tests still pass (101 unit)
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
refactor(zendriver): split element.rs + query.rs into module directories

Pre-P3 prep — element.rs and query.rs will roughly triple in surface
area during P3. Splitting now means each new method lands in a focused
file instead of one mega-file. No public-API change; no behavior change.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: Deps + ZendriverError variants

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/zendriver/Cargo.toml`
- Modify: `crates/zendriver/src/error.rs`

- [ ] **Step 1: Add workspace deps**

In `[workspace.dependencies]` of root `Cargo.toml`, add:

```toml
regex     = "1"
bitflags  = "2"
rand      = { version = "0.8", default-features = false, features = ["std", "std_rng", "small_rng"] }
```

- [ ] **Step 2: Wire in zendriver crate Cargo.toml**

Add to `crates/zendriver/Cargo.toml` `[dependencies]`:

```toml
regex.workspace    = true
bitflags.workspace = true
rand.workspace     = true
```

- [ ] **Step 3: Add error variants — write test first**

Append to `mod tests` block in `crates/zendriver/src/error.rs`:

```rust
    #[test]
    fn display_element_stale() {
        let e = ZendriverError::ElementStale;
        assert_eq!(e.to_string(), "element is stale: refresh failed or origin not refreshable");
    }

    #[test]
    fn display_not_refreshable() {
        let e = ZendriverError::NotRefreshable;
        assert_eq!(e.to_string(), "element not refreshable (was returned from a JS evaluation)");
    }

    #[test]
    fn display_not_actionable_includes_duration_and_reason() {
        let e = ZendriverError::NotActionable(Duration::from_secs(5), "not visible: display: none".into());
        assert_eq!(e.to_string(), "element not actionable within 5s: not visible: display: none");
    }
```

- [ ] **Step 4: Add the variants to `ZendriverError`**

In `crates/zendriver/src/error.rs`, inside the `pub enum ZendriverError` block, add (before `Serde` to keep error variants grouped):

```rust
    #[error("element is stale: refresh failed or origin not refreshable")]
    ElementStale,

    #[error("element not refreshable (was returned from a JS evaluation)")]
    NotRefreshable,

    #[error("element not actionable within {0:?}: {1}")]
    NotActionable(std::time::Duration, String),
```

- [ ] **Step 5: Verify**

```bash
cargo test -p zendriver --lib error
cargo build --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect existing error tests + 3 new = pass cleanly.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/zendriver/Cargo.toml crates/zendriver/src/error.rs
git commit -m "feat(zendriver): P3 deps (regex/bitflags/rand) + 3 new error variants

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: InputProfile in zendriver-stealth

**Files:**
- Create: `crates/zendriver-stealth/src/input_profile.rs`
- Modify: `crates/zendriver-stealth/src/lib.rs`
- Modify: `crates/zendriver-stealth/src/profile.rs`

- [ ] **Step 1: Write the InputProfile + tests**

Path: `crates/zendriver-stealth/src/input_profile.rs`

```rust
//! Input realism tunables — typing typo rate, mouse jitter, etc.

#[derive(Debug, Clone)]
pub struct InputProfile {
    /// Probability per character of injecting a typo + backspace. 0.0–1.0.
    pub typo_rate: f32,
    /// Range of "thinking pause" duration injected between words (ms).
    pub thinking_pause_ms_range: (u32, u32),
    /// Per-character typing delay range (ms).
    pub per_char_delay_ms_range: (u32, u32),
    /// Mouse cursor speed in pixels per millisecond when moving.
    pub mouse_speed_px_per_ms: f64,
    /// Jitter amplitude (px) applied to Bezier control points.
    pub jitter_amplitude_px: f64,
    /// Probability the mouse overshoots its target before settling.
    pub overshoot_rate: f32,
}

impl InputProfile {
    /// Fast + deterministic. Used by StealthProfile::native and Off.
    #[must_use]
    pub fn native() -> Self {
        Self {
            typo_rate: 0.0,
            thinking_pause_ms_range: (0, 0),
            per_char_delay_ms_range: (0, 0),
            mouse_speed_px_per_ms: 10.0,
            jitter_amplitude_px: 0.0,
            overshoot_rate: 0.0,
        }
    }

    /// chaser-oxide-derived realistic defaults. Used by StealthProfile::spoofed.
    #[must_use]
    pub fn spoofed() -> Self {
        Self {
            typo_rate: 0.03,
            thinking_pause_ms_range: (200, 400),
            per_char_delay_ms_range: (50, 150),
            mouse_speed_px_per_ms: 1.5,
            jitter_amplitude_px: 2.0,
            overshoot_rate: 0.20,
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn native_has_zero_realism_overhead() {
        let p = InputProfile::native();
        assert_eq!(p.typo_rate, 0.0);
        assert_eq!(p.thinking_pause_ms_range, (0, 0));
        assert_eq!(p.per_char_delay_ms_range, (0, 0));
        assert_eq!(p.jitter_amplitude_px, 0.0);
        assert_eq!(p.overshoot_rate, 0.0);
    }

    #[test]
    fn spoofed_has_nonzero_realism() {
        let p = InputProfile::spoofed();
        assert!(p.typo_rate > 0.0);
        assert!(p.per_char_delay_ms_range.0 > 0);
        assert!(p.jitter_amplitude_px > 0.0);
        assert!(p.overshoot_rate > 0.0);
    }

    #[test]
    fn native_mouse_speed_much_faster_than_spoofed() {
        assert!(InputProfile::native().mouse_speed_px_per_ms > InputProfile::spoofed().mouse_speed_px_per_ms);
    }
}
```

- [ ] **Step 2: Add module + re-export**

In `crates/zendriver-stealth/src/lib.rs`, add to module list:

```rust
pub mod input_profile;
```

And to re-exports:

```rust
pub use input_profile::InputProfile;
```

- [ ] **Step 3: Add `StealthProfile::input_profile()`**

In `crates/zendriver-stealth/src/profile.rs`, add to `impl StealthProfile`:

```rust
    /// Returns the input-realism profile appropriate for this stealth profile.
    /// `spoofed` returns realistic timings; `native` and `off` return zero-overhead.
    #[must_use]
    pub fn input_profile(&self) -> crate::InputProfile {
        match self.kind {
            ProfileKind::Spoofed => crate::InputProfile::spoofed(),
            ProfileKind::Native | ProfileKind::Off => crate::InputProfile::native(),
        }
    }
```

Add tests below:

```rust
    #[test]
    fn spoofed_profile_uses_spoofed_input_profile() {
        let ip = StealthProfile::spoofed().input_profile();
        assert!(ip.typo_rate > 0.0);
    }

    #[test]
    fn native_profile_uses_native_input_profile() {
        let ip = StealthProfile::native().input_profile();
        assert_eq!(ip.typo_rate, 0.0);
    }

    #[test]
    fn off_profile_uses_native_input_profile() {
        let ip = StealthProfile::off().input_profile();
        assert_eq!(ip.typo_rate, 0.0);
    }
```

- [ ] **Step 4: Verify**

```bash
cargo test -p zendriver-stealth --lib input_profile
cargo test -p zendriver-stealth --lib profile
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 3 new input_profile tests + 3 new profile tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/
git commit -m "feat(stealth): InputProfile + StealthProfile::input_profile() accessor

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: Key + SpecialKey + KeyModifiers + neighbor_key

**Files:**
- Modify: `crates/zendriver/src/input/keyboard.rs` (currently stub)
- Modify: `crates/zendriver/src/input/mod.rs` (currently stub — add module declarations)
- Modify: `crates/zendriver/src/lib.rs` (add `pub mod input;`)

- [ ] **Step 1: Create input/mod.rs skeleton**

Path: `crates/zendriver/src/input/mod.rs`

```rust
//! Realistic + raw input simulation: mouse paths, keyboard dispatch,
//! per-browser pointer/modifier state.

pub mod bezier;
pub mod keyboard;
pub mod mouse;
pub mod pointer_state;

pub use keyboard::{Key, KeyModifiers, SpecialKey};
pub use mouse::MouseButton;
```

Path: `crates/zendriver/src/lib.rs` — add `pub mod input;` alongside the existing module declarations, and add a re-export line:

```rust
pub use input::{Key, KeyModifiers, SpecialKey, MouseButton};
```

Also create stub files (one-line `//! Populated in Phase 3.`) for: `bezier.rs`, `mouse.rs`, `pointer_state.rs`.

- [ ] **Step 2: Write Key + SpecialKey + KeyModifiers + tests**

Path: `crates/zendriver/src/input/keyboard.rs`

```rust
//! Keyboard types + dispatch (dispatch impl lands in Task 7).

use bitflags::bitflags;

/// A single key dispatch target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Special(SpecialKey),
}

/// Named non-character keys for `Element::press`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialKey {
    Enter, Tab, Escape, Backspace, Delete, Space,
    ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
    Home, End, PageUp, PageDown,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    Insert, CapsLock, NumLock, ScrollLock,
    PrintScreen, Pause, ContextMenu,
}

impl SpecialKey {
    /// Maps to CDP `Input.dispatchKeyEvent` fields (code, key, windowsVirtualKeyCode).
    #[must_use]
    pub fn to_cdp(self) -> (&'static str, &'static str, i32) {
        match self {
            SpecialKey::Enter         => ("Enter",         "Enter",      13),
            SpecialKey::Tab           => ("Tab",           "Tab",         9),
            SpecialKey::Escape        => ("Escape",        "Escape",     27),
            SpecialKey::Backspace     => ("Backspace",     "Backspace",   8),
            SpecialKey::Delete        => ("Delete",        "Delete",     46),
            SpecialKey::Space         => ("Space",         " ",          32),
            SpecialKey::ArrowUp       => ("ArrowUp",       "ArrowUp",    38),
            SpecialKey::ArrowDown     => ("ArrowDown",     "ArrowDown",  40),
            SpecialKey::ArrowLeft     => ("ArrowLeft",     "ArrowLeft",  37),
            SpecialKey::ArrowRight    => ("ArrowRight",    "ArrowRight", 39),
            SpecialKey::Home          => ("Home",          "Home",       36),
            SpecialKey::End           => ("End",           "End",        35),
            SpecialKey::PageUp        => ("PageUp",        "PageUp",     33),
            SpecialKey::PageDown      => ("PageDown",      "PageDown",   34),
            SpecialKey::F1            => ("F1",            "F1",        112),
            SpecialKey::F2            => ("F2",            "F2",        113),
            SpecialKey::F3            => ("F3",            "F3",        114),
            SpecialKey::F4            => ("F4",            "F4",        115),
            SpecialKey::F5            => ("F5",            "F5",        116),
            SpecialKey::F6            => ("F6",            "F6",        117),
            SpecialKey::F7            => ("F7",            "F7",        118),
            SpecialKey::F8            => ("F8",            "F8",        119),
            SpecialKey::F9            => ("F9",            "F9",        120),
            SpecialKey::F10           => ("F10",           "F10",       121),
            SpecialKey::F11           => ("F11",           "F11",       122),
            SpecialKey::F12           => ("F12",           "F12",       123),
            SpecialKey::Insert        => ("Insert",        "Insert",     45),
            SpecialKey::CapsLock      => ("CapsLock",      "CapsLock",   20),
            SpecialKey::NumLock       => ("NumLock",       "NumLock",   144),
            SpecialKey::ScrollLock    => ("ScrollLock",    "ScrollLock",145),
            SpecialKey::PrintScreen   => ("PrintScreen",   "PrintScreen",44),
            SpecialKey::Pause         => ("Pause",         "Pause",      19),
            SpecialKey::ContextMenu   => ("ContextMenu",   "ContextMenu",93),
        }
    }
}

bitflags! {
    /// Composable keyboard modifier bits. Matches CDP modifier-bits encoding.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct KeyModifiers: u8 {
        const ALT     = 0b0001;
        const CTRL    = 0b0010;
        const META    = 0b0100;
        const SHIFT   = 0b1000;
    }
}

impl KeyModifiers {
    /// Encode as the integer modifier bitmask CDP expects.
    #[must_use]
    pub fn cdp_bits(self) -> i32 {
        self.bits() as i32
    }
}

/// Returns a plausible nearby QWERTY key for `c`, or None for non-alphanumeric.
/// Used by realistic typing to inject occasional typos.
pub(crate) fn neighbor_key(c: char, rng: &mut impl rand::Rng) -> Option<char> {
    use rand::seq::SliceRandom;
    let lower = c.to_ascii_lowercase();
    let neighbors: &[char] = match lower {
        'q' => &['w', 'a', 's'],
        'w' => &['q', 'e', 'a', 's', 'd'],
        'e' => &['w', 'r', 's', 'd', 'f'],
        'r' => &['e', 't', 'd', 'f', 'g'],
        't' => &['r', 'y', 'f', 'g', 'h'],
        'y' => &['t', 'u', 'g', 'h', 'j'],
        'u' => &['y', 'i', 'h', 'j', 'k'],
        'i' => &['u', 'o', 'j', 'k', 'l'],
        'o' => &['i', 'p', 'k', 'l'],
        'p' => &['o', 'l'],
        'a' => &['q', 'w', 's', 'z'],
        's' => &['a', 'd', 'w', 'e', 'z', 'x'],
        'd' => &['s', 'f', 'e', 'r', 'x', 'c'],
        'f' => &['d', 'g', 'r', 't', 'c', 'v'],
        'g' => &['f', 'h', 't', 'y', 'v', 'b'],
        'h' => &['g', 'j', 'y', 'u', 'b', 'n'],
        'j' => &['h', 'k', 'u', 'i', 'n', 'm'],
        'k' => &['j', 'l', 'i', 'o', 'm'],
        'l' => &['k', 'o', 'p'],
        'z' => &['a', 's', 'x'],
        'x' => &['z', 'c', 's', 'd'],
        'c' => &['x', 'v', 'd', 'f'],
        'v' => &['c', 'b', 'f', 'g'],
        'b' => &['v', 'n', 'g', 'h'],
        'n' => &['b', 'm', 'h', 'j'],
        'm' => &['n', 'j', 'k'],
        _   => return None,
    };
    let pick = neighbors.choose(rng)?;
    if c.is_ascii_uppercase() {
        Some(pick.to_ascii_uppercase())
    } else {
        Some(*pick)
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn modifiers_compose_with_bitor() {
        let m = KeyModifiers::CTRL | KeyModifiers::SHIFT;
        assert!(m.contains(KeyModifiers::CTRL));
        assert!(m.contains(KeyModifiers::SHIFT));
        assert!(!m.contains(KeyModifiers::ALT));
    }

    #[test]
    fn modifiers_cdp_bits_match_encoding() {
        assert_eq!(KeyModifiers::ALT.cdp_bits(), 1);
        assert_eq!(KeyModifiers::CTRL.cdp_bits(), 2);
        assert_eq!(KeyModifiers::META.cdp_bits(), 4);
        assert_eq!(KeyModifiers::SHIFT.cdp_bits(), 8);
        assert_eq!((KeyModifiers::CTRL | KeyModifiers::SHIFT).cdp_bits(), 10);
    }

    #[test]
    fn special_key_enter_maps_to_cdp_13() {
        let (code, key, vk) = SpecialKey::Enter.to_cdp();
        assert_eq!(code, "Enter");
        assert_eq!(key, "Enter");
        assert_eq!(vk, 13);
    }

    #[test]
    fn neighbor_key_returns_nearby_for_alpha() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let n = neighbor_key('r', &mut rng).expect("r has neighbors");
        assert!(['e', 't', 'd', 'f', 'g'].contains(&n));
    }

    #[test]
    fn neighbor_key_preserves_case() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let n = neighbor_key('R', &mut rng).expect("R has neighbors");
        assert!(n.is_ascii_uppercase());
    }

    #[test]
    fn neighbor_key_returns_none_for_non_alpha() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        assert!(neighbor_key('5', &mut rng).is_none());
        assert!(neighbor_key('!', &mut rng).is_none());
        assert!(neighbor_key(' ', &mut rng).is_none());
    }
}
```

- [ ] **Step 3: Verify**

```bash
cargo test -p zendriver --lib input::keyboard::tests
cargo build --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 6 pass.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver/src/input crates/zendriver/src/lib.rs
git commit -m "feat(zendriver): Key + SpecialKey + KeyModifiers + neighbor_key table

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: BezierPath + cubic_bezier helper

**Files:**
- Modify: `crates/zendriver/src/input/bezier.rs`

- [ ] **Step 1: Implement + tests**

```rust
//! Cubic Bezier path generation for realistic mouse movement.

#[derive(Debug, Clone)]
pub(crate) struct BezierPath {
    pub points: Vec<(f64, f64)>,
}

impl BezierPath {
    /// Build a cubic Bezier from `start` to `end`, with control points at
    /// ~25% and ~75% positions perturbed by up to `jitter_px`. The number of
    /// sample points scales with distance (one per ~5px, min 8, max 60).
    pub fn build(
        start: (f64, f64),
        end: (f64, f64),
        jitter_px: f64,
        rng: &mut impl rand::Rng,
    ) -> Self {
        let dx = end.0 - start.0;
        let dy = end.1 - start.1;
        let jitter = |rng: &mut dyn rand::RngCore| -> f64 {
            if jitter_px == 0.0 { return 0.0; }
            rand::Rng::gen_range(rng, -jitter_px..jitter_px)
        };
        let c1 = (
            start.0 + dx * 0.25 + jitter(rng),
            start.1 + dy * 0.25 + jitter(rng),
        );
        let c2 = (
            start.0 + dx * 0.75 + jitter(rng),
            start.1 + dy * 0.75 + jitter(rng),
        );
        let distance = (dx * dx + dy * dy).sqrt();
        let n_points = ((distance / 5.0).max(8.0) as usize).min(60);
        let mut points = Vec::with_capacity(n_points + 1);
        for i in 0..=n_points {
            let t = i as f64 / n_points as f64;
            points.push(cubic_bezier(start, c1, c2, end, t));
        }
        Self { points }
    }
}

fn cubic_bezier(
    p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), t: f64,
) -> (f64, f64) {
    let u = 1.0 - t;
    let (uu, tt) = (u * u, t * t);
    let (uuu, ttt) = (uu * u, tt * t);
    (
        uuu * p0.0 + 3.0 * uu * t * p1.0 + 3.0 * u * tt * p2.0 + ttt * p3.0,
        uuu * p0.1 + 3.0 * uu * t * p1.1 + 3.0 * u * tt * p2.1 + ttt * p3.1,
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn path_starts_at_start_and_ends_at_end() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let path = BezierPath::build((0.0, 0.0), (100.0, 100.0), 0.0, &mut rng);
        let first = path.points.first().expect("non-empty");
        let last = path.points.last().expect("non-empty");
        assert!((first.0 - 0.0).abs() < 1e-9);
        assert!((first.1 - 0.0).abs() < 1e-9);
        assert!((last.0 - 100.0).abs() < 1e-9);
        assert!((last.1 - 100.0).abs() < 1e-9);
    }

    #[test]
    fn path_point_count_scales_with_distance() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let short = BezierPath::build((0.0, 0.0), (10.0, 10.0), 0.0, &mut rng);
        let long  = BezierPath::build((0.0, 0.0), (1000.0, 1000.0), 0.0, &mut rng);
        assert!(short.points.len() < long.points.len());
        assert!(short.points.len() >= 9);    // min 8 + the start endpoint = 9
        assert!(long.points.len() <= 61);    // max 60 + start endpoint = 61
    }

    #[test]
    fn zero_jitter_produces_smooth_path() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let path = BezierPath::build((0.0, 0.0), (100.0, 0.0), 0.0, &mut rng);
        // With zero jitter + a straight line, y should stay 0 (within float tolerance).
        for (_, y) in &path.points {
            assert!(y.abs() < 1e-9, "non-zero y on straight line: {y}");
        }
    }

    #[test]
    fn seeded_rng_produces_deterministic_path_snapshot() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let path = BezierPath::build((0.0, 0.0), (100.0, 100.0), 2.0, &mut rng);
        insta::assert_yaml_snapshot!("bezier_path_seed_42", &path.points);
    }
}
```

- [ ] **Step 2: Verify (insta accept + re-run)**

```bash
cargo test -p zendriver --lib input::bezier::tests   # fails first (snapshot)
cargo insta accept
cargo test -p zendriver --lib input::bezier::tests   # 4 pass
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/input/bezier.rs crates/zendriver/src/snapshots
git commit -m "feat(zendriver): BezierPath cubic-curve mouse path generator

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: InputController + per-Browser wiring

**Files:**
- Modify: `crates/zendriver/src/input/mod.rs`
- Modify: `crates/zendriver/src/input/pointer_state.rs`
- Modify: `crates/zendriver/src/browser.rs`

- [ ] **Step 1: MouseButtonSet bitflags**

Path: `crates/zendriver/src/input/pointer_state.rs`

```rust
//! Pointer state bitflags + helpers.

use bitflags::bitflags;

bitflags! {
    /// Mouse buttons currently held down. Tracked by InputController so
    /// drag/release sequences work.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct MouseButtonSet: u8 {
        const LEFT    = 0b00001;
        const RIGHT   = 0b00010;
        const MIDDLE  = 0b00100;
        const BACK    = 0b01000;
        const FORWARD = 0b10000;
    }
}
```

- [ ] **Step 2: InputController + InputState**

Path: `crates/zendriver/src/input/mod.rs` — replace the stub with:

```rust
//! Realistic + raw input simulation: mouse paths, keyboard dispatch,
//! per-browser pointer/modifier state.

use std::sync::Arc;

use rand::SeedableRng;
use tokio::sync::Mutex;
use zendriver_stealth::InputProfile;

use crate::input::keyboard::KeyModifiers;
use crate::input::pointer_state::MouseButtonSet;

pub mod bezier;
pub mod keyboard;
pub mod mouse;
pub mod pointer_state;

pub use keyboard::{Key, KeyModifiers as ExportedKeyModifiers, SpecialKey};
pub use mouse::MouseButton;

/// Per-Browser input state holder. Wraps InputState + an InputProfile.
pub struct InputController {
    pub(crate) state: Mutex<InputState>,
    pub(crate) profile: InputProfile,
}

pub(crate) struct InputState {
    pub pointer_x: f64,
    pub pointer_y: f64,
    pub buttons_held: MouseButtonSet,
    pub modifiers_held: KeyModifiers,
    pub rng: rand::rngs::SmallRng,
}

impl InputController {
    #[must_use]
    pub fn new(profile: InputProfile) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(InputState {
                pointer_x: 0.0,
                pointer_y: 0.0,
                buttons_held: MouseButtonSet::empty(),
                modifiers_held: KeyModifiers::empty(),
                rng: rand::rngs::SmallRng::from_entropy(),
            }),
            profile,
        })
    }

    /// Test-only constructor with a seeded RNG for deterministic Bezier paths.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn new_with_seed(profile: InputProfile, seed: u64) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(InputState {
                pointer_x: 0.0,
                pointer_y: 0.0,
                buttons_held: MouseButtonSet::empty(),
                modifiers_held: KeyModifiers::empty(),
                rng: rand::rngs::SmallRng::seed_from_u64(seed),
            }),
            profile,
        })
    }
}
```

Drop the conflicting `pub use keyboard::KeyModifiers` — re-exporting twice (once direct, once renamed) is noisy. Keep only the `pub use keyboard::{Key, KeyModifiers, SpecialKey}` line; lib.rs handles user-facing re-export.

Fix `mod.rs` re-exports to be clean:

```rust
pub use keyboard::{Key, KeyModifiers, SpecialKey};
pub use mouse::MouseButton;
```

- [ ] **Step 3: Wire InputController into Browser**

In `crates/zendriver/src/browser.rs`:

Add to imports:

```rust
use crate::input::InputController;
```

Add field to `BrowserInner`:

```rust
pub(crate) input: Arc<InputController>,
```

In `BrowserBuilder::launch`, construct InputController after fingerprint resolution. Find the block where the StealthObserver is constructed; add after it:

```rust
let input_profile = self.stealth.as_ref()
    .map_or_else(zendriver_stealth::InputProfile::native, |sp| sp.input_profile());
let input = InputController::new(input_profile);
```

And in the `Browser { inner: Arc::new(BrowserInner { ... }) }` constructor at the bottom of launch, include the `input` field:

```rust
input,
```

Add accessor on `Browser`:

```rust
impl Browser {
    #[must_use]
    pub fn input(&self) -> &Arc<InputController> {
        &self.inner.input
    }
}
```

- [ ] **Step 4: Tab gains browser accessor**

In `crates/zendriver/src/tab.rs`, add a way for Element methods to reach the InputController. Tab needs to know its Browser. Two options:
- (a) Tab stores `Weak<BrowserInner>`; gets `.upgrade()` to call input
- (b) Pass InputController into Tab at construction

Pick (a) — cleaner upgrade path; Browser already owns the Tab via main_tab so the cycle is fine if it's Weak.

Modify `TabInner`:

```rust
pub(crate) struct TabInner {
    pub(crate) session: SessionHandle,
    pub(crate) isolated_world: tokio::sync::Mutex<IsolatedWorldCache>,
    pub(crate) browser: std::sync::Weak<crate::browser::BrowserInner>,
}
```

Update `Tab::new` to accept the weak ref:

```rust
pub(crate) fn new(session: SessionHandle, browser: std::sync::Weak<crate::browser::BrowserInner>) -> Self {
    Self {
        inner: Arc::new(TabInner {
            session,
            isolated_world: tokio::sync::Mutex::new(IsolatedWorldCache::default()),
            browser,
        }),
    }
}

impl Tab {
    /// Returns a strong handle to the owning Browser's input controller, if alive.
    pub(crate) fn input(&self) -> Option<Arc<InputController>> {
        self.inner.browser.upgrade().map(|b| b.input.clone())
    }
}
```

In `browser.rs`, `BrowserBuilder::launch` currently does `let session = SessionHandle::new(...); let main_tab = Tab::new(session);`. Now needs the Weak. Since `BrowserInner` is built after the Tab construction, restructure:

```rust
// Build BrowserInner with a placeholder, then construct Tab with weak ref.
// Simplest: use Arc::new_cyclic.
let browser = Browser {
    inner: Arc::new_cyclic(|weak: &std::sync::Weak<BrowserInner>| {
        let session = SessionHandle::new(conn.clone(), session_id);
        let main_tab = Tab::new(session, weak.clone());
        BrowserInner {
            conn,
            main_tab,
            child: tokio::sync::Mutex::new(Some(child)),
            _user_data: owned_tmp,
            input,
        }
    }),
};
```

`Arc::new_cyclic` is the canonical pattern for this kind of self-referential structure.

- [ ] **Step 5: Verify nothing P1/P2 broke**

```bash
cargo build --workspace --locked
cargo test --workspace --lib --locked   # all P1+P2 tests must still pass; Tab::new now takes 2 args
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

If any P1/P2 test constructs Tab directly via `Tab::new(session)`, update to `Tab::new(session, std::sync::Weak::new())`. The Weak::new() yields a never-upgradable Weak, which is fine for tests that don't call `tab.input()`.

- [ ] **Step 6: Commit**

```bash
git add crates/zendriver/src/input crates/zendriver/src/browser.rs crates/zendriver/src/tab.rs
git commit -m "feat(zendriver): InputController + per-Browser wiring via Arc::new_cyclic

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: MouseButton + mouse dispatch

**Files:**
- Modify: `crates/zendriver/src/input/mouse.rs`

- [ ] **Step 1: Implement + tests**

```rust
//! Realistic + raw mouse dispatch.

use std::time::Duration;

use serde_json::json;

use crate::error::Result;
use crate::input::bezier::BezierPath;
use crate::input::keyboard::KeyModifiers;
use crate::input::InputController;
use crate::tab::Tab;

/// CDP mouse button names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left, Middle, Right, Back, Forward,
}

impl MouseButton {
    #[must_use]
    pub fn cdp_str(self) -> &'static str {
        match self {
            MouseButton::Left    => "left",
            MouseButton::Middle  => "middle",
            MouseButton::Right   => "right",
            MouseButton::Back    => "back",
            MouseButton::Forward => "forward",
        }
    }
}

/// Move the cursor from its current position to `(target_x, target_y)` along
/// a Bezier path with realistic per-segment delay. Updates InputController
/// state to the target position on success.
pub(crate) async fn move_realistic(
    input: &InputController,
    tab: &Tab,
    target_x: f64,
    target_y: f64,
) -> Result<()> {
    let (start, path, modifiers, segment_delay) = {
        let mut state = input.state.lock().await;
        let start = (state.pointer_x, state.pointer_y);
        let modifiers = state.modifiers_held;
        let path = BezierPath::build(
            start, (target_x, target_y),
            input.profile.jitter_amplitude_px,
            &mut state.rng,
        );
        let segment_delay = if input.profile.mouse_speed_px_per_ms > 0.0 {
            Duration::from_micros(((5.0 / input.profile.mouse_speed_px_per_ms) * 1000.0) as u64)
        } else {
            Duration::ZERO
        };
        (start, path, modifiers, segment_delay)
    };
    let _ = start;
    let modifier_bits = modifiers.cdp_bits();
    for &(x, y) in &path.points {
        tab.session().call("Input.dispatchMouseEvent", json!({
            "type": "mouseMoved", "x": x, "y": y,
            "modifiers": modifier_bits,
        })).await?;
        if !segment_delay.is_zero() {
            tokio::time::sleep(segment_delay).await;
        }
    }
    let mut state = input.state.lock().await;
    state.pointer_x = target_x;
    state.pointer_y = target_y;
    Ok(())
}

/// Direct move without interpolation. Single dispatchMouseEvent.
pub(crate) async fn move_raw(
    input: &InputController,
    tab: &Tab,
    target_x: f64,
    target_y: f64,
) -> Result<()> {
    let modifier_bits = {
        let s = input.state.lock().await;
        s.modifiers_held.cdp_bits()
    };
    tab.session().call("Input.dispatchMouseEvent", json!({
        "type": "mouseMoved", "x": target_x, "y": target_y,
        "modifiers": modifier_bits,
    })).await?;
    let mut state = input.state.lock().await;
    state.pointer_x = target_x;
    state.pointer_y = target_y;
    Ok(())
}

/// Dispatch a click at `(target_x, target_y)` with `button` and `click_count`.
/// If `realistic`, prefixes with Bezier move; otherwise direct teleport.
pub(crate) async fn click_at(
    input: &InputController,
    tab: &Tab,
    target_x: f64,
    target_y: f64,
    button: MouseButton,
    click_count: u32,
    realistic: bool,
) -> Result<()> {
    if realistic {
        move_realistic(input, tab, target_x, target_y).await?;
    } else {
        move_raw(input, tab, target_x, target_y).await?;
    }
    let modifier_bits = {
        let s = input.state.lock().await;
        s.modifiers_held.cdp_bits()
    };
    tab.session().call("Input.dispatchMouseEvent", json!({
        "type": "mousePressed",
        "x": target_x, "y": target_y,
        "button": button.cdp_str(),
        "clickCount": click_count,
        "modifiers": modifier_bits,
    })).await?;
    tab.session().call("Input.dispatchMouseEvent", json!({
        "type": "mouseReleased",
        "x": target_x, "y": target_y,
        "button": button.cdp_str(),
        "clickCount": click_count,
        "modifiers": modifier_bits,
    })).await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn mouse_button_cdp_strings_match_chrome() {
        assert_eq!(MouseButton::Left.cdp_str(),    "left");
        assert_eq!(MouseButton::Right.cdp_str(),   "right");
        assert_eq!(MouseButton::Middle.cdp_str(),  "middle");
        assert_eq!(MouseButton::Back.cdp_str(),    "back");
        assert_eq!(MouseButton::Forward.cdp_str(), "forward");
    }
    // Note: dispatch fns are async + need a Tab + MockConnection — exercised in T20 click tests.
}
```

- [ ] **Step 2: Verify**

```bash
cargo test -p zendriver --lib input::mouse::tests
cargo build --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 1 pass.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/input/mouse.rs
git commit -m "feat(zendriver): MouseButton + move_realistic/move_raw/click_at dispatch

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7: Keyboard dispatch (type_text_realistic + type_text_raw + dispatch helpers)

**Files:**
- Modify: `crates/zendriver/src/input/keyboard.rs` (append below existing types)

- [ ] **Step 1: Append dispatch functions + tests**

After the existing types in `crates/zendriver/src/input/keyboard.rs`, append:

```rust
use std::time::Duration;
use serde_json::json;

use crate::error::Result;
use crate::input::InputController;
use crate::tab::Tab;

/// Dispatch a single character via Input.dispatchKeyEvent (keyDown + keyUp).
pub(crate) async fn dispatch_char(tab: &Tab, c: char, modifier_bits: i32) -> Result<()> {
    let s = c.to_string();
    tab.session().call("Input.dispatchKeyEvent", json!({
        "type": "keyDown", "text": &s, "key": &s,
        "modifiers": modifier_bits,
    })).await?;
    tab.session().call("Input.dispatchKeyEvent", json!({
        "type": "keyUp", "text": &s, "key": &s,
        "modifiers": modifier_bits,
    })).await?;
    Ok(())
}

/// Dispatch a named special key (Enter, Tab, etc).
pub(crate) async fn dispatch_special(tab: &Tab, k: SpecialKey, modifier_bits: i32) -> Result<()> {
    let (code, key, vk) = k.to_cdp();
    tab.session().call("Input.dispatchKeyEvent", json!({
        "type": "rawKeyDown",
        "code": code, "key": key,
        "windowsVirtualKeyCode": vk,
        "modifiers": modifier_bits,
    })).await?;
    tab.session().call("Input.dispatchKeyEvent", json!({
        "type": "keyUp",
        "code": code, "key": key,
        "windowsVirtualKeyCode": vk,
        "modifiers": modifier_bits,
    })).await?;
    Ok(())
}

/// Type `text` with realistic per-character timing, occasional typos, and
/// inter-word "thinking" pauses pulled from the InputProfile.
pub(crate) async fn type_text_realistic(
    input: &InputController,
    tab: &Tab,
    text: &str,
) -> Result<()> {
    let profile = input.profile.clone();
    for ch in text.chars() {
        let (per_char_delay_ms, mods, do_typo, typo_char, thinking_pause_ms) = {
            let mut s = input.state.lock().await;
            let per_char = if profile.per_char_delay_ms_range.0 == 0 && profile.per_char_delay_ms_range.1 == 0 {
                0
            } else {
                rand::Rng::gen_range(&mut s.rng,
                    profile.per_char_delay_ms_range.0..=profile.per_char_delay_ms_range.1)
            };
            let do_typo = profile.typo_rate > 0.0
                && rand::Rng::gen::<f32>(&mut s.rng) < profile.typo_rate;
            let typo_char = if do_typo { neighbor_key(ch, &mut s.rng) } else { None };
            let thinking = if ch == ' '
                && profile.thinking_pause_ms_range.0 > 0
                && rand::Rng::gen::<f32>(&mut s.rng) < 0.05
            {
                rand::Rng::gen_range(&mut s.rng,
                    profile.thinking_pause_ms_range.0..=profile.thinking_pause_ms_range.1)
            } else { 0 };
            (per_char, s.modifiers_held.cdp_bits(), do_typo, typo_char, thinking)
        };
        if per_char_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(per_char_delay_ms as u64)).await;
        }
        if thinking_pause_ms > 0 {
            tokio::time::sleep(Duration::from_millis(thinking_pause_ms as u64)).await;
        }
        if do_typo {
            if let Some(wrong) = typo_char {
                dispatch_char(tab, wrong, mods).await?;
                tokio::time::sleep(Duration::from_millis(80)).await;
                dispatch_special(tab, SpecialKey::Backspace, mods).await?;
            }
        }
        dispatch_char(tab, ch, mods).await?;
    }
    Ok(())
}

/// Type `text` as fast as possible — no delays, no typos.
pub(crate) async fn type_text_raw(
    input: &InputController,
    tab: &Tab,
    text: &str,
) -> Result<()> {
    let mods = input.state.lock().await.modifiers_held.cdp_bits();
    for ch in text.chars() {
        dispatch_char(tab, ch, mods).await?;
    }
    Ok(())
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use zendriver_stealth::InputProfile;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;
    use serde_json::Value;

    #[tokio::test]
    async fn type_text_raw_emits_keydown_keyup_per_char() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let input = InputController::new_with_seed(InputProfile::native(), 42);

        let fut = tokio::spawn({
            let input = input.clone();
            let tab = tab.clone();
            async move { type_text_raw(&input, &tab, "ab").await }
        });

        for ch in ['a', 'a', 'b', 'b'] {  // 4 events: a-down a-up b-down b-up
            let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
            let last = mock.last_sent();
            let text = last["params"]["text"].as_str().unwrap();
            assert_eq!(text, ch.to_string());
            mock.reply(id, Value::Null).await;
        }
        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn dispatch_special_enter_emits_correct_cdp_fields() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());

        let fut = tokio::spawn({
            let tab = tab.clone();
            async move { dispatch_special(&tab, SpecialKey::Enter, 0).await }
        });

        let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
        let last = mock.last_sent();
        assert_eq!(last["params"]["type"], "rawKeyDown");
        assert_eq!(last["params"]["key"], "Enter");
        assert_eq!(last["params"]["windowsVirtualKeyCode"], 13);
        mock.reply(id, Value::Null).await;

        let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
        let last = mock.last_sent();
        assert_eq!(last["params"]["type"], "keyUp");
        mock.reply(id, Value::Null).await;
        fut.await.unwrap().unwrap();
        conn.shutdown();
    }
}
```

- [ ] **Step 2: Verify**

```bash
cargo test -p zendriver --lib input::keyboard::dispatch_tests
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 2 pass.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/input/keyboard.rs
git commit -m "feat(zendriver): keyboard dispatch (type_text_realistic + type_text_raw + helpers)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8: AriaRole enum + role-to-CSS

**Files:**
- Modify: `crates/zendriver/src/query/role.rs`

- [ ] **Step 1: Implement + tests**

```rust
//! ARIA role enum + role-to-CSS-selector compilation.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AriaRole {
    Button,
    Link,
    Textbox,
    Combobox,
    Checkbox,
    Radio,
    Tab,
    Menu,
    Menuitem,
    Dialog,
    Heading,
    Banner,
    Navigation,
    Main,
    Article,
    List,
    Listitem,
    Row,
    Cell,
    Columnheader,
    Rowheader,
    /// Escape hatch for ARIA roles not in the enum above.
    Other(&'static str),
}

impl AriaRole {
    /// Compile to a CSS attribute selector. Tag-implicit roles (e.g. `<button>`
    /// implies role=button) are NOT auto-included; users querying by role get
    /// only explicit `[role="..."]` matches. This avoids surprising matches
    /// against tag-implicit elements that may have different accessibility
    /// behavior.
    #[must_use]
    pub fn to_css(self) -> String {
        let name = match self {
            AriaRole::Button       => "button",
            AriaRole::Link         => "link",
            AriaRole::Textbox      => "textbox",
            AriaRole::Combobox     => "combobox",
            AriaRole::Checkbox     => "checkbox",
            AriaRole::Radio        => "radio",
            AriaRole::Tab          => "tab",
            AriaRole::Menu         => "menu",
            AriaRole::Menuitem     => "menuitem",
            AriaRole::Dialog       => "dialog",
            AriaRole::Heading      => "heading",
            AriaRole::Banner       => "banner",
            AriaRole::Navigation   => "navigation",
            AriaRole::Main         => "main",
            AriaRole::Article      => "article",
            AriaRole::List         => "list",
            AriaRole::Listitem     => "listitem",
            AriaRole::Row          => "row",
            AriaRole::Cell         => "cell",
            AriaRole::Columnheader => "columnheader",
            AriaRole::Rowheader    => "rowheader",
            AriaRole::Other(s)     => s,
        };
        format!("[role=\"{name}\"]")
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn button_compiles_to_attribute_selector() {
        assert_eq!(AriaRole::Button.to_css(), r#"[role="button"]"#);
    }

    #[test]
    fn other_role_uses_passed_name() {
        assert_eq!(AriaRole::Other("tooltip").to_css(), r#"[role="tooltip"]"#);
    }

    #[test]
    fn all_roles_compile_snapshot() {
        let all = [
            AriaRole::Button, AriaRole::Link, AriaRole::Textbox, AriaRole::Combobox,
            AriaRole::Checkbox, AriaRole::Radio, AriaRole::Tab, AriaRole::Menu,
            AriaRole::Menuitem, AriaRole::Dialog, AriaRole::Heading, AriaRole::Banner,
            AriaRole::Navigation, AriaRole::Main, AriaRole::Article, AriaRole::List,
            AriaRole::Listitem, AriaRole::Row, AriaRole::Cell, AriaRole::Columnheader,
            AriaRole::Rowheader,
        ];
        let css: Vec<String> = all.iter().map(|r| r.to_css()).collect();
        insta::assert_yaml_snapshot!("aria_role_css_compilation", css);
    }
}
```

Re-export in `query/mod.rs` (append):

```rust
pub use role::AriaRole;
```

And in `crates/zendriver/src/lib.rs`:

```rust
pub use query::AriaRole;
```

- [ ] **Step 2: Verify + accept snapshot**

```bash
cargo test -p zendriver --lib query::role::tests
cargo insta accept
cargo test -p zendriver --lib query::role::tests
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expect 3 pass.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/query/role.rs crates/zendriver/src/query/mod.rs crates/zendriver/src/lib.rs crates/zendriver/src/snapshots
git commit -m "feat(zendriver): AriaRole enum + role-to-CSS compilation

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

---

# Tasks 9-29 — Compact form

**Note to implementer:** Tasks 9-29 below are intentionally compact. Each task references the spec section that contains the full code + behavior contract. The patterns from T0-T8 (TDD: write tests with mocked CDP first, then impl, run `cargo test -p zendriver --lib` + clippy, commit per file scope) carry forward. The spec at `docs/superpowers/specs/2026-05-23-zendriver-rs-phase3-elements-design.md` is the source of truth — when in doubt, follow it.

For each task: implement per spec, add tests covering the public behavior + happy-path + 1-2 error paths, verify build + clippy clean, commit with the message shown.

---

## Task 9: SelectorKind + CSS + XPath resolve

**Files:** `crates/zendriver/src/query/selectors.rs`
**Spec:** Section "FindBuilder selector compilation" — CSS uses `Runtime.evaluate` with `document.querySelector` (Tab scope) or `Runtime.callFunctionOn` with `this.querySelector` (Element scope). XPath uses `document.evaluate` with `XPathResult.FIRST_ORDERED_NODE_TYPE` for one and `ORDERED_NODE_SNAPSHOT_TYPE` for many.

**Implement:** `SelectorKind` enum (with stubs for Text/TextRegex/Role for T10-T11), `RemoteRef`, `QueryScope`, `resolve_one`/`resolve_many` dispatch + CSS + XPath impls + helper fns `extract_node_ref`/`extract_array_refs` using `DOM.describeNode` to fetch `backendNodeId`.

**Verify:** mock-driven test confirming CSS one sends `document.querySelector` with the selector.
**Commit:** `feat(zendriver): SelectorKind + CSS + XPath resolution`

## Task 10: Text + TextRegex selectors

**Files:** `crates/zendriver/src/query/selectors.rs` (replace stubs)
**Spec:** Section "FindBuilder selector compilation" — text uses XPath `[normalize-space(.)='<needle>']` for exact match; case-insensitive substring uses a JS tree walk filtering by `.innerText.toLowerCase().includes(needle)`. text_regex builds a JS `new RegExp(pat, flags)` and filters.

**Implement:** replace the four text-related stubs from T9 with the JS-builder helpers `build_text_query_js` + `build_text_regex_js`.

**Verify:** two mock-driven tests asserting the eval'd JS expression contains the expected substring + flags.
**Commit:** `feat(zendriver): Text + TextRegex selector resolution`

## Task 11: Role + role_named via Accessibility

**Files:** `crates/zendriver/src/query/selectors.rs`
**Spec:** Section "FindBuilder selector compilation" — role compiles to `[role="..."]` CSS attribute selector; `role_named` post-filters CSS candidates by calling `Accessibility.getPartialAXTree { backendNodeId }` per candidate and case-insensitive-substring-matching on `name.value`.

**Implement:** `resolve_role_one`/`resolve_role_many` + `accessible_name_matches` helper.

**Verify:** mock-driven test that Role(Button, None) dispatches `Runtime.evaluate` containing `[role=\"button\"]`.
**Commit:** `feat(zendriver): Role + role_named selectors via Accessibility.getPartialAXTree`

## Task 12: FindBuilder full extension

**Files:** `crates/zendriver/src/query/mod.rs`, `crates/zendriver/src/tab.rs`
**Spec:** Section "FindBuilder chain rules"

**Implement:** Replace P1 FindBuilder with the extended version having `xpath/text/text_exact/text_regex/text_regex_with_flags/role/role_named/nth/visible_only/in_frame/timeout` methods + `one`/`one_or_none` terminals (poll-loop + visible_only filter + nth pick). `text_regex(re: regex::Regex)` extracts `re.as_str()` as pattern with empty flags; `text_regex_with_flags(pat, flags)` for explicit flags.

**Tab::find:** update to call `FindBuilder::new_for_tab(self)`.

**Verify:** build clean (full integration tests land in T27).
**Commit:** `feat(zendriver): FindBuilder full extension with all P3 selector + modifier methods`

## Task 13: FindAllBuilder + many/many_or_empty

**Files:** `crates/zendriver/src/query/mod.rs`, `crates/zendriver/src/tab.rs`
**Spec:** Section "FindBuilder chain rules" (last paragraph)

**Implement:** `FindAllBuilder` mirroring FindBuilder selectors + modifiers; `many()` errors on empty result, `many_or_empty()` returns empty Vec. `Tab::find_all` accessor.

**Verify:** build clean.
**Commit:** `feat(zendriver): FindAllBuilder + many/many_or_empty + Tab::find_all`

## Task 14: Actionability checks

**Files:** `crates/zendriver/src/query/actionability.rs`
**Spec:** Section "Actionability checks"

**Implement:** `ActionabilityCheck` struct with FULL/VISIBLE_ONLY/TEXT_INPUT constants + `check_visible`/`check_stable`/`check_enabled`/`check_receives_pointer` async fns. Each calls a JS function via `Element::call_on_main` returning bool. Implementations per spec — `check_visible` checks `isConnected + offsetParent + bbox + visibility + opacity`; `check_stable` compares bbox across 2 animation frames; `check_enabled` checks `disabled` + `aria-disabled`; `check_receives_pointer` walks ancestors of `document.elementFromPoint(bbox_center)`.

**Verify:** build clean; integration coverage in T27.
**Commit:** `feat(zendriver): actionability checks (visible/stable/enabled/receives_pointer)`

## Task 15: wait_actionable poll + NotActionable

**Files:** `crates/zendriver/src/query/actionability.rs` (append)
**Spec:** Section "Actionability checks" (last paragraph)

**Implement:** `wait_actionable(el, require, timeout)` poll-loop at 50ms intervals; on deadline emit `ZendriverError::NotActionable(timeout, reason)` where `reason` is the first-failed check's description.

**Verify:** build clean.
**Commit:** `feat(zendriver): wait_actionable poll loop emitting NotActionable on timeout`

## Task 16: ElementOrigin + Element inner refactor

**Files:** `crates/zendriver/src/element/mod.rs`
**Spec:** Section "Element inner state"

**Implement:** `Element` gains `ElementOrigin` enum tracking (Query{scope_kind, selector, nth} / Traversal{parent, kind} / Origin returned from JS expression = not-refreshable). `Element::synthesize_query` + `Element::synthesize_origin_jsret` (named e.g. `synthesize_jsret` to avoid the eval word in the name; or simply `from_jsret(tab, backend, remote)`) constructors. Add `call_on_main` helper used by reads + actionability checks.

**Migrate existing P2 element_tests:** P2 constructed elements via `Element::new(tab, backend, remote)`. Rename to `Element::from_jsret(tab, backend, remote)` and update all callers. Semantically equivalent: from_jsret = origin is JS-expression = not refreshable.

**Verify:** all P1+P2 tests still pass (101 unit tests).
**Commit:** `refactor(zendriver): Element gains ElementOrigin for auto-refresh tracking`

## Task 17: with_refresh wrapper + refresh()

**Files:** `crates/zendriver/src/element/refresh.rs`
**Spec:** Section "Auto-refresh flow"

**Implement:** `Element::refresh()` re-resolves via stored ElementOrigin; `with_refresh(op)` retries once on stale-node error. `is_stale_node_error(e)` helper covers Navigation messages containing "No node with given id" / "Cannot find context" + Cdp variant with same message + ElementStale variant.

**For Traversal origin in P3:** return NotRefreshable (full traversal-chain refresh deferred to P4).

**Verify:** unit test on `is_stale_node_error` matching expected error shapes.
**Commit:** `feat(zendriver): Element::refresh + with_refresh wrapper for stale-handle recovery`

## Task 18: Element reads (6 methods)

**Files:** `crates/zendriver/src/element/reads.rs`
**Spec:** Section "Reads"

**Implement:** `attr(name)`, `attrs()`, `inner_html()`, `bounding_box()`, `is_visible()`, `is_enabled()`. Each wraps its CDP call in `self.with_refresh(...)`. `inner_text` + `outer_html` (P2 methods) get moved here from `element/mod.rs` and wrapped in with_refresh too. `bounding_box` returns `Option<BoundingBox>` from `DOM.getBoxModel`. `is_visible`/`is_enabled` delegate to `query::actionability::check_visible`/`check_enabled`.

**Add BoundingBox struct** in `crates/zendriver/src/query/mod.rs` + re-export from crate root: `{ pub x: f64, pub y: f64, pub width: f64, pub height: f64 }`.

**Verify:** mock-driven tests for `attr` (returns Some/None), `attrs` (returns HashMap), `bounding_box` (parses DOM.getBoxModel).
**Commit:** `feat(zendriver): Element reads (attr/attrs/inner_html/bounding_box/is_visible/is_enabled)`

## Task 19: Element actions — hover + focus + scroll_into_view

**Files:** `crates/zendriver/src/element/actions.rs`
**Spec:** Section "Reads" then implicit actions section

**Implement:**
- `hover()`: scroll_into_view → wait_actionable(VISIBLE_ONLY + stable + receives_pointer) → bounding_box center → `move_realistic` via InputController. `hover_raw` uses `move_raw`.
- `focus()`: wait_actionable(VISIBLE_ONLY + enabled) → `el.focus()` via `call_on_main`.
- `scroll_into_view()`: `el.scrollIntoView({block:'center',behavior:'instant'})` via `call_on_main`. No actionability gate (this IS the prereq).

All wrapped in `with_refresh`.

**Verify:** mock-driven test confirming `hover` dispatches `Input.dispatchMouseEvent` with type=mouseMoved.
**Commit:** `feat(zendriver): Element hover + focus + scroll_into_view`

## Task 20: Element actions — click upgrade with ClickOptions

**Files:** `crates/zendriver/src/element/actions.rs`
**Spec:** Section "Reads" + new public types `ClickOptions`/`MouseButton`

**Implement:**
- `ClickOptions` struct: `button`, `modifiers`, `click_count`, `force`, `realistic`, `position` (None=center+jitter, Some((dx,dy))=relative to element top-left).
- `click()` = `click_with(ClickOptions::default())`. Default: Left button, no modifiers, click_count=1, force=false, realistic=true, position=None.
- `click_with(opts)`: scroll_into_view → if !opts.force then wait_actionable(FULL) → compute target_x/y from bbox + opts.position → `mouse::click_at(input, tab, x, y, opts.button, opts.click_count, opts.realistic)`.
- `click_raw()` = `click_with(ClickOptions { realistic: false, force: true, ..Default::default() })`.

P2 P1's `click()` impl moves out of `element/mod.rs` into this file.

**Re-exports in lib.rs:** `pub use element::actions::ClickOptions;` (or via mod.rs re-export).

**Verify:** mock-driven test that `click()` issues `Input.dispatchMouseEvent` with type=mouseMoved, then mousePressed, then mouseReleased.
**Commit:** `feat(zendriver): click upgrade with ClickOptions + actionability gate + realistic dispatch`

## Task 21: Element actions — set_value + clear + upload_files

**Files:** `crates/zendriver/src/element/actions.rs`
**Spec:** Section "Reads" + non-goals (upload_files only handles direct `<input type="file">`)

**Implement:**
- `set_value(value)`: `el.value = <value>; el.dispatchEvent(new Event('input', {bubbles: true})); el.dispatchEvent(new Event('change', {bubbles: true}))` via `call_on_main`. Bypasses keydown/keyup but fires input/change events for React-style controlled inputs.
- `clear()`: `el.value = ''; el.dispatchEvent(new Event('input', {bubbles: true}))` + focus + Backspace sequence via type_text (handles contenteditable too). For P3 simplicity: just the `el.value = ''` + dispatch events path.
- `upload_files(paths)`: `DOM.setFileInputFiles { files: paths_as_strings, backendNodeId: <self> }`.

**Verify:** mock-driven tests for `set_value` (dispatchEvent input/change), `upload_files` (DOM.setFileInputFiles payload contains paths).
**Commit:** `feat(zendriver): Element set_value + clear + upload_files`

## Task 22: Element input — type_text + press

**Files:** `crates/zendriver/src/element/input.rs`
**Spec:** Section "Realistic input" — Keyboard subsection

**Implement:**
- `type_text(text)`: focus() → `input::keyboard::type_text_realistic(input_controller, tab, text)`.
- `type_text_raw(text)`: focus() → `type_text_raw(input_controller, tab, text)`.
- `press(key: Key)`: focus() → if `Key::Char(c)` then `dispatch_char`; if `Key::Special(k)` then `dispatch_special`.
- `press_with(key, mods: KeyModifiers)`: lock InputController state, set modifiers_held=mods, dispatch the key, restore modifiers_held.

All wrapped in `with_refresh`. `focus()` reused from T19.

**Verify:** mock-driven test that `type_text_raw` emits N×2 `Input.dispatchKeyEvent` calls for N characters.
**Commit:** `feat(zendriver): Element type_text + type_text_raw + press + press_with`

## Task 23: Element traversal — parent + children

**Files:** `crates/zendriver/src/element/traversal.rs`
**Spec:** Section "Traversal"

**Implement:**
- `parent() -> Option<Element>`: `el.parentElement` via `call_on_main`. Constructs Element with `ElementOrigin::Traversal { parent: <self.origin>, kind: TraversalKind::Parent }`. If null, returns None.
- `children() -> Vec<Element>`: `Array.from(el.children)` via `call_on_main`. Each child Element gets `ElementOrigin::Traversal { parent: <self.origin>, kind: TraversalKind::NthChild(i) }`.

**Verify:** mock-driven test that `parent()` issues `Runtime.callFunctionOn` with `this.parentElement`.
**Commit:** `feat(zendriver): Element traversal (parent + children)`

## Task 24: Element-scoped find + find_all

**Files:** `crates/zendriver/src/element/traversal.rs`, `crates/zendriver/src/query/mod.rs`
**Spec:** Section "Element-scoped" mentions in Traversal

**Implement:**
- `Element::find() -> FindBuilder<'_>`: returns `FindBuilder::new_for_element(self)`. Subtree-scoped queries.
- `Element::find_all() -> FindAllBuilder<'_>`: `FindAllBuilder::new_for_element(self)`.

The selectors module already handles `QueryScope::Element` (via `this.querySelector` + element-context XPath).

**Verify:** mock-driven test that `el.find().css("...")` dispatches `Runtime.callFunctionOn` (Element scope) rather than `Runtime.evaluate` (Tab scope).
**Commit:** `feat(zendriver): element-scoped find + find_all`

## Task 25: Element::evaluate true isolated world

**Files:** `crates/zendriver/src/element/isolated_eval.rs`
**Spec:** Section "True isolated-world `Element::evaluate`"

**Implement:** Per spec — `ensure_isolated_world()` on Tab returns contextId, then `DOM.resolveNode { backendNodeId, executionContextId }` returns an isolated-world objectId, then `Runtime.callFunctionOn` with that objectId + the user's JS wrapped in `function(el){ return (...) }`. After getting the result, call `Runtime.releaseObject { objectId }` (best-effort; log on failure). Wrap entire flow in `with_refresh`. The P2 `Element::evaluate` thin-delegate in `element/mod.rs` gets replaced; `Element::evaluate_main` stays where it is. Drop the P2 `TODO(P3)` comment.

**Verify:** mock-driven test exercising the full sequence (ensure_isolated_world → DOM.resolveNode → Runtime.callFunctionOn → Runtime.releaseObject).
**Commit:** `feat(zendriver): Element::evaluate true isolated-world via DOM.resolveNode`

## Task 26: Tab::screenshot + Element::screenshot

**Files:** `crates/zendriver/src/tab.rs`, `crates/zendriver/src/element/screenshot.rs`
**Spec:** Section "Reads" + non-goals (PNG only, no format options in P3)

**Implement:**
- `Tab::screenshot() -> Vec<u8>`: `Page.captureScreenshot { format: "png" }` → base64 decode `data` field.
- `Element::screenshot() -> Vec<u8>`: bounding_box() → `Page.captureScreenshot { format: "png", clip: { x, y, width, height, scale: 1 } }` → base64 decode. wait_actionable(VISIBLE_ONLY) gate first.

Add `base64 = "0.22"` to workspace + zendriver dep.

**Verify:** mock-driven tests for both — assert Tab::screenshot sends Page.captureScreenshot without clip; Element::screenshot sends with clip.
**Commit:** `feat(zendriver): Tab::screenshot + Element::screenshot`

## Task 27: P3 integration tests

**Files:** `crates/zendriver/tests/integration_phase3.rs` (NEW, gated `integration-tests`)
**Spec:** Section "Tier 2 — Integration"

**Implement:** real-Chrome + wiremock tests for: click triggers DOM event, type_text fills input, hover triggers mouseover, scroll_into_view scrolls deep child, upload_files sets file input, auto-refresh on reload, NotActionable on display:none click, isolated Element::evaluate doesn't see page globals, XPath finds nested element, text selector finds case-insensitive match, AriaRole finds [role="button"].

Each `#[serial_test::serial]` + `#![cfg(feature = "integration-tests")]`.

**Verify:** build clean under `cargo build --tests --features integration-tests --locked`. Skip running locally (no Chrome); CI exercises.
**Commit:** `test(zendriver): P3 integration tests for selectors, actions, auto-refresh, actionability`

## Task 28: Port 5 Python examples to Rust

**Files:** `crates/zendriver/examples/{name1,...,name5}.rs`
**Spec:** Section "Tier 4 — Phase 3 exit criterion suite"

**Implement:** pick 5 Python files from `/Users/rin/GitHub/zendriver/examples/`. Port each 1:1 to Rust (preserve behavior; translate Python idioms to Rust). Suggested: hello-like smoke + form fill + element_overrides + simple-find-by-text + select-dropdown. Each example becomes a `crates/zendriver/examples/<name>.rs` runnable via `cargo run --example <name> -p zendriver`.

For each example: add `[[example]]` block to `crates/zendriver/Cargo.toml` if not auto-discovered. Add `#[allow(clippy::result_large_err)]` on `main` if needed (P1 pattern).

**Verify:** `cargo build --examples --workspace --locked` clean. Don't run examples (no Chrome locally).
**Commit:** `examples(zendriver): port 5 Python zendriver examples to Rust`

## Task 29: Snapshot regen + README touch-up

**Files:** various
**Spec:** Section "Tier 3 — Snapshot tests"

**Implement:**
- Run `cargo test --workspace --lib --locked` — if any snapshots drifted (e.g. fingerprint flags added a lang flag), `cargo insta accept` + commit.
- Update README.md "Example" section to use a P3-flavored example (e.g. find by text + type_text + click).
- Update README.md "Phases" list — P3 → DONE.

**Verify:** all unit tests pass; fmt clean; clippy clean.
**Commit:** `chore: post-P3 snapshot regen + README updates`

---

## Self-review checklist

**Spec coverage:**
- [x] Selectors (CSS/XPath/Text/TextRegex/Role) — T9-T11
- [x] Selector modifiers (nth/visible_only/in_frame/timeout) — T12
- [x] FindAllBuilder + many/many_or_empty — T13
- [x] Actionability checks + wait_actionable — T14, T15
- [x] Element auto-refresh — T16, T17
- [x] Element reads (attr/attrs/inner_html/bbox/is_visible/is_enabled) — T18
- [x] Element actions (hover/focus/scroll_into_view) — T19
- [x] Click upgrade with ClickOptions — T20
- [x] set_value/clear/upload_files — T21
- [x] type_text + press — T22
- [x] Element traversal (parent/children) — T23
- [x] Element-scoped find + find_all — T24
- [x] Element::evaluate true isolated world — T25
- [x] Screenshots (Tab + Element) — T26
- [x] InputProfile + InputController + Bezier + realistic keyboard — T2, T3-T7
- [x] Key + SpecialKey + KeyModifiers + neighbor_key — T3
- [x] AriaRole enum — T8
- [x] Error variants (ElementStale/NotRefreshable/NotActionable) — T1
- [x] Integration tests — T27
- [x] 5 Python examples ported — T28
- [x] CI matrix unchanged — no task needed (T22 of P2 added the nightly job; P3 doesn't touch CI)

**Placeholder scan:** T9-T29 are intentionally compact + reference spec sections. The detailed task structure of T0-T8 demonstrates the expected pattern.

**Type consistency:** `SelectorKind`, `QueryScope`, `RemoteRef`, `ElementOrigin`, `ScopeKind`, `TraversalKind`, `BoundingBox`, `ClickOptions`, `MouseButton`, `Key`, `SpecialKey`, `KeyModifiers`, `AriaRole`, `InputController`, `InputState`, `InputProfile`, `BezierPath`, `ActionabilityCheck` — names used consistently throughout.

---

## Notes for the implementing engineer

1. **The spec is the source of truth for T9-T29.** Plan compresses task definitions because the spec already has every code fragment + behavior contract. Read the relevant spec section before each task.
2. **`Element::call_on_main` is the shared helper** for "execute JS on this element's main-world remote object". Most reads + actions use it; introduced in T16.
3. **Tests for compact tasks T9-T29: aim for 1-3 unit tests per task** covering happy path + at least one error path. The spec lists more comprehensive test ideas under "Tier 1"; pick the most representative.
4. **The `#[allow(clippy::result_large_err)]` pattern from P2** applies wherever a function returns `Result<_, ZendriverError>`.
5. **`#[allow(clippy::panic, clippy::unwrap_used)]` on `#[cfg(test)] mod tests` blocks** is the established pattern.
6. **`cargo fmt --all` after each task or every few tasks** to keep diffs clean.
7. **Branch is `worktree-phase3-elements`** in worktree under `.claude/worktrees/phase3-elements/`. All commits + builds + tests run there.
8. **InputController state Mutex must NOT be held across `.await`** that touches `tab.session().call(...)` — that would serialize all CDP calls. Pattern in T6/T7: lock briefly to read state, drop guard, then await; lock again briefly to write state back.
