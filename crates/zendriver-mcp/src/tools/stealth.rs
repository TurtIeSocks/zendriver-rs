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

use crate::state::{SessionState, StealthProfileChoice};

/// Input for `browser_set_stealth_profile`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetStealthProfileInput {
    /// New default stealth profile for this session.
    pub profile: StealthProfileChoice,
}

/// Output of `browser_set_stealth_profile`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SetStealthProfileOutput {
    /// The profile that is now configured as the session default.
    pub active_profile: StealthProfileChoice,
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
    Ok(SetStealthProfileOutput {
        active_profile: input.profile,
        takes_effect_on_next_open: s.browser.is_some(),
    })
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
            },
        )
        .await
        .expect("first set ok");
        let out = set_stealth_profile(
            state.clone(),
            SetStealthProfileInput {
                profile: StealthProfileChoice::SpoofWindows,
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
            },
        )
        .await
        .expect("idempotent set ok");
        assert_eq!(out.active_profile, StealthProfileChoice::Auto);
        assert!(!out.takes_effect_on_next_open);
    }
}
