//! Realistic + raw mouse dispatch.

use std::time::Duration;

use serde_json::json;

use crate::error::Result;
use crate::input::bezier::BezierPath;
use crate::input::InputController;
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

/// Move the cursor from its current position to `(target_x, target_y)` along
/// a Bezier path with realistic per-segment delay. Updates InputController
/// state to the target position on success.
#[allow(dead_code)]
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
            start,
            (target_x, target_y),
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
        tab.session()
            .call(
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseMoved", "x": x, "y": y,
                    "modifiers": modifier_bits,
                }),
            )
            .await?;
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
#[allow(dead_code)]
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
    tab.session()
        .call(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseMoved", "x": target_x, "y": target_y,
                "modifiers": modifier_bits,
            }),
        )
        .await?;
    let mut state = input.state.lock().await;
    state.pointer_x = target_x;
    state.pointer_y = target_y;
    Ok(())
}

/// Dispatch a click at `(target_x, target_y)` with `button` and `click_count`.
/// If `realistic`, prefixes with Bezier move; otherwise direct teleport.
#[allow(dead_code)]
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
    tab.session()
        .call(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mousePressed",
                "x": target_x, "y": target_y,
                "button": button.cdp_str(),
                "clickCount": click_count,
                "modifiers": modifier_bits,
            }),
        )
        .await?;
    tab.session()
        .call(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseReleased",
                "x": target_x, "y": target_y,
                "button": button.cdp_str(),
                "clickCount": click_count,
                "modifiers": modifier_bits,
            }),
        )
        .await?;
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
