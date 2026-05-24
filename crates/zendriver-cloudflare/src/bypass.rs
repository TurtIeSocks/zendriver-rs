//! Cloudflare Turnstile bypass driver.
//!
//! Public entry is [`CloudflareBypass`] — constructed via
//! `Tab::cloudflare()` (zendriver crate, feature-gated). The driver:
//!
//! 1. Locates the Turnstile iframe via [`crate::detection::detect_challenge`]
//!    (a shadow-DOM-aware walk of the page main world).
//! 2. Computes a click point at `bbox.x + bbox.width * 0.15,
//!    bbox.y + bbox.height * 0.50` — the canonical 15%-from-left,
//!    50%-from-top offset Python's `cloudflare.py` uses to land on the
//!    visible Turnstile checkbox inside the iframe.
//! 3. Dispatches a raw left-click via [`crate::click::click_at`] (no Bezier;
//!    Cloudflare wants a real click on a real checkbox).
//! 4. Polls every `poll_interval` (default 500ms) for either the
//!    `cf-turnstile-response` input gaining a non-empty value
//!    ([`ClearanceOutcome::TokenAcquired`]) OR the challenge container
//!    disappearing ([`ClearanceOutcome::ChallengeGone`]).
//! 5. Errors with [`CloudflareError::ClearanceTimeout`] on deadline, or
//!    [`CloudflareError::NoChallenge`] if step 1 finds nothing to clear.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::{Instant, Interval, MissedTickBehavior};
use zendriver_transport::SessionHandle;

use crate::click::click_at;
use crate::detection::detect_challenge;
use crate::error::CloudflareError;

/// Result of a clearance attempt.
#[derive(Debug, Clone)]
pub enum ClearanceOutcome {
    /// Turnstile produced a token (value of `cf-turnstile-response`).
    TokenAcquired(String),
    /// The challenge container disappeared without yielding a token.
    ChallengeGone,
}

/// Default poll interval for `wait_for_clearance`.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Drives a Cloudflare Turnstile clearance flow against a single tab's session.
///
/// Constructed via `Tab::cloudflare()`.
#[derive(Debug)]
pub struct CloudflareBypass<'a> {
    pub(crate) session: &'a SessionHandle,
    pub(crate) poll_interval: Duration,
}

impl<'a> CloudflareBypass<'a> {
    /// Create a new bypass driver bound to `session`.
    pub fn new(session: &'a SessionHandle) -> Self {
        Self {
            session,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    /// Override the default 500ms poll interval used by `wait_for_clearance`.
    #[must_use]
    pub fn poll_interval(mut self, dur: Duration) -> Self {
        self.poll_interval = dur;
        self
    }

    /// Detect a live Turnstile challenge, click its checkbox, then poll
    /// until the challenge yields a token, disappears, or `timeout` elapses.
    ///
    /// Returns:
    /// - `Ok(ClearanceOutcome::TokenAcquired(token))` — the
    ///   `cf-turnstile-response` input picked up a non-empty value.
    /// - `Ok(ClearanceOutcome::ChallengeGone)` — the challenge container
    ///   was removed without a token (e.g. a clearance cookie shortcut).
    /// - `Err(CloudflareError::NoChallenge)` — no Turnstile iframe was
    ///   mounted at the moment of the initial detect; nothing to bypass.
    /// - `Err(CloudflareError::ClearanceTimeout)` — `timeout` elapsed
    ///   before either success state was observed.
    /// - `Err(CloudflareError::Call | JsError)` — propagated from CDP /
    ///   the in-page evaluator.
    pub async fn wait_for_clearance(
        self,
        timeout: Duration,
    ) -> Result<ClearanceOutcome, CloudflareError> {
        // 1. Locate the iframe; bail if there's nothing to do.
        let bbox = detect_challenge(self.session)
            .await?
            .ok_or(CloudflareError::NoChallenge)?;

        // 2. Canonical 15% from left, 50% from top — lands on the Turnstile
        //    checkbox inside the iframe.
        let click_x = bbox.x + bbox.width * 0.15;
        let click_y = bbox.y + bbox.height * 0.50;

        // 3. Single raw left-click — see click.rs module docs.
        click_at(self.session, click_x, click_y).await?;

        // 4. Poll loop. `Instant::now() + timeout` is the hard deadline; the
        //    interval ticks on a steady cadence so a slow poll doesn't cause
        //    drift past the deadline.
        let deadline = Instant::now() + timeout;
        let mut ticker: Interval = tokio::time::interval(self.poll_interval);
        // First `tick().await` resolves immediately — `MissedTickBehavior::Skip`
        // prevents catch-up bursts if a poll runs long.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            // Bail before tick so a zero-timeout call never blocks.
            if Instant::now() >= deadline {
                return Err(CloudflareError::ClearanceTimeout);
            }
            tokio::select! {
                _ = ticker.tick() => {}
                () = tokio::time::sleep_until(deadline) => {
                    return Err(CloudflareError::ClearanceTimeout);
                }
            }

            match poll_once(self.session).await? {
                PollResult {
                    done: true,
                    token: Some(t),
                } => {
                    return Ok(ClearanceOutcome::TokenAcquired(t));
                }
                PollResult {
                    done: true,
                    token: None,
                } => {
                    return Ok(ClearanceOutcome::ChallengeGone);
                }
                _ => continue,
            }
        }
    }
}

/// Decoded payload from the poll evaluator. `done == true` means the
/// challenge has reached a terminal state; `token` is `Some` if the
/// `cf-turnstile-response` input carried a non-empty value at observe time.
#[derive(Debug, Deserialize)]
struct PollResult {
    done: bool,
    #[serde(default)]
    token: Option<String>,
}

/// In-page evaluator. Returns:
/// - `{ done: true, token: "<non-empty>" }` once Turnstile populates the
///   hidden `cf-turnstile-response` input.
/// - `{ done: true, token: null }` if the challenge iframe is no longer
///   mounted (cleared without a token — e.g. clearance cookie path).
/// - `{ done: false, token: null }` otherwise (poll again).
///
/// The walker mirrors `detect.js` — recursively descend the document and
/// every shadow root looking for `challenges.cloudflare.com` iframes.
const POLL_JS: &str = r#"
(function () {
    var input = document.querySelector('[name="cf-turnstile-response"]');
    if (input && input.value) {
        return { done: true, token: input.value };
    }
    function findIframe(root) {
        var iframes = root.querySelectorAll ? root.querySelectorAll("iframe") : [];
        for (var i = 0; i < iframes.length; i++) {
            if (iframes[i].src && iframes[i].src.includes("challenges.cloudflare.com")) {
                return true;
            }
        }
        var all = root.querySelectorAll ? root.querySelectorAll("*") : [];
        for (var j = 0; j < all.length; j++) {
            if (all[j].shadowRoot && findIframe(all[j].shadowRoot)) {
                return true;
            }
        }
        return false;
    }
    if (!findIframe(document)) {
        return { done: true, token: null };
    }
    return { done: false, token: null };
})()
"#;

/// Run [`POLL_JS`] against `session`'s main world and decode the result.
async fn poll_once(session: &SessionHandle) -> Result<PollResult, CloudflareError> {
    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": POLL_JS,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await?;

    if let Some(details) = res.get("exceptionDetails") {
        let msg = details
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("unknown")
            .to_string();
        return Err(CloudflareError::JsError(msg));
    }

    let value = res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null);

