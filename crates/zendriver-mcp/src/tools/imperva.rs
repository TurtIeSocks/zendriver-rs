//! Imperva / Incapsula bypass tool — `browser_solve_imperva`. Gated behind
//! the `imperva` feature.
//!
//! Mirrors [`tools/cloudflare.rs`][crate::tools::cloudflare]: the lib's
//! [`ImpervaBypass::wait_for_clearance`] models its terminal state as
//! `Result<ImpervaClearanceOutcome, ImpervaError>`. The MCP layer collapses
//! the lib-side [`ImpervaClearanceOutcome::TimedOut`] terminal into a
//! non-error [`Outcome::Timeout`] — a deadline in a bot-management flow is a
//! normal "didn't finish, retry or give up" signal, not a server error — and
//! keeps every `ImpervaError` (CAPTCHA-without-solver, CDP failure, JS error)
//! as a real MCP error.
//!
//! Unlike Cloudflare's three states, Imperva reports a fourth —
//! [`ImpervaClearanceOutcome::AlreadyClear`] (no surface present at call
//! time, fast path) — surfaced as a distinct [`Outcome::AlreadyClear`] so an
//! agent can skip redundant follow-up work.
//!
//! The lib's `on_captcha` solver hook is intentionally **not** wired over
//! MCP (same class as the interception stream non-goal): a CAPTCHA surface
//! without a registered solver surfaces as `ImpervaError::CaptchaRequired`,
//! which becomes a real MCP error the agent must handle out-of-band.
//!
//! [`ImpervaBypass::wait_for_clearance`]: zendriver_imperva::ImpervaBypass::wait_for_clearance

#![cfg(feature = "imperva")]

use std::sync::Arc;
use std::time::Duration;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::{ImpervaClearanceOutcome, ZendriverError};

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::common::current_tab;

/// Input for `browser_solve_imperva`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SolveImpervaInput {
    /// Maximum total wait for a terminal outcome, in milliseconds. Default
    /// 30_000 (30s). Maps to [`ImpervaBypass::timeout`].
    ///
    /// [`ImpervaBypass::timeout`]: zendriver_imperva::ImpervaBypass::timeout
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Override the bypass driver's internal poll cadence, in milliseconds.
    /// Defaults to the lib's own default. Lowering speeds up detection at the
    /// cost of more CDP `Runtime.evaluate` round-trips; raising is a safe
    /// knob for slow / sandboxed environments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_interval_ms: Option<u64>,
    /// Enable the Fetch-domain interception fast-path
    /// ([`ImpervaBypass::with_interception`]) for quicker reese84 capture on
    /// modern surfaces. Default `false`.
    ///
    /// [`ImpervaBypass::with_interception`]: zendriver_imperva::ImpervaBypass::with_interception
    #[serde(default)]
    pub with_interception: bool,
}

fn default_timeout() -> u64 {
    30_000
}

/// Terminal outcome of an Imperva bypass attempt.
///
/// `TokenAcquired` / `ChallengeGone` / `AlreadyClear` mirror the lib's
/// [`ImpervaClearanceOutcome`] variants. `Timeout` mirrors the lib's
/// [`ImpervaClearanceOutcome::TimedOut`] — a deadline in a bot-management
/// flow is a normal outcome, not an error — surfacing it on the success
/// channel so agents branch on `outcome` without try/catch around a timeout.
#[derive(Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// reese84 token acquired (value in [`SolveImpervaOutput::reese84`]).
    TokenAcquired,
    /// Body markers cleared without a reese84 token (e.g. legacy Incapsula
    /// flow). `reese84` will be `None`.
    ChallengeGone,
    /// No Imperva surface was present at call time (fast path, no waiting).
    /// `reese84` will be `None`.
    AlreadyClear,
    /// `timeout_ms` elapsed without reaching clearance. Not a hard error —
    /// agents can retry or give up. `reese84` will be `None`.
    Timeout,
}

/// Output of `browser_solve_imperva`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SolveImpervaOutput {
    /// Which terminal state the bypass reached.
    pub outcome: Outcome,
    /// reese84 cookie value. Populated only when `outcome == token_acquired`;
    /// `None` for every other outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reese84: Option<String>,
}

/// Drive the Imperva clearance flow on the current tab, returning the
/// terminal outcome within `timeout_ms`.
///
/// See module-level docs for outcome semantics.
pub async fn solve_imperva(
    state: Arc<Mutex<SessionState>>,
    input: SolveImpervaInput,
) -> Result<SolveImpervaOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    // `tab.imperva()` borrows `tab` for the bypass's lifetime; the builder
    // methods consume + return `self`, and `wait_for_clearance` consumes it,
    // so the bypass lives only for the single await below.
    let mut bypass = tab
        .imperva()
        .timeout(Duration::from_millis(input.timeout_ms));
    if let Some(p) = input.poll_interval_ms {
        bypass = bypass.poll_interval(Duration::from_millis(p));
    }
    if input.with_interception {
        bypass = bypass.with_interception();
    }
    match bypass.wait_for_clearance().await {
        Ok(ImpervaClearanceOutcome::TokenAcquired { reese84, .. }) => Ok(SolveImpervaOutput {
            outcome: Outcome::TokenAcquired,
            reese84: Some(reese84),
        }),
        Ok(ImpervaClearanceOutcome::ChallengeGone) => Ok(SolveImpervaOutput {
            outcome: Outcome::ChallengeGone,
            reese84: None,
        }),
        Ok(ImpervaClearanceOutcome::AlreadyClear) => Ok(SolveImpervaOutput {
            outcome: Outcome::AlreadyClear,
            reese84: None,
        }),
        // TimedOut is a lib-side success terminal; collapse it into the
        // success-channel `Outcome::Timeout` — see module docs.
        Ok(ImpervaClearanceOutcome::TimedOut { .. }) => Ok(SolveImpervaOutput {
            outcome: Outcome::Timeout,
            reese84: None,
        }),
        // Everything else (CAPTCHA-without-solver, CDP failure, JS error) is a
        // real error. Route through `From<ImpervaError> for ZendriverError` so
        // the existing `map_error` knows how to format it.
        Err(other) => Err(map_error(McpServerError::from(ZendriverError::from(other)))),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    //! No-browser unit coverage. The bypass flow itself needs a live Chrome +
    //! an Imperva-protected page — exercised in the integration test gated
    //! behind `integration-tests + imperva`. Here we cover the only branch
    //! reachable without a browser: no browser open surfaces `BrowserNotOpen`.

    use super::*;

    #[tokio::test]
    async fn solve_imperva_with_no_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = solve_imperva(
            state,
            SolveImpervaInput {
                timeout_ms: 100,
                poll_interval_ms: None,
                with_interception: false,
            },
        )
        .await
        .expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }
}
