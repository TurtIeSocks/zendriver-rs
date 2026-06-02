//! Page-export tools — `browser_pdf`, `browser_save_mhtml`.
//!
//! `browser_pdf` drives the full [`zendriver::PdfBuilder`] (`Page.printToPDF`);
//! `browser_save_mhtml` captures an MHTML archive (`Page.captureSnapshot`).
//! Both return the [`BlobOutput`] shape: bytes go to `save_path` on the MCP
//! host when given, else base64-inline (subject to the inline size limit).

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::common::{BlobOutput, blob_output, current_tab};

/// Input for `browser_pdf`. Every field is optional; an unset field leaves the
/// corresponding [`zendriver::PdfBuilder`] default in place.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PdfInput {
    /// Landscape orientation (default portrait).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landscape: Option<bool>,
    /// Render background graphics/colors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_background: Option<bool>,
    /// Render scale (`1.0` = 100%).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// Paper width in inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paper_width: Option<f64>,
    /// Paper height in inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paper_height: Option<f64>,
    /// Top margin in inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_top: Option<f64>,
    /// Bottom margin in inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_bottom: Option<f64>,
    /// Left margin in inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_left: Option<f64>,
    /// Right margin in inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_right: Option<f64>,
    /// Page ranges to print, e.g. `"1-3, 5"`. Empty → all pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_ranges: Option<String>,
    /// Prefer any CSS `@page` size over `paper_width`/`paper_height`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefer_css_page_size: Option<bool>,
    /// Write the PDF to this path on the MCP host instead of inlining base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_path: Option<String>,
}

/// Export the current page to PDF.
pub async fn pdf(
    state: Arc<Mutex<SessionState>>,
    input: PdfInput,
) -> Result<BlobOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let mut builder = tab.pdf_builder();
    if let Some(v) = input.landscape {
        builder = builder.landscape(v);
    }
    if let Some(v) = input.print_background {
        builder = builder.print_background(v);
    }
    if let Some(v) = input.scale {
        builder = builder.scale(v);
    }
    if let Some(v) = input.paper_width {
        builder = builder.paper_width(v);
    }
    if let Some(v) = input.paper_height {
        builder = builder.paper_height(v);
    }
    if let Some(v) = input.margin_top {
        builder = builder.margin_top(v);
    }
    if let Some(v) = input.margin_bottom {
        builder = builder.margin_bottom(v);
    }
    if let Some(v) = input.margin_left {
        builder = builder.margin_left(v);
    }
    if let Some(v) = input.margin_right {
        builder = builder.margin_right(v);
    }
    if let Some(v) = input.page_ranges {
        builder = builder.page_ranges(v);
    }
    if let Some(v) = input.prefer_css_page_size {
        builder = builder.prefer_css_page_size(v);
    }
    let bytes = builder
        .bytes()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    blob_output(&bytes, input.save_path)
}

/// Input for `browser_save_mhtml`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveMhtmlInput {
    /// Write the MHTML to this path on the MCP host instead of inlining base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_path: Option<String>,
}

/// Capture an MHTML archive of the current page.
pub async fn save_mhtml(
    state: Arc<Mutex<SessionState>>,
    input: SaveMhtmlInput,
) -> Result<BlobOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let mhtml = tab
        .snapshot_mhtml()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    blob_output(mhtml.as_bytes(), input.save_path)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pdf_with_no_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = pdf(
            state,
            PdfInput {
                landscape: None,
                print_background: None,
                scale: None,
                paper_width: None,
                paper_height: None,
                margin_top: None,
                margin_bottom: None,
                margin_left: None,
                margin_right: None,
                page_ranges: None,
                prefer_css_page_size: None,
                save_path: None,
            },
        )
        .await
        .expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }
}
