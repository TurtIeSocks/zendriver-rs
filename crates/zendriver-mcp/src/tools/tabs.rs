//! Tab management handlers — `browser_tab_list`, `_new`, `_switch`,
//! `_close`, `_activate`.
//!
//! Each handler is a free async fn that locks the shared
//! [`SessionState`][crate::state::SessionState] internally and returns a
//! typed output (or an [`rmcp::ErrorData`]). The thin `#[tool]` wrappers in
//! [`crate::server`] forward to these.
//!
//! All tools use [`TabSummary`] as their per-tab projection. The `is_current`
//! flag is computed against `state.current_tab_id`, so a freshly-listed
//! `Vec<TabSummary>` always reflects the session's view of "the focused
//! tab" — there is no separate `tabs().current()` accessor on `Browser`.
//!
//! ## API notes
//!
//! - `Browser::tabs()` is async and returns `Vec<Tab>` (owned, cheap to
//!   clone — `Tab` is `Arc`-backed).
//! - `Browser::new_tab()` opens about:blank; `new_tab_at(url)` accepts an
//!   initial URL. Plan called for an `Option<String>` flag that branches
//!   between the two.
//! - `Tab::close(self)` consumes the receiver — we fetch the tab out of
//!   `Browser::tabs()` and call `.close()` on the owned handle.
//! - `Tab::activate(&self)` borrows. No consumption; the tab stays usable.
//! - `Browser::tabs()` order is unspecified (HashMap-backed) — we lean on
//!   `target_id` for lookup, never positional indexing.

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::common::EmptyInput;

// ---------- shared types --------------------------------------------------

/// Per-tab projection returned by `browser_tab_list` + the create/switch
/// tools.
///
/// `is_current` is computed against `SessionState::current_tab_id` at the
/// time of the call. Returning it inline (rather than expecting the client
/// to cross-reference against a separate "current tab" field) keeps the
/// list response self-contained.
#[derive(Debug, Serialize, JsonSchema, PartialEq, Eq)]
pub struct TabSummary {
    /// Stable CDP target id. Use this with `browser_tab_switch` /
    /// `_close` / `_activate`.
    pub id: String,
    /// Live tab URL (best-effort; empty string on transient lookup
    /// failure, e.g. a tab that's in the middle of closing).
    pub url: String,
    /// Live tab title (best-effort; empty string on transient lookup
    /// failure).
    pub title: String,
    /// `true` iff this tab is the session's current focused tab.
    pub is_current: bool,
}

/// Build a [`TabSummary`] from a `&Tab`, classifying `is_current` against
/// the supplied `current_tab_id`.
async fn summarize_tab(tab: &zendriver::Tab, current_tab_id: Option<&str>) -> TabSummary {
    let id = tab.target_id().to_string();
    let is_current = current_tab_id == Some(id.as_str());
    let url = tab.url().await.map(|u| u.to_string()).unwrap_or_default();
    let title = tab.title().await.unwrap_or_default();
    TabSummary {
        id,
        url,
        title,
        is_current,
    }
}

// ---------- browser_tab_list ----------------------------------------------

/// Output of `browser_tab_list`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TabListOutput {
    /// All live tabs in unspecified order. Order is not guaranteed
    /// stable across calls — `Browser::tabs()` is HashMap-backed.
    pub tabs: Vec<TabSummary>,
}

/// Enumerate all live tabs.
///
/// Returns an empty list when no browser is open is NOT the contract —
/// we return [`McpServerError::BrowserNotOpen`] instead, so the agent gets
/// a `suggested_next: browser_open` hint.
pub async fn list(
    state: Arc<Mutex<SessionState>>,
    _: EmptyInput,
) -> Result<TabListOutput, ErrorData> {
    let s = state.lock().await;
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    let tabs = b.tabs().await;
    let mut out = Vec::with_capacity(tabs.len());
    let current = s.current_tab_id.as_deref();
    for t in &tabs {
        out.push(summarize_tab(t, current).await);
    }
    Ok(TabListOutput { tabs: out })
}

// ---------- browser_tab_new -----------------------------------------------

