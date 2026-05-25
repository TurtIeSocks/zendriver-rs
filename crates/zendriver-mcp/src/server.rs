//! rmcp server stack + tool router.
//!
//! Every `#[tool]` method on [`ZendriverServer`] is a one-liner that
//! delegates to a free async fn in [`crate::tools`]. The
//! [`#[tool_router(server_handler)]`][tr] form emits the `ServerHandler`
//! impl in one step — no separate `#[tool_handler]` block needed.
//!
//! Keeping the per-tool implementations in `tools/*.rs` (rather than
//! inline here) makes the surface easy to grow: a new tool group adds a
//! new module, a new wrapper here, and lands without touching the others.
//!
//! [tr]: rmcp::tool_router

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::transport::stdio;
use rmcp::{ErrorData, Json, ServiceExt, tool, tool_router};
use tokio::sync::Mutex;

use crate::state::SessionState;
use crate::tools::common::EmptyInput;
use crate::tools::{frames, lifecycle, navigation, stealth, tabs};

/// rmcp handler carrying the per-session [`SessionState`].
///
/// Clone is cheap (the only field is an `Arc<Mutex<_>>`).
#[derive(Clone)]
pub struct ZendriverServer {
    pub state: Arc<Mutex<SessionState>>,
}

#[tool_router(server_handler)]
impl ZendriverServer {
    /// Construct a server bound to the given session state.
    pub fn new(state: Arc<Mutex<SessionState>>) -> Self {
        Self { state }
    }

    // ---------- lifecycle ------------------------------------------------

