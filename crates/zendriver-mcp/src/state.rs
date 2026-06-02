//! Per-MCP-session mutable state.
//!
//! Wrapped in `Arc<tokio::sync::Mutex<_>>` and shared across tool handlers.
//! In stdio mode there is one global instance; in HTTP mode one per session.

#[cfg(any(feature = "expect", feature = "interception"))]
use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zendriver::Browser;

/// Opaque registration id for [`SessionState::expectations`].
#[cfg(feature = "expect")]
pub type ExpectationId = String;

/// Opaque registration id for [`SessionState::rules`].
#[cfg(feature = "interception")]
pub type RuleId = String;

/// Stealth profile choice carried over the MCP wire.
///
/// Concrete `StealthProfile` resolution happens inside the lifecycle
/// handler (it depends on platform detection that only matters at launch
/// time); the wire-level enum stays stable and platform-agnostic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StealthProfileChoice {
    /// Defer to `zendriver_stealth::StealthProfile::native()` (auto-detect
    /// platform via `sysinfo`).
    #[default]
    Auto,
    /// Force native (same as `Auto`, but explicit).
    Native,
    /// Spoof a macOS fingerprint regardless of host platform.
    SpoofMacos,
    /// Spoof a Linux fingerprint regardless of host platform.
    SpoofLinux,
    /// Spoof a Windows fingerprint regardless of host platform.
    SpoofWindows,
}

/// Platform to spoof via a fine-grained stealth override.
///
/// Wire mirror of `zendriver::stealth::Platform`; kept here (platform-agnostic)
/// so the override schema stays stable independent of host detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StealthPlatformChoice {
    /// Windows.
    Win32,
    /// macOS (Intel).
    MacIntel,
    /// Linux x86_64.
    LinuxX86_64,
}

/// Fine-grained stealth fingerprint overrides layered onto the chosen
/// [`StealthProfileChoice`] at the next `browser_open`.
///
/// Every field is optional — an unset field leaves the base profile's value
/// in place. Most meaningful paired with a `spoof_*` profile; applying these
/// to `native` overrides the auto-detected real fingerprint and can *reduce*
/// stealth if the values disagree with the host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StealthOverrides {
    /// Spoofed `navigator.platform` / UA platform token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<StealthPlatformChoice>,
    /// Spoofed locale (e.g. `"en-US"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Spoofed timezone (IANA name, e.g. `"America/New_York"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Spoofed `navigator.deviceMemory` in GiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<u32>,
    /// Spoofed `navigator.hardwareConcurrency`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_count: Option<u32>,
    /// Spoofed Chrome major version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chrome_version: Option<u32>,
    /// Full User-Agent string override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// Toggle Content-Security-Policy bypass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_csp: Option<bool>,
}

/// State held for the duration of a single MCP session.
///
/// `browser` is `None` until `browser_open` is called. `current_tab_id`
/// tracks the focused tab (matches `zendriver::Tab::target_id`).
pub struct SessionState {
    pub browser: Option<Browser>,
    pub current_tab_id: Option<String>,
    pub stealth_profile_choice: StealthProfileChoice,
    pub stealth_overrides: StealthOverrides,

    #[cfg(feature = "expect")]
    pub expectations: HashMap<ExpectationId, ExpectationHandle>,

    #[cfg(feature = "interception")]
    pub rules: HashMap<RuleId, InterceptRuleHandle>,
}

/// Live handle to a pending `expect_*` expectation.
///
/// The expectation is awaited inside a tokio task spawned by
/// `browser_expect_register`; the task forwards the result through
/// [`Self::rx`] (a `oneshot::Receiver`) carrying either the JSON-encoded
/// matched-event or a textual error from the spawned task. The
/// [`Self::task`] handle is retained so `browser_expect_cancel` can `.abort()`
/// the in-flight `.matched()` future instead of leaving it orphaned until its
/// inner timeout fires.
///
/// `kind` is a static label ("request" / "response" / "dialog" / "download")
/// for diagnostics — not currently surfaced, but cheap to keep alongside.
#[cfg(feature = "expect")]
pub struct ExpectationHandle {
    pub kind: &'static str,
    pub task: tokio::task::JoinHandle<()>,
    pub rx: tokio::sync::oneshot::Receiver<Result<serde_json::Value, String>>,
}

/// One MCP interception rule = one `zendriver_interception::InterceptHandle`.
///
/// Holding the handle is what keeps the rule live: dropping it (via
/// [`HashMap::remove`] or [`HashMap::clear`]) cancels the actor and tears
/// down `Fetch.enable` on that rule's session. `pattern` + `action_kind`
/// are kept alongside so `browser_intercept_list_rules` can report back
/// what each id corresponds to without poking at the handle's internals.
#[cfg(feature = "interception")]
pub struct InterceptRuleHandle {
    pub pattern: String,
    pub action_kind: &'static str,
    pub _handle: zendriver::InterceptHandle,
}

impl SessionState {
    /// Construct an empty session — no browser, no tabs, default profile.
    pub fn new() -> Self {
        Self {
            browser: None,
            current_tab_id: None,
            stealth_profile_choice: StealthProfileChoice::default(),
            stealth_overrides: StealthOverrides::default(),
            #[cfg(feature = "expect")]
            expectations: HashMap::new(),
            #[cfg(feature = "interception")]
            rules: HashMap::new(),
        }
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_empty() {
        let s = SessionState::new();
        assert!(s.browser.is_none());
        assert!(s.current_tab_id.is_none());
        assert_eq!(s.stealth_profile_choice, StealthProfileChoice::Auto);
        #[cfg(feature = "expect")]
        assert!(s.expectations.is_empty());
        #[cfg(feature = "interception")]
        assert!(s.rules.is_empty());
    }
}
