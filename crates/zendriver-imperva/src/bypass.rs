//! Imperva bypass driver.
//!
//! Public entry is [`ImpervaBypass`] — constructed via `Tab::imperva()`
//! (zendriver crate, feature-gated). Single-struct dispatch: one
//! `wait_for_clearance` runs the surface-aware poll loop, the optional
//! [`ImpervaBypass::with_interception`] hook enables a Fetch-domain
//! fast-path, and [`ImpervaBypass::on_captcha`] plugs a caller-supplied
//! solver into the CAPTCHA escalation path. See module docs of
//! [`crate::detection`] for surface inference rules.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use zendriver_transport::SessionHandle;

use crate::detection::CaptchaKind;

/// Default poll interval for [`ImpervaBypass::wait_for_clearance`].
pub(crate) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Default overall timeout for [`ImpervaBypass::wait_for_clearance`].
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// CAPTCHA escalation handed to a user-supplied solver.
#[derive(Debug, Clone)]
pub struct CaptchaChallenge {
    pub kind: CaptchaKind,
    /// Site key extracted from the embed (hCaptcha / reCAPTCHA). `None`
    /// if the kind is `ImpervaNative` or `Unknown`.
    pub site_key: Option<String>,
    /// URL of the page presenting the CAPTCHA.
    pub url: String,
}

/// Token returned by a user-supplied CAPTCHA solver.
#[derive(Debug, Clone)]
pub struct CaptchaSolution {
    /// Verification token issued by the solver service.
    pub token: String,
    /// DOM field where the token must be injected for the page to accept it
    /// (e.g. `"h-captcha-response"`, `"g-recaptcha-response"`).
    pub form_field: String,
}

/// Outcome of a successful `wait_for_clearance`.
#[derive(Debug, Clone)]
pub enum ClearanceOutcome {
    /// reese84 cookie acquired AND body markers gone (S3 hybrid signal).
    TokenAcquired {
        reese84: String,
        sessions: Vec<crate::detection::CookieSnapshot>,
    },
    /// Body markers gone but no reese84 token (e.g., legacy Incapsula flow).
    ChallengeGone,
    /// No Imperva surface present at call time. Fast path; no waiting.
    AlreadyClear,
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(crate) type CaptchaSolver = dyn Fn(
        CaptchaChallenge,
    ) -> BoxFuture<'static, Result<CaptchaSolution, Box<dyn std::error::Error + Send + Sync>>>
    + Send
    + Sync;

/// Drives an Imperva clearance flow against a single tab's session.
///
/// Constructed via `Tab::imperva()`.
pub struct ImpervaBypass<'tab> {
    #[expect(dead_code, reason = "wired in Task 4 wait_for_clearance")]
    pub(crate) session: &'tab SessionHandle,
    pub(crate) poll_interval: Duration,
    pub(crate) timeout: Duration,
    pub(crate) on_captcha: Option<Arc<CaptchaSolver>>,
    pub(crate) interceptor: Option<&'tab zendriver_interception::InterceptHandle>,
}

impl std::fmt::Debug for ImpervaBypass<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImpervaBypass")
            .field("poll_interval", &self.poll_interval)
            .field("timeout", &self.timeout)
            .field("on_captcha", &self.on_captcha.as_ref().map(|_| "..."))
            .field("interceptor", &self.interceptor.is_some())
            .finish()
    }
}

impl<'tab> ImpervaBypass<'tab> {
    /// Create a new bypass driver bound to `session` with default 250ms
    /// poll interval and 30s timeout.
    pub fn new(session: &'tab SessionHandle) -> Self {
        Self {
            session,
            poll_interval: DEFAULT_POLL_INTERVAL,
            timeout: DEFAULT_TIMEOUT,
            on_captcha: None,
            interceptor: None,
        }
    }

    /// Override the default 30s overall timeout.
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        self
    }

    /// Override the default 250ms poll interval.
    #[must_use]
    pub fn poll_interval(mut self, dur: Duration) -> Self {
        self.poll_interval = dur;
        self
    }

    /// Register a user-supplied async CAPTCHA solver. Without this, a
    /// CAPTCHA surface returns [`ImpervaError::CaptchaRequired`] immediately.
    #[must_use]
    pub fn on_captcha<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(CaptchaChallenge) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<CaptchaSolution, Box<dyn std::error::Error + Send + Sync>>>
            + Send
            + 'static,
    {
        self.on_captcha = Some(Arc::new(move |challenge| Box::pin(f(challenge))));
        self
    }

    /// Enable the Fetch-domain escape hatch: subscribe to
    /// `/_Incapsula_Resource*` and `Reese.js` responses for faster
    /// token-set detection than polling alone.
    #[must_use]
    pub fn with_interception(
        mut self,
        interceptor: &'tab zendriver_interception::InterceptHandle,
    ) -> Self {
        self.interceptor = Some(interceptor);
        self
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn builder_defaults_match_constants() {
        let (_, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let b = ImpervaBypass::new(&sess);
        assert_eq!(b.poll_interval, DEFAULT_POLL_INTERVAL);
        assert_eq!(b.timeout, DEFAULT_TIMEOUT);
        assert!(b.on_captcha.is_none());
        assert!(b.interceptor.is_none());
        conn.shutdown();
    }

    #[tokio::test]
    async fn builder_methods_override_defaults() {
        let (_, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let b = ImpervaBypass::new(&sess)
            .timeout(Duration::from_secs(60))
            .poll_interval(Duration::from_millis(100))
            .on_captcha(|_c| async move {
                Ok(CaptchaSolution {
                    token: "T".into(),
                    form_field: "f".into(),
                })
            });
        assert_eq!(b.timeout, Duration::from_secs(60));
        assert_eq!(b.poll_interval, Duration::from_millis(100));
        assert!(b.on_captcha.is_some());
        conn.shutdown();
    }
}
