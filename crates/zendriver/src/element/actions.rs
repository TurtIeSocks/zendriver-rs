//! `Element` actions: `hover` / `hover_raw` / `focus` / `scroll_into_view`.
//!
//! Each action wraps its CDP dispatch sequence in [`Element::with_refresh`]
//! so a stale handle (post-navigation, post-React-rerender) transparently
//! re-resolves once and retries.
//!
//! `hover` / `hover_raw`:
//!   1. `scroll_into_view` — bring the element into the viewport so its
//!      bbox center is a real, dispatchable coordinate.
//!   2. `wait_actionable` with `visible + stable + receives_pointer` — gate
//!      to avoid mid-transition hover races and overlay occlusion. Pointer
//!      events are dispatched at the geometric center, so we need the
//!      element to be the actual hit-test target there. `enabled` is left
//!      off — hover doesn't activate the element, so disabled controls
//!      still accept mouseover.
//!   3. Compute bbox center (`x + width / 2`, `y + height / 2`).
//!   4. Dispatch the mouse-move via the shared [`InputController`]:
//!      `hover` uses [`mouse::move_realistic`] (Bezier + jitter + timing
//!      to model human pointer paths); `hover_raw` uses [`mouse::move_raw`]
//!      (single teleport dispatch for test/automation paths that don't
//!      need behavioral realism).
//!
//! `focus`: `wait_actionable` with [`ActionabilityCheck::TEXT_INPUT`]
//! (visible + enabled, no pointer or stability requirement — focus routes
//! through the focused element, not the cursor's position), then
//! `el.focus()` via [`Element::call_on_main`].
//!
//! `scroll_into_view`: no actionability gate (this *is* the visibility
//! prereq other actions wait for). Calls `el.scrollIntoView({ block:
//! 'center', behavior: 'instant' })`. `block: 'center'` matches Playwright
//! (avoids sticky headers/footers obscuring the element after the scroll);
//! `behavior: 'instant'` skips animation so the post-scroll bbox is final
//! by the time the next CDP call runs.
//!
//! Test scaffolding limitation: `Tab::input()` returns `None` when the
//! owning `Browser`'s `Weak` ref can't upgrade — true for unit tests that
//! build a `Tab` with `std::sync::Weak::new()`. `hover` and `hover_raw`
//! surface that as `ZendriverError::Navigation("no input controller
//! available")` so tests can detect the mis-configuration without a panic.
//! `focus` and `scroll_into_view` don't dispatch pointer events and need
//! no `InputController`, so they work in those test setups unchanged.

use std::time::Duration;

use serde_json::json;

use crate::element::Element;
use crate::error::{Result, ZendriverError};
use crate::input::keyboard::KeyModifiers;
use crate::input::mouse::{self, MouseButton};
use crate::query::actionability::{self, ActionabilityCheck};

/// Default deadline for the actionability gate before each action. Matches
/// the value the spec calls out for P3; per-call override land in P4 when
/// the per-action options structs grow.
const DEFAULT_ACTIONABILITY_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-call knobs for [`Element::click_with`]. `Default` matches the
/// behavior of [`Element::click`]: a left, single, realistic click at the
/// element's bbox center with no modifiers held and full actionability
/// gating. Override fields individually for richer dispatches
/// (right-click, modifier-held click, raw teleport, etc.).
#[derive(Debug, Clone, Copy)]
pub struct ClickOptions {
    /// Which mouse button to dispatch. `MouseButton::Left` by default.
    pub button: MouseButton,
    /// Modifier keys held during the dispatch. Empty by default.
    pub modifiers: KeyModifiers,
    /// `clickCount` for the CDP dispatch. `1` by default; set `2` for a
    /// double-click in a single `click_with` call.
    pub click_count: u32,
    /// Skip the [`ActionabilityCheck::FULL`] gate when true. Use sparingly
    /// — bypasses the visibility/stability/pointer checks the gate
    /// performs. Mirrors Playwright's `force: true`.
    pub force: bool,
    /// Bezier-interpolated cursor path (`true`) vs single teleport
    /// dispatch (`false`). `true` by default; the `click_raw` shortcut
    /// flips this for deterministic test paths.
    pub realistic: bool,
    /// Click position relative to the element's bbox top-left
    /// (`(dx, dy)`). `None` clicks at the bbox center.
    pub position: Option<(f64, f64)>,
}

