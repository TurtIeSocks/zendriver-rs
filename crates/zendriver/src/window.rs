//! OS-window bounds + state control for a [`Tab`].
//!
//! Chrome models each top-level page inside an OS window with a position,
//! size, and a window state (normal / minimized / maximized / fullscreen).
//! These live in the `Browser` CDP domain, addressed by `windowId` — which is
//! itself resolved from the tab's `targetId` via
//! `Browser.getWindowForTarget`. Because they are `Browser`-domain commands,
//! they dispatch at **browser scope** (no `sessionId`), exactly like
//! [`Tab::activate`].
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! # let browser = zendriver::Browser::builder().launch().await?;
//! # let tab = browser.main_tab();
//! tab.set_window_size(1280, 800).await?;
//! tab.maximize().await?;
//! # Ok(()) }
//! ```
//!
//! ## CDP state-change rule
//!
//! `Browser.setWindowBounds` rejects a payload that mixes a non-`normal`
//! `windowState` with explicit geometry. So [`Tab::maximize`] /
//! [`Tab::minimize`] / [`Tab::fullscreen`] send `bounds: { windowState }`
//! **alone**, while [`Tab::set_window_size`] sends `bounds: { width, height }`
//! with the state left implicit (normal). [`Tab::set_window_bounds`] honors
//! whatever [`WindowBounds`] you hand it, so it is your responsibility there
//! not to combine a state change with geometry.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::error::{Result, ZendriverError};
use crate::tab::Tab;

/// State of the OS window hosting a tab.
///
/// Serializes to the CDP `Browser.WindowState` wire strings
/// (`"normal"` / `"minimized"` / `"maximized"` / `"fullscreen"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowState {
    /// Normal (windowed) — the only state that may carry explicit geometry.
    Normal,
    /// Minimized to the OS taskbar / dock.
    Minimized,
    /// Maximized to fill the screen work area.
    Maximized,
    /// Fullscreen (chrome / OS decorations hidden).
    Fullscreen,
}

impl WindowState {
    /// CDP wire string for this state.
    ///
    /// Cheap (`&'static str`); mirrors [`crate::Format::as_cdp`].
    ///
    /// # Examples
    ///
    /// ```
    /// use zendriver::WindowState;
    /// assert_eq!(WindowState::Maximized.as_cdp(), "maximized");
    /// assert_eq!(WindowState::Normal.as_cdp(), "normal");
    /// ```
    #[must_use]
    pub fn as_cdp(&self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Minimized => "minimized",
            Self::Maximized => "maximized",
            Self::Fullscreen => "fullscreen",
        }
    }
}

/// Position, size, and state of the OS window hosting a tab.
///
/// All geometry fields are optional: `None` means "leave unchanged" on a
/// [`Tab::set_window_bounds`] write, and "not reported" on a
/// [`Tab::window_bounds`] read (Chrome omits geometry for minimized windows).
/// Coordinates and dimensions are device-independent pixels.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowBounds {
    /// Window left edge (screen X), in DIP.
    pub left: Option<i64>,
    /// Window top edge (screen Y), in DIP.
    pub top: Option<i64>,
    /// Window width, in DIP.
    pub width: Option<i64>,
    /// Window height, in DIP.
    pub height: Option<i64>,
    /// Window state. `None` leaves it unchanged on a write.
    pub state: Option<WindowState>,
}

impl WindowBounds {
    /// Serialize to a CDP `Browser.Bounds` JSON object.
    ///
    /// Only the set fields are emitted; `state` becomes `windowState`.
    fn to_cdp(&self) -> Value {
        let mut bounds = Map::new();
        if let Some(v) = self.left {
            bounds.insert("left".to_string(), json!(v));
        }
        if let Some(v) = self.top {
            bounds.insert("top".to_string(), json!(v));
        }
        if let Some(v) = self.width {
            bounds.insert("width".to_string(), json!(v));
        }
        if let Some(v) = self.height {
            bounds.insert("height".to_string(), json!(v));
        }
        if let Some(state) = self.state {
            bounds.insert(
                "windowState".to_string(),
                Value::String(state.as_cdp().into()),
            );
        }
        Value::Object(bounds)
    }

