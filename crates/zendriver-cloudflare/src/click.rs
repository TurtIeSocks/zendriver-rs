//! Raw mouse-click dispatch for the Turnstile checkbox.
//!
//! Cloudflare's bot detection wants a quick, observable click on the visible
//! checkbox — not a realistic Bezier path. The flow is a three-step CDP
//! sequence: `mouseMoved` to position the pointer, then a `mousePressed` /
//! `mouseReleased` pair with `button: "left"` and `clickCount: 1`. Anything
//! more elaborate (multi-segment moves, jitter) is overkill here and risks
//! introducing latency the challenge container can react to before the click
//! lands.

use serde_json::json;
use zendriver_transport::SessionHandle;

use crate::error::CloudflareError;

/// Dispatch a single left-click at viewport coordinates `(x, y)`.
///
/// Three sequential `Input.dispatchMouseEvent` calls:
/// 1. `mouseMoved { x, y }` — positions the synthetic cursor.
/// 2. `mousePressed { x, y, button: "left", clickCount: 1 }` — press down.
/// 3. `mouseReleased { x, y, button: "left", clickCount: 1 }` — release.
///
/// No modifiers, no realistic Bezier path — see module docs for rationale.
pub(crate) async fn click_at(
    session: &SessionHandle,
    x: f64,
    y: f64,
) -> Result<(), CloudflareError> {
    session
        .call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseMoved", "x": x, "y": y }),
        )
        .await?;
    session
        .call(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mousePressed",
                "x": x,
                "y": y,
                "button": "left",
                "clickCount": 1,
            }),
        )
        .await?;
    session
        .call(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseReleased",
                "x": x,
                "y": y,
                "button": "left",
                "clickCount": 1,
            }),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn click_at_emits_moved_pressed_released_with_left_button() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { click_at(&s, 12.5, 34.0).await }
        });

        let id_move = mock.expect_cmd("Input.dispatchMouseEvent").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["type"], "mouseMoved");
        assert_eq!(sent["params"]["x"], 12.5);
        assert_eq!(sent["params"]["y"], 34.0);
        mock.reply(id_move, serde_json::json!({})).await;

        let id_press = mock.expect_cmd("Input.dispatchMouseEvent").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["type"], "mousePressed");
        assert_eq!(sent["params"]["button"], "left");
        assert_eq!(sent["params"]["clickCount"], 1);
        assert_eq!(sent["params"]["x"], 12.5);
        assert_eq!(sent["params"]["y"], 34.0);
        mock.reply(id_press, serde_json::json!({})).await;

        let id_rel = mock.expect_cmd("Input.dispatchMouseEvent").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["type"], "mouseReleased");
        assert_eq!(sent["params"]["button"], "left");
        assert_eq!(sent["params"]["clickCount"], 1);
        mock.reply(id_rel, serde_json::json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }
}
