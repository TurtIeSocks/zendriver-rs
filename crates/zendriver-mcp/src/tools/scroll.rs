//! Page-scroll tool — `browser_scroll`.
//!
//! Wraps [`zendriver::Tab::scroll_with`]. The lib's [`ScrollOptions`] follows
//! the inverted CDP gesture convention (a *negative* `yDistance` scrolls the
//! page down); this tool exposes intuitive screen axes (`+dy` = down, `+dx` =
//! right) and negates both before dispatch, so the wire API reads naturally.

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::ScrollOptions;

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::common::{current_tab, page_snapshot};

/// Input for `browser_scroll`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageScrollInput {
    /// Horizontal scroll distance in CSS pixels. Positive scrolls the view
    /// right; negative scrolls left. Default `0`.
    #[serde(default)]
    pub dx: f64,
    /// Vertical scroll distance in CSS pixels. Positive scrolls the view
    /// **down** the page; negative scrolls up. Default `0`.
    #[serde(default)]
    pub dy: f64,
    /// Optional gesture speed in pixels/second. Omitted → Chrome's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<i64>,
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

/// Output of `browser_scroll`: the page scroll offset after the gesture.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PageScrollOutput {
    /// `window.scrollX` after the gesture.
    pub scroll_x: f64,
    /// `window.scrollY` after the gesture.
    pub scroll_y: f64,
    /// Trimmed rendered HTML, populated only when `return_snapshot: true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

/// Scroll the page by a signed pixel distance (intuitive axes: `+dy` = down,
/// `+dx` = right), then report the resulting scroll offset.
pub async fn scroll(
    state: Arc<Mutex<SessionState>>,
    input: PageScrollInput,
) -> Result<PageScrollOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.scroll_with(ScrollOptions {
        dx: -input.dx,
        dy: -input.dy,
        speed: input.speed,
    })
    .await
    .map_err(|e| map_error(McpServerError::from(e)))?;
    let (scroll_x, scroll_y): (f64, f64) = tab
        .evaluate_main("[window.scrollX, window.scrollY]")
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    let snapshot = if input.return_snapshot {
        Some(page_snapshot(&tab).await?)
    } else {
        None
    };
    Ok(PageScrollOutput {
        scroll_x,
        scroll_y,
        snapshot,
    })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scroll_with_no_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = scroll(
            state,
            PageScrollInput {
                dx: 0.0,
                dy: 500.0,
                speed: None,
                return_snapshot: false,
            },
        )
        .await
        .expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }
}
