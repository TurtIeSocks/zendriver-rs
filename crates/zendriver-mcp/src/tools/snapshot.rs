//! Snapshot tools — `browser_html`, `browser_screenshot`.
//!
//! These are the two "see the page" tools for v0. The accessibility-tree
//! variant (`browser_snapshot`) was dropped — zendriver has no native AX
//! API and the lift to derive one wasn't worth the surface area. Trimmed
//! HTML covers the same "show me page structure" need; a screenshot covers
//! the "show me what it looks like" need.
//!
//! ## Image return shape
//!
//! `browser_screenshot` returns a raw [`rmcp::model::CallToolResult`]
//! rather than a typed wrapper. The inline `Content::image` block is the
//! affordance MCP clients use to surface images to the model — wrapping
//! the bytes in a JSON struct would force clients to do base64 decoding
//! themselves and lose the multimodal hook entirely. The optional
//! `structured_content` slot still carries `{ saved_path?, format,
//! byte_len }` so callers that only want metadata can ignore the image.
//!
//! ## Selector + frame_id mutual exclusion (`browser_html`)
//!
//! When both are supplied, we return `invalid_params`. The Selector's own
//! `frame_id` field already covers cross-frame element lookups; the
//! tool-level `frame_id` is for *frame-document* HTML, not element HTML.
//! Allowing both would force a choice between "selector inside that
//! frame" (already expressible via `Selector { frame_id, css, .. }`) and
//! "full frame document HTML" — keeping them split avoids the ambiguity.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;
use zendriver::{Frame, Tab, ZendriverError};

use crate::errors::{McpServerError, map_error};
use crate::selectors::Selector;
use crate::snapshot::html_trim;
use crate::state::SessionState;
use crate::tools::common::current_tab;
use crate::tools::find::resolve;

// ---------- browser_html --------------------------------------------------

/// Input for `browser_html`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HtmlInput {
    /// When set, returns `el.innerHTML` for the matched element.
    /// Mutually exclusive with the tool-level `frame_id` — use the
    /// selector's own `frame_id` to scope the lookup to a sub-frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<Selector>,
    /// Trim the result: strip `<script>` / `<style>` blocks + collapse
    /// whitespace. Default `true`. Pass `false` for byte-exact output.
    #[serde(default = "default_true")]
    pub trim: bool,
    /// Return the *frame's* document HTML rather than the tab's main frame.
    /// Mutually exclusive with `selector`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
}

const fn default_true() -> bool {
    true
}

/// Return the current page's HTML.
///
/// Resolution order:
/// 1. Both `selector` and `frame_id` set → `invalid_params`.
/// 2. `selector` set → `el.innerHTML` of the matched element.
/// 3. `frame_id` set → that frame's `document.documentElement.outerHTML`.
/// 4. Neither set → the main frame's HTML.
///
/// Returns a plain `String`; rmcp auto-wraps as a `text` content block.
pub async fn html(state: Arc<Mutex<SessionState>>, input: HtmlInput) -> Result<String, ErrorData> {
    if input.selector.is_some() && input.frame_id.is_some() {
        return Err(ErrorData::invalid_params(
            "`selector` and `frame_id` are mutually exclusive — use the selector's own `frame_id` field to scope element lookup to a sub-frame."
                .to_string(),
            None,
        ));
    }
    let s = state.lock().await;
    let tab = current_tab(&s).await?;

    let raw = if let Some(sel) = input.selector.as_ref() {
        let el = resolve(&tab, sel).await?;
        el.inner_html()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?
    } else if let Some(fid) = input.frame_id.as_deref() {
        let frame = lookup_frame(&tab, fid).await?;
        frame
            .content()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?
    } else {
        let frame = tab
            .main_frame()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
        frame
            .content()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?
    };

    Ok(if input.trim {
        html_trim::trim(&raw)
    } else {
        raw
    })
}

/// Look up a `Frame` on `tab` by id — mirror of `tools::find::lookup_frame`
/// (kept inline rather than re-exported to avoid bloating the `find`
/// module's public surface).
async fn lookup_frame(tab: &Tab, frame_id: &str) -> Result<Frame, ErrorData> {
    let frames = tab
        .frames()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    frames
        .into_iter()
        .find(|f| f.id() == frame_id)
        .ok_or_else(|| {
            map_error(McpServerError::from(ZendriverError::FrameNotFound(
                frame_id.to_string(),
            )))
        })
}

// ---------- browser_screenshot --------------------------------------------

/// Image format choice for `browser_screenshot`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImgFormat {
    /// Lossless PNG (default). Chrome ignores `quality` here.
    Png,
    /// JPEG; pair with `quality` (1..=100) to control compression.
    Jpeg,
    /// WebP; smaller than JPEG at similar visual fidelity.
    Webp,
}

const fn default_format() -> ImgFormat {
    ImgFormat::Png
}

impl ImgFormat {
    /// Mime type for this format — fed into the `Content::image` block.
    fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
    /// Wire string used in structured-content output.
    fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
        }
    }
}

