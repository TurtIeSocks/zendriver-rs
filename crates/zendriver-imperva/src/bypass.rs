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
use crate::error::ImpervaError;

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

/// Extract a CAPTCHA site key from the current page via a small inline JS
/// probe. Returns `None` if no recognizable embed is present.
async fn extract_captcha_site_key(
    session: &SessionHandle,
    kind: CaptchaKind,
) -> Result<(Option<String>, String), ImpervaError> {
    use serde_json::json;

    const PROBE_JS: &str = r#"
    (function () {
        function findKey(selector, attr) {
            var el = document.querySelector(selector);
            return el ? el.getAttribute(attr) : null;
        }
        var hcap =
            findKey(".h-captcha", "data-sitekey") ||
            findKey("[data-hcaptcha-sitekey]", "data-hcaptcha-sitekey");
        var rcap =
            findKey(".g-recaptcha", "data-sitekey") ||
            findKey("[data-recaptcha-sitekey]", "data-recaptcha-sitekey");
        return { hcap: hcap, rcap: rcap, url: location.href };
    })()
    "#;

    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": PROBE_JS,
                "returnByValue": true,
            }),
        )
        .await?;
    let value = res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    #[derive(serde::Deserialize)]
    struct Probe {
        hcap: Option<String>,
        rcap: Option<String>,
        url: String,
    }
    let probe: Probe = serde_json::from_value(value)
        .map_err(|e| ImpervaError::JsError(format!("invalid captcha probe payload: {e}")))?;

    let site_key = match kind {
        CaptchaKind::HCaptcha => probe.hcap,
        CaptchaKind::Recaptcha => probe.rcap,
        CaptchaKind::ImpervaNative | CaptchaKind::Unknown => None,
    };
    Ok((site_key, probe.url))
}

/// Inject a CAPTCHA solver token into the named form field via
/// `Runtime.evaluate`.
async fn inject_captcha_solution(
    session: &SessionHandle,
    solution: &CaptchaSolution,
) -> Result<(), ImpervaError> {
    use serde_json::json;

    let script = format!(
        r#"
        (function () {{
            var field = document.querySelector('[name="{name}"]')
                || document.getElementById("{name}");
            if (!field) {{
                var t = document.createElement("textarea");
                t.name = "{name}";
                t.id = "{name}";
                t.style.display = "none";
                document.body.appendChild(t);
                field = t;
            }}
            field.value = {token};
            field.dispatchEvent(new Event("change", {{ bubbles: true }}));
            return true;
        }})()
        "#,
        name = solution.form_field.replace('"', "\\\""),
        token = serde_json::Value::String(solution.token.clone()),
    );

    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": script,
                "returnByValue": true,
            }),
        )
        .await?;
    if let Some(details) = res.get("exceptionDetails") {
        let msg = details
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("unknown")
            .to_string();
        return Err(ImpervaError::JsError(msg));
    }
    Ok(())
}

/// Drives an Imperva clearance flow against a single tab's session.
///
/// Constructed via `Tab::imperva()`.
pub struct ImpervaBypass<'tab> {
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
    ///
    /// # Errors
    /// - [`ImpervaError::Timeout`] — overall timeout elapsed before
    ///   clearance. `last_surface` carries the most-recent observed surface.
    /// - [`ImpervaError::CaptchaRequired`] — CAPTCHA surface detected
    ///   but no `on_captcha` solver was registered.
    /// - [`ImpervaError::CaptchaSolver`] — registered solver returned an
    ///   error.
    /// - [`ImpervaError::Interception`] — Fetch-domain hook (when set via
    ///   [`with_interception`](Self::with_interception)) failed.
    /// - [`ImpervaError::Call`] / [`ImpervaError::JsError`] — CDP or
    ///   in-page evaluator failure.
    pub async fn wait_for_clearance(self) -> Result<ClearanceOutcome, ImpervaError> {
        use tokio::time::{Instant, Interval, MissedTickBehavior};

        let snapshot = crate::detection::detect_snapshot(self.session).await?;

        // Fast paths.
        if matches!(snapshot.surface, crate::detection::ImpervaSurface::None) && snapshot.body_clean
        {
            return Ok(ClearanceOutcome::AlreadyClear);
        }
        if let crate::detection::ImpervaSurface::Captcha(kind) = snapshot.surface {
            let Some(solver) = self.on_captcha.clone() else {
                return Err(ImpervaError::CaptchaRequired { kind });
            };
            let (site_key, url) = extract_captcha_site_key(self.session, kind).await?;
            let challenge = CaptchaChallenge {
                kind,
                site_key,
                url,
            };
            let solution = solver(challenge)
                .await
                .map_err(ImpervaError::CaptchaSolver)?;
            inject_captcha_solution(self.session, &solution).await?;
            // Fall through to poll loop: the page should now submit the
            // CAPTCHA and clear the Imperva surface.
        }

        let deadline = Instant::now() + self.timeout;
        let mut ticker: Interval = tokio::time::interval(self.poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        // `last_surface` carries the most-recently-observed non-clearance
        // surface for `Timeout` diagnostics. The initial assignment is dead
        // because the first loop iteration always either returns or writes
        // `last_surface` itself, but keeping the type-annotated `None` here
        // (rather than `let last_surface;` with first-write later) keeps the
        // mutation site obvious to readers.
        #[allow(unused_assignments)]
        let mut last_surface: Option<crate::detection::ImpervaSurface> = None;
        let mut next_snapshot = Some(snapshot);

        loop {
            // First iteration uses the snapshot already taken above; subsequent
            // iterations re-probe. The probe is itself raced against the
            // overall deadline so a stuck CDP call cannot deadlock the loop.
            let snap = match next_snapshot.take() {
                Some(s) => s,
                None => tokio::select! {
                    res = crate::detection::detect_snapshot(self.session) => res?,
                    () = tokio::time::sleep_until(deadline) => {
                        return Err(ImpervaError::Timeout {
                            timeout: self.timeout,
                            last_surface,
                        });
                    }
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
                (None, true, true) => return Ok(ClearanceOutcome::ChallengeGone),
                _ => last_surface = Some(snap.surface),
            }

            if Instant::now() >= deadline {
                return Err(ImpervaError::Timeout {
                    timeout: self.timeout,
                    last_surface,
                });
            }

            tokio::select! {
                _ = ticker.tick() => {}
                () = tokio::time::sleep_until(deadline) => {
                    return Err(ImpervaError::Timeout {
                        timeout: self.timeout,
                        last_surface,
                    });
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

    use crate::detection::ImpervaSurface;
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

        let err = fut.await.unwrap().unwrap_err();
        match err {
            ImpervaError::Timeout { last_surface, .. } => {
                assert_eq!(last_surface, Some(ImpervaSurface::Reese84));
            }
            other => panic!("expected Timeout, got {other:?}"),
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
}
