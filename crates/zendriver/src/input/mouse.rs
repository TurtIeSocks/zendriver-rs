//! Realistic + raw mouse dispatch.

use std::time::Duration;

use serde_json::json;

use crate::error::Result;
use crate::input::InputController;
use crate::input::bezier::BezierPath;
use crate::input::keyboard::KeyModifiers;
use crate::input::pointer_state::MouseButtonSet;
use crate::tab::Tab;

/// Mouse buttons for click dispatch.
///
/// Mirrors the CDP `MouseEvent.button` enum.
///
/// # Examples
///
/// ```
/// use zendriver::MouseButton;
/// assert_eq!(MouseButton::Left.cdp_str(), "left");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// Primary button (left for right-handed users).
    Left,
    /// Middle button (scroll wheel click).
    Middle,
    /// Secondary button (right for right-handed users).
    Right,
    /// "Back" thumb button.
    Back,
    /// "Forward" thumb button.
    Forward,
}

impl MouseButton {
    /// CDP wire string for this button.
    ///
    /// # Examples
    ///
    /// ```
    /// use zendriver::MouseButton;
    /// assert_eq!(MouseButton::Right.cdp_str(), "right");
    /// ```
    #[must_use]
    pub fn cdp_str(self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Middle => "middle",
            MouseButton::Right => "right",
            MouseButton::Back => "back",
            MouseButton::Forward => "forward",
        }
    }
}

/// The [`MouseButtonSet`] bit for a single button.
///
/// `MouseButtonSet`'s bit values are chosen to match CDP's `buttons` bitmask
/// (1 left, 2 right, 4 middle, 8 back, 16 forward), so `MouseButtonSet::bits`
/// is the wire value directly.
const fn button_bit(button: MouseButton) -> MouseButtonSet {
    match button {
        MouseButton::Left => MouseButtonSet::LEFT,
        MouseButton::Middle => MouseButtonSet::MIDDLE,
        MouseButton::Right => MouseButtonSet::RIGHT,
        MouseButton::Back => MouseButtonSet::BACK,
        MouseButton::Forward => MouseButtonSet::FORWARD,
    }
}

/// Move the cursor from its current position to `(target_x, target_y)` along
/// a Bezier path with realistic per-segment delay. Updates InputController
/// state to the target position on success.
///
/// Carries `buttons` so a move dispatched between a press and a release reads
/// as a drag. A page implementing drag with `mousemove` + `e.buttons` — the
/// standard modern check — sees nothing at all without it.
pub(crate) async fn move_realistic(
    input: &InputController,
    tab: &Tab,
    target_x: f64,
    target_y: f64,
    extra_modifiers: KeyModifiers,
) -> Result<()> {
    // Hold the lock across the full dispatch + state-update sequence so a
    // concurrent `move_*` from another task can't slip in between our last
    // `mouseMoved` and the `pointer_{x,y}` write and leave the cached
    // cursor position out of sync with the page's actual cursor.
    // InputController is per-Tab so this only serializes input on this Tab,
    // which matches Chrome's per-page input model.
    let mut state = input.state.lock().await;
    let start = (state.pointer_x, state.pointer_y);
    let modifier_bits = (state.modifiers_held | extra_modifiers).cdp_bits();
    let buttons = state.buttons_held.bits();
    let path = BezierPath::build(
        start,
        (target_x, target_y),
        input.profile.jitter_amplitude_px,
        &mut state.rng,
    );
    let speed = input.profile.mouse_speed_px_per_ms;
    // `points[0]` is the start position, so dispatching it emits a `mouseMoved`
    // to where the cursor already is. Skip it, and sleep *before* each step so
    // the delay pays for the movement that follows rather than trailing the
    // last one.
    let mut prev = path.points.first().copied().unwrap_or(start);
    for &(x, y) in path.points.iter().skip(1) {
        if speed > 0.0 {
            // Derive the delay from the segment actually emitted. `BezierPath`
            // clamps its sample count to [8, 60], so segment length scales with
            // distance; assuming a fixed 5px step ran long moves ~3x too fast
            // and short ones ~4x too slow, which made `mouse_speed_px_per_ms`
            // honest only for 40-300px journeys.
            let seg = (x - prev.0).hypot(y - prev.1);
            let delay = Duration::from_micros(((seg / speed) * 1000.0) as u64);
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
        tab.session()
            .call(
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseMoved", "x": x, "y": y,
                    "modifiers": modifier_bits,
                    "buttons": buttons,
                }),
            )
            .await?;
        prev = (x, y);
    }
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
    extra_modifiers: KeyModifiers,
) -> Result<()> {
    // Same per-Tab serialization rationale as `move_realistic`.
    let mut state = input.state.lock().await;
    let modifier_bits = (state.modifiers_held | extra_modifiers).cdp_bits();
    let buttons = state.buttons_held.bits();
    tab.session()
        .call(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseMoved", "x": target_x, "y": target_y,
                "modifiers": modifier_bits,
                "buttons": buttons,
            }),
        )
        .await?;
    state.pointer_x = target_x;
    state.pointer_y = target_y;
    Ok(())
}