impl Default for ClickOptions {
    fn default() -> Self {
        Self {
            button: MouseButton::Left,
            modifiers: KeyModifiers::empty(),
            click_count: 1,
            force: false,
            realistic: true,
            position: None,
        }
    }
}

impl Element {
    /// Click this element with all defaults — left button, single click,
    /// realistic Bezier-path cursor approach, and the full actionability
    /// gate. Equivalent to `click_with(ClickOptions::default())`. For
    /// right-click / modifier-held / double-click / raw-teleport
    /// variations, use [`Element::click_with`].
    pub async fn click(&self) -> Result<()> {
        self.click_with(ClickOptions::default()).await
    }

    /// Click this element with a deterministic raw teleport — skips the
    /// Bezier interpolation [`Element::click`] does and bypasses the
    /// actionability gate. Equivalent to
    /// `click_with(ClickOptions { realistic: false, force: true, ..Default::default() })`.
    /// Intended for test paths and fast automation flows where realism
    /// and per-action gating get in the way.
    pub async fn click_raw(&self) -> Result<()> {
        self.click_with(ClickOptions {
            realistic: false,
            force: true,
            ..Default::default()
        })
        .await
    }

    /// Click this element with explicit [`ClickOptions`]. See module
    /// docs for the dispatch sequence — same shape as `hover`
    /// (scroll → gate → bbox math → pointer dispatch) but emits the
    /// `mousePressed` + `mouseReleased` pair after the cursor arrives.
    /// `opts.force` skips the [`ActionabilityCheck::FULL`] gate;
    /// `opts.position` shifts the click point off bbox-center.
    pub async fn click_with(&self, opts: ClickOptions) -> Result<()> {
        self.with_refresh(|| async move {
            self.scroll_into_view().await?;
            if !opts.force {
                actionability::wait_actionable(
                    self,
                    ActionabilityCheck::FULL,
                    DEFAULT_ACTIONABILITY_TIMEOUT,
                )
                .await?;
            }
            let bbox = self
                .bounding_box()
                .await?
                .ok_or_else(|| ZendriverError::Navigation("element has no bounding box".into()))?;
            let (tx, ty) = match opts.position {
                Some((dx, dy)) => (bbox.x + dx, bbox.y + dy),
                None => (bbox.x + bbox.width / 2.0, bbox.y + bbox.height / 2.0),
            };
            let input = self.inner.tab.input().ok_or_else(|| {
                ZendriverError::Navigation("no input controller available".into())
            })?;
            mouse::click_at(
                &input,
                &self.inner.tab,
                tx,
                ty,
                opts.button,
                opts.click_count,
                opts.realistic,
            )
            .await
        })
        .await
    }

    /// Hover the cursor over this element's bbox center, with a realistic
    /// Bezier-interpolated mouse path. See module docs for the full
    /// sequence (`scroll_into_view` → actionability gate → bbox center →
    /// dispatch). Use [`Element::hover_raw`] when the cursor path doesn't
    /// matter (tests, fast automation paths that don't need behavioral
    /// realism).
    pub async fn hover(&self) -> Result<()> {
        self.with_refresh(|| async move {
            self.scroll_into_view().await?;
            actionability::wait_actionable(
                self,
                ActionabilityCheck {
                    visible: true,
                    stable: true,
                    enabled: false,
                    receives_pointer: true,
                },
                DEFAULT_ACTIONABILITY_TIMEOUT,
            )
            .await?;
            let bbox = self
                .bounding_box()
                .await?
                .ok_or_else(|| ZendriverError::Navigation("element has no bounding box".into()))?;
            let cx = bbox.x + bbox.width / 2.0;
            let cy = bbox.y + bbox.height / 2.0;
            let input = self.inner.tab.input().ok_or_else(|| {
                ZendriverError::Navigation("no input controller available".into())
            })?;
            mouse::move_realistic(&input, &self.inner.tab, cx, cy).await
        })
        .await
    }

