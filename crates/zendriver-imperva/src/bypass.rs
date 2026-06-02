//! Imperva bypass driver.
//!
//! Public entry is [`ImpervaBypass`] — constructed via `Tab::imperva()`
//! (zendriver crate, feature-gated). Single-struct dispatch: one
//! `wait_for_clearance` runs the surface-aware poll loop, the optional
//! [`ImpervaBypass::with_interception`] hook enables a Fetch-domain
//! fast-path, and [`ImpervaBypass::on_captcha`] plugs a caller-supplied
//! solver into the CAPTCHA escalation path. See module docs of
//! [`crate::detection`] for surface inference rules.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use zendriver_transport::SessionHandle;

use crate::captcha::{
    CaptchaChallenge, CaptchaSolution, CaptchaSolver, arc_solver, extract_captcha_site_key,
    inject_captcha_solution,
};
use crate::detection::DetectionSnapshot;
use crate::error::ImpervaError;

/// Probe `detect_snapshot` against `session`, racing against the overall
/// `deadline`. Returns `Ok(None)` if the deadline wins so that no probe —
/// including the pre-loop one — can outlive the caller's budget; the caller
/// maps that sentinel to a [`ClearanceOutcome::TimedOut`].
async fn probe_with_deadline(
    session: &SessionHandle,
    deadline: Instant,
) -> Result<Option<DetectionSnapshot>, ImpervaError> {
    tokio::select! {
        res = crate::detection::detect_snapshot(session) => res.map(Some),
        () = tokio::time::sleep_until(deadline) => Ok(None),
    }
}

/// Default poll interval for [`ImpervaBypass::wait_for_clearance`].
pub(crate) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Default overall timeout for [`ImpervaBypass::wait_for_clearance`].
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// Deadline elapsed without clearance. `last_surface` is the most recent
    /// surface the poll loop observed (`None` if the deadline won before the
    /// first probe completed).
    TimedOut {
        last_surface: Option<crate::detection::ImpervaSurface>,
    },
}

/// Drives an Imperva clearance flow against a single tab's session.
///
/// Constructed via `Tab::imperva()`.
pub struct ImpervaBypass<'tab> {
    pub(crate) session: &'tab SessionHandle,
    pub(crate) poll_interval: Duration,
    pub(crate) timeout: Duration,
    pub(crate) on_captcha: Option<Arc<CaptchaSolver>>,
    pub(crate) interception_enabled: bool,
}

