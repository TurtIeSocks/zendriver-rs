//! Shadow-DOM Turnstile challenge detection.
//!
//! Dispatches a `Runtime.evaluate` carrying `js::detect_expr`, which
//! recursively walks the document plus every shadow root looking for an
//! iframe whose `src` contains the caller's
//! [`iframe_src_contains`](TurnstileSelectors::iframe_src_contains) marker.
//! Returns the iframe's bounding box (in viewport coordinates) or `None`.
//!
//! Run against the page main world: Turnstile lives inside the page's own
//! frame graph, so an isolated-world context is unnecessary and the lookup
//! must observe the same shadow-DOM mounts as the live page.

use serde::Deserialize;
use serde_json::{Value, json};
use zendriver_transport::SessionHandle;

use crate::CloudflareBypass;
use crate::error::CloudflareError;
use crate::js;
use crate::options::TurnstileSelectors;

/// Bounding box of the Turnstile iframe, in viewport CSS pixels.
///
/// Public because it is what a caller-supplied
/// [`on_click`](CloudflareBypass::on_click) handler is measured against, as
/// [`ClickTarget::bbox`](crate::ClickTarget::bbox). The `zendriver` facade
/// re-exports it as `TurnstileBoundingBox`, since that crate already has a
/// `BoundingBox` of its own for element geometry.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct BoundingBox {
    /// Viewport-relative left edge, in CSS pixels.
    pub x: f64,
    /// Viewport-relative top edge, in CSS pixels.
    pub y: f64,
    /// Width in CSS pixels.
    pub width: f64,
    /// Height in CSS pixels.
    pub height: f64,
}

/// Evaluate `expression` in `session`'s main world and return its completion
/// value (`Value::Null` when the script produced nothing).
///
/// `expression` must already be a self-contained expression: no `contextId`
/// is sent, so this runs as a classic script in the page's own realm and
/// anything declared at its top level would land on the page's global object.
///
/// Propagates [`CloudflareError::JsError`] when the evaluation raised, and
/// [`CloudflareError::Call`] when the CDP call itself failed.
pub(crate) async fn eval_main_world(
    session: &SessionHandle,
    expression: &str,
) -> Result<Value, CloudflareError> {
    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": expression,
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

    Ok(res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

/// Run the shadow-DOM walker against `session`'s main world.
///
/// Returns `Ok(Some(bbox))` when a Turnstile iframe is mounted, `Ok(None)`
/// otherwise.
///
/// Deliberately a different question from the poll evaluator's `bbox`: this
/// reports a *mounted* iframe, `PollState::bbox` reports a *clickable* one.
/// A 0×0 invisible-Turnstile iframe is mounted but must never be clicked.
pub(crate) async fn detect_challenge(
    session: &SessionHandle,
    selectors: &TurnstileSelectors,
) -> Result<Option<BoundingBox>, CloudflareError> {
    let value = eval_main_world(session, &js::detect_expr(selectors)).await?;

    if value.is_null() {
        return Ok(None);
    }

    serde_json::from_value(value)
        .map(Some)
        .map_err(|e| CloudflareError::JsError(format!("invalid bbox payload: {e}")))
}

impl CloudflareBypass<'_> {
    /// Returns `true` if a Turnstile challenge iframe is currently mounted
    /// anywhere in the page (including shadow roots), matched against this
    /// driver's [`selectors`](CloudflareBypass::selectors).
    ///
    /// # Errors
    /// - [`CloudflareError::Call`] / [`CloudflareError::JsError`] — CDP or
    ///   in-page evaluator failure.
    pub async fn is_challenge_present(&self) -> Result<bool, CloudflareError> {
        Ok(detect_challenge(self.session, &self.selectors)
            .await?
            .is_some())
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testutil::expect_cmd;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn is_challenge_present_returns_true_when_bbox_yielded() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let bypass_sess = sess.clone();
            async move {
                let b = CloudflareBypass::new(&bypass_sess);
                b.is_challenge_present().await
            }
        });

        let id = expect_cmd(&mut mock, "Runtime.evaluate").await;
        let sent = mock.last_sent();
        assert!(
            sent["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("challenges.cloudflare.com"),
            "the detect evaluator should be inlined as the expression"
        );
        assert_eq!(sent["params"]["returnByValue"], true);

        mock.reply(
            id,
            json!({
                "result": {
                    "type": "object",
                    "value": { "x": 100.0, "y": 200.0, "width": 50.0, "height": 50.0 }
                }
            }),
        )
        .await;

        let got = fut.await.unwrap().unwrap();
        assert!(
            got,
            "challenge bbox returned → is_challenge_present == true"
        );

        conn.shutdown();
    }

    /// The detector used to inject a standalone script file carrying its own
    /// copy of the Cloudflare hostname, so configuring the marker on the
    /// bypass changed the poll loop and left the detector answering about a
    /// different widget.
    #[tokio::test]
    async fn is_challenge_present_uses_the_configured_marker() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let bypass_sess = sess.clone();
            async move {
                let b = CloudflareBypass::new(&bypass_sess).selectors(TurnstileSelectors {
                    iframe_src_contains: "gate.alien.invalid".into(),
                    ..TurnstileSelectors::default()
                });
                b.is_challenge_present().await
            }
        });

        let id = expect_cmd(&mut mock, "Runtime.evaluate").await;
        let expr = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(expr.contains("gate.alien.invalid"), "custom marker ignored");
        assert!(
            !expr.contains("challenges.cloudflare.com"),
            "the detector still carries its own copy of the default marker"
        );
        mock.reply(id, json!({ "result": { "type": "object" } }))
            .await;

        assert!(!fut.await.unwrap().unwrap());
        conn.shutdown();
    }
}
