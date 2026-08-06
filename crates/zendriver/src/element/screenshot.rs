//! [`Element::screenshot`] — element-scoped PNG capture.
//!
//! Dispatch sequence (with internal refresh-on-stale recovery), matching the
//! `scroll → gate → measure → dispatch` shape every action in
//! [`mod@crate::element::actions`] uses:
//!   1. [`Element::scroll_into_view`] — the clip in step 4 is
//!      viewport-relative, so the element must be on-screen before it is
//!      measured (and before the gate, which requires the same).
//!   2. Visibility gate — we need pixels to capture; overlay occlusion +
//!      disabled state are irrelevant here, so the gate is the lightest
//!      preset.
//!   3. [`Element::bounding_box`] — viewport-relative quad of the element.
//!   4. `Page.getLayoutMetrics` — the visual viewport's page offset, added
//!      to that quad. `Page.captureScreenshot` reads `clip` in document
//!      coordinates, so a viewport-relative rect would crop the wrong band
//!      of a scrolled page.
//!   5. `Page.captureScreenshot { format: "png", clip: { x, y, width,
//!      height, scale: 1 } }` — crop to the element's rect at native scale.
//!   6. base64-decode the `data` field into raw PNG bytes.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

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
    /// Scrolls the element into view, waits up to 5 s for it to become
    /// visible, reads its bbox via `DOM.getBoxModel`, converts that to
    /// document coordinates with `Page.getLayoutMetrics`, then sends
    /// `Page.captureScreenshot` with a matching `clip` rect (at `scale: 1`).
    /// Returns the raw PNG bytes.
    ///
    /// The scroll leads because an off-screen element is neither rendered nor
    /// judged visible by the gate; the coordinate conversion follows because
    /// Chrome reads the clip in document space while the box model is
    /// viewport-relative.
    ///
    /// For full-viewport captures, see [`crate::Tab::screenshot`].
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::NotActionable`] if the element doesn't
    /// become visible within the 5s gate timeout;
    /// [`ZendriverError::Navigation`] if Chrome returns no bbox or no
    /// screenshot data.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let card = tab.find().css(".card").one().await?;
    /// let png_bytes = card.screenshot().await?;
    /// tokio::fs::write("card.png", png_bytes).await?;
    /// # Ok(()) }
    /// ```
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        self.with_refresh(|| async move {
            // Scroll first, exactly as every action in `element::actions`
            // does. Two reasons: the clip below is viewport-relative, so an
            // element measured while off-screen is cropped from the wrong
            // place; and the visibility predicate itself requires the element
            // to overlap the viewport, so gating before the scroll would
            // reject every below-the-fold element.
            self.scroll_into_view().await?;
            actionability::wait_actionable(
                self,
                ActionabilityCheck::VISIBLE_ONLY,
                DEFAULT_ACTIONABILITY_TIMEOUT,
                None,
            )
            .await?;
            let bbox = self
                .bounding_box()
                .await?
                .ok_or_else(|| ZendriverError::Navigation("element has no bounding box".into()))?;

            // Chrome reads `clip` in DOCUMENT coordinates while
            // `bounding_box` is viewport-relative, so the scroll offset has
            // to be added back or a scrolled page gets cropped from the wrong
            // region entirely. The offset comes from CDP rather than
            // `window.scrollX` — one less page-controlled input on a path a
            // page would happily lie to.
            let metrics = self
                .inner
                .tab
                .call("Page.getLayoutMetrics", json!({}))
                .await?;
            let viewport = metrics.get("cssVisualViewport");
            let page_x = viewport
                .and_then(|v| v.get("pageX"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let page_y = viewport
                .and_then(|v| v.get("pageY"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);

            let res = self
                .inner
                .tab
                .call(
                    "Page.captureScreenshot",
                    json!({
                        "format": "png",
                        "clip": {
                            "x": bbox.x + page_x,
                            "y": bbox.y + page_y,
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
    use crate::test_support::{expect, serve_call_js};
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

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

        // Step 1: scroll_into_view — MUST land before both the gate and the
        // measurement, or an off-screen element is rejected as "not visible"
        // and, past that, clipped from stale coordinates.
        let id = expect(&mut mock, "Runtime.callFunctionOn").await;
        assert!(
            mock.last_sent()["params"]["functionDeclaration"]
                .as_str()
                .unwrap()
                .contains("scrollIntoView"),
            "the scroll must be dispatched before the gate and DOM.getBoxModel",
        );
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        // Step 2: actionability gate (VISIBLE_ONLY = only check_visible).
        //
        // The predicate is asserted by what it is NOT: its JS is owned by
        // `query::actionability` and gets rewritten there, but "the gate is
        // not the scroll" is the property this test needs.
        let gate_js = serve_call_js(&mut mock, json!({ "value": true, "type": "boolean" })).await;
        assert!(
            gate_js.contains("getBoundingClientRect") && !gate_js.contains("scrollIntoView"),
            "expected the visibility predicate after the scroll, got: {gate_js}",
        );

        // Step 3: bounding_box → DOM.getBoxModel.
        let id = expect(&mut mock, "DOM.getBoxModel").await;
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

        // Step 4: the scroll offset that turns the viewport-relative box into
        // the document-relative rect Chrome's clip actually wants.
        let id = expect(&mut mock, "Page.getLayoutMetrics").await;
        mock.reply(
            id,
            json!({
                "cssVisualViewport": { "pageX": 5.0, "pageY": 3000.0, "clientWidth": 800,
                                        "clientHeight": 600, "offsetX": 0, "offsetY": 0,
                                        "scale": 1, "zoom": 1 },
            }),
        )
        .await;

        // Step 5: Page.captureScreenshot, clip offset into document space:
        // x = 10 + 5, y = 20 + 3000.
        let id = expect(&mut mock, "Page.captureScreenshot").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["format"], "png");
        let clip = &sent["params"]["clip"];
        assert_eq!(clip["x"], 15.0);
        assert_eq!(
            clip["y"], 3020.0,
            "the clip must be in document coordinates, not viewport ones",
        );
        assert_eq!(clip["width"], 100.0);
        assert_eq!(clip["height"], 50.0);
        assert_eq!(clip["scale"], 1);
        mock.reply(id, json!({ "data": "UE5HIQ==" })).await;

        let bytes = fut.await.unwrap().unwrap();
        assert_eq!(bytes, b"PNG!");
        conn.shutdown();
    }
}
