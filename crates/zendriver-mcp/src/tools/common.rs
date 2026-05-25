//! Shared helpers used by multiple tool modules.
//!
//! - [`EmptyInput`] — JSON-schema-bearing placeholder for tools that take no
//!   arguments.
//! - [`current_tab`] — resolves the current `zendriver::Tab` from a locked
//!   [`SessionState`]. Returns an owned (cheap-to-clone) handle so callers
//!   don't fight the borrow checker against `Browser::tabs(&self).await`'s
//!   `Vec<Tab>`.

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;

/// Placeholder input struct for tools with no arguments.
///
/// Required so rmcp can synthesize a JSON schema for the tool's args (an
/// absent arg block would yield `null` schema).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyInput {}

/// Resolve the currently-focused tab.
///
/// Returns an owned [`zendriver::Tab`] — `Tab` is an `Arc`-backed handle so
/// cloning is cheap. The function takes ownership of the search rather than
/// borrowing into the `Vec<Tab>` from `Browser::tabs()` because that vec
/// gets dropped at the end of the function and we can't hand back a borrow.
///
/// # Errors
///
/// - [`McpServerError::BrowserNotOpen`] if `state.browser` is `None`.
/// - [`McpServerError::NoCurrentTab`] if no tab matches
///   `state.current_tab_id` (or `current_tab_id` is unset).
pub async fn current_tab(s: &SessionState) -> Result<zendriver::Tab, ErrorData> {
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    let id = s
        .current_tab_id
        .as_deref()
        .ok_or_else(|| map_error(McpServerError::NoCurrentTab))?;
    let tabs = b.tabs().await;
    tabs.into_iter()
        .find(|t| t.target_id() == id)
        .ok_or_else(|| map_error(McpServerError::NoCurrentTab))
}
