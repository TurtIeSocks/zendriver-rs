//! `Element::screenshot` — element-scoped PNG capture.
//!
//! Dispatch sequence (wrapped in [`Element::with_refresh`] so post-navigation
//! / post-rerender stale handles transparently re-resolve once and retry):
//!   1. `wait_actionable` with [`ActionabilityCheck::VISIBLE_ONLY`] — we
//!      need pixels to capture; overlay occlusion + disabled state are
//!      irrelevant here, so the gate is the lightest preset.
//!   2. `bounding_box` — viewport-relative quad of the element.
//!   3. `Page.captureScreenshot { format: "png", clip: { x, y, width,
//!      height, scale: 1 } }` — crop to the element's bbox at native scale.
//!   4. base64-decode the `data` field into raw PNG bytes.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::json;

use crate::element::Element;
use crate::error::{Result, ZendriverError};
use crate::query::actionability::{self, ActionabilityCheck};

/// Default deadline for the visibility gate before the capture call.
/// Matches the value used by other Element actions; per-call override
/// lands in P4 when the per-action options structs grow.
const DEFAULT_ACTIONABILITY_TIMEOUT: Duration = Duration::from_secs(5);

impl Element {
    /// Capture a PNG screenshot cropped to this element's bounding box.
    ///
    /// Waits up to 5 s for the element to become visible (via
    /// [`ActionabilityCheck::VISIBLE_ONLY`]), reads its viewport-relative
    /// bbox via `DOM.getBoxModel`, then sends `Page.captureScreenshot`
    /// with a matching `clip` rect (at `scale: 1`). Returns the raw PNG
    /// bytes.
    ///
    /// For full-viewport captures, see [`crate::tab::Tab::screenshot`].
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        self.with_refresh(|| async move {
            actionability::wait_actionable(
                self,
                ActionabilityCheck::VISIBLE_ONLY,
                DEFAULT_ACTIONABILITY_TIMEOUT,
            )
            .await?;
            let bbox = self
                .bounding_box()
                .await?
                .ok_or_else(|| ZendriverError::Navigation("element has no bounding box".into()))?;
            let res = self
                .inner
                .tab
                .call(
                    "Page.captureScreenshot",
                    json!({
                        "format": "png",
                        "clip": {
                            "x": bbox.x,
                            "y": bbox.y,
                            "width": bbox.width,
                            "height": bbox.height,
                            "scale": 1,
                        },
                    }),
                )
                .await?;
            let data = res.get("data").and_then(|v| v.as_str()).ok_or_else(|| {
                ZendriverError::Navigation("Page.captureScreenshot returned no data".into())
            })?;
            BASE64.decode(data).map_err(|e| {
                ZendriverError::Navigation(format!("invalid base64 in screenshot: {e}"))
            })
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tab::Tab;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn screenshot_sends_page_capturescreenshot_with_clip_matching_bbox() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 7, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.screenshot().await }
        });

        // Step 1: actionability gate (VISIBLE_ONLY = only check_visible).
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        // Sanity-check we're calling the visibility predicate, not some
        // other JS thunk.
        assert!(sent["params"]["functionDeclaration"]
            .as_str()
            .unwrap()
            .contains("offsetParent"));
        mock.reply(
            id,
            json!({ "result": { "value": true, "type": "boolean" } }),
        )
        .await;

        // Step 2: bounding_box → DOM.getBoxModel.
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

        // Step 3: Page.captureScreenshot with clip { x=10, y=20, w=100, h=50, scale=1 }.
        let id = mock.expect_cmd("Page.captureScreenshot").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["format"], "png");
        let clip = &sent["params"]["clip"];
        assert_eq!(clip["x"], 10.0);
        assert_eq!(clip["y"], 20.0);
        assert_eq!(clip["width"], 100.0);
        assert_eq!(clip["height"], 50.0);
        assert_eq!(clip["scale"], 1);
        mock.reply(id, json!({ "data": "UE5HIQ==" })).await;

        let bytes = fut.await.unwrap().unwrap();
        assert_eq!(bytes, b"PNG!");
        conn.shutdown();
    }
}
