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
/// # Tab-scoped expectation / interception-rule teardown
///
/// `SessionState::expectations` and `SessionState::rules` entries carry the
/// `tab_id` of the tab they were registered against (set at
/// `browser_expect_register` / `browser_intercept_add_rule` time). Once the
/// closed tab's id is known, [`drain_tab_scoped`] removes every entry whose
/// `tab_id` matches it: expectation tasks are `.abort()`ed (a bare
/// `JoinHandle` drop only detaches, it does not cancel), and rule handles
/// are simply dropped from the map — `InterceptRuleHandle::_handle`'s `Drop`
/// impl cancels the actor. Without this, both kinds of handle would
/// otherwise keep running (the expectation's spawned task holding its
/// `.matched()` future open until `pre_await_timeout_ms` fires; the
/// interception actor staying live) until either the agent explicitly calls
/// `browser_expect_cancel` / `browser_intercept_remove_rule`, or the whole
/// browser is torn down via `browser_close` (which also drains both
/// registries — see `crate::tools::lifecycle::close`).
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

    // Tear down tab-scoped expectations/rules for the closed tab — see the
    // `close` doc comment above and `drain_tab_scoped`'s own doc comment.
    #[cfg(any(feature = "expect", feature = "interception"))]
    drain_tab_scoped(&mut s, &target_id);

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

/// Remove every `expectations` / `rules` entry registered against
/// `closed_id`, tearing each one down.
///
/// Pulled out of [`close`] (rather than inlined) so it's unit-testable
/// without a live `Browser`/`Tab` — `close` itself can't run past its
/// `BrowserNotOpen` guard without one.
///
/// The two maps need different teardown because of what `retain` drops:
/// - `expectations`: the removed value's `task` field is a
///   `tokio::task::JoinHandle` — dropping a `JoinHandle` merely *detaches*
///   the task (it keeps running), it does not cancel it. So the retain
///   closure explicitly `.abort()`s each removed entry's task before
///   returning `false`.
/// - `rules`: the removed value's `_handle` field is a
///   `zendriver::InterceptHandle`, whose `Drop` impl cancels the
///   interception actor. A plain `retain` (letting the removed values drop)
///   is therefore enough — no explicit teardown call needed.
///
/// Not currently called for `SessionState::monitors`: `MonitorState` has no
/// `tab_id` field today, so there is no cheap per-tab filter to apply here
/// without first threading a tab id through `browser_monitor_start` /
/// `MonitorState` — left as a follow-up (tracked in the deferred backlog,
/// §1 MCP surface) rather than folded into this pass.
#[cfg(any(feature = "expect", feature = "interception"))]
fn drain_tab_scoped(s: &mut SessionState, closed_id: &str) {
    #[cfg(feature = "expect")]
    s.expectations.retain(|_, h| {
        if h.tab_id == closed_id {
            h.task.abort();
            false
        } else {
            true
        }
    });

    #[cfg(feature = "interception")]
    s.rules.retain(|_, h| h.tab_id != closed_id);
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

    /// `drain_tab_scoped` must reap only the closed tab's entries: an
    /// expectation's spawned `task` is `.abort()`ed (dropping a `JoinHandle`
    /// alone detaches rather than cancels), while a rule's `InterceptHandle`
    /// tears down via `Drop` — so a plain `retain` suffices for rules. A
    /// survivor on a different `tab_id` must remain in both maps untouched.
    #[cfg(all(feature = "expect", feature = "interception"))]
    #[tokio::test]
    async fn drain_tab_scoped_reaps_closed_tab_keeps_survivor() {
        use crate::state::{ExpectationHandle, InterceptRuleHandle};

        let mut s = SessionState::new();

        // Expectation on the closed tab: parks forever, so only `.abort()`
        // (not a bare drop) can end it.
        let (_tx_keep_alive, rx_closed) =
            tokio::sync::oneshot::channel::<Result<serde_json::Value, String>>();
        let task_closed = tokio::spawn(async move {
            std::future::pending::<()>().await;
        });
        let closed_task_check = task_closed.abort_handle();
        s.expectations.insert(
            "exp-closed".into(),
            ExpectationHandle {
                kind: "request",
                task: task_closed,
                rx: rx_closed,
                tab_id: "closed-tab-id".into(),
            },
        );

        // Survivor expectation on a different tab.
        let (_tx_keep_alive2, rx_survivor) =
            tokio::sync::oneshot::channel::<Result<serde_json::Value, String>>();
        let task_survivor = tokio::spawn(async move {
            std::future::pending::<()>().await;
        });
        let survivor_task_check = task_survivor.abort_handle();
        s.expectations.insert(
            "exp-survivor".into(),
            ExpectationHandle {
                kind: "request",
                task: task_survivor,
                rx: rx_survivor,
                tab_id: "other-tab-id".into(),
            },
        );

        s.rules.insert(
            "rule-closed".into(),
            InterceptRuleHandle {
                pattern: "*/ads/*".into(),
                action_kind: "block",
                _handle: zendriver_interception::InterceptHandle::for_tests(),
                tab_id: "closed-tab-id".into(),
            },
        );
        s.rules.insert(
            "rule-survivor".into(),
            InterceptRuleHandle {
                pattern: "*/api/*".into(),
                action_kind: "respond",
                _handle: zendriver_interception::InterceptHandle::for_tests(),
                tab_id: "other-tab-id".into(),
            },
        );

        drain_tab_scoped(&mut s, "closed-tab-id");

        assert!(
            !s.expectations.contains_key("exp-closed"),
            "closed tab's expectation must be drained"
        );
        assert!(
            s.expectations.contains_key("exp-survivor"),
            "other tab's expectation must survive"
        );
        assert!(
            !s.rules.contains_key("rule-closed"),
            "closed tab's rule must be drained"
        );
        assert!(
            s.rules.contains_key("rule-survivor"),
            "other tab's rule must survive"
        );

        tokio::task::yield_now().await;
        assert!(
            closed_task_check.is_finished(),
            "closed tab's expectation task should be aborted"
        );
        assert!(
            !survivor_task_check.is_finished(),
            "surviving tab's expectation task should NOT be aborted"
        );
    }
}