impl std::fmt::Debug for ImpervaBypass<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImpervaBypass")
            .field("poll_interval", &self.poll_interval)
            .field("timeout", &self.timeout)
            .field("on_captcha", &self.on_captcha.as_ref().map(|_| "..."))
            .field("interception_enabled", &self.interception_enabled)
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
            interception_enabled: false,
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
    ///
    /// ```no_run
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_imperva::ImpervaError> {
    /// use zendriver_imperva::{CaptchaSolution, ImpervaBypass};
    ///
    /// let outcome = ImpervaBypass::new(tab)
    ///     .on_captcha(|challenge| async move {
    ///         // Call your 2captcha / anticaptcha integration here.
    ///         Ok(CaptchaSolution {
    ///             token: "SOLVER_TOKEN".into(),
    ///             form_field: "h-captcha-response".into(),
    ///         })
    ///     })
    ///     .wait_for_clearance()
    ///     .await?;
    /// println!("{outcome:?}");
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn on_captcha<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(CaptchaChallenge) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<
                Output = Result<CaptchaSolution, Box<dyn std::error::Error + Send + Sync>>,
            > + Send
            + 'static,
    {
        self.on_captcha = Some(arc_solver(f));
        self
    }

    /// Enable the Fetch-domain escape hatch: the bypass driver spins up
    /// its own [`InterceptBuilder::subscribe`] hook against this session,
    /// listening for `Reese.js` / `_Incapsula_Resource` 2xx responses to
    /// signal clearance faster than polling alone.
    ///
    /// ```no_run
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_imperva::ImpervaError> {
    /// use zendriver_imperva::ImpervaBypass;
    ///
    /// let outcome = ImpervaBypass::new(tab)
    ///     .with_interception()
    ///     .wait_for_clearance()
    ///     .await?;
    /// println!("{outcome:?}");
    /// # Ok(()) }
    /// ```
    ///
    /// [`InterceptBuilder::subscribe`]: zendriver_interception::InterceptBuilder::subscribe
    #[must_use]
    pub fn with_interception(mut self) -> Self {
        self.interception_enabled = true;
        self
    }

    /// Run the surface-aware poll loop until clearance is achieved or
    /// the configured timeout elapses.
    ///
    /// # Returns
    /// - `Ok(ClearanceOutcome::AlreadyClear)` — no Imperva surface present
    ///   at call time. Fast path; no waiting.
    /// - `Ok(ClearanceOutcome::TokenAcquired { reese84, sessions })` —
    ///   reese84 cookie was observed AND the page body no longer contains
    ///   Imperva challenge markers (hybrid AND signal).
    /// - `Ok(ClearanceOutcome::ChallengeGone)` — body markers cleared but
    ///   no reese84 token was ever observed (e.g., legacy Incapsula).
    /// - `Ok(ClearanceOutcome::TimedOut { last_surface })` — overall timeout
    ///   elapsed before clearance. `last_surface` carries the most-recent
    ///   observed surface. Not a fault: a deadline in a bot-management flow
    ///   is a normal "didn't finish, retry or give up" terminal.
    ///
    /// # Errors
    /// - [`ImpervaError::CaptchaRequired`] — CAPTCHA surface detected
    ///   but no `on_captcha` solver was registered.
    /// - [`ImpervaError::CaptchaSolver`] — registered solver returned an
    ///   error.
    /// - [`ImpervaError::Interception`] — Fetch-domain hook (when set via
    ///   [`with_interception`](Self::with_interception)) failed.
    /// - [`ImpervaError::Call`] / [`ImpervaError::JsError`] — CDP or
    ///   in-page evaluator failure.
    ///
    /// ```no_run
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_imperva::ImpervaError> {
    /// use std::time::Duration;
    /// use zendriver_imperva::ImpervaBypass;
    ///
    /// let outcome = ImpervaBypass::new(tab)
    ///     .poll_interval(Duration::from_millis(500))
    ///     .timeout(Duration::from_secs(45))
    ///     .wait_for_clearance()
    ///     .await?;
    /// println!("{outcome:?}");
    /// # Ok(()) }
    /// ```
    pub async fn wait_for_clearance(self) -> Result<ClearanceOutcome, ImpervaError> {
        let deadline = Instant::now() + self.timeout;

        // Every probe — including the first — is raced against the deadline.
        // A hung CDP call cannot exceed the caller's stated timeout budget.
        // `None` means the deadline won before the first probe resolved.
        let Some(snapshot) = probe_with_deadline(self.session, deadline).await? else {
            return Ok(ClearanceOutcome::TimedOut { last_surface: None });
        };

        // Fast path: nothing to clear.
        if matches!(snapshot.surface, crate::detection::ImpervaSurface::None) && snapshot.body_clean
        {
            return Ok(ClearanceOutcome::AlreadyClear);
        }

        // CAPTCHA escalation: dispatch to solver or fast-fail.
        if let crate::detection::ImpervaSurface::Captcha(kind) = snapshot.surface {
            let Some(solver) = self.on_captcha.clone() else {
                return Err(ImpervaError::CaptchaRequired { kind });
            };
            let (site_key, url) = extract_captcha_site_key(self.session, kind).await?;
            let solution = solver(CaptchaChallenge {
                kind,
                site_key,
                url,
            })
            .await
            .map_err(ImpervaError::CaptchaSolver)?;
            inject_captcha_solution(self.session, &solution).await?;
            // Drop the pre-injection snapshot so the loop re-probes
            // immediately rather than wasting an iteration on stale state.
        }

        // Prime the loop with the pre-loop snapshot UNLESS we just ran the
        // CAPTCHA branch — in that case the page has been mutated since the
        // snapshot was taken, so force a re-probe.
        let next_snapshot = match snapshot.surface {
            crate::detection::ImpervaSurface::Captcha(_) => None,
            _ => Some(snapshot),
        };

        // Optional Fetch-domain fast-path. The guard's `Drop` cooperatively
        // cancels the spawned task (token check at every loop boundary)
        // with `abort()` as a backstop.
        let (interception_rx, interception_guard) = if self.interception_enabled {
            let (rx, guard) = crate::interception::spawn_signal(self.session);
            (Some(rx), Some(guard))
        } else {
            (None, None)
        };

        self.poll_loop(deadline, next_snapshot, interception_rx, interception_guard)
            .await
    }

    /// Core poll loop, factored out so unit tests can drive it with a
    /// controlled interception receiver without standing up the real
    /// InterceptBuilder + Fetch.enable plumbing. Production callers go
    /// through [`wait_for_clearance`](Self::wait_for_clearance), which
    /// constructs the receiver via [`spawn_signal`].
    ///
    /// [`spawn_signal`]: crate::interception::spawn_signal
    pub(crate) async fn poll_loop(
        self,
        deadline: Instant,
        mut next_snapshot: Option<DetectionSnapshot>,
        mut interception_rx: Option<tokio::sync::oneshot::Receiver<()>>,
        _interception_guard: Option<crate::interception::InterceptionGuard>,
    ) -> Result<ClearanceOutcome, ImpervaError> {
        use tokio::time::{Interval, MissedTickBehavior};

        let mut ticker: Interval = tokio::time::interval(self.poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        // Initial `None` is unreachable in practice (first iter always either
        // returns or writes `last_surface` before any read), but the
        // deadline-check branch keeps a read path alive for the borrow
        // checker.
        #[allow(unused_assignments)]
        let mut last_surface: Option<crate::detection::ImpervaSurface> = None;

        let mut prev_surface: Option<crate::detection::ImpervaSurface> = None;
        let mut stall_ticks: u32 = 0;
        let mut warned_stall = false;

        loop {
            let snap = match next_snapshot.take() {
                Some(s) => s,
                None => match probe_with_deadline(self.session, deadline).await? {
                    Some(s) => s,
                    // Deadline won the probe race — terminate with the
                    // most-recent surface observed so far.
                    None => return Ok(ClearanceOutcome::TimedOut { last_surface }),
                },
            };

            let token = snap.reese84.as_ref().filter(|v| !v.is_empty());
            let surface_clear = matches!(snap.surface, crate::detection::ImpervaSurface::None);
            match (token, snap.body_clean, surface_clear) {
                (Some(token), true, _) => {
                    return Ok(ClearanceOutcome::TokenAcquired {
                        reese84: token.clone(),
                        sessions: snap.sessions,
                    });
                }
                // ChallengeGone requires `surface_clear` in addition to the
                // body+no-token spec criteria: lingering Legacy cookies
                // (`incap_ses_*`, `___utmvc`) indicate the page is still
                // tracking the challenge even after body markers drop, and
                // returning ChallengeGone there would race the page's own
                // post-clearance navigation.
                (None, true, true) => return Ok(ClearanceOutcome::ChallengeGone),
                _ => last_surface = Some(snap.surface),
            }

            stall_ticks = if Some(snap.surface) == prev_surface {
                stall_ticks + 1
            } else {
                0
            };
            prev_surface = Some(snap.surface);
            if stall_ticks == 10 && !warned_stall {
                tracing::warn!(
                    surface = ?snap.surface,
                    poll_interval_ms = self.poll_interval.as_millis() as u64,
                    "imperva clearance stalled — is BrowserBuilder::stealth enabled?"
                );
                warned_stall = true;
            }

            if Instant::now() >= deadline {
                return Ok(ClearanceOutcome::TimedOut { last_surface });
            }

            tokio::select! {
                _ = ticker.tick() => {}
                () = tokio::time::sleep_until(deadline) => {
                    return Ok(ClearanceOutcome::TimedOut { last_surface });
                }
                Ok(()) = async {
                    match interception_rx.as_mut() {
                        Some(rx) => rx.await,
                        // Never-resolves fallback keeps this arm inert when
                        // interception is disabled; `select!` polls the
                        // remaining arms unaffected.
                        None => std::future::pending().await,
                    }
                } => {
                    // Drop the receiver so we don't re-await a closed channel
                    // on the next iteration; loop body re-probes immediately.
                    interception_rx = None;
                }
            }
        }
    }

    /// One-shot probe: returns `true` iff [`detect_surface`] returns
    /// anything other than `ImpervaSurface::None`.
    ///
    /// [`detect_surface`]: crate::detection::detect_surface
    pub async fn is_challenge_present(&self) -> Result<bool, ImpervaError> {
        Ok(!matches!(
            crate::detection::detect_surface(self.session).await?,
            crate::detection::ImpervaSurface::None,
        ))
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
        assert!(!b.interception_enabled);
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

    use crate::detection::{CaptchaKind, ImpervaSurface};
    use serde_json::json;

    fn snapshot_reply(payload: serde_json::Value) -> serde_json::Value {
        json!({
            "result": {
                "type": "object",
                "value": payload,
            }
        })
    }

    #[tokio::test]
    async fn wait_for_clearance_returns_already_clear_on_clean_page() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { ImpervaBypass::new(&s).wait_for_clearance().await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": null,
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": false,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(matches!(outcome, ClearanceOutcome::AlreadyClear));
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_returns_token_when_both_signals_hit() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .wait_for_clearance()
                    .await
            }
        });

        // First probe: surface present, no clearance yet.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Reese84" },
                "reese84": null,
                "body_clean": false,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        // Second probe: token + body clean → TokenAcquired.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": "TOK_ABC",
                "body_clean": true,
                "sessions": [{ "name": "reese84", "value": "TOK_ABC" }],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        match outcome {
            ClearanceOutcome::TokenAcquired { reese84, sessions } => {
                assert_eq!(reese84, "TOK_ABC");
                assert_eq!(sessions.len(), 1);
                assert_eq!(sessions[0].name, "reese84");
            }
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_holds_when_cookie_only_no_body_clean() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .timeout(Duration::from_millis(50))
                    .wait_for_clearance()
                    .await
            }
        });

        // Always reply cookie-only.
        for _ in 0..50 {
            let Ok(id) = tokio::time::timeout(
                Duration::from_millis(100),
                mock.expect_cmd("Runtime.evaluate"),
            )
            .await
            else {
                break;
            };
            mock.reply(
                id,
                snapshot_reply(json!({
                    "surface": { "kind": "Reese84" },
                    "reese84": "TOK",
                    "body_clean": false,
                    "sessions": [],
                    "has_imperva_signal": true,
                })),
            )
            .await;
        }

        let outcome = fut.await.unwrap().unwrap();
        match outcome {
            ClearanceOutcome::TimedOut { last_surface } => {
                assert_eq!(last_surface, Some(ImpervaSurface::Reese84));
            }
            other => panic!("expected TimedOut, got {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_returns_challenge_gone_when_body_clean_no_token() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .wait_for_clearance()
                    .await
            }
        });

        // Surface present at start.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Legacy" },
                "reese84": null,
                "body_clean": false,
                "sessions": [{ "name": "incap_ses_123", "value": "X" }],
                "has_imperva_signal": true,
            })),
        )
        .await;

        // Then body becomes clean, but no reese84 ever sets.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": null,
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": false,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(matches!(outcome, ClearanceOutcome::ChallengeGone));
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_returns_captcha_required_when_no_solver() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { ImpervaBypass::new(&s).wait_for_clearance().await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            snapshot_reply(json!({
                "surface": { "kind": "Captcha", "captcha": "HCaptcha" },
                "reese84": null,
                "body_clean": false,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let err = fut.await.unwrap().unwrap_err();
        assert!(matches!(
            err,
            ImpervaError::CaptchaRequired {
                kind: CaptchaKind::HCaptcha
            }
        ));
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_treats_empty_reese84_as_unset() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .wait_for_clearance()
                    .await
            }
        });

        // Surface present, cookie is empty string → not yet set.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Reese84" },
                "reese84": "",
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        // Expect that the loop did NOT return TokenAcquired with empty token.
        // Next probe returns ChallengeGone (no reese84, body clean).
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": null,
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": false,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(
            matches!(outcome, ClearanceOutcome::ChallengeGone),
            "empty reese84 must not produce TokenAcquired"
        );
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_invokes_captcha_callback_and_resumes() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .on_captcha(|c| async move {
                        assert!(matches!(c.kind, CaptchaKind::HCaptcha));
                        assert_eq!(c.site_key.as_deref(), Some("KEY_ABC"));
                        Ok(CaptchaSolution {
                            token: "SOLVED_TOK".into(),
                            form_field: "h-captcha-response".into(),
                        })
                    })
                    .wait_for_clearance()
                    .await
            }
        });

        // 1. Detect snapshot → CAPTCHA.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Captcha", "captcha": "HCaptcha" },
                "reese84": null,
                "body_clean": false,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        // 2. Site-key probe.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "hcap": "KEY_ABC",
                        "rcap": null,
                        "url": "https://example.com/protected",
                    }
                }
            }),
        )
        .await;

        // 3. Solution injection.
        let id3 = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent();
        assert!(
            sent["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("SOLVED_TOK"),
            "injection script should contain the solver token"
        );
        assert!(
            sent["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("h-captcha-response"),
            "injection script should target the form_field"
        );
        mock.reply(
            id3,
            json!({ "result": { "type": "boolean", "value": true } }),
        )
        .await;

        // 4. Resumed poll: cleared.
        let id4 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id4,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": "TOK_FINAL",
                "body_clean": true,
                "sessions": [{ "name": "reese84", "value": "TOK_FINAL" }],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(matches!(
            outcome,
            ClearanceOutcome::TokenAcquired { reese84, .. } if reese84 == "TOK_FINAL"
        ));
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_propagates_captcha_solver_error() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .on_captcha(|_c| async move {
                        Err(Box::<dyn std::error::Error + Send + Sync>::from(
                            "solver down",
                        ))
                    })
                    .wait_for_clearance()
                    .await
            }
        });

        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Captcha", "captcha": "Recaptcha" },
                "reese84": null,
                "body_clean": false,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "hcap": null,
                        "rcap": "RKEY",
                        "url": "https://x.com/",
                    }
                }
            }),
        )
        .await;

        let err = fut.await.unwrap().unwrap_err();
        assert!(matches!(err, ImpervaError::CaptchaSolver(_)));
        conn.shutdown();
    }

    #[tokio::test]
    async fn poll_loop_interception_signal_preempts_polling_tick() {
        // Real fast-path behavior: drive `poll_loop` directly with a
        // controlled oneshot. With a long poll_interval (1s), the only way
        // the loop runs a second probe within the test's wall-clock budget
        // is if the interception arm wakes it. Sending on the oneshot
        // before the first tick should preempt the sleep and trigger an
        // immediate re-probe.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let bypass = ImpervaBypass::new(&s).poll_interval(Duration::from_secs(1));
                let deadline = Instant::now() + Duration::from_secs(5);
                bypass.poll_loop(deadline, None, Some(rx), None).await
            }
        });

        // First probe (immediate, no tick wait yet) — surface present, no
        // clearance signals.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Reese84" },
                "reese84": null,
                "body_clean": false,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        // Fire the interception oneshot — should preempt the 1s tick wait.
        tx.send(()).unwrap();

        // Second probe arrives almost immediately because the interception
        // arm won the select!. Reply with clearance.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": "FAST_PATH_TOK",
                "body_clean": true,
                "sessions": [{ "name": "reese84", "value": "FAST_PATH_TOK" }],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(matches!(
            outcome,
            ClearanceOutcome::TokenAcquired { reese84, .. } if reese84 == "FAST_PATH_TOK"
        ));
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_with_interception_disabled_polls_normally() {
        // Regression guard for the select! shape: with
        // `interception_enabled = false`, the never-resolves fallback arm
        // must not starve the polling arm. Plain poll-to-clearance run.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = ImpervaBypass::new(&s).poll_interval(Duration::from_millis(1));
                // Sanity-check the new builder still flips the flag we
                // expect — the rest of the run is poll-only.
                assert!(!b.interception_enabled);
                b.wait_for_clearance().await
            }
        });

        // First probe: surface present, no clearance — drives one loop
        // iteration so the new select! runs at least once.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Reese84" },
                "reese84": null,
                "body_clean": false,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        // Second probe: token + body clean → TokenAcquired.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": "X",
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(matches!(
            outcome,
            ClearanceOutcome::TokenAcquired { reese84, .. } if reese84 == "X"
        ));
        conn.shutdown();
    }
}
