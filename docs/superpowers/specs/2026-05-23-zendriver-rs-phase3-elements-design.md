# zendriver-rs — Phase 3: Element API Completeness

**Date:** 2026-05-23
**Status:** Approved (delegate-mode brainstorming complete, ready for implementation plan)
**Phase:** 3 of 6 (element + input + actionability — see [Roadmap](#roadmap))
**Depends on:** Phase 1 (foundation, `12bf170`) and Phase 2 (stealth, `7c10c8e`), both on `main`

## Summary

Complete the `Element` + `FindBuilder` surface so users can do realistic browser automation: locate elements by CSS/XPath/text/ARIA-role, traverse the DOM, read attributes + bounding boxes + visibility, click/hover/focus/scroll/type/upload with action-time auto-waiting and realistic input simulation (Bezier mouse paths, per-key typing delay with occasional typo/pause, scroll wheel deltas). Also lands the true isolated-world `Element::evaluate` deferred from P2 T15.

Phase 3 exit criterion: 5 randomly-chosen Python `zendriver/examples/*.py` scripts port to Rust 1:1 (modulo Rust syntax) and all run green against real Chrome under `cargo test --features integration-tests`.

## Goals

- Extended `FindBuilder`: `xpath`, `text` (case-insensitive substring), `text_exact`, `text_regex`, `role` (ARIA), `nth`, `visible_only`, `in_frame` modifiers + `FindAllBuilder` with `many` / `many_or_empty` terminals.
- `Element` reads: `attr`, `attrs`, `inner_html`, `bounding_box`, `is_visible`, `is_enabled` (plus existing `inner_text`/`outer_html` gain auto-refresh).
- `Element` actions: `hover`, `focus`, `scroll_into_view`, `set_value`, `clear`, `upload_files`, plus `click` upgrade.
- `Element` input: `type_text`, `type_text_raw`, `press`, `press_with` + new `Key` / `KeyModifiers` / `SpecialKey` types.
- `Element` traversal: `parent`, `children`, element-scoped `find` / `find_all`.
- `Element::evaluate` upgraded to true isolated-world via `DOM.resolveNode { executionContextId }` (P2 T15 follow-up).
- `Element::screenshot` returns PNG bytes of the element's bounding box.
- Auto-refresh on stale handles: Element memoizes its selector, retries once on `"No node with given id"` errors.
- Playwright-style action-time actionability checks: `visible` + `connected` + `enabled` + `receives_pointer`. Each action waits up to a timeout (default 5s) for the element to become actionable; `*_force()` variants skip checks; `ClickOptions { force: true }` for click-specific override.
- Realistic input by default: Bezier mouse paths with overshoot + jitter; per-key typing with configurable typo rate + thinking-pause range. `*_raw()` opt-outs for tests and speed-critical use.
- `InputController` (per-Browser) tracks pointer position + modifier state across calls.
- `InputProfile` in `zendriver-stealth` (typo rate, thinking pause range, mouse speed, jitter amplitude) wired to the active StealthProfile (`spoofed`-tuned vs `native`-tuned defaults).

## Non-goals

Explicitly **out of scope** for Phase 3; deferred:
- Multi-tab session management, popup handling, frame typing as a first-class type (P4 — `in_frame(&str)` in P3 takes a string frame_id as a placeholder).
- Cookies + localStorage / sessionStorage (P4).
- Network interception (P5 — `Fetch.*` domain wrapper).
- Cloudflare bypass (P5).
- `expect()` API for wait conditions on requests/responses (P5).
- Realistic drag-and-drop (`Element::drag_to(...)`) — defer to P4 if demand surfaces.
- Custom mouse device pressure / pen / touch — defer until proven needed.
- IME (input method) handling for CJK input — defer.

## Architecture

### Crate + file layout

```
crates/zendriver/src/
├── lib.rs                  # +pub mod input/query; +re-exports Key, KeyModifiers, AriaRole, BoundingBox, ClickOptions, MouseButton
├── browser.rs              # +InputController owned by BrowserInner; +Browser::input() accessor
├── tab.rs                  # +Tab::screenshot (P3-minimal: full page bytes)
├── element/                # P1's element.rs split into a module directory
│   ├── mod.rs              # Element struct + Arc<Inner> + module re-exports
│   ├── reads.rs            # attr/attrs/inner_text/inner_html/outer_html/bounding_box/is_visible/is_enabled
│   ├── actions.rs          # click/click_raw/click_with/hover/focus/scroll_into_view/set_value/clear/upload_files
│   ├── input.rs            # type_text/type_text_raw/press/press_with
│   ├── traversal.rs        # parent/children/find/find_all (element-scoped)
│   ├── isolated_eval.rs    # true isolated-world Element::evaluate via DOM.resolveNode
│   ├── refresh.rs          # auto-refresh on stale handle: stored selector, retry-once, traversal-derived ref pattern
│   └── screenshot.rs       # element-scoped screenshot via Page.captureScreenshot { clip }
├── query/                  # P1's query.rs split into a module directory
│   ├── mod.rs              # FindBuilder + FindAllBuilder + builder state
│   ├── selectors.rs        # SelectorKind enum + per-selector CDP/JS resolution
│   ├── modifiers.rs        # nth/visible_only/in_frame/timeout
│   ├── actionability.rs    # check_visible/check_stable/check_enabled/check_receives_pointer
│   └── role.rs             # AriaRole enum + role-to-selector compilation
└── input/                  # NEW
    ├── mod.rs              # InputController + InputState struct
    ├── bezier.rs           # cubic Bezier path generation
    ├── mouse.rs            # realistic + raw mouse move/click/scroll via Input.dispatchMouseEvent
    ├── keyboard.rs         # Key + SpecialKey + KeyModifiers + realistic typing dispatch
    └── pointer_state.rs    # MouseButtonSet + state tracking

crates/zendriver-stealth/src/
└── input_profile.rs        # InputProfile (NEW): typo_rate, thinking_pause_ms_range, mouse_speed_px_per_ms, jitter_amplitude
```

### Dependencies added

```toml
regex     = "1"           # text_regex selector
bitflags  = "2"           # KeyModifiers
rand      = { version = "0.8", default-features = false, features = ["std", "std_rng", "small_rng"] }  # Bezier jitter + typing pauses
```

`rand`'s `SmallRng` lets tests use `SmallRng::seed_from_u64(42)` for reproducible Bezier paths + typing patterns. Production code uses `SmallRng::from_entropy()`.

### Per-Browser `InputController`

```rust
// crates/zendriver/src/input/mod.rs
pub(crate) struct InputController {
    state: tokio::sync::Mutex<InputState>,
    profile: zendriver_stealth::InputProfile,
}

pub(crate) struct InputState {
    pub pointer_x: f64,
    pub pointer_y: f64,
    pub buttons_held: MouseButtonSet,
    pub modifiers_held: KeyModifiers,
    pub rng: rand::rngs::SmallRng,
}
```

`BrowserInner` gains `input: Arc<InputController>`. Construction at `BrowserBuilder::launch`:

```rust
let input_profile = stealth_profile.as_ref()
    .map_or_else(InputProfile::native, |sp| sp.input_profile());
let input = Arc::new(InputController::new(input_profile));
```

`StealthProfile` gains `input_profile() -> InputProfile` returning either the chaser-oxide tuned values (spoofed) or fast/deterministic values (native/off).

## Components — Section 2: FindBuilder selectors + actionability

### `SelectorKind`

```rust
// crates/zendriver/src/query/selectors.rs

#[derive(Debug, Clone)]
pub(crate) enum SelectorKind {
    Css(String),
    Xpath(String),
    Text { needle: String, exact: bool },
    TextRegex(regex::Regex),
    Role(AriaRole, Option<String>),  // role + optional accessible name
}

impl SelectorKind {
    /// Resolve this selector against a scope (Tab main frame or element subtree).
    /// Returns the matching `RemoteObjectId` for `.one()` or all matches for `.many()`.
    pub(crate) async fn resolve_one(&self, scope: &QueryScope<'_>) -> Result<Option<RemoteRef>>;
    pub(crate) async fn resolve_many(&self, scope: &QueryScope<'_>) -> Result<Vec<RemoteRef>>;
}

pub(crate) enum QueryScope<'a> {
    Tab(&'a Tab),
    Element(&'a Element),
}
```

**Per-kind implementation:**
- **CSS** — `Runtime.evaluate` with `document.querySelector(<sel>)` (Tab scope) or `el.querySelector(<sel>)` via `Runtime.callFunctionOn` (Element scope). Returns `RemoteObjectId`.
- **XPath** — `Runtime.evaluate` with `document.evaluate(<expr>, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue`. Element scope passes element as the context node.
- **Text** — JS-driven tree walk: `document.evaluate("//*[contains(translate(., 'ABC…XYZ', 'abc…xyz'), '<lowercased-needle>')]", ...)` for case-insensitive substring. `exact=true` uses `[normalize-space(.)='<needle>']`.
- **TextRegex** — JS-driven tree walk with serialized regex: `Array.from(document.querySelectorAll('*')).filter(el => /pattern/.test(el.innerText))[0]`. Regex pattern + flags serialized via `regex::Regex::as_str()`.
- **Role** — compose CSS `[role="<role>"]` first; if `name` provided, additionally check `aria-label` / `aria-labelledby` / accessible name computed via `Accessibility.getPartialAXTree`. P3 implementation: CSS-only `[role="<role>"]` + post-filter on accessible name if specified, using `Accessibility.getPartialAXTree` per candidate. Slower than Playwright's native locator but correctness wins for v0.

### `FindBuilder` chain rules

```rust
pub struct FindBuilder<'tab> {
    scope: QueryScope<'tab>,
    selector: Option<SelectorKind>,
    timeout: Duration,
    nth: Option<usize>,
    visible_only: bool,
    in_frame: Option<String>,
}

impl<'tab> FindBuilder<'tab> {
    pub fn css(mut self, sel: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Css(sel.into())); self
    }
    pub fn xpath(mut self, expr: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Xpath(expr.into())); self
    }
    pub fn text(mut self, needle: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Text { needle: needle.into(), exact: false }); self
    }
    pub fn text_exact(mut self, needle: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Text { needle: needle.into(), exact: true }); self
    }
    pub fn text_regex(mut self, re: regex::Regex) -> Self {
        self.selector = Some(SelectorKind::TextRegex(re)); self
    }
    pub fn role(mut self, role: AriaRole) -> Self {
        self.selector = Some(SelectorKind::Role(role, None)); self
    }
    pub fn role_named(mut self, role: AriaRole, name: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Role(role, Some(name.into()))); self
    }
    pub fn nth(mut self, idx: usize) -> Self { self.nth = Some(idx); self }
    pub fn visible_only(mut self, on: bool) -> Self { self.visible_only = on; self }
    pub fn in_frame(mut self, frame_id: impl Into<String>) -> Self {
        self.in_frame = Some(frame_id.into()); self
    }
    pub fn timeout(mut self, dur: Duration) -> Self { self.timeout = dur; self }

    pub async fn one(self) -> Result<Element> { /* poll-loop until matched, returns first or nth */ }
    pub async fn one_or_none(self) -> Result<Option<Element>>;
}
```

Selector kinds are mutually exclusive — calling `.css()` after `.xpath()` overwrites. Rule documented; first-call-wins behavior would surprise users.

### Actionability checks

```rust
// crates/zendriver/src/query/actionability.rs

pub(crate) struct ActionabilityCheck {
    pub visible: bool,
    pub stable: bool,
    pub enabled: bool,
    pub receives_pointer: bool,
}

pub(crate) async fn check_visible(el: &Element) -> Result<bool> {
    // Calls JS: el is visible if offsetParent !== null (handles `display: none` ancestors)
    // AND getBoundingClientRect width/height > 0
    // AND computed style 'visibility' !== 'hidden'
    // AND computed style 'opacity' !== '0'
}

pub(crate) async fn check_stable(el: &Element) -> Result<bool> {
    // Compare bounding box across two frames (16ms apart via setTimeout).
    // Stable if x/y/w/h match within 0.5px.
}

pub(crate) async fn check_enabled(el: &Element) -> Result<bool> {
    // For form elements: el.disabled === false
    // For aria-disabled: getAttribute('aria-disabled') !== 'true'
    // For non-form: always true
}

pub(crate) async fn check_receives_pointer(el: &Element) -> Result<bool> {
    // Compute bbox center.
    // Call document.elementFromPoint(cx, cy).
    // Walk up the returned element's ancestors. If our element appears, pointer reaches us.
    // If a sibling overlay covers us, returns false.
}

pub(crate) async fn wait_actionable(
    el: &Element,
    require: ActionabilityCheck,
    timeout: Duration,
) -> Result<()> {
    // Poll every 50ms until all required checks pass or deadline hit.
    // On deadline: ZendriverError::NotActionable with details of which check failed.
}
```

Each action invokes `wait_actionable` before its CDP dispatch:
- `click` requires `{ visible, stable, enabled, receives_pointer }`
- `type_text` requires `{ visible, enabled }` (text input doesn't need pointer)
- `hover` requires `{ visible, stable, receives_pointer }`
- `focus` requires `{ visible, enabled }`
- `scroll_into_view` requires nothing — it's the precondition for the others
- `screenshot` requires `{ visible }`

`ClickOptions { force: true }` and `*_force()` variants bypass.

## Components — Section 3: Element auto-refresh + traversal + reads

### `Element` inner state

```rust
// crates/zendriver/src/element/mod.rs

#[derive(Clone)]
pub struct Element { pub(crate) inner: Arc<ElementInner> }

pub(crate) struct ElementInner {
    pub(crate) tab: Tab,
    pub(crate) backend_node_id: tokio::sync::Mutex<Option<i64>>,    // None when stale, refilled on refresh
    pub(crate) remote_object_id: tokio::sync::Mutex<Option<String>>,
    pub(crate) origin: ElementOrigin,                               // for auto-refresh
}

pub(crate) enum ElementOrigin {
    Query {
        scope_kind: ScopeKind,             // Tab(main_frame) | Element(parent_origin)
        selector: SelectorKind,
        nth: Option<usize>,
    },
    Traversal {
        parent: Box<ElementOrigin>,
        kind: TraversalKind,               // Parent | NthChild(usize) | NthSibling(usize)
    },
    Evaluation {
        // No selector; element was returned from JS expression.
        // Auto-refresh impossible — refresh() errors with NotRefreshable.
    },
}
```

### Auto-refresh flow

```rust
// crates/zendriver/src/element/refresh.rs

impl Element {
    pub(crate) async fn with_refresh<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: Fn() -> Fut + Send,
        Fut: std::future::Future<Output = Result<T>> + Send,
    {
        match f().await {
            Ok(v) => Ok(v),
            Err(e) if is_stale_node_error(&e) => {
                self.refresh().await?;
                f().await
            }
            Err(e) => Err(e),
        }
    }

    pub async fn refresh(&self) -> Result<()> {
        let (new_backend, new_remote) = match &self.inner.origin {
            ElementOrigin::Query { scope_kind, selector, nth } => {
                let scope = resolve_scope(scope_kind, &self.inner.tab).await?;
                let scope_q = QueryScope::from(&scope);
                let candidates = selector.resolve_many(&scope_q).await?;
                let idx = nth.unwrap_or(0);
                let r = candidates.get(idx)
                    .ok_or(ZendriverError::ElementNotFound { selector: format!("{selector:?}") })?;
                (r.backend_node_id, r.remote_object_id.clone())
            }
            ElementOrigin::Traversal { parent, kind } => {
                // Recursively refresh the parent, then re-traverse to this element.
                let parent_el = Element::synthesize_from_origin((**parent).clone(), self.inner.tab.clone()).await?;
                let resolved = traverse_kind(&parent_el, *kind).await?;
                (resolved.backend_node_id, resolved.remote_object_id)
            }
            ElementOrigin::Evaluation => {
                return Err(ZendriverError::NotRefreshable);
            }
        };
        *self.inner.backend_node_id.lock().await = Some(new_backend);
        *self.inner.remote_object_id.lock().await = Some(new_remote);
        Ok(())
    }
}

fn is_stale_node_error(e: &ZendriverError) -> bool {
    matches!(e, ZendriverError::Navigation(m) if m.contains("No node with given id") || m.contains("Cannot find context"))
        || matches!(e, ZendriverError::Cdp { message, .. } if message.contains("No node with given id"))
}
```

Every read + action method wraps its CDP call in `with_refresh`. `Element::evaluate` (isolated-world) wraps too. `Element::evaluate_main` wraps. The retry-on-stale logic is centralized.

### Reads

```rust
// crates/zendriver/src/element/reads.rs

impl Element {
    pub async fn attr(&self, name: &str) -> Result<Option<String>> {
        self.with_refresh(|| async {
            let res = self.call_on(
                &format!("function(){{ return this.getAttribute({}); }}", json!(name)),
                json!([]),
            ).await?;
            Ok(res.get("value").and_then(|v| v.as_str()).map(String::from))
        }).await
    }

    pub async fn attrs(&self) -> Result<HashMap<String, String>>;          // via Object.fromEntries + Array.from(el.attributes)
    pub async fn inner_text(&self) -> Result<String>;                       // existing, now wrapped with_refresh
    pub async fn inner_html(&self) -> Result<String>;
    pub async fn outer_html(&self) -> Result<String>;                       // existing, now wrapped
    pub async fn bounding_box(&self) -> Result<Option<BoundingBox>>;        // DOM.getBoxModel
    pub async fn is_visible(&self) -> Result<bool>;                         // delegates to actionability::check_visible
    pub async fn is_enabled(&self) -> Result<bool>;                         // delegates to actionability::check_enabled
}
```

### Traversal

```rust
// crates/zendriver/src/element/traversal.rs

impl Element {
    pub async fn parent(&self) -> Result<Option<Element>> {
        let res = self.call_on("function(){ return this.parentElement; }", json!([])).await?;
        // ... extract objectId if non-null, build child Element with Traversal origin ...
    }

    pub async fn children(&self) -> Result<Vec<Element>>;
    pub fn find(&self) -> FindBuilder<'_> { FindBuilder::new_for_element(self) }
    pub fn find_all(&self) -> FindAllBuilder<'_> { FindAllBuilder::new_for_element(self) }
}
```

## Components — Section 4: Realistic input

### `InputProfile`

```rust
// crates/zendriver-stealth/src/input_profile.rs

#[derive(Debug, Clone)]
pub struct InputProfile {
    /// Probability of injecting a typo + backspace per character. 0.0–1.0.
    pub typo_rate: f32,
    /// Range of "thinking pause" duration injected between words.
    pub thinking_pause_ms_range: (u32, u32),
    /// Per-character typing delay range.
    pub per_char_delay_ms_range: (u32, u32),
    /// Mouse cursor speed in pixels per millisecond when moving.
    pub mouse_speed_px_per_ms: f64,
    /// Jitter amplitude (px) applied to Bezier control points.
    pub jitter_amplitude_px: f64,
    /// Probability the mouse overshoots its target by up to 20% before settling.
    pub overshoot_rate: f32,
}

impl InputProfile {
    pub fn native() -> Self {
        // Fast, deterministic. Used when StealthProfile::native or Off.
        Self {
            typo_rate: 0.0,
            thinking_pause_ms_range: (0, 0),
            per_char_delay_ms_range: (0, 0),
            mouse_speed_px_per_ms: 10.0,    // 10px/ms = 100ms for 1000px traverse
            jitter_amplitude_px: 0.0,
            overshoot_rate: 0.0,
        }
    }

    pub fn spoofed() -> Self {
        // chaser-oxide-derived realistic defaults.
        Self {
            typo_rate: 0.03,
            thinking_pause_ms_range: (200, 400),
            per_char_delay_ms_range: (50, 150),
            mouse_speed_px_per_ms: 1.5,     // slower = more human
            jitter_amplitude_px: 2.0,
            overshoot_rate: 0.20,
        }
    }
}
```

`StealthProfile::input_profile()` returns the matching variant.

### Bezier mouse path

```rust
// crates/zendriver/src/input/bezier.rs

pub(crate) struct BezierPath {
    pub points: Vec<(f64, f64)>,
}

impl BezierPath {
    /// Build a cubic Bezier from `start` to `end` with two control points
    /// at ~25% and ~75% positions perturbed by `jitter_px`. Sample
    /// `n_points` along the curve.
    pub fn build(
        start: (f64, f64),
        end: (f64, f64),
        jitter_px: f64,
        rng: &mut impl rand::Rng,
    ) -> Self {
        let dx = end.0 - start.0;
        let dy = end.1 - start.1;
        // Control points along the line, perturbed perpendicular.
        let c1 = (
            start.0 + dx * 0.25 + rng.gen_range(-jitter_px..jitter_px),
            start.1 + dy * 0.25 + rng.gen_range(-jitter_px..jitter_px),
        );
        let c2 = (
            start.0 + dx * 0.75 + rng.gen_range(-jitter_px..jitter_px),
            start.1 + dy * 0.75 + rng.gen_range(-jitter_px..jitter_px),
        );
        let distance = (dx * dx + dy * dy).sqrt();
        let n_points = ((distance / 5.0).max(8.0) as usize).min(60);  // 5px per step; min 8; max 60
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
```

### Mouse dispatch

```rust
// crates/zendriver/src/input/mouse.rs

pub(crate) async fn move_realistic(
    input: &InputController,
    tab: &Tab,
    target_x: f64,
    target_y: f64,
) -> Result<()> {
    let mut state = input.state.lock().await;
    let path = BezierPath::build(
        (state.pointer_x, state.pointer_y),
        (target_x, target_y),
        input.profile.jitter_amplitude_px,
        &mut state.rng,
    );
    let speed = input.profile.mouse_speed_px_per_ms;
    drop(state);

    for &(x, y) in &path.points {
        tab.session().call("Input.dispatchMouseEvent", json!({
            "type": "mouseMoved", "x": x, "y": y,
            "modifiers": modifier_bits(input.modifiers_held().await),
        })).await?;
        // Sleep proportional to segment distance.
        // (Simplified: sleep 1/speed ms per 5px segment.)
        tokio::time::sleep(Duration::from_micros((5.0 / speed * 1000.0) as u64)).await;
    }
    let mut state = input.state.lock().await;
    state.pointer_x = target_x;
    state.pointer_y = target_y;
    Ok(())
}

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
        tab.session().call("Input.dispatchMouseEvent", json!({
            "type": "mouseMoved", "x": target_x, "y": target_y,
        })).await?;
    }
    tab.session().call("Input.dispatchMouseEvent", json!({
        "type": "mousePressed",
        "x": target_x, "y": target_y,
        "button": button.cdp_str(),
        "clickCount": click_count,
    })).await?;
    tab.session().call("Input.dispatchMouseEvent", json!({
        "type": "mouseReleased",
        "x": target_x, "y": target_y,
        "button": button.cdp_str(),
        "clickCount": click_count,
    })).await?;
    Ok(())
}
```

### Keyboard

```rust
// crates/zendriver/src/input/keyboard.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Special(SpecialKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialKey {
    Enter, Tab, Escape, Backspace, Delete,
    ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
    Home, End, PageUp, PageDown,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    Insert, CapsLock, NumLock, ScrollLock,
    PrintScreen, Pause, ContextMenu, Space,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct KeyModifiers: u8 {
        const ALT     = 0b0001;
        const CTRL    = 0b0010;
        const META    = 0b0100;
        const SHIFT   = 0b1000;
    }
}

pub(crate) async fn type_text_realistic(
    input: &InputController,
    tab: &Tab,
    text: &str,
) -> Result<()> {
    let profile = input.profile.clone();
    let mut rng = input.state.lock().await.rng.clone();
    for ch in text.chars() {
        // Per-char delay.
        let delay_ms = rng.gen_range(profile.per_char_delay_ms_range.0..=profile.per_char_delay_ms_range.1);
        tokio::time::sleep(Duration::from_millis(delay_ms as u64)).await;

        // Thinking pause between words.
        if ch == ' ' && rng.gen::<f32>() < 0.05 {
            let pause = rng.gen_range(profile.thinking_pause_ms_range.0..=profile.thinking_pause_ms_range.1);
            tokio::time::sleep(Duration::from_millis(pause as u64)).await;
        }

        // Possible typo: dispatch wrong neighbor key, then backspace.
        if profile.typo_rate > 0.0 && rng.gen::<f32>() < profile.typo_rate {
            if let Some(wrong) = neighbor_key(ch, &mut rng) {
                dispatch_char(tab, wrong).await?;
                tokio::time::sleep(Duration::from_millis(80)).await;
                dispatch_special(tab, SpecialKey::Backspace).await?;
            }
        }
        dispatch_char(tab, ch).await?;
    }
    Ok(())
}

async fn dispatch_char(tab: &Tab, c: char) -> Result<()> {
    let s = c.to_string();
    tab.session().call("Input.dispatchKeyEvent", json!({
        "type": "keyDown", "text": &s, "key": &s,
    })).await?;
    tab.session().call("Input.dispatchKeyEvent", json!({
        "type": "keyUp", "text": &s, "key": &s,
    })).await?;
    Ok(())
}

async fn dispatch_special(tab: &Tab, k: SpecialKey) -> Result<()> {
    let (code, key, key_code) = special_key_to_cdp(k);
    tab.session().call("Input.dispatchKeyEvent", json!({
        "type": "rawKeyDown", "code": code, "key": key, "windowsVirtualKeyCode": key_code,
    })).await?;
    tab.session().call("Input.dispatchKeyEvent", json!({
        "type": "keyUp", "code": code, "key": key, "windowsVirtualKeyCode": key_code,
    })).await?;
    Ok(())
}
```

`neighbor_key(c, rng)` returns a plausible nearby QWERTY key (e.g. `r` neighbors `e t d f`). Small lookup table; falls back to None for non-alphanumeric.

## Components — Section 5: True isolated-world `Element::evaluate`

P2 T15 left `Element::evaluate` as a thin delegate to `evaluate_main` because true isolated-world Element handling needed `DOM.resolveNode { executionContextId }`. P3 lands it.

```rust
// crates/zendriver/src/element/isolated_eval.rs

impl Element {
    /// Re-resolves this element's RemoteObject inside the Tab's isolated world,
    /// then dispatches the function call against that handle.
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        self.with_refresh(|| async {
            let ctx_id = self.inner.tab.ensure_isolated_world().await?;
            let backend_node_id = self.inner.backend_node_id.lock().await
                .ok_or(ZendriverError::ElementStale)?;
            // Re-resolve our node in the isolated world.
            let resolved = self.inner.tab.call("DOM.resolveNode", json!({
                "backendNodeId": backend_node_id,
                "executionContextId": ctx_id,
            })).await?;
            let isolated_object_id = resolved["object"]["objectId"].as_str()
                .ok_or(ZendriverError::Navigation("DOM.resolveNode returned no objectId".into()))?
                .to_string();
            let function = format!("function(el){{ return ({}) }}", js.as_ref());
            let result = self.inner.tab.call("Runtime.callFunctionOn", json!({
                "objectId": &isolated_object_id,
                "functionDeclaration": function,
                "arguments": [{ "objectId": &isolated_object_id }],
                "returnByValue": true,
                "awaitPromise": true,
            })).await?;
            if let Some(details) = result.get("exceptionDetails") {
                let msg = details.get("exception").and_then(|e| e.get("description"))
                    .and_then(|d| d.as_str()).unwrap_or("unknown").to_string();
                return Err(ZendriverError::JsException(msg));
            }
            let value = result.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null);
            // Release the isolated handle so it doesn't leak.
            let _ = self.inner.tab.call("Runtime.releaseObject", json!({
                "objectId": isolated_object_id,
            })).await;
            serde_json::from_value(value).map_err(ZendriverError::Serde)
        }).await
    }
}
```

`evaluate_main` keeps P2 behavior (callFunctionOn on the main-world `remote_object_id`).

## Components — Section 6: Error handling + testing + sizing

### New error variants

```rust
// crates/zendriver/src/error.rs — add to ZendriverError:
#[error("element is stale: refresh failed or origin not refreshable")]
ElementStale,

#[error("element not refreshable (was returned from a JS evaluation)")]
NotRefreshable,

#[error("element not actionable within {0:?}: {1}")]
NotActionable(std::time::Duration, String),  // e.g. "not visible: display: none"
```

`#[non_exhaustive]` from P1 means these additions are non-breaking. Update spec assumption #8 from P2 — actually any pre-1.0 API churn is fine per memory, no concern.

### Test surface

**Tier 1 — Unit (MockConnection):**
- SelectorKind: each kind compiles to expected CDP/JS payload (snapshots).
- AriaRole → CSS attribute compilation: `AriaRole::Button → [role="button"]`, etc.
- Bezier path: known seed produces known path (snapshot of points vector).
- Actionability: stub element returns specific JS values → expected ActionabilityCheck result.
- Auto-refresh: stale error on first call triggers re-query then success on retry.
- Stale-refresh on Evaluation origin returns NotRefreshable.
- type_text_realistic with seeded rng + typo_rate=1.0 always emits typo+backspace pattern.
- Bezier overshoot: with `overshoot_rate=1.0` and seed, path includes a point beyond target before settling.
- Keyboard: `Key::Char('A')` dispatches keyDown/keyUp with `text="A"`; SpecialKey::Enter dispatches with `key="Enter"`.

**Tier 2 — Integration (real Chrome + wiremock):**
- Click triggers DOM event (P1 has this; updated to expect realistic timing range 100–300ms).
- type_text into `<input>` produces correct .value (realistic mode).
- type_text_raw produces same .value in <50ms (deterministic mode).
- hover triggers `mouseover` event.
- scroll_into_view scrolls a deep child into viewport.
- upload_files sets `<input type="file">`.files correctly.
- Auto-refresh: trigger a `location.reload()` mid-flow; subsequent Element op auto-refreshes.
- Actionability: click on `display:none` element returns `NotActionable("not visible")` within timeout.
- evaluate (isolated) sees `el` parameter as a real Element in the isolated world; does NOT see page's `window.evil` global.
- XPath selector locates `//div[@id="foo"]/span[2]` correctly.
- text selector finds element by case-insensitive substring.
- text_regex matches with serialized regex.
- AriaRole locates `<button>` via `[role="button"]`.

**Tier 3 — Snapshot:**
- Bezier path with seed=42, start=(0,0), end=(100,100), jitter=2.0 → known point vector.
- Realistic typing schedule with seed=42, text="hello", typo_rate=0.5 → known event sequence with timings.
- Native InputProfile + Spoofed InputProfile snapshot of all fields.
- AriaRole → CSS compilation table snapshot.

**Tier 4 — Phase 3 exit criterion suite:**
Port 5 Python `zendriver/examples/*.py` to Rust. Each becomes a `crates/zendriver/examples/*.rs` AND an integration test that runs it. Suggested picks:
- `examples/simple.py` → smoke test (already covered by P1 hello.rs)
- `examples/form_demo.py` → fills a form via type_text, asserts submission
- `examples/element_overrides.py` → exercises set_value + attr reads
- `examples/iframe_basic.py` → exercises in_frame() — note: P3's `in_frame` takes a string, which is the placeholder for P4's proper Frame type; the test may need to use raw CDP for the frame_id discovery
- `examples/select_dropdown.py` → exercises text/role selector + click

If a chosen example needs P4 features not yet shipped (e.g. multi-tab, full iframe handling), substitute with a simpler example. The hard requirement is "5 randomly-chosen examples port and run."

### Determinism in tests

| Source of nondeterminism | Mitigation |
|---|---|
| `SmallRng` per-call | Seed via `SmallRng::seed_from_u64(42)` in test setup; production uses `from_entropy` |
| Realistic typing timing | Tests assert duration within tolerance (e.g. 100ms ± 50ms) or use `type_text_raw` |
| Bezier path coordinates | Snapshot tests use fixed seed; integration tests assert "click landed on element" not "click at exact coords" |
| Real Chrome animation timing | Actionability checks wait for stability; tests use stable fixtures |
| External sites (stealth-tests P2) | Untouched by P3 |

### CI matrix delta

No new CI jobs. Existing matrix covers:
- `test-unit` runs new unit tests
- `test-integration` runs new integration tests behind `integration-tests` feature
- `test-snapshot` runs new snapshots
- `nightly-stealth-tests` (cron) unchanged

### Sizing

Estimated ~30 implementation tasks; matches P1 (28) and P2 (23) cadence. Solo estimate 3-4 weeks.

## Assumptions (delegate mode — judgement calls made on user's behalf)

These are calls I made without explicit user input. Review and push back on any:

1. **Element-scoped `find` allows all selectors.** XPath, text, role all work scoped to an element (not just CSS). Implementation uses `el.querySelector` for CSS and `document.evaluate` with `el` as context node for XPath. Text/regex walks `el` subtree only.
2. **`text()` selector is case-insensitive substring by default.** `text_exact()` for case-sensitive exact match (whitespace-collapsed via `normalize-space`). `text_regex()` for full regex. Matches Playwright's `getByText` defaults.
3. **AriaRole is a small enum, not all 80+ ARIA roles.** P3 ships the ~20 most-common roles (Button, Link, Textbox, Combobox, Checkbox, Radio, Tab, Menu, Menuitem, Dialog, Heading, Banner, Navigation, Main, Article, List, Listitem, Row, Cell, Columnheader, Rowheader). Other roles available via `AriaRole::Other(&'static str)` escape hatch.
4. **`role_named()` uses `Accessibility.getPartialAXTree` per candidate.** Slower than Playwright's native locator (which uses an internal accessibility cache). Performance acceptable for v0; revisit if users complain.
5. **Auto-refresh retries once.** If first retry also fails with stale, errors with `ElementStale`. No infinite-loop guard needed; either the page is racing faster than we can catch or something is truly broken.
6. **`Element::evaluate` (isolated) releases the temporary objectId via `Runtime.releaseObject`.** Without this, isolated-world handles leak per call. Best-effort: if `releaseObject` fails, log and continue (the next isolated-world resolve will still work).
7. **`KeyModifiers` is a `bitflags!` u8.** Composable: `KeyModifiers::CTRL | KeyModifiers::SHIFT`. Matches CDP's modifier-bits encoding.
8. **Special keys are an enum, not strings.** `SpecialKey::Enter` not `"Enter"`. Compile-time-checked; users can't typo `"Entr"`.
9. **`InputProfile` defaults are tuned from chaser-oxide's published values**, not original research. Adjust if real-world detection rates suggest different numbers.
10. **Typo neighbor key lookup is a small static table for QWERTY only.** Non-QWERTY layouts disable typos (typo_rate effectively 0 for non-alphanumeric chars). P5+ could add layout configurability.
11. **Bezier path point density is dynamic** based on distance: 5px per step, min 8 points, max 60. Below 5px = direct move (no interpolation).
12. **Scrolling is a separate verb from mouse move.** `Element::scroll_into_view()` uses `element.scrollIntoView({ block: 'center', behavior: 'instant' })` via JS, NOT Input.dispatchMouseEvent wheel scrolling. Wheel scrolling defer to P4 if needed (e.g. for infinite-scroll pages).
13. **`upload_files` only handles `<input type="file">` direct elements.** Button-triggered file pickers (React-style abstracted upload widgets) defer to P5 (`Page.fileChooserOpened` event handler).
14. **`Tab::screenshot()` returns full-page PNG**, `Element::screenshot()` returns element-bbox PNG. No format options in P3 (always PNG). JPEG / full-page-with-fixed-headers etc deferred.
15. **`ClickOptions::position` is relative to element top-left in element-local pixels**, not page pixels. Matches Playwright.
16. **`set_value` bypasses input/change events.** Use `type_text` if you need events. `set_value` is the "I have 10000 values to fill" speed path.
17. **`scroll_into_view` is the precondition for click/hover.** Their actionability check sequence: `scroll_into_view` first (always), then `wait_actionable` poll. Scroll never fails (worst case: element off-screen, scrolling does nothing, actionability fails with `not_visible`).
18. **`InputController` is per-`BrowserInner`.** Multi-tab in P4 will revisit — Chrome's pointer-position is actually per-tab in practice (each tab activates its own cursor on focus). For P3 single-main-tab world, per-Browser is correct.
19. **`Element::evaluate` always returns `T: DeserializeOwned`.** No remote-handle return (`evaluate_handle` in Playwright) in P3. Add later if needed.
20. **`pinned()` opt-out for auto-refresh is NOT added in P3.** Per the brainstorming choice, default auto-refresh is sufficient; we'll add the opt-out if real users ask for it.

## Roadmap

| Phase | Status | Goal |
|---|---|---|
| 1 | DONE | Foundation: transport + minimal Tab/Element |
| 2 | DONE | Stealth: launch flags + JS patches + TargetObserver + chaser-oxide additions |
| **3 (this spec)** | IN PROGRESS | Element + FindBuilder completeness + realistic input + actionability + true isolated-world Element::evaluate |
| 4 | planned | Tab/Browser completeness (cookies, storage, screenshots, multi-tab, iframes Frame type) |
| 5 | planned | Optional gated features (interception, cloudflare, expect, fetcher) |
| 6 | planned | Polish + crates.io publish |

Rough P3 sizing: 3-4 weeks solo.

## Brainstorm cross-ref

Decisions locked during brainstorming:
- **Input realism:** realistic by default (Bezier mouse, per-key typing with typo/pause), `*_raw()` opt-outs for tests + speed paths.
- **Stale handles:** auto-refresh transparently; Element memoizes its origin (selector + scope or traversal kind); `NotRefreshable` for evaluation-derived elements.
- **Actionability:** Playwright-style auto-wait checks (visible + stable + enabled + receives_pointer) on every action, with `force` per-call override.
- **Architecture:** all P3 surface stays in `zendriver` crate; element.rs + query.rs split into module directories due to size; new `input/` module; `InputProfile` lives in `zendriver-stealth`.
