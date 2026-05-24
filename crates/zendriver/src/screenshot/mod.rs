//! Chainable screenshot capture for a [`Tab`].
//!
//! [`ScreenshotBuilder`] lets callers pick PNG / JPEG / WebP, clip to a rect,
//! capture beyond the viewport (`full_page`), set the JPEG quality knob, and
//! toggle transparent backgrounds. Terminate with
//! [`ScreenshotBuilder::bytes`] (raw image bytes) or
//! [`ScreenshotBuilder::save`] (write to file).
//!
//! Construct via [`Tab::screenshot_builder`]; for a parameterless PNG of the
//! viewport, [`Tab::screenshot`] is the shortcut.
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! # let browser = zendriver::Browser::builder().launch().await?;
//! # let tab = browser.main_tab();
//! tab.goto("https://example.com").await?;
//! tab.screenshot_builder()
//!     .full_page(true)
//!     .jpeg()
//!     .quality(85)
//!     .save("page.jpg").await?;
//! # Ok(()) }
//! ```
//!
//! ## Quality knob semantics
//!
//! [`ScreenshotBuilder::quality`] stores the value regardless of the active
//! format. Chrome silently ignores `quality` on PNG / WebP captures, so the
//! cross-product `jpeg().quality(80)` and `png().quality(80)` both produce
//! valid screenshots — the second just wastes the parameter. Documented
//! explicitly here rather than enforced via a runtime panic: builders are
//! often constructed lazily through layers of helpers where conditional-on-
//! format checks would force every caller into noisy error handling for a
//! mistake that produces correct output anyway. Pick the format you want;
//! `quality` only matters for `jpeg()`.
//!
//! ## Full-page dispatch sequence
//!
//! When [`ScreenshotBuilder::full_page`] is `true`, [`ScreenshotBuilder::bytes`]
//! sends an extra `Page.getLayoutMetrics` first, reads `cssContentSize.{width,height}` from
//! the response, and forwards both as a `clip` rect with `scale: 1` plus
//! `captureBeyondViewport: true` on the subsequent `Page.captureScreenshot`.
//! Chrome handles the scroll-to-render-each-tile dance internally. With
//! `full_page: false` the builder just passes `clip` through verbatim
//! (or omits it when unset) and the capture is viewport-sized.

use std::path::Path;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::error::{Result, ZendriverError};
use crate::query::BoundingBox;
use crate::tab::Tab;

/// Output image format.
///
/// Maps 1:1 to CDP `Page.captureScreenshot`'s `format` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Lossless PNG. Default. Larger files but no compression artifacts.
    Png,
    /// JPEG. Smaller files; pair with [`ScreenshotBuilder::quality`].
    Jpeg,
    /// WebP. Smaller files than JPEG at comparable quality.
    Webp,
}

impl Format {
    /// CDP wire string for this format.
    ///
    /// Cheap (`&'static str`) — the enum crosses the FFI boundary as a tag,
    /// not a heap allocation per call.
    ///
    /// # Examples
    ///
    /// ```
    /// use zendriver::Format;
    /// assert_eq!(Format::Png.as_cdp(), "png");
    /// assert_eq!(Format::Jpeg.as_cdp(), "jpeg");
    /// ```
    #[must_use]
    pub fn as_cdp(&self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
        }
    }
}

/// Chainable screenshot capture bound to a [`Tab`].
///
/// Default state: PNG format, no clip, viewport-sized (not full-page), no
/// quality override, opaque background. Terminate the chain with
/// [`Self::bytes`] (raw image bytes) or [`Self::save`] (write to file).
#[derive(Debug)]
pub struct ScreenshotBuilder<'tab> {
    tab: &'tab Tab,
    format: Format,
    full_page: bool,
    clip: Option<BoundingBox>,
    quality: Option<u8>,
    omit_background: bool,
}

