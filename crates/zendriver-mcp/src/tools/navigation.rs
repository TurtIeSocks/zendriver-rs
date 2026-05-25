//! Navigation handlers — `browser_goto`, `_back`, `_forward`, `_reload`,
//! `_wait_for_idle`.
//!
//! All five tools share the [`crate::tools::common::current_tab`] helper
//! and (for the URL-changing variants) a [`NavOutput`] return type that
//! reports the post-navigation `url` + `title` plus an optional rendered
//! HTML snapshot.

use std::sync::Arc;
use std::time::Duration;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::errors::{McpServerError, map_error};
use crate::snapshot::html_trim;
use crate::state::SessionState;
use crate::tools::common::{EmptyInput, current_tab};

// ---------- shared types --------------------------------------------------

/// Common output of navigation tools that change the page URL.
///
/// `snapshot` is populated only when the caller passes
/// `return_snapshot: true` on tools that accept that flag.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NavOutput {
    /// Current tab URL after the operation completes.
    pub url: String,
    /// Current tab title after the operation completes.
    pub title: String,
    /// Trimmed rendered HTML of the page (drops script/style + collapses
    /// whitespace). Only populated when the caller asks for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

/// Build a [`NavOutput`] from the current tab, optionally collecting a
/// trimmed HTML snapshot of the rendered page.
async fn nav_output(tab: &zendriver::Tab, return_snapshot: bool) -> Result<NavOutput, ErrorData> {
    let url = tab.url().await.map(|u| u.to_string()).unwrap_or_default();
    let title = tab.title().await.unwrap_or_default();
    let snapshot = if return_snapshot {
        Some(snapshot_now(tab).await?)
    } else {
        None
    };
    Ok(NavOutput {
        url,
        title,
        snapshot,
    })
}

/// Collect the current rendered HTML and trim it.
///
/// Uses `document.documentElement.outerHTML` rather than CDP's
/// `Page.captureSnapshot` so the result reflects post-script DOM mutations.
async fn snapshot_now(tab: &zendriver::Tab) -> Result<String, ErrorData> {
    let html: String = tab
        .evaluate_main("document.documentElement.outerHTML")
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(html_trim::trim(&html))
}

// ---------- browser_goto --------------------------------------------------

/// `browser_goto` arg block.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GotoInput {
    /// Absolute URL to navigate to.
    pub url: String,
    /// Post-navigation wait strategy. Default: `load`.
    #[serde(default = "default_wait")]
    pub wait_for: WaitFor,
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

/// Wait strategy options for `browser_goto`.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitFor {
    /// Wait for the `load` event to fire.
    #[default]
    Load,
    /// Wait until the network has been idle for ~500ms (5s outer timeout).
    Idle,
    /// Return immediately after the navigation request is dispatched.
    None,
}

const fn default_wait() -> WaitFor {
    WaitFor::Load
}

/// Navigate the current tab to a URL.
pub async fn goto(
    state: Arc<Mutex<SessionState>>,
    input: GotoInput,
) -> Result<NavOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.goto(&input.url)
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    match input.wait_for {
        WaitFor::Load => tab
            .wait_for_load()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?,
        WaitFor::Idle => tab
            .wait_for_idle_with(Duration::from_millis(5000), Duration::from_millis(500))
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?,
        WaitFor::None => {}
    }
    nav_output(&tab, input.return_snapshot).await
}

// ---------- browser_back / _forward / _reload -----------------------------

/// Arg block for `browser_back` / `_forward` / `_reload`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryInput {
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

/// Step back one entry in the tab's history.
pub async fn back(
    state: Arc<Mutex<SessionState>>,
    input: HistoryInput,
) -> Result<NavOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.back()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    nav_output(&tab, input.return_snapshot).await
}

/// Step forward one entry in the tab's history.
pub async fn forward(
    state: Arc<Mutex<SessionState>>,
    input: HistoryInput,
) -> Result<NavOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.forward()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    nav_output(&tab, input.return_snapshot).await
}

/// Reload the current page.
pub async fn reload(
    state: Arc<Mutex<SessionState>>,
    input: HistoryInput,
) -> Result<NavOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.reload()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    nav_output(&tab, input.return_snapshot).await
}

// ---------- browser_wait_for_idle -----------------------------------------

/// Arg block for `browser_wait_for_idle`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdleInput {
    /// Outer timeout in milliseconds (default: 5000).
    #[serde(default = "default_idle_timeout")]
    pub timeout_ms: u64,
}

const fn default_idle_timeout() -> u64 {
    5000
}

impl Default for IdleInput {
    fn default() -> Self {
        Self {
            timeout_ms: default_idle_timeout(),
        }
    }
}

/// Output of `browser_wait_for_idle`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct IdleOutput {
    /// `true` when the network reached the idle state before the timeout.
    pub idle: bool,
}

/// Wait until the current tab's network has been idle for ~500ms.
///
/// Quiet window is fixed at 500ms; `timeout_ms` is the outer bound.
pub async fn wait_for_idle(
    state: Arc<Mutex<SessionState>>,
    input: IdleInput,
) -> Result<IdleOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.wait_for_idle_with(
        Duration::from_millis(input.timeout_ms),
        Duration::from_millis(500),
    )
    .await
    .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(IdleOutput { idle: true })
}

// ---------- shim -----------------------------------------------------------

/// `browser_back` / `_forward` / `_reload` accept either no args or an arg
/// block with `return_snapshot`. The thin wrapper in `server.rs` always
/// passes an `EmptyInput`; alternative call sites can pre-build a
/// [`HistoryInput`].
impl From<EmptyInput> for HistoryInput {
    fn from(_: EmptyInput) -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn goto_with_no_browser_suggests_browser_open() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = goto(
            state,
            GotoInput {
                url: "https://example.com".into(),
                wait_for: WaitFor::Load,
                return_snapshot: false,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"), "msg: {}", err.message);
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn back_with_no_browser_suggests_browser_open() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = back(state, HistoryInput::default())
            .await
            .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn forward_with_no_browser_suggests_browser_open() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = forward(state, HistoryInput::default())
            .await
            .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn reload_with_no_browser_suggests_browser_open() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = reload(state, HistoryInput::default())
            .await
            .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn wait_for_idle_with_no_browser_suggests_browser_open() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = wait_for_idle(state, IdleInput::default())
            .await
            .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }
}
