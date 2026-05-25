//! Browser lifecycle handlers — `browser_open`, `browser_close`,
//! `browser_status`.
//!
//! Each handler is a free async fn that locks the shared
//! [`SessionState`][crate::state::SessionState] internally and returns a
//! typed output (or an [`rmcp::ErrorData`]). The thin `#[tool]` wrappers in
//! [`crate::server`] forward to these.

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::Browser;
use zendriver::stealth::{Platform, StealthProfile};

use crate::errors::{McpServerError, map_error};
use crate::state::{SessionState, StealthProfileChoice};
use crate::tools::common::EmptyInput;

// ---------- browser_open --------------------------------------------------

/// Input for `browser_open`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenInput {
    /// Run Chrome with `--headless=new` (default: `true`).
    #[serde(default = "default_true")]
    pub headless: bool,
    /// Override the session's default stealth profile for this launch.
    /// When `None`, the session-wide default (set via CLI / construct time)
    /// is used.
    #[serde(default)]
    pub stealth_profile: Option<StealthProfileChoice>,
}

const fn default_true() -> bool {
    true
}

/// Output of `browser_open`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct OpenOutput {
    /// Detected Chrome version string. Empty in v0 (the zendriver lib does
    /// not expose a version accessor); reserved for a follow-up dispatch.
    pub chrome_version: String,
    /// Effective headless flag for the launched browser.
    pub headless: bool,
    /// Effective stealth profile for the launched browser.
    pub profile: StealthProfileChoice,
}

/// Launch Chrome with stealth defaults.
///
/// Records the resulting `Browser` and the id of its initial tab in the
/// session state. Returns [`McpServerError::BrowserAlreadyOpen`] if a
/// browser is already attached.
pub async fn open(
    state: Arc<Mutex<SessionState>>,
    input: OpenInput,
) -> Result<OpenOutput, ErrorData> {
    let mut s = state.lock().await;
    if s.browser.is_some() {
        return Err(map_error(McpServerError::BrowserAlreadyOpen));
    }
    let profile = input.stealth_profile.unwrap_or(s.stealth_profile_choice);
    let stealth = stealth_profile_for(profile);
    let browser = Browser::builder()
        .headless(input.headless)
        .stealth(stealth)
        .launch()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    let tabs = browser.tabs().await;
    s.current_tab_id = tabs.first().map(|t| t.target_id().to_string());
    s.browser = Some(browser);
    s.stealth_profile_choice = profile;
    Ok(OpenOutput {
        chrome_version: String::new(),
        headless: input.headless,
        profile,
    })
}

/// Map the wire-level [`StealthProfileChoice`] to a concrete
/// [`StealthProfile`].
///
/// `Auto` and `Native` both call [`StealthProfile::native`] (auto-detects
/// platform via `sysinfo`). The `Spoof*` variants build a `spoofed()`
/// profile and pin the platform.
fn stealth_profile_for(choice: StealthProfileChoice) -> StealthProfile {
    match choice {
        StealthProfileChoice::Auto | StealthProfileChoice::Native => StealthProfile::native(),
        StealthProfileChoice::SpoofMacos => StealthProfile::spoofed().platform(Platform::MacIntel),
        StealthProfileChoice::SpoofLinux => {
            StealthProfile::spoofed().platform(Platform::LinuxX86_64)
        }
        StealthProfileChoice::SpoofWindows => StealthProfile::spoofed().platform(Platform::Win32),
    }
}

// ---------- browser_close -------------------------------------------------

/// Output of `browser_close`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CloseOutput {
    /// Always `true`. Present so the tool's structured output is
    /// non-empty (rmcp clients sometimes treat `{}` as "no payload").
    pub ok: bool,
}

/// Close the open browser. Idempotent — no error if no browser is open.
pub async fn close(
    state: Arc<Mutex<SessionState>>,
    _: EmptyInput,
) -> Result<CloseOutput, ErrorData> {
    let mut s = state.lock().await;
    if let Some(b) = s.browser.take() {
        b.close()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
    }
    s.current_tab_id = None;
    Ok(CloseOutput { ok: true })
}

// ---------- browser_status ------------------------------------------------

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

/// Report whether a browser is open, the current tab (if any), and the
/// configured stealth profile.
pub async fn status(
    state: Arc<Mutex<SessionState>>,
    _: EmptyInput,
) -> Result<StatusOutput, ErrorData> {
    let s = state.lock().await;
    let Some(b) = s.browser.as_ref() else {
        return Ok(StatusOutput {
            open: false,
            tab_count: 0,
            current_tab: None,
            profile: s.stealth_profile_choice,
        });
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
    Ok(StatusOutput {
        open: true,
        tab_count: tabs.len(),
        current_tab,
        profile: s.stealth_profile_choice,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn close_with_no_browser_is_noop() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let out = close(state, EmptyInput {}).await.expect("close ok");
        assert!(out.ok);
    }

    #[tokio::test]
    async fn status_with_no_browser_reports_closed() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let out = status(state, EmptyInput {}).await.expect("status ok");
        assert!(!out.open);
        assert_eq!(out.tab_count, 0);
        assert!(out.current_tab.is_none());
        assert_eq!(out.profile, StealthProfileChoice::Auto);
    }
}
