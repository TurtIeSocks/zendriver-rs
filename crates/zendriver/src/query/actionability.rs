//! Playwright-style actionability predicates: visible / stable / enabled /
//! receives_pointer. Each runs a small JS function on the element's remote
//! handle via `Element::call_on_main` and returns a `bool`.
//!
//! `call_on_main` binds the element as the first positional argument, so
//! every body opens `function(el, ...)` and any caller-supplied arguments
//! follow it.
//!
//! The aggregate gate ([`wait_actionable`]) polls the four predicates in
//! order and raises `NotActionable` on deadline; `Element::click_with`,
//! `hover`, `focus`, `type_text`, and `screenshot` (in `element::actions`
//! / `element::screenshot`) each pick the [`ActionabilityCheck`] preset
//! matching what they need before dispatching.
//!
//! # A note on this module's unit tests
//!
//! The predicates are JavaScript, and `MockConnection` replays CDP frames
//! without a JS engine, so the `probe_source_*` tests below assert on the
//! *source text* each predicate puts on the wire. They pin that a clause is
//! present and spelled the way a past defect proved it has to be; they do
//! not execute it, and a mutation that rewrites a clause into something
//! equally well-formed but wrong passes them.
//!
//! The live-Chrome tier reaches part of that, and does not gate a merge.
//! `tests/find_visible_only.rs` (gated on the `integration-tests` feature)
//! drives `visible_only(true)` against a real browser, so `check_visible`'s
//! `display: none` path does execute there — but every test in that file is
//! `#[ignore]`d, and the per-PR integration job runs plain
//! `cargo nextest run -E 'kind(test)'` with no `--run-ignored`. So it runs
//! only in the scheduled `nightly-ignored-tests` lane, which carries
//! `continue-on-error: true`.
//!
//! Nothing else is covered behaviorally: the opacity, viewport and
//! quirks-mode clauses are pinned only by the source-text assertions below,
//! and `check_stable` / `check_enabled` / `check_receives_pointer` have no
//! live fixture at all. Widening any of those clauses means writing the
//! fixture that would catch the mistake; do not assume one is waiting.

use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use crate::element::Element;
use crate::error::{Result, ZendriverError};

/// Set of actionability checks an action wants the element to satisfy
/// before its CDP dispatch. Per-field booleans gate the corresponding
/// `check_*` predicate in `wait_actionable`, and `hit_point` carries the
/// coordinate the hit-test should probe. Four named presets cover the
/// common combinations (`FULL`, `HOVER`, `VISIBLE_ONLY`, `TEXT_INPUT`);
/// callers may also construct ad-hoc sets directly, and a caller with an
/// explicit click position writes
/// `ActionabilityCheck { hit_point: opts.position, ..FULL }`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ActionabilityCheck {
    pub visible: bool,
    pub stable: bool,
    pub enabled: bool,
    pub receives_pointer: bool,
    /// Offset from the bbox top-left that [`check_receives_pointer`] should
    /// hit-test, matching [`crate::ClickOptions::position`]. `None` probes
    /// the bbox centre. Ignored unless `receives_pointer` is set.
    pub hit_point: Option<(f64, f64)>,
}

impl ActionabilityCheck {
    /// All four predicates — used by `click` and similar pointer-driven
    /// actions where layout stability + an unobstructed hit point matter.
    pub(crate) const FULL: Self = Self {
        visible: true,
        stable: true,
        enabled: true,
        receives_pointer: true,
        hit_point: None,
    };

    /// `FULL` minus the enabled check — used by `hover` / `hover_fast`.
    /// Hovering does not activate the element, so a disabled control still
    /// accepts `mouseover` and gating on `enabled` would reject a hover the
    /// browser performs happily.
    pub(crate) const HOVER: Self = Self {
        visible: true,
        stable: true,
        enabled: false,
        receives_pointer: true,
        hit_point: None,
    };