    serde_json::from_value(value)
        .map_err(|e| CloudflareError::JsError(format!("invalid poll payload: {e}")))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    /// End-to-end happy path: detect_challenge yields a bbox → click_at fires
    /// the three mouse events at (15% × width, 50% × height) inside the bbox
    /// → first poll observes the token → TokenAcquired.
    #[tokio::test]
    async fn wait_for_clearance_clicks_at_bbox_offset_then_returns_token() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        // bbox at (100, 200) with 60×40 → click at
        //   x = 100 + 60 * 0.15 = 109
        //   y = 200 + 40 * 0.50 = 220
        const BBOX_X: f64 = 100.0;
        const BBOX_Y: f64 = 200.0;
        const BBOX_W: f64 = 60.0;
        const BBOX_H: f64 = 40.0;
        const EXPECTED_CLICK_X: f64 = BBOX_X + BBOX_W * 0.15;
        const EXPECTED_CLICK_Y: f64 = BBOX_Y + BBOX_H * 0.50;

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_secs(5)).await
            }
        });

        // 1. detect_challenge → bbox.
        let id_detect = mock.expect_cmd("Runtime.evaluate").await;
        assert!(
            mock.last_sent()["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("challenges.cloudflare.com"),
            "first eval should be the detect.js shadow-DOM walker"
        );
        mock.reply(
            id_detect,
            json!({
                "result": {
                    "type": "object",
                    "value": { "x": BBOX_X, "y": BBOX_Y, "width": BBOX_W, "height": BBOX_H }
                }
            }),
        )
        .await;

        // 2. click_at → mouseMoved → mousePressed → mouseReleased.
        let id_move = mock.expect_cmd("Input.dispatchMouseEvent").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["type"], "mouseMoved");
        assert_eq!(sent["params"]["x"], EXPECTED_CLICK_X);
        assert_eq!(sent["params"]["y"], EXPECTED_CLICK_Y);
        mock.reply(id_move, json!({})).await;

        let id_press = mock.expect_cmd("Input.dispatchMouseEvent").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["type"], "mousePressed");
        assert_eq!(sent["params"]["button"], "left");
        assert_eq!(sent["params"]["clickCount"], 1);
        assert_eq!(sent["params"]["x"], EXPECTED_CLICK_X);
        assert_eq!(sent["params"]["y"], EXPECTED_CLICK_Y);
        mock.reply(id_press, json!({})).await;

        let id_rel = mock.expect_cmd("Input.dispatchMouseEvent").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["type"], "mouseReleased");
        assert_eq!(sent["params"]["button"], "left");
        assert_eq!(sent["params"]["clickCount"], 1);
        mock.reply(id_rel, json!({})).await;

        // 3. Poll eval → returns done + token immediately.
        let id_poll = mock.expect_cmd("Runtime.evaluate").await;
        assert!(
            mock.last_sent()["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("cf-turnstile-response"),
            "poll eval should look at the cf-turnstile-response input"
        );
        mock.reply(
            id_poll,
            json!({
                "result": {
                    "type": "object",
                    "value": { "done": true, "token": "TOKEN_XYZ" }
                }
            }),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        match outcome {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, "TOKEN_XYZ"),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        conn.shutdown();
    }
}
