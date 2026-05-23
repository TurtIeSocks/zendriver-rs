//! Playwright-style actionability predicates: visible / stable / enabled /
//! receives_pointer. Each runs a small JS function on the element's remote
//! handle via `Element::call_on_main` and returns a `bool`.
//!
//! The aggregate gate (`wait_actionable`) and `NotActionable` emission land
//! in T15; T14 only ships the four predicate primitives + the
//! `ActionabilityCheck` requirements struct that downstream actions use to
//! describe which checks they need.

use serde_json::json;

use crate::element::Element;
use crate::error::Result;

/// Set of actionability checks an action wants the element to satisfy
/// before its CDP dispatch. Per-field booleans gate the corresponding
/// `check_*` predicate in `wait_actionable` (T15). Three named presets
/// cover the common combinations (`FULL`, `VISIBLE_ONLY`, `TEXT_INPUT`);
/// callers may also construct ad-hoc sets directly.
#[allow(dead_code)] // First caller (`wait_actionable`) lands in T15.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActionabilityCheck {
    pub visible: bool,
    pub stable: bool,
    pub enabled: bool,
    pub receives_pointer: bool,
}

impl ActionabilityCheck {
    /// All four predicates — used by `click` and similar pointer-driven
    /// actions where layout stability + an unobstructed hit point matter.
    #[allow(dead_code)] // Consumed by `click_with` in T20.
    pub(crate) const FULL: Self = Self {
        visible: true,
        stable: true,
        enabled: true,
        receives_pointer: true,
    };

    /// Visibility only — used by `screenshot` (we just need pixels to
    /// capture; we don't care if a sibling overlay covers part of the
    /// element).
    #[allow(dead_code)] // Consumed by `Element::screenshot` in T26.
    pub(crate) const VISIBLE_ONLY: Self = Self {
        visible: true,
        stable: false,
        enabled: false,
        receives_pointer: false,
    };

    /// Text-input combo — used by `type_text` / `focus` where the element
    /// must be visible + enabled but doesn't need a hit-tested pointer
    /// path (keystrokes route through the focused element, not the
    /// cursor's position).
    #[allow(dead_code)] // Consumed by `type_text` / `focus` in T19/T22.
    pub(crate) const TEXT_INPUT: Self = Self {
        visible: true,
        stable: false,
        enabled: true,
        receives_pointer: false,
    };
}

/// `true` iff the element is rendered: attached to the document, has a
/// non-`null` `offsetParent` (catches `display: none` ancestors), a
/// positive bbox, computed `visibility !== 'hidden'`, and computed
/// `opacity !== '0'`.
#[allow(dead_code)] // First callers (`is_visible` + `wait_actionable`) land in T15/T18.
pub(crate) async fn check_visible(el: &Element) -> Result<bool> {
    let js = r#"
        function(el) {
            if (!el || !el.isConnected) return false;
            // offsetParent === null catches `display: none` on the element or any ancestor
            // (except for fixed-position elements, which are handled by the bbox check below).
            if (el.offsetParent === null && getComputedStyle(el).position !== 'fixed') return false;
            const rect = el.getBoundingClientRect();
            if (rect.width <= 0 || rect.height <= 0) return false;
            const style = getComputedStyle(el);
            if (style.visibility === 'hidden') return false;
            if (style.opacity === '0') return false;
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
#[allow(dead_code)] // First caller (`wait_actionable`) lands in T15.
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
#[allow(dead_code)] // First callers (`is_enabled` + `wait_actionable`) land in T15/T18.
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

/// `true` iff a synthesized click at the element's bbox center would land
/// on the element (or one of its descendants). Walks the ancestor chain
/// of `document.elementFromPoint(cx, cy)`; if our element appears in that
/// chain, pointer events reach it. Returns `false` when a sibling overlay
/// covers the hit point.
#[allow(dead_code)] // First caller (`wait_actionable`) lands in T15.
pub(crate) async fn check_receives_pointer(el: &Element) -> Result<bool> {
    let js = r#"
        function(el) {
            if (!el || !el.isConnected) return false;
            const rect = el.getBoundingClientRect();
            if (rect.width <= 0 || rect.height <= 0) return false;
            const cx = rect.left + rect.width / 2;
            const cy = rect.top + rect.height / 2;
            let hit = document.elementFromPoint(cx, cy);
            while (hit) {
                if (hit === el) return true;
                hit = hit.parentElement;
            }
            return false;
        }
    "#;
    let res = el.call_on_main(js, json!([])).await?;
    Ok(res.get("value").and_then(|v| v.as_bool()).unwrap_or(false))
}
