//! Cloudflare Turnstile bypass driver.
//!
//! Public entry is [`CloudflareBypass`] — constructed via
//! `Tab::cloudflare()` (zendriver crate, feature-gated). The driver runs a
//! single CDP poll loop that, per tick:
//!
//! 1. Re-evaluates a unified shadow-DOM walker in the page's main world
//!    (private [`POLL_JS`]) returning, in one round-trip:
//!    - the `cf-turnstile-response` (or legacy `cf_challenge_response`) token
//!      if present,
//!    - the Turnstile challenge iframe's bounding box if mounted,
//!    - whether *any* challenge marker exists on the page
//!      (container, hidden input, or live iframe).
//! 2. Resolves to [`ClearanceOutcome::TokenAcquired`] the first tick a
//!    non-empty token is observed — including the **invisible Turnstile**
//!    path where the iframe never mounts and the token is populated
//!    directly.
//! 3. If the interactive checkbox iframe is mounted *and we have not yet
//!    clicked*, dispatches a single raw left-click at the canonical
//!    `bbox.x + bbox.width * 0.15, bbox.y + bbox.height * 0.50` offset
//!    (15% from left, 50% from top — Python's `cloudflare.py` convention).
//! 4. If we previously observed an iframe + clicked, and the iframe is now
//!    gone without a token, resolves to [`ClearanceOutcome::ChallengeGone`]
//!    (clearance-cookie shortcut).
//! 5. Resolves to [`ClearanceOutcome::TimedOut`] on deadline, carrying
//!    `saw_challenge` — `true` when challenge markers were seen but never
//!    resolved, `false` when the entire timeout window elapsed without
//!    observing any challenge markers (the caller likely invoked the bypass
//!    on a page that has no Cloudflare gate at all).

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Instant;
use zendriver_transport::SessionHandle;

use crate::click::click_at;
use crate::detection::BoundingBox;
use crate::error::CloudflareError;

