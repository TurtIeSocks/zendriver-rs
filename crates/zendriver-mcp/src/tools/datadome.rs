//! DataDome bypass tool — `browser_solve_datadome`. Gated behind the
//! `datadome` feature. Mirrors `tools/imperva.rs`: all flow-terminals
//! (cleared / challenge_gone / already_clear / blocked / timed_out) are
//! success-channel outcomes; only genuine faults (captcha-without-solver, CDP,
//! JS) are MCP errors. The `on_captcha` solver hook is intentionally not wired
//! over MCP (documented non-goal, same as imperva): a captcha surface without
//! a solver surfaces as a `CaptchaRequired` error the agent handles
//! out-of-band.

#![cfg(feature = "datadome")]

use std::sync::Arc;
use std::time::Duration;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::{DataDomeClearanceOutcome, ZendriverError};

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::common::current_tab;

/// Input for `browser_solve_datadome`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SolveDataDomeInput {
    /// Maximum total wait for a terminal outcome, in milliseconds. Default
    /// 30_000 (30s).
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Override the bypass driver's internal poll cadence, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_interval_ms: Option<u64>,
    /// Enable the Fetch-domain interception fast-path. Default `false`.
    #[serde(default)]
    pub with_interception: bool,
}

fn default_timeout() -> u64 {
    30_000
}

/// Terminal outcome of a DataDome bypass attempt.
#[derive(Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// datadome cookie acquired (value in [`SolveDataDomeOutput::datadome`]).
    Cleared,
    /// Body markers cleared without a datadome cookie.
    ChallengeGone,
    /// No DataDome surface present at call time (fast path).
    AlreadyClear,
    /// `window.dd.t == 'bv'` — IP banned. Caller must change IP.
    Blocked,
    /// `timeout_ms` elapsed without a terminal state. Not a hard error.
    Timeout,
}

/// Output of `browser_solve_datadome`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SolveDataDomeOutput {
    /// Which terminal state the bypass reached.
    pub outcome: Outcome,
    /// datadome cookie value. Populated only when `outcome == cleared`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datadome: Option<String>,
}

/// Drive the DataDome clearance flow on the current tab, returning the
/// terminal outcome within `timeout_ms`.
///
/// See module-level docs for outcome semantics.
pub async fn solve_datadome(
    state: Arc<Mutex<SessionState>>,
    input: SolveDataDomeInput,
) -> Result<SolveDataDomeOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let mut bypass = tab
        .datadome()
        .timeout(Duration::from_millis(input.timeout_ms));
    if let Some(p) = input.poll_interval_ms {
        bypass = bypass.poll_interval(Duration::from_millis(p));
    }
    if input.with_interception {
        bypass = bypass.with_interception();
    }
    match bypass.wait_for_clearance().await {
        Ok(DataDomeClearanceOutcome::Cleared { datadome }) => Ok(SolveDataDomeOutput {
            outcome: Outcome::Cleared,
            datadome: Some(datadome),
        }),
        Ok(DataDomeClearanceOutcome::ChallengeGone) => Ok(SolveDataDomeOutput {
            outcome: Outcome::ChallengeGone,
            datadome: None,
        }),
        Ok(DataDomeClearanceOutcome::AlreadyClear) => Ok(SolveDataDomeOutput {
            outcome: Outcome::AlreadyClear,
            datadome: None,
        }),
        Ok(DataDomeClearanceOutcome::Blocked) => Ok(SolveDataDomeOutput {
            outcome: Outcome::Blocked,
            datadome: None,
        }),
        // TimedOut is a lib-side success terminal; collapse it into the
        // success-channel `Outcome::Timeout` — see module docs.
        Ok(DataDomeClearanceOutcome::TimedOut { .. }) => Ok(SolveDataDomeOutput {
            outcome: Outcome::Timeout,
            datadome: None,
        }),
        // Everything else (CAPTCHA-without-solver, CDP failure, JS error) is a
        // real error. Route through `From<DataDomeError> for ZendriverError` so
        // the existing `map_error` knows how to format it.
        Err(other) => Err(map_error(McpServerError::from(ZendriverError::from(other)))),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    //! No-browser unit coverage. The bypass flow itself needs a live Chrome +
    //! a DataDome-protected page — exercised in the integration test gated
    //! behind `integration-tests + datadome`. Here we cover the only branch
    //! reachable without a browser: no browser open surfaces `BrowserNotOpen`.

    use super::*;

    #[tokio::test]
    async fn solve_datadome_with_no_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = solve_datadome(
            state,
            SolveDataDomeInput {
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
