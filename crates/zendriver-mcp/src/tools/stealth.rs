//! Stealth-profile configuration — `browser_set_stealth_profile`.
//!
//! Mutates only the session-level `stealth_profile_choice` — the chosen
//! profile takes effect on the NEXT `browser_open` call. We do NOT
//! re-fingerprint an already-running browser; the underlying stealth
//! patches are injected at launch and there's no live "switch profile"
//! lever in the lib.

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::UserAgentOverride;

use crate::errors::{McpServerError, map_error};
use crate::state::{SessionState, StealthOverrides, StealthProfileChoice};
use crate::tools::actions::AckOutput;
use crate::tools::common::current_tab;

/// Input for `browser_set_stealth_profile`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetStealthProfileInput {
    /// New default stealth profile for this session.
    pub profile: StealthProfileChoice,
    /// Fine-grained fingerprint overrides layered onto `profile` at the next
    /// `browser_open`. Replaces any previously-set overrides; omit (or pass
    /// `{}`) to clear them.
    #[serde(default)]
    pub overrides: StealthOverrides,
}

/// Output of `browser_set_stealth_profile`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SetStealthProfileOutput {
    /// The profile that is now configured as the session default.
    pub active_profile: StealthProfileChoice,
    /// The fine-grained overrides now configured for the next open.
    pub active_overrides: StealthOverrides,
    /// `true` iff a browser is currently open. When `true`, the new
    /// profile only applies after `browser_close` + `browser_open`. When
    /// `false`, the next `browser_open` call will pick it up directly.
    pub takes_effect_on_next_open: bool,
}

/// Configure the session's default stealth profile.
///
/// Always succeeds — the operation only mutates `SessionState` and never
/// touches the running browser. Callers wanting the change to apply to a
/// currently-open browser must follow up with `browser_close` +
/// `browser_open`.
pub async fn set_stealth_profile(
    state: Arc<Mutex<SessionState>>,
    input: SetStealthProfileInput,
) -> Result<SetStealthProfileOutput, ErrorData> {
    let mut s = state.lock().await;
    s.stealth_profile_choice = input.profile;
    s.stealth_overrides = input.overrides.clone();
    Ok(SetStealthProfileOutput {
        active_profile: input.profile,
        active_overrides: input.overrides,
        takes_effect_on_next_open: s.browser.is_some(),
    })
}

/// Input for `browser_set_user_agent`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetUserAgentInput {
    /// Full User-Agent string to send via `Emulation.setUserAgentOverride`.
    pub user_agent: String,
    /// Optional `Accept-Language` override applied alongside the UA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_language: Option<String>,
    /// Optional `navigator.platform` override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

/// Override the current tab's User-Agent at runtime.
///
/// This is last-write-wins and sends NO `userAgentMetadata` — under a Spoofed
/// stealth profile it can clobber UA-Client-Hints coherence and *increase*
/// detectability. Prefer the stealth profile for stealth-sensitive tabs; use
/// this for non-stealth tabs or a deliberate per-tab UA change.
pub async fn set_user_agent(
    state: Arc<Mutex<SessionState>>,
    input: SetUserAgentInput,
) -> Result<AckOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.set_user_agent_with(UserAgentOverride {
        user_agent: input.user_agent,
        accept_language: input.accept_language,
        platform: input.platform,
    })
    .await
    .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(AckOutput { ok: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Arc<Mutex<SessionState>> {
        Arc::new(Mutex::new(SessionState::new()))
    }

    #[tokio::test]
    async fn set_profile_without_browser_reports_immediate_effect() {
        let state = fresh();
        let out = set_stealth_profile(
            state.clone(),
            SetStealthProfileInput {
                profile: StealthProfileChoice::SpoofLinux,
                overrides: StealthOverrides::default(),
            },
        )
        .await
        .expect("set profile ok");
        assert_eq!(out.active_profile, StealthProfileChoice::SpoofLinux);
        assert!(
            !out.takes_effect_on_next_open,
            "no browser open, so the next open call picks the new profile up directly"
        );
        // And the state was mutated.
        let s = state.lock().await;
        assert_eq!(s.stealth_profile_choice, StealthProfileChoice::SpoofLinux);
    }

    #[tokio::test]
    async fn set_profile_overwrites_previous_choice() {
        let state = fresh();
        set_stealth_profile(
            state.clone(),
            SetStealthProfileInput {
                profile: StealthProfileChoice::SpoofMacos,
                overrides: StealthOverrides::default(),
            },
        )
        .await
        .expect("first set ok");
        let out = set_stealth_profile(
            state.clone(),
            SetStealthProfileInput {
                profile: StealthProfileChoice::SpoofWindows,
                overrides: StealthOverrides::default(),
            },
        )
        .await
        .expect("second set ok");
        assert_eq!(out.active_profile, StealthProfileChoice::SpoofWindows);
        let s = state.lock().await;
        assert_eq!(s.stealth_profile_choice, StealthProfileChoice::SpoofWindows);
    }

    #[tokio::test]
    async fn set_profile_does_not_fail_when_choice_matches_existing() {
        // No-op semantic: setting the same profile again should still
        // succeed (and confirm "takes_effect_on_next_open=false").
        let state = fresh();
        let out = set_stealth_profile(
            state,
            SetStealthProfileInput {
                profile: StealthProfileChoice::Auto,
                overrides: StealthOverrides::default(),
            },
        )
        .await
        .expect("idempotent set ok");
        assert_eq!(out.active_profile, StealthProfileChoice::Auto);
        assert!(!out.takes_effect_on_next_open);
    }
}