/// Result of a clearance attempt.
#[derive(Debug, Clone)]
pub enum ClearanceOutcome {
    /// Turnstile produced a token (value of `cf-turnstile-response`).
    TokenAcquired(String),
    /// The challenge container disappeared without yielding a token.
    ChallengeGone,
    /// Deadline elapsed without a terminal clearance state. `saw_challenge`
    /// is `true` if any challenge marker (container, hidden input, or live
    /// iframe) was ever observed; `false` if none ever appeared — the caller
    /// likely invoked the bypass on a page that has no Cloudflare gate at all.
    /// Not a fault: a deadline in a bot-management flow is a normal "didn't
    /// finish, retry or give up" terminal.
    TimedOut { saw_challenge: bool },
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
    /// Create a new bypass driver bound to `session` with a default 500ms
    /// poll interval.
    pub fn new(session: &'a SessionHandle) -> Self {
        Self {
            session,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    /// Override the default 500ms poll interval used by
    /// [`wait_for_clearance`](Self::wait_for_clearance).
    #[must_use]
    pub fn poll_interval(mut self, dur: Duration) -> Self {
        self.poll_interval = dur;
        self
    }

    /// Poll the page until a Turnstile clearance terminal state is reached or
    /// `timeout` elapses.
    ///
    /// Handles both the **interactive** flow (challenge iframe mounts → we
    /// click it → token appears) and the **invisible** flow (no iframe;
    /// token is populated directly by Cloudflare's loader script). Resolution
    /// rules per tick are documented at module level.
    ///
    /// # Returns
    /// - `Ok(ClearanceOutcome::TokenAcquired(token))` — the
    ///   `cf-turnstile-response` input picked up a non-empty value, either
    ///   after we clicked the interactive iframe or because the page uses
    ///   invisible Turnstile.
    /// - `Ok(ClearanceOutcome::ChallengeGone)` — the interactive iframe was
    ///   present, we clicked it, and the challenge container then
    ///   disappeared without yielding a token (e.g. a clearance cookie
    ///   shortcut).
    /// - `Ok(ClearanceOutcome::TimedOut { saw_challenge })` — `timeout`
    ///   elapsed without a terminal clearance state. `saw_challenge` is
    ///   `true` if challenge markers were observed (markers present but
    ///   never resolved); `false` if none ever appeared (the caller likely
    ///   invoked the bypass on a page that has no Cloudflare gate). Not a
    ///   fault — a deadline here is a normal "retry or give up" signal.
    ///
    /// # Errors
    /// - [`CloudflareError::Call`] / [`CloudflareError::JsError`] — CDP or
    ///   in-page evaluator failure.
    ///
    /// ```no_run
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_cloudflare::CloudflareError> {
    /// use std::time::Duration;
    /// use zendriver_cloudflare::CloudflareBypass;
    ///
    /// let bypass = CloudflareBypass::new(tab)
    ///     .poll_interval(Duration::from_millis(250));
    /// let outcome = bypass.wait_for_clearance(Duration::from_secs(15)).await?;
    /// println!("{outcome:?}");
    /// # Ok(()) }
    /// ```
    pub async fn wait_for_clearance(
        self,
        timeout: Duration,
    ) -> Result<ClearanceOutcome, CloudflareError> {
        let deadline = Instant::now() + timeout;
        let mut clicked = false;
        let mut ever_seen_markers = false;

        let mut stall_ticks: u32 = 0;
        let mut warned_stall = false;

        loop {
            let state = poll_state(self.session).await?;
            if state.has_markers {
                ever_seen_markers = true;
            }

            if let Some(token) = state.token {
                return Ok(ClearanceOutcome::TokenAcquired(token));
            }

            match state.bbox {
                Some(bbox) if !clicked => {
                    let click_x = bbox.x + bbox.width * 0.15;
                    let click_y = bbox.y + bbox.height * 0.50;
                    click_at(self.session, click_x, click_y).await?;
                    clicked = true;
                }
                None if clicked => {
                    // Iframe was present, we clicked, and now the challenge
                    // container has been torn down without a token — the
                    // clearance-cookie shortcut.
                    return Ok(ClearanceOutcome::ChallengeGone);
                }
                _ => {
                    stall_ticks += 1;
                    if stall_ticks == 10 && !warned_stall {
                        tracing::warn!(
                            poll_interval_ms = self.poll_interval.as_millis() as u64,
                            "cloudflare clearance stalled — is BrowserBuilder::stealth enabled?"
                        );
                        warned_stall = true;
                    }
                }
            }

            if Instant::now() >= deadline {
                return Ok(ClearanceOutcome::TimedOut {
                    saw_challenge: ever_seen_markers,
                });
            }

            tokio::select! {
                () = tokio::time::sleep(self.poll_interval) => {}
                () = tokio::time::sleep_until(deadline) => {}
            }
        }
    }
}

/// Decoded payload from the unified poll evaluator. Combines token check,
/// iframe bbox, and a coarse "any challenge marker present" flag so each
/// tick is a single CDP round-trip.
#[derive(Debug, Deserialize)]
struct PollState {
    /// Non-empty `cf-turnstile-response` / `cf_challenge_response` value, if
    /// any.
    #[serde(default)]
    token: Option<String>,
    /// Bounding box of the Turnstile challenge iframe, if mounted. Walks
    /// shadow roots so closed shadow trees still surface the iframe.
    #[serde(default)]
    bbox: Option<BoundingBox>,
    /// `true` when the page has any of: a `.cf-turnstile` / `.turnstile`
    /// container, a `cf-turnstile-response` hidden input, or a live
    /// challenge iframe. Used to distinguish "wrong page" (no markers ever
    /// seen) from "real challenge that timed out".
    #[serde(default, rename = "hasMarkers")]
    has_markers: bool,
}

/// Unified in-page evaluator. Returns:
/// - `token` — non-empty `cf-turnstile-response` (or legacy
///   `cf_challenge_response`) input value, else null.
/// - `bbox` — the Turnstile challenge iframe's bounding box (shadow-DOM
///   aware walk), else null.
/// - `hasMarkers` — true when *any* Cloudflare challenge marker is present
///   (container, hidden input, or live iframe).
///
/// The shadow-DOM walk is necessary because Cloudflare sometimes hosts the
/// challenge iframe inside an open shadow root attached to a host element on
/// the page. The token input itself is in light DOM by design (page JS reads
/// it to submit forms), so `document.querySelector` is sufficient there.
const POLL_JS: &str = r#"
(function () {
    function findBbox(root) {
        var iframes = root.querySelectorAll ? root.querySelectorAll("iframe") : [];
        for (var i = 0; i < iframes.length; i++) {
            var f = iframes[i];
            if (f.src && f.src.includes("challenges.cloudflare.com")) {
                var r = f.getBoundingClientRect();
                return { x: r.left, y: r.top, width: r.width, height: r.height };
            }
        }
        var all = root.querySelectorAll ? root.querySelectorAll("*") : [];
        for (var j = 0; j < all.length; j++) {
            if (all[j].shadowRoot) {
                var sub = findBbox(all[j].shadowRoot);
                if (sub) return sub;
            }
        }
        return null;
    }
    var bbox = findBbox(document);
    var input =
        document.querySelector('[name="cf-turnstile-response"]') ||
        document.querySelector('[name="cf_challenge_response"]');
    var token = (input && input.value) ? input.value : null;
    var hasContainer = !!document.querySelector('.cf-turnstile, .turnstile, [data-sitekey]');
    var hasMarkers = hasContainer || !!input || !!bbox;
    return { token: token, bbox: bbox, hasMarkers: hasMarkers };
})()
"#;

/// Run [`POLL_JS`] against `session`'s main world and decode the result.
async fn poll_state(session: &SessionHandle) -> Result<PollState, CloudflareError> {
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

    /// Interactive happy path: poll #1 yields a bbox → click_at fires the
    /// three mouse events at (15% × width, 50% × height) inside the bbox →
    /// poll #2 observes the token → TokenAcquired.
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

        // Poll #1: unified evaluator returns bbox, no token, markers present.
        let id_poll1 = mock.expect_cmd("Runtime.evaluate").await;
        assert!(
            mock.last_sent()["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("challenges.cloudflare.com"),
            "poll eval should walk for challenges.cloudflare.com iframe"
        );
        assert!(
            mock.last_sent()["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("cf-turnstile-response"),
            "poll eval should look at the cf-turnstile-response input"
        );
        mock.reply(
            id_poll1,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "token": null,
                        "bbox": { "x": BBOX_X, "y": BBOX_Y, "width": BBOX_W, "height": BBOX_H },
                        "hasMarkers": true,
                    }
                }
            }),
        )
        .await;

        // click_at → mouseMoved → mousePressed → mouseReleased.
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

        // Poll #2: token appears → TokenAcquired terminates the loop.
        let id_poll2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_poll2,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "token": "TOKEN_XYZ",
                        "bbox": null,
                        "hasMarkers": true,
                    }
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