/// Input for `browser_tab_new`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TabNewInput {
    /// Initial URL for the new tab. When `None` the tab opens at
    /// `about:blank` (via `Browser::new_tab`).
    #[serde(default)]
    pub url: Option<String>,
    /// When `true` (the default), the new tab becomes the session's
    /// current tab — subsequent navigation / find / action tools target
    /// it. When `false` the focus stays on the previous current tab and
    /// the caller can switch later via `browser_tab_switch`.
    #[serde(default = "default_true")]
    pub activate: bool,
}

const fn default_true() -> bool {
    true
}

/// Open a new tab.
///
/// When `input.url` is `Some(_)` the underlying call is `new_tab_at(url)`;
/// otherwise the tab opens at `about:blank`. If `input.activate` is `true`
/// the session's current tab is updated to the freshly-opened tab.
pub async fn new_tab(
    state: Arc<Mutex<SessionState>>,
    input: TabNewInput,
) -> Result<TabSummary, ErrorData> {
    let mut s = state.lock().await;
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    let tab = match input.url.as_deref() {
        Some(u) => b
            .new_tab_at(u)
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?,
        None => b
            .new_tab()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?,
    };
    let id = tab.target_id().to_string();
    if input.activate {
        s.current_tab_id = Some(id.clone());
    }
    // `summarize_tab` reads the current `s.current_tab_id` (possibly the
    // freshly-set id above) so `is_current` lines up with the state we
    // just wrote.
    let current = s.current_tab_id.as_deref();
    Ok(summarize_tab(&tab, current).await)
}

// ---------- browser_tab_switch --------------------------------------------

/// Input for `browser_tab_switch`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TabSwitchInput {
    /// CDP target id of the tab to switch to. Obtain from
    /// `browser_tab_list`.
    pub tab_id: String,
}

/// Switch the session's current tab to `tab_id`.
///
/// Validates the id against the live tab list first — if not found,
/// returns [`McpServerError::NoCurrentTab`] (which suggests
/// `browser_tab_new`) plus an error message pointing at
/// `browser_tab_list`. We deliberately surface a richer message than the
/// canonical `NoCurrentTab` carries: the agent's most likely next step is
/// to re-enumerate tabs, not open a new one.
pub async fn switch(
    state: Arc<Mutex<SessionState>>,
    input: TabSwitchInput,
) -> Result<TabSummary, ErrorData> {
    let mut s = state.lock().await;
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    let tabs = b.tabs().await;
    let tab = tabs
        .iter()
        .find(|t| t.target_id() == input.tab_id)
        .cloned()
        .ok_or_else(|| {
            map_error(McpServerError::from(
                zendriver::ZendriverError::TabNotFound(input.tab_id.clone()),
            ))
        })?;
    s.current_tab_id = Some(input.tab_id.clone());
    let current = s.current_tab_id.as_deref();
    Ok(summarize_tab(&tab, current).await)
}

// ---------- browser_tab_close ---------------------------------------------

/// Input for `browser_tab_close`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TabCloseInput {
    /// CDP target id of the tab to close. When `None`, the current tab
    /// is closed.
    #[serde(default)]
    pub tab_id: Option<String>,
}

/// Output of `browser_tab_close`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TabCloseOutput {
    /// CDP target id of the tab that was closed.
    pub closed_id: String,
    /// Session's current tab after the close. `None` when the last tab
    /// was closed (or when the closed tab was the current tab and no
    /// other tabs remained).
    pub current_tab_id: Option<String>,
}