    /// Parse a CDP `Browser.Bounds` JSON object into a [`WindowBounds`].
    fn from_cdp(v: &Value) -> Self {
        Self {
            left: v.get("left").and_then(Value::as_i64),
            top: v.get("top").and_then(Value::as_i64),
            width: v.get("width").and_then(Value::as_i64),
            height: v.get("height").and_then(Value::as_i64),
            state: v
                .get("windowState")
                .and_then(Value::as_str)
                .and_then(|s| match s {
                    "normal" => Some(WindowState::Normal),
                    "minimized" => Some(WindowState::Minimized),
                    "maximized" => Some(WindowState::Maximized),
                    "fullscreen" => Some(WindowState::Fullscreen),
                    _ => None,
                }),
        }
    }
}

impl Tab {
    /// Resolve the CDP `windowId` for this tab's target.
    ///
    /// Browser-scope `Browser.getWindowForTarget { targetId }` (no
    /// `sessionId`), mirroring [`Tab::activate`]'s dispatch.
    async fn window_id(&self) -> Result<i64> {
        let target_id = self.target_id().to_string();
        let res = self
            .inner
            .session
            .connection()
            .call_raw(
                "Browser.getWindowForTarget",
                json!({ "targetId": target_id }),
                None,
            )
            .await?;
        res.get("windowId").and_then(Value::as_i64).ok_or_else(|| {
            ZendriverError::Navigation("Browser.getWindowForTarget returned no windowId".into())
        })
    }