    /// Invisible-Turnstile path: no iframe ever mounts, the token is
    /// populated directly on the first poll → TokenAcquired without any
    /// click.
    #[tokio::test]
    async fn wait_for_clearance_returns_token_without_click_for_invisible_turnstile() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_secs(5)).await
            }
        });

        let id_poll = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_poll,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "token": "INVISIBLE_TOKEN",
                        "bbox": null,
                        "hasMarkers": true,
                    }
                }
            }),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        match outcome {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, "INVISIBLE_TOKEN"),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        // No Input.dispatchMouseEvent should have been queued — the invisible
        // path never clicks. The mock connection asserts via its own drop
        // semantics that no pending sends remain.
        conn.shutdown();
    }

    /// ChallengeGone path: poll #1 yields a bbox → click → poll #2 reports
    /// no bbox and no token → ChallengeGone.
    #[tokio::test]
    async fn wait_for_clearance_returns_challenge_gone_when_iframe_disappears_after_click() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_secs(5)).await
            }
        });

        // Poll #1: bbox present.
        let id_poll1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_poll1,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "token": null,
                        "bbox": { "x": 10.0, "y": 20.0, "width": 40.0, "height": 40.0 },
                        "hasMarkers": true,
                    }
                }
            }),
        )
        .await;

        // click sequence.
        for _ in 0..3 {
            let id = mock.expect_cmd("Input.dispatchMouseEvent").await;
            mock.reply(id, json!({})).await;
        }

        // Poll #2: iframe gone, no token.
        let id_poll2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_poll2,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "token": null,
                        "bbox": null,
                        "hasMarkers": true,
                    }
                }
            }),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(matches!(outcome, ClearanceOutcome::ChallengeGone));
        conn.shutdown();
    }
}