    /// Visibility only — used by `screenshot` (we just need pixels to
    /// capture; we don't care if a sibling overlay covers part of the
    /// element).
    pub(crate) const VISIBLE_ONLY: Self = Self {
        visible: true,
        stable: false,
        enabled: false,
        receives_pointer: false,
        hit_point: None,
    };

    /// Text-input combo — used by `type_text` / `focus` where the element
    /// must be visible + enabled but doesn't need a hit-tested pointer
    /// path (keystrokes route through the focused element, not the
    /// cursor's position).
    pub(crate) const TEXT_INPUT: Self = Self {
        visible: true,
        stable: false,
        enabled: true,
        receives_pointer: false,
        hit_point: None,
    };

    /// How many `Runtime.callFunctionOn` probes one pass of
    /// [`wait_actionable`] sends for this set — one per enabled predicate.
    ///
    /// Exists so a test fixture can serve exactly the gate's calls without
    /// hard-coding a count per preset: a count that drifts from the set does
    /// not fail cleanly, it eats the *next* unrelated call and the failure
    /// surfaces at a later, unrelated `expect`.
    #[cfg(test)]
    pub(crate) const fn probe_count(self) -> usize {
        self.visible as usize
            + self.enabled as usize
            + self.stable as usize
            + self.receives_pointer as usize
    }
}

