//! DataDome bypass driver.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use zendriver_transport::SessionHandle;

use crate::captcha::CaptchaSolver;
use crate::detection::{DataDomeSurface, DetectionSnapshot};
use crate::error::DataDomeError;

pub(crate) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Terminal outcome of a clearance attempt. All flow-terminals are `Ok`;
/// only genuine faults are [`DataDomeError`].
#[derive(Debug, Clone)]
pub enum ClearanceOutcome {
    /// datadome cookie acquired AND challenge markers gone.
    Cleared { datadome: String },
    /// Markers cleared but no datadome cookie observed (rare / legacy path).
    ChallengeGone,
    /// No DataDome surface present at call time. Fast path; no waiting.
    AlreadyClear,
    /// window.dd.t == 'bv' — IP banned. Nothing in-browser can clear this;
    /// the caller must change IP (e.g. residential proxy).
    Blocked,
    /// Deadline elapsed without reaching a terminal state.
    TimedOut {
        last_surface: Option<DataDomeSurface>,
    },
}

/// Drives a DataDome clearance flow against a single tab's session.
///
/// Constructed via `Tab::datadome()`.
pub struct DataDomeBypass<'tab> {
    pub(crate) session: &'tab SessionHandle,
    pub(crate) poll_interval: Duration,
    pub(crate) timeout: Duration,
    pub(crate) on_captcha: Option<Arc<CaptchaSolver>>,
    pub(crate) interception_enabled: bool,
}

impl std::fmt::Debug for DataDomeBypass<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataDomeBypass")
            .field("poll_interval", &self.poll_interval)
            .field("timeout", &self.timeout)
            .field("on_captcha", &self.on_captcha.as_ref().map(|_| "..."))
            .field("interception_enabled", &self.interception_enabled)
            .finish()
    }
}

impl<'tab> DataDomeBypass<'tab> {
    /// New driver with default 250ms poll interval + 30s timeout.
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

    /// Enable the Fetch-domain fast-path. Spawns a `Fetch` subscription that
    /// signals on first 2xx response to `captcha-delivery.com` or any
    /// `datadome*` URL, waking the poll loop immediately.
    #[must_use]
    pub fn with_interception(mut self) -> Self {
        self.interception_enabled = true;
        self
    }

    /// Register a caller-supplied async CAPTCHA solver. Without it, a CAPTCHA
    /// surface yields [`DataDomeError::CaptchaRequired`].
    ///
    /// The solver receives a [`DataDomeChallenge`] (captcha URL, page URL, UA,
    /// cid, hash) and returns a [`DataDomeSolution`] carrying the solved
    /// `datadome` cookie — wire it to a service like 2captcha / capsolver.
    ///
    /// [`DataDomeChallenge`]: crate::captcha::DataDomeChallenge
    /// [`DataDomeSolution`]: crate::captcha::DataDomeSolution
    #[must_use]
    pub fn on_captcha<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(crate::captcha::DataDomeChallenge) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<
                Output = Result<
                    crate::captcha::DataDomeSolution,
                    Box<dyn std::error::Error + Send + Sync>,
                >,
            > + Send
            + 'static,
    {
        self.on_captcha = Some(crate::captcha::arc_solver(f));
        self
    }

    /// Run the surface-aware poll loop until clearance, block, or timeout.
    ///
    /// # Returns
    /// - `Cleared { datadome }` — cookie landed and markers gone.
    /// - `ChallengeGone` — markers gone, no cookie observed.
    /// - `AlreadyClear` — no surface at call time.
    /// - `Blocked` — `window.dd.t == 'bv'` (IP banned).
    /// - `TimedOut { last_surface }` — deadline elapsed.
    ///
    /// # Errors
    /// - [`DataDomeError::CaptchaRequired`] — captcha surface, no solver.
    /// - [`DataDomeError::CaptchaSolver`] — registered solver errored.
    /// - [`DataDomeError::Interception`] / [`DataDomeError::Call`] /
    ///   [`DataDomeError::JsError`].
    pub async fn wait_for_clearance(self) -> Result<ClearanceOutcome, DataDomeError> {
        let deadline = Instant::now() + self.timeout;

        let snapshot = match tokio::time::timeout_at(
            deadline,
            crate::detection::detect_snapshot(self.session),
        )
        .await
        {
            Ok(res) => res?,
            Err(_) => return Ok(ClearanceOutcome::TimedOut { last_surface: None }),
        };

        // Only the "nothing to clear" fast path is decided out here — it is
        // the one terminal the loop cannot express (inside the loop, a clean
        // page means the challenge *resolved*). Block and CAPTCHA escalation
        // live in the loop so they also fire when the surface changes
        // mid-flight, which is the normal shape: a device check hands off to
        // a CAPTCHA, or to a ban, seconds in.
        if snapshot.surface == DataDomeSurface::None && snapshot.body_clean {
            return Ok(ClearanceOutcome::AlreadyClear);
        }

        // Optional Fetch-domain fast-path. Spawned before any escalation so
        // the CAPTCHA route gets the same fast wake-up as the device-check
        // route. The guard's `Drop` cooperatively cancels the spawned task
        // (token check at every loop boundary) with `abort()` as a backstop.
        let (interception_rx, interception_guard) = if self.interception_enabled {
            let (rx, guard) = crate::interception::spawn_signal(self.session);
            (Some(rx), Some(guard))
        } else {
            (None, None)
        };
        self.poll_loop(
            deadline,
            Some(snapshot),
            interception_rx,
            interception_guard,
        )
        .await
    }