impl<'tab> ScreenshotBuilder<'tab> {
    /// Construct a fresh builder bound to `tab` with default settings.
    ///
    /// Default: PNG, viewport-sized, no clip, no quality override, opaque
    /// background. Most users go through [`Tab::screenshot_builder`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::ScreenshotBuilder;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let bytes = ScreenshotBuilder::new(&tab).bytes().await?;
    /// # let _ = bytes;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn new(tab: &'tab Tab) -> Self {
        Self {
            tab,
            format: Format::Png,
            full_page: false,
            clip: None,
            quality: None,
            omit_background: false,
        }
    }

    /// Set the output format to PNG (the default).
    ///
    /// Idempotent; useful when the format was previously switched and you
    /// want to switch back.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.screenshot_builder().png().save("out.png").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn png(mut self) -> Self {
        self.format = Format::Png;
        self
    }

    /// Set the output format to JPEG.
    ///
    /// Pair with [`Self::quality`] to control compression (1–100); without
    /// one, Chrome uses its default (~80).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.screenshot_builder().jpeg().quality(70).save("out.jpg").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn jpeg(mut self) -> Self {
        self.format = Format::Jpeg;
        self
    }

    /// Set the output format to WebP.
    ///
    /// Smaller files than JPEG at comparable visual quality; not all
    /// downstream tooling reads WebP, so PNG / JPEG remain the safer
    /// interop choices.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.screenshot_builder().webp().save("out.webp").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn webp(mut self) -> Self {
        self.format = Format::Webp;
        self
    }

    /// Toggle full-page mode.
    ///
    /// When `on`, [`Self::bytes`] queries `Page.getLayoutMetrics` first and
    /// passes the document's `cssContentSize` as the clip rect plus
    /// `captureBeyondViewport: true`, producing a single image of the entire
    /// scrollable page. Mutually exclusive in effect with [`Self::clip`]:
    /// when both are set, the layout-metrics clip overrides the manual one.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.screenshot_builder().full_page(true).save("full.png").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn full_page(mut self, on: bool) -> Self {
        self.full_page = on;
        self
    }

    /// Crop the capture to `bbox`.
    ///
    /// Coordinates are CSS pixels relative to the viewport top-left.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::BoundingBox;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let bbox = BoundingBox { x: 0.0, y: 0.0, width: 800.0, height: 600.0 };
    /// tab.screenshot_builder().clip(bbox).save("top.png").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn clip(mut self, bbox: BoundingBox) -> Self {
        self.clip = Some(bbox);
        self
    }

    /// Set the JPEG quality (1–100).
    ///
    /// Chrome ignores this knob on PNG / WebP captures, so the value is
    /// harmless on non-JPEG formats but only meaningful with [`Self::jpeg`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.screenshot_builder().jpeg().quality(60).save("out.jpg").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn quality(mut self, q: u8) -> Self {
        self.quality = Some(q);
        self
    }

    /// Toggle transparent background.
    ///
    /// PNG / WebP only — JPEG has no alpha channel and Chrome ignores the
    /// flag there. Maps to `Page.captureScreenshot { omitBackground: true }`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.screenshot_builder().png().omit_background(true).save("clear.png").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn omit_background(mut self, on: bool) -> Self {
        self.omit_background = on;
        self
    }

    /// Execute the capture and return the raw image bytes.
    ///
    /// Dispatch sequence:
    ///   1. If `full_page`, send `Page.getLayoutMetrics` and read
    ///      `cssContentSize.{width,height}` to build the document-spanning
    ///      clip rect.
    ///   2. Send `Page.captureScreenshot` with the chosen `format`, `clip`,
    ///      `quality`, `omitBackground`, and `captureBeyondViewport` flags.
    ///   3. Base64-decode the response's `data` field into the returned
    ///      `Vec<u8>`.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] when Chrome returns no
    /// screenshot data or malformed base64.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let bytes = tab.screenshot_builder().png().bytes().await?;
    /// tokio::fs::write("out.png", bytes).await?;
    /// # Ok(()) }
    /// ```
    pub async fn bytes(self) -> Result<Vec<u8>> {
        let mut params = Map::new();
        params.insert(
            "format".to_string(),
            Value::String(self.format.as_cdp().into()),
        );

        // Full-page mode overrides any user-supplied clip: ask Chrome for the
        // current document size, then clip to that.
        let effective_clip = if self.full_page {
            let metrics = self.tab.call("Page.getLayoutMetrics", json!({})).await?;
            let css_content_size = metrics.get("cssContentSize").ok_or_else(|| {
                ZendriverError::Navigation(
                    "Page.getLayoutMetrics response missing cssContentSize".into(),
                )
            })?;
            let width = css_content_size
                .get("width")
                .and_then(Value::as_f64)
                .ok_or_else(|| {
                    ZendriverError::Navigation(
                        "Page.getLayoutMetrics cssContentSize missing width".into(),
                    )
                })?;
            let height = css_content_size
                .get("height")
                .and_then(Value::as_f64)
                .ok_or_else(|| {
                    ZendriverError::Navigation(
                        "Page.getLayoutMetrics cssContentSize missing height".into(),
                    )
                })?;
            params.insert("captureBeyondViewport".to_string(), Value::Bool(true));
            Some(BoundingBox {
                x: 0.0,
                y: 0.0,
                width,
                height,
            })
        } else {
            self.clip
        };

        if let Some(bbox) = effective_clip {
            params.insert(
                "clip".to_string(),
                json!({
                    "x": bbox.x,
                    "y": bbox.y,
                    "width": bbox.width,
                    "height": bbox.height,
                    "scale": 1,
                }),
            );
        }

        if let Some(q) = self.quality {
            params.insert("quality".to_string(), json!(q));
        }

        if self.omit_background {
            params.insert("omitBackground".to_string(), Value::Bool(true));
        }

        let res = self
            .tab
            .call("Page.captureScreenshot", Value::Object(params))
            .await?;
        let data = res.get("data").and_then(|v| v.as_str()).ok_or_else(|| {
            ZendriverError::Navigation("Page.captureScreenshot returned no data".into())
        })?;
        BASE64
            .decode(data)
            .map_err(|e| ZendriverError::Navigation(format!("invalid base64 in screenshot: {e}")))
    }

    /// Execute the capture and write the raw image bytes to `path`.
    ///
    /// Convenience wrapper over [`Self::bytes`] + [`tokio::fs::write`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.screenshot_builder().full_page(true).save("full.png").await?;
    /// # Ok(()) }
    /// ```
    pub async fn save(self, path: impl AsRef<Path>) -> Result<()> {
        let bytes = self.bytes().await?;
        tokio::fs::write(path, bytes).await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    /// Default builder = PNG, viewport-sized (no clip), no full-page. The
    /// terminal dispatches a single `Page.captureScreenshot` with `format=png`
    /// and no `clip` / `quality` / `captureBeyondViewport` keys.
    #[tokio::test]
    async fn default_is_png_no_clip_no_full_page() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { ScreenshotBuilder::new(&t).bytes().await }
        });

        let id = mock.expect_cmd("Page.captureScreenshot").await;
        let sent = mock.last_sent();
        let params = &sent["params"];
        assert_eq!(params["format"], "png");
        assert!(params.get("clip").is_none());
        assert!(params.get("quality").is_none());
        assert!(params.get("captureBeyondViewport").is_none());
        assert!(params.get("omitBackground").is_none());
        mock.reply(id, json!({ "data": "UE5HIQ==" })).await;

        let bytes = fut.await.unwrap().unwrap();
        assert_eq!(bytes, b"PNG!");
        conn.shutdown();
    }

    /// `jpeg().quality(80)` configures `format=jpeg` plus the quality knob;
    /// both surface on the wire payload exactly once.
    #[tokio::test]
    async fn jpeg_quality_sets_format_and_quality_on_wire() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { ScreenshotBuilder::new(&t).jpeg().quality(80).bytes().await }
        });

        let id = mock.expect_cmd("Page.captureScreenshot").await;
        let sent = mock.last_sent();
        let params = &sent["params"];
        assert_eq!(params["format"], "jpeg");
        assert_eq!(params["quality"], 80);
        // No full-page side effects on a viewport jpeg.
        assert!(params.get("captureBeyondViewport").is_none());
        assert!(params.get("clip").is_none());
        mock.reply(id, json!({ "data": "SlBHIQ==" })).await;

        let bytes = fut.await.unwrap().unwrap();
        assert_eq!(bytes, b"JPG!");
        conn.shutdown();
    }

    /// `full_page(true)` dispatches `Page.getLayoutMetrics` first, then sends
    /// `Page.captureScreenshot` with `captureBeyondViewport: true` and a
    /// clip rect derived from the layout-metrics `cssContentSize`.
    #[tokio::test]
    async fn full_page_uses_layout_metrics_and_capture_beyond_viewport() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { ScreenshotBuilder::new(&t).full_page(true).bytes().await }
        });

        // Step 1: layout metrics lookup.
        let metrics_id = mock.expect_cmd("Page.getLayoutMetrics").await;
        mock.reply(
            metrics_id,
            json!({
                "cssLayoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": 800, "clientHeight": 600 },
                "cssVisualViewport": { "pageX": 0, "pageY": 0, "clientWidth": 800, "clientHeight": 600,
                                        "offsetX": 0, "offsetY": 0, "scale": 1, "zoom": 1 },
                "cssContentSize": { "x": 0, "y": 0, "width": 1280.0, "height": 4200.0 },
            }),
        )
        .await;

        // Step 2: screenshot with derived clip + captureBeyondViewport.
        let shot_id = mock.expect_cmd("Page.captureScreenshot").await;
        let sent = mock.last_sent();
        let params = &sent["params"];
        assert_eq!(params["format"], "png");
        assert_eq!(params["captureBeyondViewport"], true);
        let clip = &params["clip"];
        assert_eq!(clip["x"], 0.0);
        assert_eq!(clip["y"], 0.0);
        assert_eq!(clip["width"], 1280.0);
        assert_eq!(clip["height"], 4200.0);
        assert_eq!(clip["scale"], 1);
        mock.reply(shot_id, json!({ "data": "UE5HIQ==" })).await;

        let bytes = fut.await.unwrap().unwrap();
        assert_eq!(bytes, b"PNG!");
        conn.shutdown();
    }
}
