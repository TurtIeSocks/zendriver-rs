//! rmcp server stack + tool router.
//!
//! Tools live as `#[tool]` methods on [`ZendriverServer`]. The
//! [`#[tool_router(server_handler)]`][tr] form emits the `ServerHandler`
//! impl in one step — no separate `#[tool_handler]` block needed.
//!
//! When the tool surface grows past a single screen of methods the
//! per-method bodies should be extracted into `tools/*.rs` async fns and
//! the `#[tool]` methods reduced to one-line delegations (see plan
//! "API Reality" section). For now (`browser_status` only) the body
//! lives inline.
//!
//! [tr]: rmcp::tool_router

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::transport::stdio;
use rmcp::{ErrorData, Json, ServiceExt, schemars, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::state::{SessionState, StealthProfileChoice};

/// rmcp handler with one tool: `browser_status`.
///
/// Additional tools land in follow-up dispatches.
#[derive(Clone)]
pub struct ZendriverServer {
    pub state: Arc<Mutex<SessionState>>,
}

/// Empty input struct — required so rmcp can synthesize a JSON schema for
/// the tool's args (an absent arg block would yield `null` schema).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyInput {}

/// Lightweight summary of the current tab — returned inside [`StatusOutput`].
#[derive(Debug, Serialize, JsonSchema)]
pub struct TabSummary {
    pub id: String,
    pub url: String,
    pub title: String,
}

/// Output of `browser_status`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StatusOutput {
    /// `true` iff a Browser is currently launched in this session.
    pub open: bool,
    /// Number of live tabs (0 when no browser is open).
    pub tab_count: usize,
    /// `id` / `url` / `title` of the currently-focused tab, or `null`.
    pub current_tab: Option<TabSummary>,
    /// Configured stealth profile choice for this session.
    pub profile: StealthProfileChoice,
}

#[tool_router(server_handler)]
impl ZendriverServer {
    /// Construct a server bound to the given session state.
    pub fn new(state: Arc<Mutex<SessionState>>) -> Self {
        Self { state }
    }

    /// Report whether a browser is open in this session, the current tab
    /// (if any), and the configured stealth profile.
    #[tool(
        name = "browser_status",
        description = "Report open browser + current tab + stealth profile."
    )]
    pub async fn browser_status(
        &self,
        _: Parameters<EmptyInput>,
    ) -> Result<Json<StatusOutput>, ErrorData> {
        let s = self.state.lock().await;
        let Some(b) = s.browser.as_ref() else {
            return Ok(Json(StatusOutput {
                open: false,
                tab_count: 0,
                current_tab: None,
                profile: s.stealth_profile_choice,
            }));
        };
        let tabs = b.tabs().await;
        let current_tab = match &s.current_tab_id {
            Some(id) => {
                let mut found = None;
                for t in &tabs {
                    if t.target_id() == id {
                        let url = t.url().await.map(|u| u.to_string()).unwrap_or_default();
                        let title = t.title().await.unwrap_or_default();
                        found = Some(TabSummary {
                            id: t.target_id().to_string(),
                            url,
                            title,
                        });
                        break;
                    }
                }
                found
            }
            None => None,
        };
        Ok(Json(StatusOutput {
            open: true,
            tab_count: tabs.len(),
            current_tab,
            profile: s.stealth_profile_choice,
        }))
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