/// `true` iff the element is rendered **and on-screen**. An element is
/// visible when all of the following hold:
///
/// - it is attached to the document (`isConnected`);
/// - it passes the platform's own `Element.checkVisibility` test with
///   `opacityProperty` + `visibilityProperty` + `contentVisibilityAuto`,
///   which covers `display: none`, `visibility: hidden` / `collapse`,
///   `content-visibility`, and `opacity: 0` on the element **or any
///   ancestor**;
/// - it has a positive bounding box;
/// - its bounding box **intersects the viewport** — any overlap counts, so
///   a partially-scrolled-in element still passes;
/// - its *effective* opacity — the product of its own computed opacity and
///   every ancestor's — is at least 1%.
///
/// The last two are the reason this isn't just a `checkVisibility` call.
/// `checkVisibility` only rejects an *exact* `opacity: 0`, so the standard
/// `opacity: 0.001` scraping honeypot passes it while being invisible to a
/// human; and `checkVisibility` says nothing about where the element sits
/// relative to the viewport.
///
/// Requires Chrome 121+, not 105+. `Element.checkVisibility` itself shipped
/// in 105, but the three option names passed below (`opacityProperty`,
/// `visibilityProperty`, `contentVisibilityAuto`) shipped in 121 — the first
/// two as aliases for 105's `checkOpacity` / `checkVisibilityCSS`, the third
/// as a new check (crbug feature 5070043440480256). Below 105 the call
/// throws, surfacing as a `JsException`: a loud failure rather than a
/// silently wrong answer. On 105–120 it does NOT throw — WebIDL drops
/// dictionary members it doesn't recognize — so the call silently degrades
/// to the default `display: none` test and stops covering
/// `visibility: hidden`/`collapse` and `content-visibility: auto`. The
/// effective-opacity loop below is independent of that and still holds.
///
/// Live callers: `Element::is_visible` and the `visible_only` filter in
/// `FindBuilder`/`FindAllBuilder` (`query::mod`).
pub(crate) async fn check_visible(el: &Element) -> Result<bool> {
    let js = r#"
        function(el) {
            if (!el || !el.isConnected) return false;

            // Platform primitive: handles `display: none`, `content-visibility`,
            // `visibility: hidden`/`collapse` and `opacity: 0` on the element OR any
            // ancestor — none of which the hand-rolled predecessor caught, since it
            // only ever read the element's own computed style, and its layout probe
            // reported a false negative for `position: fixed`.
            if (!el.checkVisibility({
                opacityProperty: true,
                visibilityProperty: true,
                contentVisibilityAuto: true
            })) return false;

            const rect = el.getBoundingClientRect();
            if (rect.width <= 0 || rect.height <= 0) return false;

            // On-screen test: the bbox must overlap the viewport by at least a
            // sliver. `getBoundingClientRect` is already viewport-relative, so
            // the box it must be compared against is the LAYOUT viewport. Two
            // things about reading that box are load-bearing:
            //
            // 1. It must never come from `window.inner*`. Every profile except
            //    `StealthProfile::off()` — including `native()`, which
            //    `Browser::builder()` installs by default — runs
            //    `zendriver-stealth`'s `patches/screen.js`, which rewrites
            //    `window.inner*` to the persona's screen size minus a chrome
            //    inset, while `scroll_into_view` moves the element with
            //    Chrome's real viewport. The inset is whatever the caller's
            //    `ScreenSpec` measured, falling back to a derived 86px when no
            //    capture supplied one, so its size is not knowable from here —
            //    which is the point: no constant can compensate for it.
            //    Measured under the default profile (the 86px fallback):
            //    `innerHeight` 994 against a real 1080. Comparing a
            //    real-geometry scroll against that fabricated viewport leaves
            //    every element landing in the inset band permanently "not
            //    visible", which broke `focus` — and with it `type_text` /
            //    `press` / `type_keys` — on any below-the-fold field.
            //
            // 2. WHICH element reports the layout viewport depends on the
            //    rendering mode, and reading the wrong one fails silently.
            //    Standards mode: `documentElement`. Quirks (`BackCompat`):
            //    `documentElement.client*` reports the document's CONTENT box
            //    and `body.client*` reports the viewport. Measured on a 6000px
            //    doctype-less page: `documentElement.clientHeight` 6024,
            //    `body.clientHeight` 1080. Reading `documentElement`
            //    unconditionally therefore compares the bbox against the whole
            //    document on any page without a doctype — a no-op that fails
            //    open, on exactly the sloppy legacy pages most likely to be
            //    carrying a honeypot.
            const box = document.compatMode === 'BackCompat'
                ? document.body
                : document.documentElement;
            // The `BackCompat` arm can be null: a document whose `<body>` was
            // removed still renders elements appended to `documentElement`, and
            // still answers `isConnected` for them (reproduced against Chrome).
            // The fallback is `visualViewport` rather than `window.inner*`
            // because it is the other unspoofed source, and it reads the real
            // 1080 in both modes. It is the VISUAL viewport, which diverges
            // from the layout viewport under pinch-zoom or an on-screen
            // keyboard — neither reachable through this crate's emulation
            // surface, and neither triggerable by page script.
            const vw = box ? box.clientWidth : window.visualViewport.width;
            const vh = box ? box.clientHeight : window.visualViewport.height;
            if (rect.right <= 0 || rect.bottom <= 0 || rect.left >= vw || rect.top >= vh) {
                return false;
            }

            // Effective opacity: `checkVisibility` rejects only an exact `opacity: 0`,
            // so a near-zero honeypot (`opacity: 0.001`) sails through it. Multiply the
            // ancestor chain the way the compositor does and reject anything below 1%.
            // The threshold is deliberately not `> 0`: exact-zero is the case honeypots
            // avoid precisely because it is the one everyone already checks. 1% sits well
            // below anything a real UI renders at rest and well above the honeypot range.
            // Done last — `getComputedStyle` per ancestor is the priciest step here.
            // Opacity is in [0, 1], so the running product only ever decreases and
            // cannot climb back over the threshold: once it is under, stop walking.
            let effective = 1;
            for (let node = el; node; node = node.parentElement) {
                const own = parseFloat(getComputedStyle(node).opacity);
                if (Number.isFinite(own)) effective *= own;
                if (effective < 0.01) return false;
            }

            return true;
        }
    "#;
    let res = el.call_on_main(js, json!([])).await?;
    Ok(res.get("value").and_then(|v| v.as_bool()).unwrap_or(false))
}

