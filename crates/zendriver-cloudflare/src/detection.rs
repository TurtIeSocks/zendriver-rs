//! Shadow-DOM Turnstile challenge detection.
//!
//! Dispatches a `Runtime.evaluate` carrying [`detect.js`](./detect.js), which
//! recursively walks the document plus every shadow root looking for an
//! iframe whose `src` includes `challenges.cloudflare.com`. Returns the
//! iframe's bounding box (in viewport coordinates) or `None`.
//!
//! Run against the page main world: Turnstile lives inside the page's own
//! frame graph, so an isolated-world context is unnecessary and the lookup
//! must observe the same shadow-DOM mounts as the live page.

use serde::Deserialize;
use serde_json::{json, Value};
use zendriver_transport::SessionHandle;

use crate::error::CloudflareError;
use crate::CloudflareBypass;

/// Bounding box of the Turnstile iframe, in viewport CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub(crate) struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Run the shadow-DOM walker against `session`'s main world.
///
/// Returns `Ok(Some(bbox))` when a Turnstile iframe is mounted, `Ok(None)`
/// otherwise. Propagates [`CloudflareError::JsError`] if the evaluation
/// raised, and [`CloudflareError::Call`] if the underlying CDP call failed.
pub(crate) async fn detect_challenge(
    session: &SessionHandle,
) -> Result<Option<BoundingBox>, CloudflareError> {
    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": include_str!("detect.js"),
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

    if value.is_null() {
        return Ok(None);
    }

    serde_json::from_value(value)
        .map(Some)
        .map_err(|e| CloudflareError::JsError(format!("invalid bbox payload: {e}")))
}

impl CloudflareBypass<'_> {
    /// Returns `true` if a Cloudflare Turnstile challenge iframe is currently
    /// mounted anywhere in the page (including shadow roots).
    pub async fn is_challenge_present(&self) -> Result<bool, CloudflareError> {
        Ok(detect_challenge(self.session).await?.is_some())
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
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

        let id = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent();
        assert!(
            sent["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("challenges.cloudflare.com"),
            "detect.js script should be inlined as the expression"
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
}
