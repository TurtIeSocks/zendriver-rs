//! Window / viewport tools — `browser_get_window`, `browser_set_window`.
//!
//! Wraps [`zendriver::Tab`]'s window controls (`window_bounds`,
//! `set_window_bounds`, `set_window_size`, `maximize`, `minimize`,
//! `fullscreen`). Geometry fields are device-independent pixels; Chrome omits
//! geometry for a minimized window, so every field on [`WindowBoundsDto`] is
//! optional.

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::{WindowBounds, WindowState};

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::common::current_tab;

/// Wire mirror of [`zendriver::WindowState`].
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowStateDto {
    /// Normal (windowed) — the only state that may carry explicit geometry.
    Normal,
    /// Minimized to the OS taskbar / dock.
    Minimized,
    /// Maximized to fill the screen work area.
    Maximized,
    /// Fullscreen (chrome / OS decorations hidden).
    Fullscreen,
}

impl From<WindowStateDto> for WindowState {
    fn from(s: WindowStateDto) -> Self {
        match s {
            WindowStateDto::Normal => WindowState::Normal,
            WindowStateDto::Minimized => WindowState::Minimized,
            WindowStateDto::Maximized => WindowState::Maximized,
            WindowStateDto::Fullscreen => WindowState::Fullscreen,
        }
    }
}

impl From<WindowState> for WindowStateDto {
    fn from(s: WindowState) -> Self {
        match s {
            WindowState::Normal => WindowStateDto::Normal,
            WindowState::Minimized => WindowStateDto::Minimized,
            WindowState::Maximized => WindowStateDto::Maximized,
            WindowState::Fullscreen => WindowStateDto::Fullscreen,
        }
    }
}

/// Window geometry + state DTO (output of both window tools).
#[derive(Debug, Serialize, JsonSchema)]
pub struct WindowBoundsDto {
    /// Window left edge (screen X), in DIP. `None` when not reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<i64>,
    /// Window top edge (screen Y), in DIP. `None` when not reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top: Option<i64>,
    /// Window width, in DIP. `None` when not reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    /// Window height, in DIP. `None` when not reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    /// Current window state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<WindowStateDto>,
}

impl From<WindowBounds> for WindowBoundsDto {
    fn from(b: WindowBounds) -> Self {
        Self {
            left: b.left,
            top: b.top,
            width: b.width,
            height: b.height,
            state: b.state.map(Into::into),
        }
    }
}

/// Read the current window bounds + state.
pub async fn get_window(state: Arc<Mutex<SessionState>>) -> Result<WindowBoundsDto, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let bounds = tab
        .window_bounds()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(bounds.into())
}

/// Which window operation `browser_set_window` performs.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetWindowMode {
    /// Set explicit bounds (`left`/`top`/`width`/`height`/`state`, any subset).
    Bounds,
    /// Resize to `width` × `height` (both required).
    Size,
    /// Maximize the window.
    Maximize,
    /// Minimize the window.
    Minimize,
    /// Enter fullscreen.
    Fullscreen,
}

/// Input for `browser_set_window`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetWindowInput {
    /// Which operation to perform.
    pub mode: SetWindowMode,
    /// Window width in DIP (used by `size` and optionally `bounds`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    /// Window height in DIP (used by `size` and optionally `bounds`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    /// Window left edge in DIP (used by `bounds`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<i64>,
    /// Window top edge in DIP (used by `bounds`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<i64>,
    /// Window state to set (used by `bounds`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<WindowStateDto>,
}

/// Apply a window operation, then report the resulting bounds.
pub async fn set_window(
    state: Arc<Mutex<SessionState>>,
    input: SetWindowInput,
) -> Result<WindowBoundsDto, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    match input.mode {
        SetWindowMode::Maximize => tab
            .maximize()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?,
        SetWindowMode::Minimize => tab
            .minimize()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?,
        SetWindowMode::Fullscreen => tab
            .fullscreen()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?,
        SetWindowMode::Size => {
            let w = input.width.ok_or_else(|| {
                ErrorData::invalid_params("`width` is required for mode `size`".to_string(), None)
            })?;
            let h = input.height.ok_or_else(|| {
                ErrorData::invalid_params("`height` is required for mode `size`".to_string(), None)
            })?;
            tab.set_window_size(w, h)
                .await
                .map_err(|e| map_error(McpServerError::from(e)))?;
        }
        SetWindowMode::Bounds => {
            let bounds = WindowBounds {
                left: input.left,
                top: input.top,
                width: input.width,
                height: input.height,
                state: input.state.map(Into::into),
            };
            tab.set_window_bounds(bounds)
                .await
                .map_err(|e| map_error(McpServerError::from(e)))?;
        }
    }
    let bounds = tab
        .window_bounds()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(bounds.into())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_window_with_no_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = set_window(
            state,
            SetWindowInput {
                mode: SetWindowMode::Maximize,
                width: None,
                height: None,
                left: None,
                top: None,
                state: None,
            },
        )
        .await
        .expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }
}
