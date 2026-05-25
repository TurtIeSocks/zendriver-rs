//! Cloudflare Turnstile bypass tool — `browser_solve_turnstile`. Gated
//! behind the `cloudflare` feature.
//!
//! ## Outcome mapping
//!
//! The lib's [`CloudflareBypass::wait_for_clearance`] models its terminal
//! state as a `Result<ClearanceOutcome, CloudflareError>` where:
//!
//! - `Ok(ClearanceOutcome::TokenAcquired(t))` — the
//!   `cf-turnstile-response` input picked up a token.
//! - `Ok(ClearanceOutcome::ChallengeGone)` — the challenge container
//!   vanished without a token (e.g. clearance cookie shortcut).
//! - `Err(CloudflareError::ClearanceTimeout)` — the per-call deadline
//!   elapsed.
//!
//! Agents typically want all three lumped into a single discriminated
//! union of *expected* outcomes — a timeout in turnstile flow is a normal
//! "didn't finish, try again or give up" signal, not a server error. So
//! the MCP layer collapses `ClearanceTimeout` from the `Err` arm into a
//! third [`Outcome::Timeout`] variant on the `Ok` side, and keeps every
//! other `CloudflareError` (network failure, JS error, no challenge
//! present at all) as a real MCP error.
//!
//! [`CloudflareBypass::wait_for_clearance`]: zendriver_cloudflare::CloudflareBypass::wait_for_clearance

#![cfg(feature = "cloudflare")]

use std::sync::Arc;
use std::time::Duration;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::{ClearanceOutcome, CloudflareError, ZendriverError};

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::common::current_tab;

/// Input for `browser_solve_turnstile`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SolveInput {
    /// Maximum total wait for a terminal outcome, in milliseconds. Default
    /// 30_000 (30s) — the lib's documented sane default for a real Turnstile
    /// flow. Lower values can be used in tests or when the agent wants to
    /// fail fast and fall back to another strategy.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Override the bypass driver's internal poll cadence, in milliseconds.
    /// Defaults to the lib's own default (500ms). Lowering speeds up
    /// detection at the cost of more CDP `Runtime.evaluate` round-trips;
    /// raising is a safe knob for slow / sandboxed environments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_interval_ms: Option<u64>,
}

fn default_timeout() -> u64 {
    30_000
}

/// Terminal outcome of a turnstile bypass attempt.
///
/// `Solved` and `ChallengeGone` mirror the lib's `ClearanceOutcome`
/// variants. `Timeout` is the MCP layer's collapse of
/// `CloudflareError::ClearanceTimeout` into the success channel — agents
/// can branch on `outcome` without try/catch around a timeout.
#[derive(Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Turnstile produced a token (value of `cf-turnstile-response`). The
    /// token is available in [`SolveOutput::token`].
    Solved,
    /// The challenge container disappeared without yielding a token (e.g.
    /// a clearance cookie shortcut). `token` will be `None`.
    ChallengeGone,
    /// `timeout_ms` elapsed without reaching either success state. `token`
    /// will be `None`. Not a hard error — agents can retry or give up.
    Timeout,
}

/// Output of `browser_solve_turnstile`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SolveOutput {
    /// Which terminal state the bypass reached.
    pub outcome: Outcome,
    /// Turnstile response token. Populated only when `outcome == Solved`;
    /// `None` for `ChallengeGone` and `Timeout`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Drive the Turnstile clearance flow on the current tab, returning the
/// terminal outcome (`Solved`, `ChallengeGone`, or `Timeout`) within
/// `timeout_ms`.
///
/// See module-level docs for outcome semantics.
pub async fn solve_turnstile(
    state: Arc<Mutex<SessionState>>,
    input: SolveInput,
) -> Result<SolveOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    // Build the bypass driver. `tab.cloudflare()` borrows `tab` for the
    // bypass's `'_` lifetime, but `wait_for_clearance` consumes `self`, so
    // the bypass lives only for the single await we're about to do.
    let mut bypass = tab.cloudflare();
    if let Some(p) = input.poll_interval_ms {
        bypass = bypass.poll_interval(Duration::from_millis(p));
    }
    match bypass
        .wait_for_clearance(Duration::from_millis(input.timeout_ms))
        .await
    {
        Ok(ClearanceOutcome::TokenAcquired(t)) => Ok(SolveOutput {
            outcome: Outcome::Solved,
            token: Some(t),
        }),
        Ok(ClearanceOutcome::ChallengeGone) => Ok(SolveOutput {
            outcome: Outcome::ChallengeGone,
            token: None,
        }),
        // ClearanceTimeout collapses into the success channel — see
        // module docs for the rationale.
        Err(CloudflareError::ClearanceTimeout) => Ok(SolveOutput {
            outcome: Outcome::Timeout,
            token: None,
        }),
        // Everything else (no challenge, CDP call failed, JS error) is a
        // real error the agent should surface. Route through the lib's
        // `From<CloudflareError> for ZendriverError` so the existing
        // `map_error` knows how to format it.
        Err(other) => Err(map_error(McpServerError::from(ZendriverError::from(other)))),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    //! No-browser unit coverage.
    //!
    //! The bypass flow itself needs a live Chrome + a Cloudflare Turnstile
    //! page — that path is exercised in the integration test gated behind
    //! `integration-tests + cloudflare`. Here we cover the only branch
    //! reachable without a browser: calling the tool with no browser open
    //! surfaces `BrowserNotOpen`.

    use super::*;

    #[tokio::test]
    async fn solve_with_no_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = solve_turnstile(
            state,
            SolveInput {
                timeout_ms: 100,
                poll_interval_ms: None,
            },
        )
        .await
        .expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }
}