/// Input for `browser_screenshot`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotInput {
    /// Output format. Default PNG.
    #[serde(default = "default_format")]
    pub format: ImgFormat,
    /// `true` → capture the full scrollable document (issues
    /// `Page.getLayoutMetrics` first to size the clip). Default `false`
    /// (viewport-sized).
    #[serde(default)]
    pub full_page: bool,
    /// When set, clip the screenshot to the element's bounding box (after
    /// resolving the selector). Mutually compatible with all formats;
    /// overrides `full_page`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<Selector>,
    /// `true` → transparent background. PNG / WebP only (JPEG has no
    /// alpha channel; Chrome silently ignores the flag).
    #[serde(default)]
    pub omit_background: bool,
    /// JPEG compression quality (1..=100). Ignored on PNG / WebP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<u8>,
    /// When set, also write the raw bytes to this path on the host running
    /// the MCP server. The inline image block is still returned regardless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_path: Option<String>,
}

/// Capture a screenshot of the current tab.
///
/// Returns a [`CallToolResult`] carrying:
/// - One `Content::image` block with base64-encoded image bytes + mime.
/// - `structured_content: { format, byte_len, saved_path? }` for callers
///   that want metadata without decoding the image block.
pub async fn screenshot(
    state: Arc<Mutex<SessionState>>,
    input: ScreenshotInput,
) -> Result<CallToolResult, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;

    // Resolve the optional clip rect from the selector BEFORE constructing
    // the builder — the builder's lifetime is tied to `tab` and we want to
    // do the find / bbox lookup against the same tab handle without
    // re-borrowing.
    let clip_bbox = if let Some(sel) = input.selector.as_ref() {
        let el = resolve(&tab, sel).await?;
        let bbox = el
            .bounding_box()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?
            .ok_or_else(|| {
                ErrorData::invalid_request(
                    "Selected element has no bounding box (likely `display: none` or detached). Cannot clip the screenshot."
                        .to_string(),
                    Some(json!({ "suggested_next": "browser_element_state" })),
                )
            })?;
        Some(bbox)
    } else {
        None
    };

    let mut builder = tab.screenshot_builder();
    builder = match input.format {
        ImgFormat::Png => builder.png(),
        ImgFormat::Jpeg => builder.jpeg(),
        ImgFormat::Webp => builder.webp(),
    };
    builder = builder.full_page(input.full_page);
    builder = builder.omit_background(input.omit_background);
    if let Some(q) = input.quality {
        builder = builder.quality(q);
    }
    if let Some(bbox) = clip_bbox {
        builder = builder.clip(bbox);
    }

    let bytes = builder
        .bytes()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;

    let byte_len = bytes.len();

    if let Some(p) = input.save_path.as_deref() {
        tokio::fs::write(p, &bytes).await.map_err(|e| {
            ErrorData::internal_error(format!("Failed to write screenshot to `{p}`: {e}"), None)
        })?;
    }

    let encoded = BASE64.encode(&bytes);
    let image = Content::image(encoded, input.format.mime());

    let mut meta = serde_json::Map::new();
    meta.insert("format".into(), json!(input.format.as_str()));
    meta.insert("byte_len".into(), json!(byte_len));
    if let Some(p) = input.save_path.as_deref() {
        meta.insert("saved_path".into(), json!(p));
    }

    let mut result = CallToolResult::success(vec![image]);
    result.structured_content = Some(serde_json::Value::Object(meta));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Arc<Mutex<SessionState>> {
        Arc::new(Mutex::new(SessionState::new()))
    }

    fn css_sel(s: &str) -> Selector {
        Selector {
            css: Some(s.into()),
            xpath: None,
            text: None,
            text_exact: None,
            text_regex: None,
            role: None,
            role_name: None,
            nth: None,
            visible_only: true,
            timeout_ms: 5000,
            frame_id: None,
        }
    }

    #[tokio::test]
    async fn html_with_no_browser_suggests_browser_open() {
        let err = html(
            fresh(),
            HtmlInput {
                selector: None,
                trim: true,
                frame_id: None,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"), "msg: {}", err.message);
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn html_rejects_selector_and_frame_id_combo_with_invalid_params() {
        // Important: mutual-exclusion check must run BEFORE the
        // browser-not-open check, so the agent gets the configuration error
        // even when no browser is open.
        let err = html(
            fresh(),
            HtmlInput {
                selector: Some(css_sel("h1")),
                trim: true,
                frame_id: Some("frame-X".into()),
            },
        )
        .await
        .expect_err("selector+frame_id must error");
        assert!(
            err.message.contains("mutually exclusive"),
            "msg: {}",
            err.message
        );
        // `invalid_params` rmcp errors carry code -32602.
        assert_eq!(err.code.0, -32602, "expected invalid_params code");
    }

    #[tokio::test]
    async fn screenshot_with_no_browser_suggests_browser_open() {
        let err = screenshot(
            fresh(),
            ScreenshotInput {
                format: ImgFormat::Png,
                full_page: false,
                selector: None,
                omit_background: false,
                quality: None,
                save_path: None,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"), "msg: {}", err.message);
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }
}
