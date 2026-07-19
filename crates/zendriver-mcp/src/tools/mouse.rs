//! Coordinate-mouse tool — `browser_mouse`.
//!
//! Wraps [`zendriver::Tab`]'s raw pointer API (`mouse_move`,
//! `mouse_click_with`, `mouse_drag`, `tap`) for canvas / drag-and-drop / map /
//! game interactions that aren't reachable through element-targeted action
//! tools. Coordinates are CSS pixels relative to the viewport.

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::ClickOptions;

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::actions::{ActionOutput, MouseButtonArg};
use crate::tools::common::{ModifierArg, current_tab, modifiers_to_bits, page_snapshot};

/// Which pointer operation `browser_mouse` performs.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    /// Move the cursor to `(x, y)` along a realistic Bezier path.
    Move,
    /// Click at `(x, y)`.
    Click,
    /// Press at `(x, y)`, drag to `(to_x, to_y)` over `steps`, release.
    Drag,
    /// Tap at `(x, y)`: a bare `touchStart`/`touchEnd` pair, no mouse
    /// events. Touch only — see [`zendriver::Tab::tap`]'s rustdoc for the
    /// touch-capability-emulation caveat.
    Tap,
}

/// Input for `browser_mouse`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MouseInput {
    /// Which pointer operation to perform.
    pub action: MouseAction,
    /// Target X in CSS pixels (viewport-relative). For `drag`, the start X.
    pub x: f64,
    /// Target Y in CSS pixels (viewport-relative). For `drag`, the start Y.
    pub y: f64,
    /// Drag destination X (required for `drag`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_x: Option<f64>,
    /// Drag destination Y (required for `drag`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_y: Option<f64>,
    /// Which button to dispatch (`click` only). Default `left`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<MouseButtonArg>,
    /// `clickCount` for `click` (set `2` for a double-click). Default `1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click_count: Option<u32>,
    /// Modifier keys held during a `click`. Default none.
    #[serde(default)]
    pub modifiers: Vec<ModifierArg>,
    /// Interpolation steps for `drag`. Default `20`.
    #[serde(default = "default_steps")]
    pub steps: usize,
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

fn default_steps() -> usize {
    20
}

/// Dispatch a coordinate-anchored pointer action on the current tab.
pub async fn mouse(
    state: Arc<Mutex<SessionState>>,
    input: MouseInput,
) -> Result<ActionOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    match input.action {
        MouseAction::Move => tab
            .mouse_move(input.x, input.y)
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?,
        MouseAction::Click => {
            let opts = ClickOptions {
                button: input.button.unwrap_or_default().into(),
                click_count: input.click_count.unwrap_or(1),
                modifiers: modifiers_to_bits(&input.modifiers),
                ..Default::default()
            };
            tab.mouse_click_with(input.x, input.y, opts)
                .await
                .map_err(|e| map_error(McpServerError::from(e)))?;
        }
        MouseAction::Drag => {
            let to_x = input.to_x.ok_or_else(|| {
                ErrorData::invalid_params(
                    "`to_x` and `to_y` are required for action `drag`".to_string(),
                    None,
                )
            })?;
            let to_y = input.to_y.ok_or_else(|| {
                ErrorData::invalid_params(
                    "`to_x` and `to_y` are required for action `drag`".to_string(),
                    None,
                )
            })?;
            tab.mouse_drag((input.x, input.y), (to_x, to_y), input.steps)
                .await
                .map_err(|e| map_error(McpServerError::from(e)))?;
        }
        MouseAction::Tap => tab
            .tap(input.x, input.y)
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?,
    }
    let snapshot = if input.return_snapshot {
        Some(page_snapshot(&tab).await?)
    } else {
        None
    };
    Ok(ActionOutput { ok: true, snapshot })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mouse_with_no_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = mouse(
            state,
            MouseInput {
                action: MouseAction::Move,
                x: 10.0,
                y: 10.0,
                to_x: None,
                to_y: None,
                button: None,
                click_count: None,
                modifiers: Vec::new(),
                steps: 20,
                return_snapshot: false,
            },
        )
        .await
        .expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }
}
