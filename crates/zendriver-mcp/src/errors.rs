//! MCP-layer errors + mapping to rmcp's wire format.
//!
//! Two error kinds:
//! - [`McpServerError`] — MCP-server-specific concerns that don't exist in
//!   the zendriver lib (e.g. "browser not open", "expectation not found").
//!   These wrap a [`zendriver::ZendriverError`] when the underlying cause
//!   is a lib failure.
//! - [`rmcp::ErrorData`] — wire format expected by rmcp. Built by
//!   [`map_error`], which also attaches `_meta.suggested_next` hints so an
//!   agent client knows which tool to call to recover.

use rmcp::ErrorData;
use serde_json::json;
use zendriver::ZendriverError;

/// MCP-server-layer error.
///
/// Carries either an MCP-specific failure or a wrapped zendriver lib error.
#[derive(Debug, thiserror::Error)]
pub enum McpServerError {
    /// `browser_open` has not been called yet (or `browser_close` already ran).
    #[error("Browser not open. Call `browser_open` first.")]
    BrowserNotOpen,

    /// `browser_open` was called while a Browser is already attached to this
    /// session.
    #[error("Browser already open. Call `browser_close` first.")]
    BrowserAlreadyOpen,

    /// `current_tab_id` is `None` or doesn't resolve to any live tab.
    #[error("No current tab. Open a tab via `browser_tab_new` or `browser_open`.")]
    NoCurrentTab,

    /// `browser_expect_await` / `_cancel` was passed an unknown id.
    #[error("Expectation `{0}` not found. Did you call `browser_expect_register` first?")]
    ExpectationNotFound(String),

    /// `browser_intercept_remove_rule` was passed an unknown id.
    #[error("Intercept rule `{0}` not found.")]
    RuleNotFound(String),

    /// Wrapped lib failure.
    #[error(transparent)]
    Zendriver(#[from] ZendriverError),
}

/// Map any [`McpServerError`]-convertible error into an rmcp wire error.
///
/// Attaches `_meta.suggested_next` (an MCP-spec metadata field) pointing
/// the agent at the tool it should likely call next.
pub fn map_error(err: impl Into<McpServerError>) -> ErrorData {
    let err: McpServerError = err.into();
    let (msg, suggested_next) = match &err {
        McpServerError::BrowserNotOpen => (err.to_string(), Some("browser_open")),
        McpServerError::BrowserAlreadyOpen => (err.to_string(), Some("browser_close")),
        McpServerError::NoCurrentTab => (err.to_string(), Some("browser_tab_new")),
        McpServerError::ExpectationNotFound(_) => {
            (err.to_string(), Some("browser_expect_register"))
        }
        McpServerError::RuleNotFound(_) => (err.to_string(), Some("browser_intercept_add_rule")),
        McpServerError::Zendriver(ze) => map_zendriver(ze),
    };
    let data = suggested_next.map(|hint| json!({ "suggested_next": hint }));
    ErrorData::invalid_request(msg, data)
}

fn map_zendriver(err: &ZendriverError) -> (String, Option<&'static str>) {
    match err {
        ZendriverError::ElementNotFound { selector } => (
            format!(
                "No element matched `{selector}`. Try `browser_snapshot` or `browser_html` to inspect current page."
            ),
            Some("browser_snapshot"),
        ),
        ZendriverError::Timeout(d) => (
            format!(
                "Operation timed out after {d:?}. Retry with a larger `timeout_ms` or inspect with `browser_snapshot`."
            ),
            Some("browser_snapshot"),
        ),
        ZendriverError::NotActionable(_, reason) => (
            format!(
                "Element not actionable: {reason}. Inspect with `browser_snapshot` or wait for the page to settle."
            ),
            Some("browser_snapshot"),
        ),
        ZendriverError::TabNotFound(id) => (
            format!("Tab `{id}` not found. Use `browser_tab_list` to enumerate live tabs."),
            Some("browser_tab_list"),
        ),
        ZendriverError::FrameNotFound(id) => (
            format!("Frame `{id}` not found. Use `browser_frame_list` to enumerate frames."),
            Some("browser_frame_list"),
        ),
        ZendriverError::Navigation(msg) => (format!("Navigation failed: {msg}"), None),
        ZendriverError::JsException(msg) => (
            format!("JavaScript exception during evaluation: {msg}"),
            None,
        ),
        ZendriverError::ElementStale => (
            "Element handle is stale. Re-run the find before retrying.".into(),
            Some("browser_find"),
        ),
        ZendriverError::NotRefreshable => (
            "Element handle is not refreshable (came from raw JS eval). Re-find it with a selector.".into(),
            Some("browser_find"),
        ),
        _ => (err.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_not_open_suggests_browser_open() {
        let e = map_error(McpServerError::BrowserNotOpen);
        assert!(e.message.contains("browser_open"));
        let data = e.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[test]
    fn browser_already_open_suggests_browser_close() {
        let e = map_error(McpServerError::BrowserAlreadyOpen);
        assert!(e.message.contains("browser_close"));
        let data = e.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_close");
    }

    #[test]
    fn element_not_found_suggests_snapshot() {
        let inner = ZendriverError::ElementNotFound {
            selector: "css(button.primary)".into(),
        };
        let e = map_error(McpServerError::from(inner));
        assert!(e.message.contains("`css(button.primary)`"));
        let data = e.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_snapshot");
    }

    #[test]
    fn from_zendriver_error_lets_question_mark_work() {
        // Smoke: `?` operator goes ZendriverError -> McpServerError -> ErrorData.
        fn inner() -> Result<(), McpServerError> {
            let z: zendriver::Result<()> = Err(ZendriverError::TabNotFound("T9".into()));
            z?;
            Ok(())
        }
        let e = map_error(inner().unwrap_err());
        assert_eq!(
            e.data.as_ref().unwrap()["suggested_next"],
            "browser_tab_list"
        );
    }
}