    /// Hover the cursor over this element's bbox center via a single
    /// dispatchMouseEvent teleport. Skips the Bezier interpolation
    /// [`Element::hover`] does — same actionability gate + bbox math, but
    /// no human-pointer modeling. Intended for paths where deterministic
    /// timing matters more than realism.
    pub async fn hover_raw(&self) -> Result<()> {
        self.with_refresh(|| async move {
            self.scroll_into_view().await?;
            actionability::wait_actionable(
                self,
                ActionabilityCheck {
                    visible: true,
                    stable: true,
                    enabled: false,
                    receives_pointer: true,
                },
                DEFAULT_ACTIONABILITY_TIMEOUT,
            )
            .await?;
            let bbox = self
                .bounding_box()
                .await?
                .ok_or_else(|| ZendriverError::Navigation("element has no bounding box".into()))?;
            let cx = bbox.x + bbox.width / 2.0;
            let cy = bbox.y + bbox.height / 2.0;
            let input = self.inner.tab.input().ok_or_else(|| {
                ZendriverError::Navigation("no input controller available".into())
            })?;
            mouse::move_raw(&input, &self.inner.tab, cx, cy).await
        })
        .await
    }

    /// Move keyboard focus to this element by calling `el.focus()`. Gated
    /// by [`ActionabilityCheck::TEXT_INPUT`] (visible + enabled) so disabled
    /// controls + hidden elements surface a `NotActionable` error rather
    /// than silently no-op on the page side. Reused by `type_text` /
    /// `press` in T22 — they focus first so keystrokes reach this element.
    pub async fn focus(&self) -> Result<()> {
        self.with_refresh(|| async move {
            actionability::wait_actionable(
                self,
                ActionabilityCheck::TEXT_INPUT,
                DEFAULT_ACTIONABILITY_TIMEOUT,
            )
            .await?;
            let _ = self
                .call_on_main("function(){ this.focus(); }", json!([]))
                .await?;
            Ok(())
        })
        .await
    }

