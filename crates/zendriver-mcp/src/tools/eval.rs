//! JS evaluation tools — `browser_evaluate`, `browser_evaluate_main`.
//!
//! The two flavors differ only in execution world:
//! - [`evaluate`] → [`zendriver::Tab::evaluate`] (isolated world). Default
//!   choice. Page globals set by page scripts are *not* visible, so
//!   stealth fingerprint shims remain hidden from the page.
//! - [`evaluate_main`] → [`zendriver::Tab::evaluate_main`] (main world).
//!   Page globals are visible. Required when you must call functions the
//!   page itself defined; **breaks stealth isolation** if used carelessly
//!   because the page can observe the call.
//!
//! ## `await_promise` arg
//!
//! Currently observational — the lib's `evaluate*` methods send
//! `awaitPromise: true` to CDP unconditionally (see
//! `crates/zendriver/src/tab.rs:684` and `:757`). We surface the flag so
//! the schema is stable when the lib gains a "don't await" variant; for
//! now it's a documented no-op.
//!
//! ## `frame_id` routing
//!
//! When set, dispatches through [`zendriver::Frame::evaluate`] /
//! `evaluate_main` instead of the tab-level helpers. The OOPIF case still
//! works — `Frame::evaluate_main` lands in the frame's own main world.

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::common::{current_tab, lookup_frame};

/// Input for `browser_evaluate` / `browser_evaluate_main`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvalInput {
    /// JavaScript expression (NOT a statement block). Examples: `"1 + 2"`,
    /// `"document.title"`. For multi-line logic, wrap in an IIFE:
    /// `"(() => { /* ... */ return result; })()"`.
    pub expression: String,
    /// If the expression resolves to a promise, await it before returning
    /// the value. Default `true`. Currently observational — the lib always
    /// awaits promises in `Runtime.evaluate`.
    #[serde(default = "default_await")]
    pub await_promise: bool,
    /// When set, evaluate inside the specified frame instead of the tab's
    /// main frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
}

const fn default_await() -> bool {
    true
}

/// Output of `browser_evaluate` / `browser_evaluate_main`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EvalOutput {
    /// Whatever the expression returned, serialized as JSON. `undefined` →
    /// JSON `null`.
    pub value: serde_json::Value,
}

/// Evaluate `expression` in the page's **isolated** world.
///
/// Preferred over [`evaluate_main`] for everything that doesn't require
/// page globals — stays invisible to the page's own JS.
pub async fn evaluate(
    state: Arc<Mutex<SessionState>>,
    input: EvalInput,
) -> Result<EvalOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let value: serde_json::Value = if let Some(fid) = input.frame_id.as_deref() {
        let frame = lookup_frame(&tab, fid).await?;
        frame
            .evaluate(&input.expression)
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?
    } else {
        tab.evaluate(&input.expression)
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?
    };
    Ok(EvalOutput { value })
}

/// Evaluate `expression` in the page's **main** world.
///
/// Page globals are visible (and the page can observe the eval). Use only
/// when isolated-world semantics don't fit — for stealth-sensitive flows
/// prefer [`evaluate`].
pub async fn evaluate_main(
    state: Arc<Mutex<SessionState>>,
    input: EvalInput,
) -> Result<EvalOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let value: serde_json::Value = if let Some(fid) = input.frame_id.as_deref() {
        let frame = lookup_frame(&tab, fid).await?;
        frame
            .evaluate_main(&input.expression)
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?
    } else {
        tab.evaluate_main(&input.expression)
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?
    };
    Ok(EvalOutput { value })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Arc<Mutex<SessionState>> {
        Arc::new(Mutex::new(SessionState::new()))
    }

    #[tokio::test]
    async fn evaluate_with_no_browser_suggests_browser_open() {
        let err = evaluate(
            fresh(),
            EvalInput {
                expression: "1 + 2".into(),
                await_promise: true,
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
    async fn evaluate_main_with_no_browser_suggests_browser_open() {
        let err = evaluate_main(
            fresh(),
            EvalInput {
                expression: "document.title".into(),
                await_promise: true,
                frame_id: None,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"), "msg: {}", err.message);
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }
}