/// Close a tab.
///
/// When `tab_id` is `None`, the current tab is closed. After the close,
/// if we just removed the current tab, the next live tab from
/// `Browser::tabs()` becomes the current tab (deterministic-enough for
/// agent flows that don't care which fallback wins; reorder via
/// `browser_tab_switch` if you do).
///
/// `Tab::close(self)` consumes the receiver, so we have to fetch the
/// owned `Tab` out of `Browser::tabs()` before calling `.close()`.
///
/// # v0 limitation: tab-scoped expectations and interception rules leak
///
/// `SessionState::expectations` and `SessionState::rules` are flat
/// `HashMap`s keyed by opaque ids — neither tracks which tab a handle
/// was originally bound to. A `browser_tab_close` here therefore does
/// **not** reap expectations registered via `browser_expect_register`
/// or rules registered via `browser_intercept_add_rule` against the
/// closing tab; they continue running (the expectation's spawned task
/// holds its `.matched()` future open until its inner
/// `pre_await_timeout_ms` fires, the interception actor's
/// `Fetch.disable` only dispatches when its handle drops) until
/// either: (a) the agent explicitly calls `browser_expect_cancel` /
/// `browser_intercept_remove_rule`, or (b) the whole browser is torn
/// down via `browser_close` (which DOES drain both registries —
/// see `crate::tools::lifecycle::close`).
///
/// Fixing this requires extending each handle struct with a `tab_id`
/// field plus a per-tab filter pass on close — out of scope for v0
/// (would also need to handle the "tab id mid-rotation" case where the
/// CDP target id changes under us). Tracked for a follow-up release.
pub async fn close(
    state: Arc<Mutex<SessionState>>,
    input: TabCloseInput,
) -> Result<TabCloseOutput, ErrorData> {
    let mut s = state.lock().await;
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    let target_id = match input.tab_id {
        Some(id) => id,
        None => s
            .current_tab_id
            .clone()
            .ok_or_else(|| map_error(McpServerError::NoCurrentTab))?,
    };
    let tabs = b.tabs().await;
    let tab = tabs
        .iter()
        .find(|t| t.target_id() == target_id)
        .cloned()
        .ok_or_else(|| {
            map_error(McpServerError::from(
                zendriver::ZendriverError::TabNotFound(target_id.clone()),
            ))
        })?;
    tab.close()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;

    // Reset / promote the current tab id.
    let was_current = s.current_tab_id.as_deref() == Some(target_id.as_str());
    if was_current {
        // Re-read the tab list now that the close has landed; the
        // browser's internal registrar may take a beat to remove the
        // closed entry, so we filter explicitly by `target_id`.
        let b = s.browser.as_ref().expect("browser still attached");
        let remaining = b.tabs().await;
        s.current_tab_id = remaining
            .iter()
            .find(|t| t.target_id() != target_id)
            .map(|t| t.target_id().to_string());
    }
    Ok(TabCloseOutput {
        closed_id: target_id,
        current_tab_id: s.current_tab_id.clone(),
    })
}

// ---------- browser_tab_activate ------------------------------------------

/// Input for `browser_tab_activate`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TabActivateInput {
    /// CDP target id of the tab to bring to the foreground.
    pub tab_id: String,
}

/// Output of `browser_tab_activate`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TabActivateOutput {
    /// CDP target id of the tab that was activated.
    pub id: String,
}

/// Bring `tab_id` to the foreground in Chrome.
///
/// Sends `Target.activateTarget` via [`zendriver::Tab::activate`]. Also
/// updates `state.current_tab_id` so the session's "current tab" tracks
/// the foreground tab — otherwise a caller could end up with a current
/// tab that's hidden behind the foregrounded one, which is rarely what
/// they want.
pub async fn activate(
    state: Arc<Mutex<SessionState>>,
    input: TabActivateInput,
) -> Result<TabActivateOutput, ErrorData> {
    let mut s = state.lock().await;
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    let tabs = b.tabs().await;
    let tab = tabs
        .iter()
        .find(|t| t.target_id() == input.tab_id)
        .cloned()
        .ok_or_else(|| {
            map_error(McpServerError::from(
                zendriver::ZendriverError::TabNotFound(input.tab_id.clone()),
            ))
        })?;
    tab.activate()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    // Also raise the page within its window (`Page.bringToFront`) so the
    // activated tab is actually frontmost, not just the CDP-active target.
    tab.bring_to_front()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    s.current_tab_id = Some(input.tab_id.clone());
    Ok(TabActivateOutput { id: input.tab_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Arc<Mutex<SessionState>> {
        Arc::new(Mutex::new(SessionState::new()))
    }

    #[tokio::test]
    async fn list_with_no_browser_suggests_browser_open() {
        let err = list(fresh(), EmptyInput {})
            .await
            .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"), "msg: {}", err.message);
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn new_with_no_browser_suggests_browser_open() {
        let err = new_tab(
            fresh(),
            TabNewInput {
                url: None,
                activate: true,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn switch_with_no_browser_suggests_browser_open() {
        let err = switch(
            fresh(),
            TabSwitchInput {
                tab_id: "T0".into(),
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn close_with_no_browser_suggests_browser_open() {
        let err = close(fresh(), TabCloseInput::default())
            .await
            .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn activate_with_no_browser_suggests_browser_open() {
        let err = activate(
            fresh(),
            TabActivateInput {
                tab_id: "T0".into(),
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }
}