    /// Launch Chrome with stealth defaults. Errors if a browser is
    /// already open in this session — call `browser_close` first.
    #[tool(
        name = "browser_open",
        description = "Launch Chrome with stealth defaults. Errors if a browser is already open in this session — call `browser_close` first."
    )]
    pub async fn browser_open(
        &self,
        Parameters(input): Parameters<lifecycle::OpenInput>,
    ) -> Result<Json<lifecycle::OpenOutput>, ErrorData> {
        lifecycle::open(self.state.clone(), input).await.map(Json)
    }

    /// Close the open browser. Idempotent — no error if no browser is open.
    #[tool(
        name = "browser_close",
        description = "Close the open browser. Idempotent — no error if no browser is open."
    )]
    pub async fn browser_close(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<lifecycle::CloseOutput>, ErrorData> {
        lifecycle::close(self.state.clone(), input).await.map(Json)
    }

    /// Report open browser + current tab + stealth profile.
    #[tool(
        name = "browser_status",
        description = "Report open browser + current tab + stealth profile."
    )]
    pub async fn browser_status(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<lifecycle::StatusOutput>, ErrorData> {
        lifecycle::status(self.state.clone(), input).await.map(Json)
    }

    // ---------- navigation -----------------------------------------------

    /// Navigate the current tab to a URL.
    #[tool(
        name = "browser_goto",
        description = "Navigate the current tab to a URL. `wait_for` selects load / idle / no wait."
    )]
    pub async fn browser_goto(
        &self,
        Parameters(input): Parameters<navigation::GotoInput>,
    ) -> Result<Json<navigation::NavOutput>, ErrorData> {
        navigation::goto(self.state.clone(), input).await.map(Json)
    }

    /// Step back one entry in the current tab's history.
    #[tool(
        name = "browser_back",
        description = "Step back one entry in the current tab's history."
    )]
    pub async fn browser_back(
        &self,
        Parameters(input): Parameters<navigation::HistoryInput>,
    ) -> Result<Json<navigation::NavOutput>, ErrorData> {
        navigation::back(self.state.clone(), input).await.map(Json)
    }

    /// Step forward one entry in the current tab's history.
    #[tool(
        name = "browser_forward",
        description = "Step forward one entry in the current tab's history."
    )]
    pub async fn browser_forward(
        &self,
        Parameters(input): Parameters<navigation::HistoryInput>,
    ) -> Result<Json<navigation::NavOutput>, ErrorData> {
        navigation::forward(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Reload the current tab.
    #[tool(name = "browser_reload", description = "Reload the current tab.")]
    pub async fn browser_reload(
        &self,
        Parameters(input): Parameters<navigation::HistoryInput>,
    ) -> Result<Json<navigation::NavOutput>, ErrorData> {
        navigation::reload(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Wait until the current tab's network has been idle for ~500ms,
    /// bounded by `timeout_ms`.
    #[tool(
        name = "browser_wait_for_idle",
        description = "Wait until the current tab's network has been idle for ~500ms, bounded by `timeout_ms`."
    )]
    pub async fn browser_wait_for_idle(
        &self,
        Parameters(input): Parameters<navigation::IdleInput>,
    ) -> Result<Json<navigation::IdleOutput>, ErrorData> {
        navigation::wait_for_idle(self.state.clone(), input)
            .await
            .map(Json)
    }

    // ---------- tabs -----------------------------------------------------

    /// Enumerate all live tabs.
    #[tool(
        name = "browser_tab_list",
        description = "Enumerate all live tabs. Each entry includes the CDP target id, URL, title, and an `is_current` flag relative to the session's focused tab."
    )]
    pub async fn browser_tab_list(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<tabs::TabListOutput>, ErrorData> {
        tabs::list(self.state.clone(), input).await.map(Json)
    }

    /// Open a new tab.
    #[tool(
        name = "browser_tab_new",
        description = "Open a new tab. `url` selects the initial URL (defaults to about:blank). When `activate` is true (the default) the new tab becomes the session's current tab."
    )]
    pub async fn browser_tab_new(
        &self,
        Parameters(input): Parameters<tabs::TabNewInput>,
    ) -> Result<Json<tabs::TabSummary>, ErrorData> {
        tabs::new_tab(self.state.clone(), input).await.map(Json)
    }

    /// Switch the session's current tab.
    #[tool(
        name = "browser_tab_switch",
        description = "Switch the session's current tab to the given `tab_id`. Use `browser_tab_list` to enumerate available ids."
    )]
    pub async fn browser_tab_switch(
        &self,
        Parameters(input): Parameters<tabs::TabSwitchInput>,
    ) -> Result<Json<tabs::TabSummary>, ErrorData> {
        tabs::switch(self.state.clone(), input).await.map(Json)
    }

    /// Close a tab.
    #[tool(
        name = "browser_tab_close",
        description = "Close a tab. When `tab_id` is omitted, the session's current tab is closed; if that was the focused tab, focus falls back to one of the remaining tabs (or `None` if none remain)."
    )]
    pub async fn browser_tab_close(
        &self,
        Parameters(input): Parameters<tabs::TabCloseInput>,
    ) -> Result<Json<tabs::TabCloseOutput>, ErrorData> {
        tabs::close(self.state.clone(), input).await.map(Json)
    }

    /// Bring a tab to the foreground.
    #[tool(
        name = "browser_tab_activate",
        description = "Bring `tab_id` to the foreground in Chrome (sends Target.activateTarget). Also updates the session's current tab so subsequent calls target the foregrounded tab."
    )]
    pub async fn browser_tab_activate(
        &self,
        Parameters(input): Parameters<tabs::TabActivateInput>,
    ) -> Result<Json<tabs::TabActivateOutput>, ErrorData> {
        tabs::activate(self.state.clone(), input).await.map(Json)
    }

    // ---------- frames ---------------------------------------------------

    /// Enumerate all frames on the current tab.
    #[tool(
        name = "browser_frame_list",
        description = "Enumerate all frames on the current tab. Each entry includes the frame id, URL, parent id (None for the main frame), optional `name`, and `is_main` flag."
    )]
    pub async fn browser_frame_list(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<frames::FrameListOutput>, ErrorData> {
        frames::list(self.state.clone(), input).await.map(Json)
    }

    // ---------- stealth --------------------------------------------------

    /// Configure the session's default stealth profile.
    #[tool(
        name = "browser_set_stealth_profile",
        description = "Configure the session's default stealth profile. NOTE: takes effect on the NEXT `browser_open` call; does NOT re-fingerprint an already-open browser. Call `browser_close` + `browser_open` to apply live."
    )]
    pub async fn browser_set_stealth_profile(
        &self,
        Parameters(input): Parameters<stealth::SetStealthProfileInput>,
    ) -> Result<Json<stealth::SetStealthProfileOutput>, ErrorData> {
        stealth::set_stealth_profile(self.state.clone(), input)
            .await
            .map(Json)
    }
}

/// Build a fresh server handler bound to the given state.
pub fn build_handler(state: Arc<Mutex<SessionState>>) -> ZendriverServer {
    ZendriverServer::new(state)
}

/// Run the MCP server over stdio until the peer disconnects.
pub async fn run_stdio(state: Arc<Mutex<SessionState>>) -> Result<(), Box<dyn std::error::Error>> {
    let handler = build_handler(state);
    let service = handler.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StealthProfileChoice;

    #[tokio::test]
    async fn browser_status_with_no_browser_reports_closed() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let server = build_handler(state);
        let out = server
            .browser_status(Parameters(EmptyInput {}))
            .await
            .expect("status call");
        let body = out.0;
        assert!(!body.open);
        assert_eq!(body.tab_count, 0);
        assert!(body.current_tab.is_none());
        assert_eq!(body.profile, StealthProfileChoice::Auto);
    }
}