/// `true` iff the element's bounding box is unchanged across two
/// `requestAnimationFrame` ticks (within 0.5px on each of x/y/w/h). This
/// catches mid-transition layout shifts that would race a synthesized
/// click.
pub(crate) async fn check_stable(el: &Element) -> Result<bool> {
    let js = r#"
        function(el) {
            return new Promise(resolve => {
                if (!el || !el.isConnected) { resolve(false); return; }
                const first = el.getBoundingClientRect();
                requestAnimationFrame(() => {
                    requestAnimationFrame(() => {
                        const second = el.getBoundingClientRect();
                        const stable =
                            Math.abs(first.x - second.x) < 0.5 &&
                            Math.abs(first.y - second.y) < 0.5 &&
                            Math.abs(first.width - second.width) < 0.5 &&
                            Math.abs(first.height - second.height) < 0.5;
                        resolve(stable);
                    });
                });
            });
        }
    "#;
    let res = el.call_on_main(js, json!([])).await?;
    Ok(res.get("value").and_then(|v| v.as_bool()).unwrap_or(false))
}

/// `true` iff the element is not disabled: native `el.disabled` is
/// false-ish AND `aria-disabled` is not `'true'`. Non-form elements
/// (which have no `disabled` property) are considered enabled.
pub(crate) async fn check_enabled(el: &Element) -> Result<bool> {
    let js = r#"
        function(el) {
            if (!el) return false;
            // `disabled === false` for form controls; `undefined` for non-form elements
            // (which we treat as enabled).
            if (el.disabled === true) return false;
            const ariaDisabled = el.getAttribute && el.getAttribute('aria-disabled');
            if (ariaDisabled === 'true') return false;
            return true;
        }
    "#;
    let res = el.call_on_main(js, json!([])).await?;
    Ok(res.get("value").and_then(|v| v.as_bool()).unwrap_or(false))
}

/// `true` iff a synthesized click at the point that will actually be clicked
/// would land on the element (or one of its descendants). Walks the ancestor
/// chain of `document.elementFromPoint(cx, cy)`; if our element appears in that
/// chain, pointer events reach it. Returns `false` when a sibling overlay
/// covers the hit point.
///
/// `position` is the caller's offset from the bbox top-left, matching
/// [`crate::ClickOptions::position`]; `None` hit-tests the centre. Probing a
/// point the dispatch will not use lets the gate pass while the real click
/// lands on an overlay.
pub(crate) async fn check_receives_pointer(
    el: &Element,
    position: Option<(f64, f64)>,
) -> Result<bool> {
    let js = r#"
        function(el, dx, dy) {
            if (!el || !el.isConnected) return false;
            const rect = el.getBoundingClientRect();
            if (rect.width <= 0 || rect.height <= 0) return false;
            // A caller with no explicit position passes no arguments at all,
            // so dx/dy arrive as `undefined` — `Number.isFinite` covers that
            // and any non-numeric junk in one test, falling back to the centre.
            const cx = Number.isFinite(dx) ? rect.left + dx : rect.left + rect.width / 2;
            const cy = Number.isFinite(dy) ? rect.top + dy : rect.top + rect.height / 2;
            let hit = document.elementFromPoint(cx, cy);
            while (hit) {
                if (hit === el) return true;
                hit = hit.parentElement;
            }
            return false;
        }
    "#;
    // `Runtime.callFunctionOn` takes an array of CallArgument *objects*, so
    // each coordinate has to be wrapped in `{ "value": … }`; passing the bare
    // numbers made Chrome reject the whole call with "Invalid parameters"
    // ("Failed to deserialize params.arguments"), which failed every gated
    // `click` / `hover` / `tap` against a real browser. The mock tests could
    // not catch it — `MockConnection` doesn't validate CDP parameter shapes.
    //
    // `call_on_main` prepends the element handle, so these land at
    // arguments[1] / arguments[2] — which is where the leading `el`
    // parameter in the declaration above leaves `dx` / `dy`.
    let args = match position {
        Some((dx, dy)) => json!([{ "value": dx }, { "value": dy }]),
        None => json!([]),
    };
    let res = el.call_on_main(js, args).await?;
    Ok(res.get("value").and_then(|v| v.as_bool()).unwrap_or(false))
}