    /// Scroll this element into view, centered vertically + horizontally
    /// in its scroll container, with no animation. The synchronous
    /// (`behavior: 'instant'`) variant keeps the post-scroll bbox final by
    /// the time the next CDP call (e.g. `bounding_box`) runs — important
    /// because subsequent action steps assume the layout is settled.
    ///
    /// No actionability gate: this method IS the visibility prerequisite
    /// for the other actions; gating it on visibility would deadlock.
    pub async fn scroll_into_view(&self) -> Result<()> {
        self.with_refresh(|| async move {
            let _ = self
                .call_on_main(
                    "function(){ this.scrollIntoView({block:'center',behavior:'instant'}); }",
                    json!([]),
                )
                .await?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::InputController;
    use crate::tab::Tab;
    use zendriver_stealth::InputProfile;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn hover_dispatches_input_dispatchmouseevent_with_type_mousemoved() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        // `native` profile: fast (10 px/ms ⇒ 0.5 ms segment delay) and
        // zero jitter ⇒ stable Bezier output. Deterministic seed pins the
        // RNG path in case a future profile tweak adds entropy.
        let input = InputController::new_with_seed(InputProfile::native(), 0xC0FFEE);
        let tab = Tab::new_with_input(sess, input);
        let el = Element::from_jsret(tab.clone(), 99, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.hover().await }
        });

        // Step 1: scroll_into_view → Runtime.callFunctionOn.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert!(sent["params"]["functionDeclaration"]
            .as_str()
            .unwrap()
            .contains("scrollIntoView"));
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        // Step 2: actionability gate runs check_visible first.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(
            id,
            json!({ "result": { "value": true, "type": "boolean" } }),
        )
        .await;
        // check_stable (gate order: visible → enabled → stable → receives_pointer;
        // enabled is disabled for hover, so stable is next).
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(
            id,
            json!({ "result": { "value": true, "type": "boolean" } }),
        )
        .await;
        // check_receives_pointer.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(
            id,
            json!({ "result": { "value": true, "type": "boolean" } }),
        )
        .await;

        // Step 3: bounding_box → DOM.getBoxModel.
        let id = mock.expect_cmd("DOM.getBoxModel").await;
        mock.reply(
            id,
            json!({
                "model": {
                    "content": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "padding": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "border":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "margin":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "width":  100,
                    "height": 50
                }
            }),
        )
        .await;

        // Step 4: mouse move — Bezier path emits N=9..=61
        // `Input.dispatchMouseEvent { type: mouseMoved }` calls. Drain
        // each one, asserting type=mouseMoved along the way; stop once
        // the future completes (no more dispatches arrive within the
        // window).
        let mut saw_mouse_moved = false;
        loop {
            let next = tokio::time::timeout(
                Duration::from_millis(500),
                mock.expect_cmd("Input.dispatchMouseEvent"),
            )
            .await;
            match next {
                Ok(id) => {
                    let sent = mock.last_sent();
                    let kind = sent["params"]["type"].as_str().unwrap_or("");
                    assert_eq!(
                        kind, "mouseMoved",
                        "hover should only emit mouseMoved events"
                    );
                    saw_mouse_moved = true;
                    mock.reply(id, json!({})).await;
                }
                Err(_) => break,
            }
        }

        let res = fut.await.unwrap();
        res.unwrap();
        assert!(
            saw_mouse_moved,
            "expected at least one Input.dispatchMouseEvent with type=mouseMoved"
        );
        conn.shutdown();
    }

    #[tokio::test]
    async fn click_dispatches_mousemoved_then_mousepressed_then_mousereleased() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        // `native` profile: fast (zero realism overhead) — Bezier path still
        // emits, but with zero jitter + 10 px/ms each segment dispatches at
        // 0.5 ms intervals. Deterministic seed pins the path in case future
        // profile tweaks change entropy expectations.
        let input = InputController::new_with_seed(InputProfile::native(), 0xC0FFEE);
        let tab = Tab::new_with_input(sess, input);
        let el = Element::from_jsret(tab.clone(), 99, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.click().await }
        });

        // Step 1: scroll_into_view → Runtime.callFunctionOn.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        // Step 2: actionability gate (FULL = visible → enabled → stable →
        // receives_pointer); reply true to each.
        for _ in 0..4 {
            let id = mock.expect_cmd("Runtime.callFunctionOn").await;
            mock.reply(
                id,
                json!({ "result": { "value": true, "type": "boolean" } }),
            )
            .await;
        }

        // Step 3: bounding_box → DOM.getBoxModel.
        let id = mock.expect_cmd("DOM.getBoxModel").await;
        mock.reply(
            id,
            json!({
                "model": {
                    "content": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "padding": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "border":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "margin":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "width":  100,
                    "height": 50
                }
            }),
        )
        .await;

        // Step 4: dispatch sequence. Bezier emits N mouseMoved frames, then
        // exactly one mousePressed + one mouseReleased. Walk all
        // `Input.dispatchMouseEvent` calls and assert ordering: every
        // mouseMoved precedes mousePressed, which precedes mouseReleased.
        let mut saw_pressed = false;
        let mut saw_released = false;
        let mut last_kind = String::new();
        loop {
            let next = tokio::time::timeout(
                Duration::from_millis(500),
                mock.expect_cmd("Input.dispatchMouseEvent"),
            )
            .await;
            match next {
                Ok(id) => {
                    let sent = mock.last_sent();
                    let kind = sent["params"]["type"].as_str().unwrap_or("").to_string();
                    match kind.as_str() {
                        "mouseMoved" => {
                            assert!(
                                !saw_pressed && !saw_released,
                                "mouseMoved arrived after mousePressed/Released"
                            );
                        }
                        "mousePressed" => {
                            assert!(!saw_pressed, "duplicate mousePressed");
                            assert!(!saw_released, "mousePressed after mouseReleased");
                            saw_pressed = true;
                        }
                        "mouseReleased" => {
                            assert!(saw_pressed, "mouseReleased before mousePressed");
                            assert!(!saw_released, "duplicate mouseReleased");
                            saw_released = true;
                        }
                        other => panic!("unexpected dispatch type: {other}"),
                    }
                    last_kind = kind;
                    mock.reply(id, json!({})).await;
                }
                Err(_) => break,
            }
        }

        let res = fut.await.unwrap();
        res.unwrap();
        assert!(saw_pressed, "expected a mousePressed dispatch");
        assert!(saw_released, "expected a mouseReleased dispatch");
        assert_eq!(
            last_kind, "mouseReleased",
            "final dispatch should be mouseReleased"
        );
        conn.shutdown();
    }
}