    /// Escalate a CAPTCHA surface to the registered solver and apply the
    /// solved cookie. Errors with [`DataDomeError::CaptchaRequired`] when no
    /// solver is registered. On success the page has been reloaded, so the
    /// caller must re-probe rather than reuse `snap`.
    async fn solve_captcha(&self, snap: &DetectionSnapshot) -> Result<(), DataDomeError> {
        let Some(solver) = self.on_captcha.clone() else {
            return Err(DataDomeError::CaptchaRequired);
        };
        let challenge = crate::captcha::build_challenge(self.session, snap).await?;
        let site_url = challenge.site_url.clone();
        let solution = solver(challenge)
            .await
            .map_err(DataDomeError::CaptchaSolver)?;
        crate::captcha::apply_solution(self.session, &solution, &site_url).await
    }

    /// Core poll loop. Owns every mid-flight terminal — clearance, ban, and
    /// CAPTCHA escalation — so a surface that changes after the pre-loop
    /// probe is handled exactly like one present from the start.
    ///
    /// Factored out so unit tests can drive it with a controlled interception
    /// receiver without standing up the real `Fetch.enable` plumbing.
    pub(crate) async fn poll_loop(
        self,
        deadline: Instant,
        mut next_snapshot: Option<DetectionSnapshot>,
        mut interception_rx: Option<tokio::sync::oneshot::Receiver<()>>,
        _interception_guard: Option<crate::interception::InterceptionGuard>,
    ) -> Result<ClearanceOutcome, DataDomeError> {
        use tokio::time::{Interval, MissedTickBehavior};

        let mut ticker: Interval = tokio::time::interval(self.poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        // First iter always either returns or writes `last_surface` before
        // any read, but the deadline-check branches keep a read path alive.
        #[allow(unused_assignments)]
        let mut last_surface: Option<DataDomeSurface> = None;

        loop {
            let snap = match next_snapshot.take() {
                Some(s) => s,
                None => match tokio::time::timeout_at(
                    deadline,
                    crate::detection::detect_snapshot(self.session),
                )
                .await
                {
                    Ok(res) => res?,
                    Err(_) => return Ok(ClearanceOutcome::TimedOut { last_surface }),
                },
            };

            // Escalations are polled, not assumed to arrive on the first
            // probe: DataDome routinely upgrades a device check to a CAPTCHA
            // (or drops it to a ban) a few seconds into the interstitial.
            match snap.surface {
                DataDomeSurface::Block => return Ok(ClearanceOutcome::Blocked),
                DataDomeSurface::Captcha => {
                    self.solve_captcha(&snap).await?;
                    // Page reloaded — next iteration re-probes from scratch.
                    last_surface = Some(snap.surface);
                }
                _ => {
                    let cookie = snap.datadome.as_ref().filter(|v| !v.is_empty());
                    match (cookie, snap.body_clean) {
                        (Some(c), true) => {
                            return Ok(ClearanceOutcome::Cleared {
                                datadome: c.clone(),
                            });
                        }
                        (None, true) if snap.surface == DataDomeSurface::None => {
                            return Ok(ClearanceOutcome::ChallengeGone);
                        }
                        _ => last_surface = Some(snap.surface),
                    }
                }
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
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn builder_defaults_and_overrides() {
        let (_, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let b = DataDomeBypass::new(&sess);
        assert_eq!(b.poll_interval, DEFAULT_POLL_INTERVAL);
        assert_eq!(b.timeout, DEFAULT_TIMEOUT);
        assert!(b.on_captcha.is_none());
        assert!(!b.interception_enabled);

        let b = b
            .timeout(std::time::Duration::from_secs(45))
            .poll_interval(std::time::Duration::from_millis(100))
            .with_interception();
        assert_eq!(b.timeout, std::time::Duration::from_secs(45));
        assert_eq!(b.poll_interval, std::time::Duration::from_millis(100));
        assert!(b.interception_enabled);
        conn.shutdown();
    }

    use crate::detection::DataDomeSurface;
    use serde_json::json;

    fn snap_reply(v: serde_json::Value) -> serde_json::Value {
        json!({ "result": { "type": "object", "value": v } })
    }

    #[tokio::test]
    async fn already_clear_on_clean_page() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move { DataDomeBypass::new(&s).wait_for_clearance().await }
        });
        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            snap_reply(
                json!({"surface":"none","datadome":"DD","dd":null,"captcha_url":null,"body_clean":true}),
            ),
        )
        .await;
        assert!(matches!(
            fut.await.unwrap().unwrap(),
            ClearanceOutcome::AlreadyClear
        ));
        conn.shutdown();
    }

    #[tokio::test]
    async fn block_is_immediate_terminal() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move { DataDomeBypass::new(&s).wait_for_clearance().await }
        });
        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            snap_reply(
                json!({"surface":"block","datadome":null,"dd":{"cid":"C","hsh":"H","t":"bv","host":"h"},"captcha_url":null,"body_clean":false}),
            ),
        )
        .await;
        assert!(matches!(
            fut.await.unwrap().unwrap(),
            ClearanceOutcome::Blocked
        ));
        conn.shutdown();
    }

    #[tokio::test]
    async fn device_check_clears_when_cookie_lands_and_body_clean() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                DataDomeBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .wait_for_clearance()
                    .await
            }
        });
        // Probe 1: device-check, no cookie.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snap_reply(
                json!({"surface":"device_check","datadome":null,"dd":{"cid":"C","hsh":"H","t":"fe","host":"h"},"captcha_url":null,"body_clean":false}),
            ),
        )
        .await;
        // Probe 2: cleared.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            snap_reply(
                json!({"surface":"none","datadome":"COOKIE_OK","dd":null,"captcha_url":null,"body_clean":true}),
            ),
        )
        .await;
        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::Cleared { datadome } => assert_eq!(datadome, "COOKIE_OK"),
            other => panic!("expected Cleared, got {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn times_out_when_device_check_never_clears() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                DataDomeBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .timeout(Duration::from_millis(40))
                    .wait_for_clearance()
                    .await
            }
        });
        for _ in 0..50 {
            let Ok(id) = tokio::time::timeout(
                Duration::from_millis(80),
                mock.expect_cmd("Runtime.evaluate"),
            )
            .await
            else {
                break;
            };
            mock.reply(
                id,
                snap_reply(
                    json!({"surface":"device_check","datadome":null,"dd":{"cid":"C","hsh":"H","t":"fe","host":"h"},"captcha_url":null,"body_clean":false}),
                ),
            )
            .await;
        }
        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::TimedOut { last_surface } => {
                assert_eq!(last_surface, Some(DataDomeSurface::DeviceCheck));
            }
            other => panic!("expected TimedOut, got {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn captcha_with_solver_applies_cookie_then_clears() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                DataDomeBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .on_captcha(|ch| async move {
                        assert!(ch.captcha_url.contains("captcha-delivery.com"));
                        Ok(crate::captcha::DataDomeSolution {
                            datadome_cookie: "FROM_SOLVER".into(),
                        })
                    })
                    .wait_for_clearance()
                    .await
            }
        });

        // Pre-loop probe: captcha surface.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snap_reply(
                json!({"surface":"captcha","datadome":"OLD","dd":{"cid":"C","hsh":"H","t":"fe","host":"h"},"captcha_url":"https://geo.captcha-delivery.com/captcha/?cid=C","body_clean":false}),
            ),
        )
        .await;
        // build_challenge eval (url + ua).
        let id_ctx = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_ctx,
            json!({"result":{"type":"object","value":{"url":"https://shop.example.com/p","ua":"Mozilla/5.0"}}}),
        )
        .await;
        // apply_solution: setCookie + reload.
        let id_cookie = mock.expect_cmd("Network.setCookie").await;
        mock.reply(id_cookie, json!({"success":true})).await;
        let id_reload = mock.expect_cmd("Page.reload").await;
        mock.reply(id_reload, json!({})).await;
        // Post-reload poll: cleared.
        let id_poll = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_poll,
            snap_reply(
                json!({"surface":"none","datadome":"FROM_SOLVER","dd":null,"captcha_url":null,"body_clean":true}),
            ),
        )
        .await;

        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::Cleared { datadome } => assert_eq!(datadome, "FROM_SOLVER"),
            other => panic!("expected Cleared, got {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn pre_loop_probe_respects_timeout() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                DataDomeBypass::new(&s)
                    .timeout(Duration::from_millis(30))
                    .wait_for_clearance()
                    .await
            }
        });
        // Receive the probe command but NEVER reply → the timeout must fire.
        let _id = mock.expect_cmd("Runtime.evaluate").await;
        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::TimedOut { last_surface } => assert_eq!(last_surface, None),
            other => panic!("expected TimedOut, got {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn captcha_without_solver_errors() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move { DataDomeBypass::new(&s).wait_for_clearance().await }
        });
        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            snap_reply(
                json!({"surface":"captcha","datadome":null,"dd":null,"captcha_url":"https://geo.captcha-delivery.com/captcha/?cid=C","body_clean":false}),
            ),
        )
        .await;
        assert!(matches!(
            fut.await.unwrap().unwrap_err(),
            DataDomeError::CaptchaRequired
        ));
        conn.shutdown();
    }

    #[tokio::test]
    async fn poll_loop_probe_hang_respects_deadline() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                DataDomeBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .poll_loop(Instant::now() + Duration::from_millis(30), None, None, None)
                    .await
            }
        });
        // First in-loop probe is received but NEVER answered → deadline fires.
        let _id = mock.expect_cmd("Runtime.evaluate").await;
        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::TimedOut { .. } => {}
            other => panic!("expected TimedOut, got {other:?}"),
        }
        conn.shutdown();
    }

    /// Generic bounded read of the next outbound command, for flows whose
    /// command order is not fully deterministic (the interception task emits
    /// `Fetch.enable` from its own spawn).
    async fn next_cmd(mock: &mut MockConnection) -> Option<(String, u64)> {
        for _ in 0..300 {
            if let Some(cmd) = mock.try_recv_cmd() {
                return Some(cmd);
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        None
    }

    fn device_check_snap() -> serde_json::Value {
        json!({"surface":"device_check","datadome":null,"dd":{"cid":"C","hsh":"H","t":"fe","host":"h"},"captcha_url":null,"body_clean":false})
    }

    fn captcha_snap() -> serde_json::Value {
        json!({"surface":"captcha","datadome":"OLD","dd":{"cid":"C","hsh":"H","t":"fe","host":"h"},"captcha_url":"https://geo.captcha-delivery.com/captcha/?cid=C","body_clean":false})
    }

    /// The normal DataDome escalation shape: the interstitial starts as a
    /// device check and only *then* hands off to a CAPTCHA. Escalation used
    /// to be checked on the pre-loop probe only, so this arrival never
    /// reached the registered solver.
    #[tokio::test]
    async fn device_check_escalating_to_captcha_invokes_solver() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                DataDomeBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .on_captcha(|ch| async move {
                        assert!(ch.captcha_url.contains("captcha-delivery.com"));
                        Ok(crate::captcha::DataDomeSolution {
                            datadome_cookie: "FROM_SOLVER".into(),
                        })
                    })
                    .wait_for_clearance()
                    .await
            }
        });

        // Probe 1 (pre-loop): device check — no escalation yet.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id1, snap_reply(device_check_snap())).await;

        // Probe 2 (in-loop): the device check escalated to a CAPTCHA.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id2, snap_reply(captcha_snap())).await;

        // Solver path: build_challenge eval → setCookie → reload.
        let id_ctx = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_ctx,
            json!({"result":{"type":"object","value":{"url":"https://shop.example.com/p","ua":"Mozilla/5.0"}}}),
        )
        .await;
        let id_cookie = mock.expect_cmd("Network.setCookie").await;
        assert_eq!(mock.last_sent()["params"]["value"], "FROM_SOLVER");
        mock.reply(id_cookie, json!({"success":true})).await;
        let id_reload = mock.expect_cmd("Page.reload").await;
        mock.reply(id_reload, json!({})).await;

        // Probe 3: cleared.
        let id3 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id3,
            snap_reply(
                json!({"surface":"none","datadome":"FROM_SOLVER","dd":null,"captcha_url":null,"body_clean":true}),
            ),
        )
        .await;

        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::Cleared { datadome } => assert_eq!(datadome, "FROM_SOLVER"),
            other => panic!("expected Cleared, got {other:?}"),
        }
        conn.shutdown();
    }

    /// A CAPTCHA that arrives mid-flight with no solver registered must still
    /// produce the actionable `CaptchaRequired` error, not a silent timeout.
    #[tokio::test]
    async fn mid_flight_captcha_without_solver_errors() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                DataDomeBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .wait_for_clearance()
                    .await
            }
        });

        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id1, snap_reply(device_check_snap())).await;
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id2, snap_reply(captcha_snap())).await;

        assert!(matches!(
            fut.await.unwrap().unwrap_err(),
            DataDomeError::CaptchaRequired
        ));
        conn.shutdown();
    }

    /// A ban that lands after the pre-loop probe must terminate as `Blocked`
    /// — the caller needs "change your IP", not "timed out".
    #[tokio::test]
    async fn device_check_escalating_to_block_returns_blocked() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                DataDomeBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .wait_for_clearance()
                    .await
            }
        });

        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id1, snap_reply(device_check_snap())).await;
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            snap_reply(
                json!({"surface":"block","datadome":null,"dd":{"cid":"C","hsh":"H","t":"bv","host":"h"},"captcha_url":null,"body_clean":false}),
            ),
        )
        .await;

        assert!(matches!(
            fut.await.unwrap().unwrap(),
            ClearanceOutcome::Blocked
        ));
        conn.shutdown();
    }

    /// `with_interception()` used to be dropped on the CAPTCHA route: that
    /// branch returned into the poll loop with no receiver and no guard, so
    /// the fast path was silently off exactly where it matters most. A
    /// `Fetch.enable` on the wire proves the subscription is armed.
    #[tokio::test]
    async fn captcha_route_arms_the_interception_fast_path() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                DataDomeBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .with_interception()
                    .on_captcha(|_ch| async move {
                        Ok(crate::captcha::DataDomeSolution {
                            datadome_cookie: "FROM_SOLVER".into(),
                        })
                    })
                    .wait_for_clearance()
                    .await
            }
        });

        let mut saw_fetch_enable = false;
        let mut reloaded = false;
        while let Some((method, id)) = next_cmd(&mut mock).await {
            match method.as_str() {
                "Fetch.enable" => {
                    saw_fetch_enable = true;
                    mock.reply(id, json!({})).await;
                }
                "Runtime.evaluate" => {
                    let expr = mock.last_sent()["params"]["expression"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    if expr.contains("location.href") {
                        mock.reply(
                            id,
                            json!({"result":{"type":"object","value":{"url":"https://shop.example.com/p","ua":"UA"}}}),
                        )
                        .await;
                    } else if reloaded {
                        mock.reply(
                            id,
                            snap_reply(
                                json!({"surface":"none","datadome":"FROM_SOLVER","dd":null,"captcha_url":null,"body_clean":true}),
                            ),
                        )
                        .await;
                    } else {
                        mock.reply(id, snap_reply(captcha_snap())).await;
                    }
                }
                "Network.setCookie" => mock.reply(id, json!({"success":true})).await,
                "Page.reload" => {
                    reloaded = true;
                    mock.reply(id, json!({})).await;
                }
                _ => mock.reply(id, json!({})).await,
            }
        }

        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::Cleared { datadome } => assert_eq!(datadome, "FROM_SOLVER"),
            other => panic!("expected Cleared, got {other:?}"),
        }
        assert!(
            saw_fetch_enable,
            "with_interception() must arm the Fetch fast path on the captcha route too"
        );
        conn.shutdown();
    }

    #[tokio::test]
    async fn interception_signal_triggers_reprobe() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let (tx, rx) = tokio::sync::oneshot::channel();
        tx.send(()).unwrap(); // signal already fired
        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                DataDomeBypass::new(&s)
                    .poll_interval(Duration::from_secs(10))
                    .poll_loop(
                        Instant::now() + Duration::from_secs(5),
                        None,
                        Some(rx),
                        None,
                    )
                    .await
            }
        });
        // The signal arm fires immediately → one detect probe → cleared.
        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            snap_reply(
                json!({"surface":"none","datadome":"DD","dd":null,"captcha_url":null,"body_clean":true}),
            ),
        )
        .await;
        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::Cleared { datadome } => assert_eq!(datadome, "DD"),
            other => panic!("expected Cleared, got {other:?}"),
        }
        conn.shutdown();
    }
}
