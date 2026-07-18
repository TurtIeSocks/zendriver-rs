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
use zendriver::{IdleLossPolicy, IdleOptions, ReadyState, ReloadOptions};

use crate::errors::{McpServerError, map_error};
use crate::snapshot::html_trim;
use crate::state::SessionState;
use crate::tools::actions::AckOutput;
use crate::tools::common::{EmptyInput, current_tab, lookup_frame};

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

/// Arg block for `browser_reload`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReloadInput {
    /// Bypass the HTTP cache (hard reload) when `true`. Default `false`
    /// (normal soft reload).
    #[serde(default)]
    pub ignore_cache: bool,
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

/// Reload the current page, optionally bypassing the cache.
pub async fn reload(
    state: Arc<Mutex<SessionState>>,
    input: ReloadInput,
) -> Result<NavOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    if input.ignore_cache {
        tab.reload_with(ReloadOptions {
            ignore_cache: true,
            ..ReloadOptions::default()
        })
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    } else {
        tab.reload()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
    }
    nav_output(&tab, input.return_snapshot).await
}

/// Document-ready milestone `browser_wait_for_load` can target.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadyStateArg {
    /// `document.readyState === "interactive"` — DOM parsed, sub-resources
    /// may still be loading.
    Interactive,
    /// `document.readyState === "complete"` — the `load` event has fired.
    Complete,
}

impl From<ReadyStateArg> for ReadyState {
    fn from(r: ReadyStateArg) -> Self {
        match r {
            ReadyStateArg::Interactive => ReadyState::Interactive,
            ReadyStateArg::Complete => ReadyState::Complete,
        }
    }
}

/// Input for `browser_wait_for_load`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitForLoadInput {
    /// Wait until the document reaches this `readyState`. When unset (and no
    /// `frame_id`), waits for the tab's `load` event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_state: Option<ReadyStateArg>,
    /// When set, wait for the given frame's load instead of the tab's main
    /// frame. `ready_state` is ignored in this case (frames expose only a
    /// load wait).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
}

/// Wait for a load milestone on the tab (or a specific frame).
pub async fn wait_for_load(
    state: Arc<Mutex<SessionState>>,
    input: WaitForLoadInput,
) -> Result<NavOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    if let Some(fid) = input.frame_id.as_deref() {
        let frame = lookup_frame(&tab, fid).await?;
        frame
            .wait_for_load()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
    } else if let Some(rs) = input.ready_state {
        tab.wait_for_ready_state(rs.into())
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
    } else {
        tab.wait_for_load()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
    }
    nav_output(&tab, false).await
}

/// Click through Chrome's "Your connection is not private" interstitial on
/// the current tab (the `thisisunsafe` bypass).
pub async fn bypass_insecure_warning(
    state: Arc<Mutex<SessionState>>,
    _: EmptyInput,
) -> Result<AckOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.bypass_insecure_connection_warning()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(AckOutput { ok: true })
}

// ---------- browser_wait_for_idle -----------------------------------------

/// Arg block for `browser_wait_for_idle`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdleInput {
    /// Outer timeout in milliseconds (default: 5000).
    #[serde(default = "default_idle_timeout")]
    pub timeout_ms: u64,
    /// Ignore requests in flight longer than this many milliseconds when
    /// judging idle — treat them as stuck/background (a hung beacon, long-poll,
    /// SSE stream, …) so idle can resolve even while one is still open. Omit to
    /// wait for every request to complete (the default).
    #[serde(default)]
    pub max_inflight_age_ms: Option<u64>,
    /// Abort the wait with an error if the underlying event stream loses
    /// delivery continuity (a lagging subscriber, a reconnect, or a dropped
    /// connection) while waiting. Default `false`: a delivery gap is
    /// tolerated and the wait still resolves on a best-effort basis.
    #[serde(default)]
    pub strict: bool,
}

const fn default_idle_timeout() -> u64 {
    5000
}

impl Default for IdleInput {
    fn default() -> Self {
        Self {
            timeout_ms: default_idle_timeout(),
            max_inflight_age_ms: None,
            strict: false,
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
/// `max_inflight_age_ms`, when set, lets idle resolve even while a stuck /
/// background request is still open. `strict`, when `true`, fails the call
/// instead of silently tolerating a delivery gap on the underlying event
/// stream.
pub async fn wait_for_idle(
    state: Arc<Mutex<SessionState>>,
    input: IdleInput,
) -> Result<IdleOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.wait_for_idle_opts(IdleOptions {
        timeout: Duration::from_millis(input.timeout_ms),
        quiet_window: Duration::from_millis(500),
        max_inflight_age: input.max_inflight_age_ms.map(Duration::from_millis),
        loss_policy: if input.strict {
            IdleLossPolicy::Strict
        } else {
            IdleLossPolicy::Lenient
        },
    })
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
        let err = reload(state, ReloadInput::default())
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