    /// Read this tab's current OS-window bounds + state.
    ///
    /// Sends `Browser.getWindowForTarget { targetId }` at browser scope and
    /// parses the response's `bounds`. Minimized windows may omit geometry
    /// (those fields come back as `None`).
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] when Chrome's response is
    /// missing `windowId` or `bounds`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let bounds = tab.window_bounds().await?;
    /// println!("{:?}", bounds.state);
    /// # Ok(()) }
    /// ```
    pub async fn window_bounds(&self) -> Result<WindowBounds> {
        let target_id = self.target_id().to_string();
        let res = self
            .inner
            .session
            .connection()
            .call_raw(
                "Browser.getWindowForTarget",
                json!({ "targetId": target_id }),
                None,
            )
            .await?;
        let bounds = res.get("bounds").ok_or_else(|| {
            ZendriverError::Navigation("Browser.getWindowForTarget returned no bounds".into())
        })?;
        Ok(WindowBounds::from_cdp(bounds))
    }

    /// Set this tab's OS-window bounds + state.
    ///
    /// Resolves the `windowId` (via `Browser.getWindowForTarget`), then sends
    /// `Browser.setWindowBounds { windowId, bounds }` at browser scope. The
    /// `bounds` object carries only the [`WindowBounds`] fields you set.
    ///
    /// Per the CDP contract, do **not** combine a non-`normal`
    /// [`WindowState`] with explicit geometry in a single call — use the
    /// dedicated [`Tab::maximize`] / [`Tab::minimize`] / [`Tab::fullscreen`]
    /// helpers for state changes and [`Tab::set_window_size`] for sizing.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] when the `windowId` lookup
    /// fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::WindowBounds;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.set_window_bounds(WindowBounds {
    ///     left: Some(0),
    ///     top: Some(0),
    ///     width: Some(1024),
    ///     height: Some(768),
    ///     state: None,
    /// }).await?;
    /// # Ok(()) }
    /// ```
    pub async fn set_window_bounds(&self, bounds: WindowBounds) -> Result<()> {
        let window_id = self.window_id().await?;
        self.inner
            .session
            .connection()
            .call_raw(
                "Browser.setWindowBounds",
                json!({ "windowId": window_id, "bounds": bounds.to_cdp() }),
                None,
            )
            .await?;
        Ok(())
    }

    /// Resize this tab's OS window to `width` × `height` DIP.
    ///
    /// Sends `bounds: { width, height }` with the window state implicitly
    /// `normal` — no `windowState` key — so it never trips the CDP
    /// state-vs-geometry rule.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.set_window_size(1280, 800).await?;
    /// # Ok(()) }
    /// ```
    pub async fn set_window_size(&self, width: i64, height: i64) -> Result<()> {
        self.set_window_bounds(WindowBounds {
            width: Some(width),
            height: Some(height),
            ..Default::default()
        })
        .await
    }

    /// Maximize this tab's OS window.
    ///
    /// Sends `bounds: { windowState: "maximized" }` alone (no geometry), per
    /// the CDP state-change rule.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.maximize().await?;
    /// # Ok(()) }
    /// ```
    pub async fn maximize(&self) -> Result<()> {
        self.set_window_state(WindowState::Maximized).await
    }

    /// Minimize this tab's OS window.
    ///
    /// Sends `bounds: { windowState: "minimized" }` alone (no geometry), per
    /// the CDP state-change rule.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.minimize().await?;
    /// # Ok(()) }
    /// ```
    pub async fn minimize(&self) -> Result<()> {
        self.set_window_state(WindowState::Minimized).await
    }

    /// Put this tab's OS window into fullscreen.
    ///
    /// Sends `bounds: { windowState: "fullscreen" }` alone (no geometry), per
    /// the CDP state-change rule.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.fullscreen().await?;
    /// # Ok(()) }
    /// ```
    pub async fn fullscreen(&self) -> Result<()> {
        self.set_window_state(WindowState::Fullscreen).await
    }

    /// Send a bare `windowState`-only bounds change.
    ///
    /// Shared by [`Tab::maximize`] / [`Tab::minimize`] / [`Tab::fullscreen`]:
    /// the `bounds` object carries exactly one key, `windowState`, satisfying
    /// the CDP rule that a state transition must not be mixed with geometry.
    async fn set_window_state(&self, state: WindowState) -> Result<()> {
        self.set_window_bounds(WindowBounds {
            state: Some(state),
            ..Default::default()
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

    /// `window_bounds()` dispatches `Browser.getWindowForTarget { targetId }`
    /// at browser scope (no sessionId) and parses the mocked windowId +
    /// bounds into a [`WindowBounds`].
    #[tokio::test]
    async fn window_bounds_dispatches_get_window_for_target() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S42");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.window_bounds().await }
        });

        let id = mock.expect_cmd("Browser.getWindowForTarget").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["targetId"], "test-target-S42");
        // Browser-scope command — no session_id.
        assert!(sent.get("sessionId").is_none());
        mock.reply(
            id,
            json!({
                "windowId": 7,
                "bounds": { "left": 0, "top": 0, "width": 1280, "height": 800, "windowState": "normal" },
            }),
        )
        .await;

        let bounds = fut.await.unwrap().unwrap();
        assert_eq!(bounds.width, Some(1280));
        assert_eq!(bounds.height, Some(800));
        assert_eq!(bounds.state, Some(WindowState::Normal));
        conn.shutdown();
    }

    /// `maximize()` first resolves the windowId, then sends
    /// `Browser.setWindowBounds` whose `bounds` has `windowState: "maximized"`
    /// and NO geometry keys.
    #[tokio::test]
    async fn maximize_sends_only_window_state() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.maximize().await }
        });

        // Step 1: windowId lookup.
        let lookup_id = mock.expect_cmd("Browser.getWindowForTarget").await;
        mock.reply(lookup_id, json!({ "windowId": 3, "bounds": {} }))
            .await;

        // Step 2: setWindowBounds with windowState-only bounds.
        let set_id = mock.expect_cmd("Browser.setWindowBounds").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["windowId"], 3);
        let bounds = &sent["params"]["bounds"];
        assert_eq!(bounds["windowState"], "maximized");
        // CDP rule: a state change must carry no geometry.
        assert!(bounds.get("width").is_none());
        assert!(bounds.get("height").is_none());
        assert!(bounds.get("left").is_none());
        assert!(bounds.get("top").is_none());
        // Browser-scope command — no session_id.
        assert!(sent.get("sessionId").is_none());
        mock.reply(set_id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    /// `set_window_size(w, h)` sends `bounds: { width, height }` with NO
    /// `windowState` (implicitly normal), satisfying the CDP geometry rule.
    #[tokio::test]
    async fn set_window_size_sends_dimensions() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.set_window_size(1024, 768).await }
        });

        let lookup_id = mock.expect_cmd("Browser.getWindowForTarget").await;
        mock.reply(lookup_id, json!({ "windowId": 9, "bounds": {} }))
            .await;

        let set_id = mock.expect_cmd("Browser.setWindowBounds").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["windowId"], 9);
        let bounds = &sent["params"]["bounds"];
        assert_eq!(bounds["width"], 1024);
        assert_eq!(bounds["height"], 768);
        // Sizing must not carry a windowState key.
        assert!(bounds.get("windowState").is_none());
        mock.reply(set_id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }
}
