//! Cloudflare Turnstile bypass driver.
//!
//! Skeleton — `is_challenge_present` lands in T14 and `wait_for_clearance`
//! in T15.

use std::time::Duration;

use zendriver_transport::SessionHandle;

/// Result of a clearance attempt.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ClearanceOutcome {
    /// Turnstile produced a token (value of `cf-turnstile-response`).
    TokenAcquired(String),
    /// The challenge container disappeared without yielding a token.
    ChallengeGone,
}

/// Default poll interval for `wait_for_clearance`.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Drives a Cloudflare Turnstile clearance flow against a single tab's session.
///
/// Constructed via `Tab::cloudflare()` (lands in T15).
pub struct CloudflareBypass<'a> {
    #[allow(dead_code)]
    pub(crate) session: &'a SessionHandle,
    #[allow(dead_code)]
    pub(crate) poll_interval: Duration,
}

impl<'a> CloudflareBypass<'a> {
    /// Create a new bypass driver bound to `session`.
    pub fn new(session: &'a SessionHandle) -> Self {
        Self {
            session,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    /// Override the default 500ms poll interval used by `wait_for_clearance`.
    #[must_use]
    pub fn poll_interval(mut self, dur: Duration) -> Self {
        self.poll_interval = dur;
        self
    }
}