/// Poll each predicate in `require` at 50 ms intervals until all enabled
/// checks pass, or `timeout` elapses. On deadline, returns
/// [`ZendriverError::NotActionable`] with the first-failing check's
/// human-readable reason ("not visible", "not enabled", "not stable (still
/// animating)", or "occluded by overlay").
///
/// Predicates are evaluated in fixed order: visible → enabled → stable →
/// receives_pointer — the two cheap reads first, so the expensive
/// two-frame stability wait and the hit-test never run while the element is
/// still hidden or disabled. The *set* of checks is Playwright's; this
/// ordering is not, and does not claim to be — Playwright's actionability
/// docs list visible → stable → receives events → enabled.
///
/// `require.hit_point` is the offset the caller will click at, forwarded to
/// the hit-test so the gate probes the point the dispatch actually uses.
/// `None` probes the bbox centre. It is re-read from the live bbox on every
/// poll, so an element that moves mid-wait is hit-tested where it currently
/// is.
pub(crate) async fn wait_actionable(
    el: &Element,
    require: ActionabilityCheck,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let poll_interval = Duration::from_millis(50);
    loop {
        let mut failed_reason: Option<&'static str> = None;
        if require.visible && !check_visible(el).await? {
            failed_reason = Some("not visible");
        } else if require.enabled && !check_enabled(el).await? {
            failed_reason = Some("not enabled");
        } else if require.stable && !check_stable(el).await? {
            failed_reason = Some("not stable (still animating)");
        } else if require.receives_pointer && !check_receives_pointer(el, require.hit_point).await?
        {
            failed_reason = Some("occluded by overlay");
        }
        match failed_reason {
            None => return Ok(()),
            Some(reason) => {
                if Instant::now() >= deadline {
                    return Err(ZendriverError::NotActionable(timeout, reason.to_owned()));
                }
                tokio::time::sleep(poll_interval).await;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tab::Tab;
    use crate::test_support::{js_params, serve_call, serve_call_js};
    use serde_json::Value;
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

    /// Collapse every run of whitespace to a single space, so a source
    /// assertion can span what the JS literal wraps across lines without
    /// pinning its indentation.
    fn one_line(js: &str) -> String {
        js.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Drive `check_visible` against a mock connection, answer its probe with
    /// `value`, and hand back the JS source the probe actually sent plus the
    /// resolved verdict.
    async fn probe_check_visible(value: bool) -> (String, bool) {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab.clone(), 99, "R1".to_string());

        let fut = tokio::spawn(async move { check_visible(&el).await });

        let js = serve_call_js(&mut mock, json!({ "value": value, "type": "boolean" })).await;

        let verdict = fut.await.unwrap().unwrap();
        conn.shutdown();
        (js, verdict)
    }

    #[tokio::test]
    async fn probe_source_delegates_to_check_visibility() {
        let (js, verdict) = probe_check_visible(true).await;
        assert!(verdict, "a `true` probe result must resolve to visible");
        assert!(js.contains("checkVisibility"), "{js}");
        // The three flags are what make `checkVisibility` cover ancestors —
        // without them it only reports `display: none`.
        assert!(js.contains("opacityProperty: true"), "{js}");
        assert!(js.contains("visibilityProperty: true"), "{js}");
        assert!(js.contains("contentVisibilityAuto: true"), "{js}");
    }

    #[tokio::test]
    async fn probe_source_tests_viewport_intersection() {
        let (js, _) = probe_check_visible(true).await;
        // `visible_only(true)` promises offscreen candidates are filtered
        // out; that requires comparing the bbox against the viewport box.
        assert!(js.contains("rect.right <= 0"), "{js}");
        assert!(js.contains("rect.bottom <= 0"), "{js}");
        assert!(js.contains("rect.left >= vw"), "{js}");
        assert!(js.contains("rect.top >= vh"), "{js}");
    }

    /// The viewport box must be read from an unspoofed source, and from the
    /// element that actually reports the layout viewport in *both* rendering
    /// modes.
    ///
    /// Two separate defects are pinned here, both of which shipped:
    ///
    /// - Reading `window.inner*` at all. Every profile except
    ///   `StealthProfile::off()` rewrites those, so they must not appear —
    ///   not even as a trailing fallback. The predecessor of this test
    ///   asserted only that `documentElement` came *first*, which a
    ///   `documentElement.clientWidth || window.innerWidth` expression
    ///   satisfies while still reaching the spoofed value.
    /// - Reading `documentElement` unconditionally. In quirks mode that is
    ///   the document's content box (6024 on a 6000px page), which is
    ///   truthy, so the `||` fallback never fired and the whole on-screen
    ///   clause degraded to a no-op. Asserting merely that *some* viewport
    ///   is read would pass that.
    ///
    /// The whole-source substring search is safe because the probe's own
    /// comments spell the spoofable pair `window.inner*` rather than naming
    /// either property.
    ///
    /// The ternary is asserted whole rather than as three independent
    /// substrings. Naming `compatMode`, `body` and `documentElement`
    /// separately is satisfied just as well by the arms swapped the wrong way
    /// round — which is the exact defect the second bullet describes, so a
    /// pin that cannot tell the two apart pins nothing.
    #[tokio::test]
    async fn probe_source_reads_the_layout_viewport_in_both_rendering_modes() {
        let (js, _) = probe_check_visible(true).await;
        assert!(!js.contains("window.innerWidth"), "{js}");
        assert!(!js.contains("window.innerHeight"), "{js}");
        assert!(
            one_line(&js).contains(
                "document.compatMode === 'BackCompat' ? document.body : document.documentElement"
            ),
            "quirks mode must select `body` and standards mode `documentElement`, \
             in that order: {js}"
        );
    }

    #[tokio::test]
    async fn probe_source_multiplies_ancestor_opacity_against_a_threshold() {
        let (js, _) = probe_check_visible(true).await;
        // Honeypot guard: walk the ancestor chain and compare the product
        // against a non-zero floor, so `opacity: 0.001` (self OR ancestor)
        // reads as hidden.
        assert!(js.contains("node.parentElement"), "{js}");
        assert!(js.contains("getComputedStyle(node).opacity"), "{js}");
        assert!(js.contains("effective *= own"), "{js}");
        // The threshold test sits *inside* the walk: the product only ever
        // decreases, so a chain that has already gone under can stop early.
        assert!(
            one_line(&js).contains(
                "if (Number.isFinite(own)) effective *= own; if (effective < 0.01) return false; }"
            ),
            "the opacity floor must be checked inside the ancestor walk: {js}"
        );
    }

    #[tokio::test]
    async fn probe_source_drops_the_buggy_predecessors() {
        let (js, _) = probe_check_visible(true).await;
        // Regression pins for the three defects this probe replaced:
        // a string compare against '0' (missed `opacity: 0.001`), an
        // own-element-only style read (missed ancestor opacity), and the
        // `offsetParent` hack (false negative on `position: fixed`).
        assert!(!js.contains("offsetParent"), "{js}");
        assert!(!js.contains("style.opacity"), "{js}");
        assert!(!js.contains("=== '0'"), "{js}");
    }

    /// Not a source pin: this one covers the Rust side of `check_visible`,
    /// namely that it reads the probe's verdict out of `result.value` rather
    /// than defaulting to the `unwrap_or(false)` fallback.
    #[tokio::test]
    async fn check_visible_resolves_the_probe_result_it_was_handed() {
        let (_, hidden) = probe_check_visible(false).await;
        assert!(!hidden, "a `false` probe result must resolve to hidden");
        let (_, shown) = probe_check_visible(true).await;
        assert!(shown, "a `true` probe result must resolve to visible");
    }

    /// `check_receives_pointer` must wrap its coordinates as CallArgument
    /// *objects* (`{ "value": … }`).
    ///
    /// Regression pin for a live-only bug: the bare `[dx, dy]` this used to
    /// send made Chrome reject the whole call with `-32602 Invalid parameters`
    /// ("Failed to deserialize params.arguments"), so every gated `click` /
    /// `hover` / `tap` failed against a real browser while the mock suite
    /// stayed green — `MockConnection` replays frames without validating CDP
    /// parameter shapes. Hence an explicit assertion on the shape.
    #[tokio::test]
    async fn check_receives_pointer_wraps_its_coordinates_as_call_arguments() {
        // `call_on_main` prepends the element handle, so the coordinates are
        // the second and third arguments — each one an object, never a bare
        // number.
        assert_eq!(
            probe_pointer_args(Some((3.0, 4.0))).await,
            json!([{ "objectId": "R1" }, { "value": 3.0 }, { "value": 4.0 }]),
            "an explicit click position must travel as two CallArgument objects",
        );

        // No position → no coordinates at all, leaving dx/dy `undefined` in
        // the JS, which is what its `Number.isFinite` centre-fallback reads.
        assert_eq!(
            probe_pointer_args(None).await,
            json!([{ "objectId": "R1" }]),
            "a centre hit-test must send no coordinates rather than bare nulls",
        );
    }

    /// Drive one `check_receives_pointer` probe against a fresh mock and hand
    /// back the `arguments` array it put on the wire.
    async fn probe_pointer_args(at: Option<(f64, f64)>) -> Value {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 99, "R1".to_string());

        // Spawn before serving: nothing goes out until the probe is running.
        let fut = tokio::spawn(async move { check_receives_pointer(&el, at).await });

        let params = serve_call(&mut mock, json!({ "value": true, "type": "boolean" })).await;

        assert!(fut.await.unwrap().unwrap());
        conn.shutdown();
        params["arguments"].clone()
    }

    /// Every predicate binds the element as its first positional argument,
    /// and `check_receives_pointer` takes its coordinates in `(dx, dy)` order.
    ///
    /// `call_on_main` prepends the element handle to `arguments`, so a body
    /// that forgot its leading `el` parameter would silently read the handle
    /// as its first *caller* argument — and, for the three no-argument
    /// predicates, see `el` as `undefined` and answer `false` for everything.
    /// This drives all four past that in one go.
    ///
    /// The whole parameter list is asserted, not just the leading name: a
    /// `starts_with("function(el")` check accepts `function(elephant, dy, dx)`
    /// — right prefix, transposed coordinates, every hit-test off by the
    /// difference between the two offsets.
    #[tokio::test]
    async fn every_predicate_binds_the_element_as_its_first_argument() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab.clone(), 99, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move {
                (
                    check_visible(&e).await.unwrap(),
                    check_enabled(&e).await.unwrap(),
                    check_stable(&e).await.unwrap(),
                    check_receives_pointer(&e, None).await.unwrap(),
                )
            }
        });

        let mut calls = Vec::new();
        for _ in 0..4 {
            calls.push(serve_call(&mut mock, json!({ "value": true, "type": "boolean" })).await);
        }

        assert_eq!(fut.await.unwrap(), (true, true, true, true));

        let sources: Vec<&str> = calls
            .iter()
            .map(|c| c["functionDeclaration"].as_str().unwrap())
            .collect();

        // Pin which source belongs to which predicate, so a future reorder of
        // the gate can't quietly re-point one of these assertions.
        assert!(sources[0].contains("checkVisibility"), "{}", sources[0]);
        assert!(sources[1].contains("aria-disabled"), "{}", sources[1]);
        assert!(
            sources[2].contains("requestAnimationFrame"),
            "{}",
            sources[2]
        );
        assert!(sources[3].contains("elementFromPoint"), "{}", sources[3]);

        // The three no-argument predicates bind `el` and nothing else; the
        // hit-test binds the two offsets after it, dx before dy.
        let expected = [vec!["el"], vec!["el"], vec!["el"], vec!["el", "dx", "dy"]];
        for ((call, js), want) in calls.iter().zip(&sources).zip(expected) {
            assert_eq!(js_params(js), want, "{js}");
            assert_eq!(
                call["arguments"][0]["objectId"], "R1",
                "the element must be the first argument the body reads",
            );
        }

        conn.shutdown();
    }
}
