//! Frame enumeration handler — `browser_frame_list`.
//!
//! Returns one [`FrameSummary`] per frame on the current tab. The shape
//! deliberately mirrors what the zendriver `Frame` API exposes
//! (no `is_oopif` — that accessor does not exist on `Frame`).

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::actions::AckOutput;
use crate::tools::common::{EmptyInput, current_tab, lookup_frame};

/// Per-frame projection returned by `browser_frame_list`.
///
/// Field choice matches `zendriver::Frame`'s public accessors. Notable
/// gaps from the original plan:
/// - `is_oopif` does NOT exist on `Frame` — `is_main` + `parent_id`
///   together are sufficient for the main "what's the frame tree
///   layout?" question agents care about.
/// - `url` is a plain `String` (not `Result<String>`), so we don't need
///   the empty-string fallback that `TabSummary` uses.
#[derive(Debug, Serialize, JsonSchema, PartialEq, Eq)]
pub struct FrameSummary {
    /// Frame id (`Frame::id()`).
    pub id: String,
    /// Live frame URL (`Frame::url().await` — infallible).
    pub url: String,
    /// Parent frame id, or `None` for the main frame.
    pub parent_id: Option<String>,
    /// Frame `name` attribute, or `None` if unset.
    pub name: Option<String>,
    /// `true` for the main (top-level) frame.
    pub is_main: bool,
}

/// Output of `browser_frame_list`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FrameListOutput {
    /// All frames on the current tab. Order follows
    /// `zendriver::Tab::frames` (unspecified but typically tree-traversal
    /// order).
    pub frames: Vec<FrameSummary>,
}

/// Enumerate all frames on the current tab.
///
/// Routes through [`current_tab`], which surfaces
/// [`McpServerError::BrowserNotOpen`] / `NoCurrentTab` when appropriate.
pub async fn list(
    state: Arc<Mutex<SessionState>>,
    _: EmptyInput,
) -> Result<FrameListOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let frames = tab
        .frames()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    let mut out = Vec::with_capacity(frames.len());
    for f in &frames {
        // `Frame::url` returns plain `String` (no Result) per API Reality.
        let url = f.url().await;
        out.push(FrameSummary {
            id: f.id().to_string(),
            url,
            parent_id: f.parent_id().map(str::to_string),
            name: f.name().map(str::to_string),
            is_main: f.is_main(),
        });
    }
    Ok(FrameListOutput { frames: out })
}

// ---------- browser_frame_goto -------------------------------------------

/// Input for `browser_frame_goto`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameGotoInput {
    /// Target frame id (from `browser_frame_list`).
    pub frame_id: String,
    /// URL to navigate the frame to.
    pub url: String,
}

/// Navigate a specific frame to a URL and wait for its load.
///
/// `Frame::goto` only drives same-document navigation on the main frame of an
/// out-of-process iframe; for in-process child frames it surfaces the lib's
/// error through the standard pipeline.
pub async fn frame_goto(
    state: Arc<Mutex<SessionState>>,
    input: FrameGotoInput,
) -> Result<AckOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let frame = lookup_frame(&tab, &input.frame_id).await?;
    frame
        .goto(&input.url)
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    frame
        .wait_for_load()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(AckOutput { ok: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Arc<Mutex<SessionState>> {
        Arc::new(Mutex::new(SessionState::new()))
    }

    #[tokio::test]
    async fn list_with_no_browser_suggests_browser_open() {
        let err = list(fresh(), EmptyInput {})
            .await
            .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"), "msg: {}", err.message);
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }
}
