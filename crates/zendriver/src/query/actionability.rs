//! Playwright-style actionability predicates: visible / stable / enabled /
//! receives_pointer. Each runs a small JS function on the element's remote
//! handle via `Element::call_on_main` and returns a `bool`.
//!
//! The aggregate gate ([`wait_actionable`]) polls the four predicates in
//! order and raises `NotActionable` on deadline; `Element::click_with`,
//! `hover`, `focus`, `type_text`, and `screenshot` (in `element::actions`
//! / `element::screenshot`) each pick the [`ActionabilityCheck`] preset
//! matching what they need before dispatching.

use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use crate::element::Element;
use crate::error::{Result, ZendriverError};

/// Set of actionability checks an action wants the element to satisfy
/// before its CDP dispatch. Per-field booleans gate the corresponding
/// `check_*` predicate in `wait_actionable`. Three named presets
/// cover the common combinations (`FULL`, `VISIBLE_ONLY`, `TEXT_INPUT`);
/// callers may also construct ad-hoc sets directly.
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
    pub(crate) const FULL: Self = Self {
        visible: true,
        stable: true,
        enabled: true,
        receives_pointer: true,
    };

    /// Visibility only — used by `screenshot` (we just need pixels to
    /// capture; we don't care if a sibling overlay covers part of the
    /// element).
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
    pub(crate) const TEXT_INPUT: Self = Self {
        visible: true,
        stable: false,
        enabled: true,
        receives_pointer: false,
    };
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
/// Requires Chrome 105+ (`Element.checkVisibility`). On an older build the
/// JS throws, which surfaces as a `JsException` — a loud failure rather
/// than a silently wrong answer.
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
            // sliver. `getBoundingClientRect` is already viewport-relative.
            const vw = window.innerWidth || document.documentElement.clientWidth;
            const vh = window.innerHeight || document.documentElement.clientHeight;
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
            let effective = 1;
            for (let node = el; node; node = node.parentElement) {
                const own = parseFloat(getComputedStyle(node).opacity);
                if (Number.isFinite(own)) effective *= own;
            }
            if (effective < 0.01) return false;

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
            const cx = dx === null ? rect.left + rect.width / 2 : rect.left + dx;
            const cy = dy === null ? rect.top + rect.height / 2 : rect.top + dy;
            let hit = document.elementFromPoint(cx, cy);
            while (hit) {
                if (hit === el) return true;
                hit = hit.parentElement;
            }
            return false;
        }
    "#;
    let (dx, dy) = match position {
        Some((dx, dy)) => (json!(dx), json!(dy)),
        None => (json!(null), json!(null)),
    };
    let res = el.call_on_main(js, json!([dx, dy])).await?;
    Ok(res.get("value").and_then(|v| v.as_bool()).unwrap_or(false))
}

/// Poll each predicate in `require` at 50 ms intervals until all enabled
/// checks pass, or `timeout` elapses. On deadline, returns
/// [`ZendriverError::NotActionable`] with the first-failing check's
/// human-readable reason ("not visible", "not enabled", "not stable (still
/// animating)", or "occluded by overlay").
///
/// Predicates are evaluated in fixed order: visible → enabled → stable →
/// receives_pointer. This matches Playwright's gate ordering and avoids
/// running the more-expensive stability + hit-testing checks while the
/// element is still hidden or disabled.
///
/// `position` is the offset the caller will click at, forwarded to the
/// hit-test so the gate probes the point the dispatch actually uses. `None`
/// probes the bbox centre. It is re-read from the live bbox on every poll, so
/// an element that moves mid-wait is hit-tested where it currently is.
pub(crate) async fn wait_actionable(
    el: &Element,
    require: ActionabilityCheck,
    timeout: Duration,
    position: Option<(f64, f64)>,
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
        } else if require.receives_pointer && !check_receives_pointer(el, position).await? {
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
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

    /// Drive `check_visible` against a mock connection, answer its single
    /// `Runtime.callFunctionOn` with `value`, and hand back the JS source
    /// the probe actually sent plus the resolved verdict.
    async fn probe_check_visible(value: bool) -> (String, bool) {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab.clone(), 99, "R1".to_string());

        let fut = tokio::spawn(async move { check_visible(&el).await });

        // `expect_cmd` drops non-matching frames and never times out on its
        // own — bound the wait so a regression fails loudly instead of hanging.
        let id = tokio::time::timeout(
            Duration::from_secs(5),
            mock.expect_cmd("Runtime.callFunctionOn"),
        )
        .await
        .expect("check_visible should send exactly one Runtime.callFunctionOn");
        let js = mock.last_sent()["params"]["functionDeclaration"]
            .as_str()
            .unwrap()
            .to_string();
        mock.reply(
            id,
            json!({ "result": { "value": value, "type": "boolean" } }),
        )
        .await;

        let verdict = fut.await.unwrap().unwrap();
        (js, verdict)
    }

    #[tokio::test]
    async fn check_visible_probe_delegates_to_check_visibility() {
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
    async fn check_visible_probe_tests_viewport_intersection() {
        let (js, _) = probe_check_visible(true).await;
        // `visible_only(true)` promises offscreen candidates are filtered
        // out; that requires comparing the bbox against the viewport box.
        assert!(js.contains("window.innerWidth"), "{js}");
        assert!(js.contains("window.innerHeight"), "{js}");
        assert!(js.contains("rect.right <= 0"), "{js}");
        assert!(js.contains("rect.bottom <= 0"), "{js}");
        assert!(js.contains("rect.left >= vw"), "{js}");
        assert!(js.contains("rect.top >= vh"), "{js}");
    }

    #[tokio::test]
    async fn check_visible_probe_multiplies_ancestor_opacity_against_a_threshold() {
        let (js, _) = probe_check_visible(true).await;
        // Honeypot guard: walk the ancestor chain and compare the product
        // against a non-zero floor, so `opacity: 0.001` (self OR ancestor)
        // reads as hidden.
        assert!(js.contains("node.parentElement"), "{js}");
        assert!(js.contains("getComputedStyle(node).opacity"), "{js}");
        assert!(js.contains("effective *= own"), "{js}");
        assert!(js.contains("effective < 0.01"), "{js}");
    }

    #[tokio::test]
    async fn check_visible_probe_drops_the_buggy_predecessors() {
        let (js, _) = probe_check_visible(true).await;
        // Regression pins for the three defects this probe replaced:
        // a string compare against '0' (missed `opacity: 0.001`), an
        // own-element-only style read (missed ancestor opacity), and the
        // `offsetParent` hack (false negative on `position: fixed`).
        assert!(!js.contains("offsetParent"), "{js}");
        assert!(!js.contains("style.opacity"), "{js}");
        assert!(!js.contains("=== '0'"), "{js}");
    }

    #[tokio::test]
    async fn check_visible_reports_false_when_the_page_says_hidden() {
        let (_, verdict) = probe_check_visible(false).await;
        assert!(!verdict, "a `false` probe result must resolve to hidden");
    }
}
