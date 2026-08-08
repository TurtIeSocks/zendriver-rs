//! Raw touch dispatch — backs [`crate::Tab::tap`] / [`crate::Element::tap`].
//!
//! A tap is a bare `Input.dispatchTouchEvent` `touchStart` → `touchEnd`
//! pair, with no `Emulation.setTouchEmulationEnabled` preamble. That CDP
//! call flips touch-*capability* signals (`ontouchstart` in `window`,
//! `navigator.maxTouchPoints`, `matchMedia('(pointer: coarse)')`) — a
//! separate mobile-emulation concern (Phase 7), not something a tap needs.
//! The bare dispatch already fires the page's `touchstart` / `touchend`
//! handlers (and, on a clickable element, the browser's own synthesized
//! `click`), which is the whole point of a tap.

use serde_json::json;

use crate::error::Result;
use crate::tab::Tab;

/// Dispatch a tap at `(x, y)` in viewport coordinates: `touchStart` with a
/// single touch point, then `touchEnd` with an empty `touchPoints` array —
/// the CDP contract for a lifted finger.
pub(crate) async fn tap_at(tab: &Tab, x: f64, y: f64) -> Result<()> {
    tab.session()
        .call(
            "Input.dispatchTouchEvent",
            json!({
                "type": "touchStart",
                "touchPoints": [{ "x": x, "y": y }],
            }),
        )
        .await?;
    tab.session()
        .call(
            "Input.dispatchTouchEvent",
            json!({
                "type": "touchEnd",
                "touchPoints": [],
            }),
        )
        .await?;
    // A tap moves the pointer as far as the page is concerned, so keep the
    // controller's cached cursor in step. Without this the next realistic mouse
    // move builds its Bezier from a stale origin and opens by teleporting back
    // to wherever the last mouse action landed.
    {
        let mut s = tab.input().state.lock().await;
        s.pointer_x = x;
        s.pointer_y = y;
    }
    Ok(())
}

// `tap_at` is exercised via its callers' MockConnection tests — see
// `Tab::tap`'s `tap_dispatches_touchstart_with_point_then_touchend_empty`
// (`crate::tab::tests`) and `Element::tap`'s equivalent in
// `crate::element::actions::tests` — same pattern `mouse.rs` uses for
// `click_at`/`move_realistic` (dispatch fns need a live `Tab` +
// `MockConnection`, so the assertion lives at the call site).