/// Dispatch a click at `(target_x, target_y)`.
///
/// Reads `button` / `click_count` / `realistic` / `modifiers` from `opts`;
/// `force` and `position` belong to the element-gating layer and are resolved
/// by the caller before it picks a target coordinate.
///
/// `opts.modifiers` are OR'd with whatever the keyboard currently holds, so a
/// caller-requested Ctrl/Cmd/Shift-click works without the caller having to
/// drive a real key-down first. They ride the approach `mouseMoved` frames too,
/// the way a real browser reports `ctrlKey` on mousemove.
///
/// A `click_count` above 1 emits that many press/release pairs with an
/// increasing `clickCount`, which is what Chrome produces for a real
/// double-click. Collapsing it into one pair carrying `clickCount: 2` is
/// invisible to any page counting `mousedown` events.
pub(crate) async fn click_at(
    input: &InputController,
    tab: &Tab,
    target_x: f64,
    target_y: f64,
    opts: &crate::element::actions::ClickOptions,
) -> Result<()> {
    let (button, click_count, extra_modifiers) = (opts.button, opts.click_count, opts.modifiers);
    if opts.realistic {
        move_realistic(input, tab, target_x, target_y, extra_modifiers).await?;
    } else {
        move_raw(input, tab, target_x, target_y, extra_modifiers).await?;
    }
    let bit = button_bit(button);
    // `buttons_held` lives on the per-Tab InputController, which outlives this
    // call, so a `?` between the press and its matching release would strand
    // `bit` set for the rest of the tab's life: every later `mouseMoved` would
    // then report a button held with no preceding mousedown. That impossible
    // state is worse than the missing-`buttons` bug this replaced — behavioral
    // anti-bot scoring reads exactly this stream.
    //
    // The fix is a single fallible section plus one cleanup below, rather than
    // a `Drop` guard: clearing the bit needs the async `Mutex`, `Drop` cannot
    // await, and a `try_lock` inside `Drop` would silently fail under
    // contention — precisely when another task is mid-dispatch and would go on
    // to read the stale bit. A guard here would be more machinery and a weaker
    // guarantee.
    //
    // Residual case it does not cover: if the caller drops this future
    // mid-gesture (cancellation, an outer `tokio::time::timeout`), the cleanup
    // never runs and the bit stays set. A `Drop` guard would not reliably close
    // that either, for the `try_lock` reason above.
    let sequence: Result<()> = async {
        for n in 1..=click_count.max(1) {
            // `buttons` must reflect the set held *after* the press and *after*
            // the release, so take it from the same locked section that mutates
            // it.
            let (modifier_bits, buttons) = {
                let mut s = input.state.lock().await;
                s.buttons_held.insert(bit);
                (
                    (s.modifiers_held | extra_modifiers).cdp_bits(),
                    s.buttons_held.bits(),
                )
            };
            tab.session()
                .call(
                    "Input.dispatchMouseEvent",
                    json!({
                        "type": "mousePressed",
                        "x": target_x, "y": target_y,
                        "button": button.cdp_str(),
                        "clickCount": n,
                        "modifiers": modifier_bits,
                        "buttons": buttons,
                    }),
                )
                .await?;
            let (modifier_bits, buttons) = {
                let mut s = input.state.lock().await;
                s.buttons_held.remove(bit);
                (
                    (s.modifiers_held | extra_modifiers).cdp_bits(),
                    s.buttons_held.bits(),
                )
            };
            tab.session()
                .call(
                    "Input.dispatchMouseEvent",
                    json!({
                        "type": "mouseReleased",
                        "x": target_x, "y": target_y,
                        "button": button.cdp_str(),
                        "clickCount": n,
                        "modifiers": modifier_bits,
                        "buttons": buttons,
                    }),
                )
                .await?;
        }
        Ok(())
    }
    .await;
    // Single exit for the latched bit. A no-op on the happy path (the last
    // release already cleared it) and on any failure before the first press.
    input.state.lock().await.buttons_held.remove(bit);
    sequence
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::{SessionHandle, testing::MockConnection};

    #[test]
    fn mouse_button_cdp_strings_match_chrome() {
        assert_eq!(MouseButton::Left.cdp_str(), "left");
        assert_eq!(MouseButton::Right.cdp_str(), "right");
        assert_eq!(MouseButton::Middle.cdp_str(), "middle");
        assert_eq!(MouseButton::Back.cdp_str(), "back");
        assert_eq!(MouseButton::Forward.cdp_str(), "forward");
    }
    // Note: dispatch fns are async + need a Tab + MockConnection — exercised in T20 click tests.

    /// Drive one `Input.dispatchMouseEvent` off the mock: assert the frame is
    /// the expected `type`, then hand it to `respond`. `expect_cmd` has no
    /// built-in timeout, so every wait is bounded here.
    async fn next_dispatch(mock: &mut MockConnection, expected_type: &str) -> u64 {
        let id = tokio::time::timeout(
            Duration::from_secs(5),
            mock.expect_cmd("Input.dispatchMouseEvent"),
        )
        .await
        .expect("no Input.dispatchMouseEvent arrived");
        assert_eq!(
            mock.last_sent()["params"]["type"],
            expected_type,
            "unexpected dispatch order: {}",
            mock.last_sent()
        );
        id
    }

    /// A failed `mousePressed` must not strand the held-button bit. The
    /// InputController is per-Tab and long-lived, so a latched bit makes every
    /// subsequent `mouseMoved` on this tab report a button held with no
    /// preceding mousedown — incoherent pointer telemetry, and strictly worse
    /// than omitting the field.
    #[tokio::test]
    async fn click_at_clears_the_held_bit_when_the_press_fails() {
        let (mut mock, conn) = MockConnection::pair();
        let tab = Tab::new_for_test(SessionHandle::new(conn.clone(), "S1"));

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                // `realistic: false` keeps the approach to a single teleport
                // `mouseMoved` — no Bezier path, no per-segment sleeps.
                let opts = crate::element::actions::ClickOptions {
                    realistic: false,
                    ..Default::default()
                };
                click_at(t.input(), &t, 10.0, 20.0, &opts).await
            }
        });

        let mv = next_dispatch(&mut mock, "mouseMoved").await;
        mock.reply(mv, serde_json::json!({})).await;
        let press = next_dispatch(&mut mock, "mousePressed").await;
        mock.reply_err(press, -32000, "dispatch rejected").await;

        assert!(
            fut.await.unwrap().is_err(),
            "a failed mousePressed must propagate"
        );
        assert!(
            tab.input().state.lock().await.buttons_held.is_empty(),
            "click_at leaked a latched button bit after a failed mousePressed"
        );
        conn.shutdown();
    }

    /// `mouse_drag` latches the bit *after* the press lands, so the failure
    /// that leaks it is a dispatch between the press and the release — an
    /// interpolated `mouseMoved` is the realistic one.
    #[tokio::test]
    async fn mouse_drag_clears_the_held_bit_when_a_move_fails() {
        let (mut mock, conn) = MockConnection::pair();
        let tab = Tab::new_for_test(SessionHandle::new(conn.clone(), "S1"));

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.mouse_drag((10.0, 10.0), (200.0, 10.0), 5).await }
        });

        let press = next_dispatch(&mut mock, "mousePressed").await;
        mock.reply(press, serde_json::json!({})).await;
        // The bit is held from here until the release; fail inside that window.
        let mv = next_dispatch(&mut mock, "mouseMoved").await;
        mock.reply_err(mv, -32000, "dispatch rejected").await;

        assert!(
            fut.await.unwrap().is_err(),
            "a failed mouseMoved must propagate"
        );
        assert!(
            tab.input().state.lock().await.buttons_held.is_empty(),
            "mouse_drag leaked a latched button bit after a failed mouseMoved"
        );
        conn.shutdown();
    }
}
