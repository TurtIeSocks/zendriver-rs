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
    for n in 1..=click_count.max(1) {
        // `buttons` must reflect the set held *after* the press and *after* the
        // release, so take it from the same locked section that mutates it.
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

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn mouse_button_cdp_strings_match_chrome() {
        assert_eq!(MouseButton::Left.cdp_str(), "left");
        assert_eq!(MouseButton::Right.cdp_str(), "right");
        assert_eq!(MouseButton::Middle.cdp_str(), "middle");
        assert_eq!(MouseButton::Back.cdp_str(), "back");
        assert_eq!(MouseButton::Forward.cdp_str(), "forward");
    }
    // Note: dispatch fns are async + need a Tab + MockConnection — exercised in T20 click tests.
}
