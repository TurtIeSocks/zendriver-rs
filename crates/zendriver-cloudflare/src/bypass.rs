//! Cloudflare Turnstile bypass driver.
//!
//! Public entry is [`CloudflareBypass`] — constructed via
//! `Tab::cloudflare()` (zendriver crate, feature-gated). The driver runs a
//! single CDP poll loop that, per tick:
//!
//! 1. Re-evaluates a unified shadow-DOM walker in the page's main world
//!    (`js::poll_expr`) returning, in one round-trip:
//!    - the clearance token if one of the
//!      [`token_inputs`](TurnstileSelectors::token_inputs) holds a value,
//!    - the Turnstile challenge iframe's bounding box if mounted,
//!    - whether *any* challenge marker exists on the page
//!      (container, token input, or live iframe).
//! 2. Resolves to [`ClearanceOutcome::TokenAcquired`] the first tick a
//!    non-empty token is observed — including the **invisible Turnstile**
//!    path where the iframe never mounts and the token is populated
//!    directly.
//! 3. If a *clickable* challenge iframe is mounted (non-zero size and
//!    visible), scrolls it into view, re-reads its box, and clicks the point
//!    the [`ClickPolicy`] chose inside it — by default
//!    `bbox.x + bbox.width * 0.15, bbox.y + bbox.height * 0.50`, the 15%
//!    from left / 50% from top position of the checkbox. Up to
//!    [`ClickPolicy::max_attempts`] clicks are spent, spaced
//!    [`ClickPolicy::retry_ticks`] ticks apart, so a swallowed first click
//!    is not the end of the run. A caller-supplied
//!    [`on_click`](CloudflareBypass::on_click) handler replaces the built-in
//!    raw mouse dispatch.
//! 4. Resolves to [`ClearanceOutcome::ChallengeGone`] when the challenge
//!    stops being actionable without yielding a token — either the iframe we
//!    clicked is no longer a valid click target, or every challenge marker
//!    observed on an earlier tick has vanished (the JS-only interstitial
//!    that clears itself). The first of those is weaker than it sounds; see
//!    the variant's own docs before treating it as clearance.
//! 5. Resolves to [`ClearanceOutcome::TimedOut`] on deadline, carrying
//!    `saw_challenge` — `true` when challenge markers were seen but never
//!    resolved, `false` when the entire timeout window elapsed without
//!    observing any challenge markers (the caller likely invoked the bypass
//!    on a page that has no Cloudflare gate at all).
//!
//! Every marker the evaluators look for is caller data — see
//! [`TurnstileSelectors`] for why none of it is a literal in this crate.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::Deserialize;
use tokio::time::Instant;
use zendriver_transport::SessionHandle;

use crate::click::click_at;
use crate::detection::{BoundingBox, eval_main_world};
use crate::error::CloudflareError;
use crate::js;
use crate::options::{ClickPolicy, ClickTarget, TurnstileSelectors};

/// Result of a clearance attempt.
#[derive(Debug, Clone)]
pub enum ClearanceOutcome {
    /// Turnstile produced a token — the value held by the first of the
    /// configured [`token_inputs`](TurnstileSelectors::token_inputs) to
    /// match, `[name="cf-turnstile-response"]` by default.
    TokenAcquired(String),
    /// The challenge stopped being actionable without yielding a token — the
    /// iframe we clicked is no longer a valid click target, or all challenge
    /// markers seen earlier in the run have vanished (a JS-only interstitial
    /// that cleared itself). Usually the clearance-cookie shortcut.
    ///
    /// **Not proof the gate was passed.** "No longer a valid click target"
    /// is `clickableRect` returning null, which it also does for a widget
    /// that is still mounted but zero-size, `visibility: hidden`,
    /// `display: none` or `opacity: 0`. So once a click has landed, a tick
    /// where the widget is merely between states reaches this terminal too.
    /// Confirm the page is actually through the gate before treating what
    /// you scrape next as clean.
    ChallengeGone,
    /// Deadline elapsed without a terminal clearance state. `saw_challenge`
    /// is `true` if any challenge marker (container, hidden input, or live
    /// iframe) was ever observed; `false` if none ever appeared — the caller
    /// likely invoked the bypass on a page that has no Cloudflare gate at all.
    /// Not a fault: a deadline in a bot-management flow is a normal "didn't
    /// finish, retry or give up" terminal.
    TimedOut { saw_challenge: bool },
}

/// Default poll interval for `wait_for_clearance`.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A caller-supplied click implementation, as stored on the driver.
///
/// The public entry is [`CloudflareBypass::on_click`], which is generic over
/// an ordinary async closure and does the boxing. The session is handed over
/// by value — it is `Arc`-backed and cheap to clone — so the returned future
/// borrows nothing and needs no lifetime of its own.
type ClickHandler = Box<
    dyn Fn(
            SessionHandle,
            ClickTarget,
        ) -> Pin<Box<dyn Future<Output = Result<(), CloudflareError>> + Send>>
        + Send
        + Sync,
>;

/// Drives a Cloudflare Turnstile clearance flow against a single tab's session.
///
/// Constructed via `Tab::cloudflare()`.
pub struct CloudflareBypass<'a> {
    pub(crate) session: &'a SessionHandle,
    pub(crate) poll_interval: Duration,
    pub(crate) selectors: TurnstileSelectors,
    pub(crate) click_policy: ClickPolicy,
    pub(crate) on_click: Option<ClickHandler>,
}

/// Hand-written because `ClickHandler` is a boxed closure and closures are
/// not `Debug`. Reports whether a handler is installed, which is the only
/// thing about it worth printing.
impl std::fmt::Debug for CloudflareBypass<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudflareBypass")
            .field("session", &self.session)
            .field("poll_interval", &self.poll_interval)
            .field("selectors", &self.selectors)
            .field("click_policy", &self.click_policy)
            .field("on_click", &self.on_click.is_some())
            .finish()
    }
}

impl<'a> CloudflareBypass<'a> {
    /// Create a new bypass driver bound to `session` with a default 500ms
    /// poll interval, the default [`TurnstileSelectors`] and the default
    /// [`ClickPolicy`].
    pub fn new(session: &'a SessionHandle) -> Self {
        Self {
            session,
            poll_interval: DEFAULT_POLL_INTERVAL,
            selectors: TurnstileSelectors::default(),
            click_policy: ClickPolicy::default(),
            on_click: None,
        }
    }

    /// Override the default 500ms poll interval used by
    /// [`wait_for_clearance`](Self::wait_for_clearance).
    #[must_use]
    pub fn poll_interval(mut self, dur: Duration) -> Self {
        self.poll_interval = dur;
        self
    }

    /// Override the page markers that identify the widget. Defaults to
    /// [`TurnstileSelectors::default`].
    ///
    /// The same set feeds the detector, the poll loop and the
    /// scroll-and-measure step, so Cloudflare renaming something is one
    /// change here rather than a release of this crate.
    #[must_use]
    pub fn selectors(mut self, selectors: TurnstileSelectors) -> Self {
        self.selectors = selectors;
        self
    }

    /// Override whether, how often and where the widget is clicked. Defaults
    /// to [`ClickPolicy::default`].
    #[must_use]
    pub fn click_policy(mut self, policy: ClickPolicy) -> Self {
        self.click_policy = policy;
        self
    }

    /// Replace the built-in click with `handler`.
    ///
    /// The built-in click is three raw `Input.dispatchMouseEvent` calls at
    /// the point [`ClickPolicy`] chose. That is the volatile half of this
    /// flow — the half that has to change when Cloudflare changes what a
    /// convincing click looks like — so it is handed over whole rather than
    /// grown a knob at a time. A handler might move the pointer along a
    /// human-ish path, drive a solver service, or click through a higher
    /// level of `zendriver` instead.
    ///
    /// The handler is called once per attempt, with the session and the
    /// widget's post-scroll [`ClickTarget`]. It counts against
    /// [`ClickPolicy::max_attempts`] exactly as the built-in click does, and
    /// an `Err` from it fails the whole run rather than being retried.
    ///
    /// It replaces the click, not the decision to click. The driver still
    /// owns what counts as clickable, and only invokes the handler for a
    /// widget that cleared that bar twice — once when the poll evaluator
    /// measured it, once after the scroll — where the bar is non-zero size
    /// and not hidden by `visibility` / `display` / `opacity`. A handler
    /// installed specifically to reach a hidden or zero-size widget never
    /// runs; that widget needs a different approach entirely.
    ///
    /// ```no_run
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_cloudflare::CloudflareError> {
    /// use std::time::Duration;
    /// use zendriver_cloudflare::CloudflareBypass;
    ///
    /// let outcome = CloudflareBypass::new(tab)
    ///     .on_click(|session, target| async move {
    ///         tracing::info!(x = target.x, y = target.y, "clicking turnstile");
    ///         session
    ///             .call(
    ///                 "Input.dispatchMouseEvent",
    ///                 serde_json::json!({
    ///                     "type": "mouseMoved", "x": target.x, "y": target.y,
    ///                 }),
    ///             )
    ///             .await?;
    ///         Ok(())
    ///     })
    ///     .wait_for_clearance(Duration::from_secs(30))
    ///     .await?;
    /// # let _ = outcome;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn on_click<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(SessionHandle, ClickTarget) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), CloudflareError>> + Send + 'static,
    {
        self.on_click = Some(Box::new(move |session, target| {
            Box::pin(handler(session, target))
        }));
        self
    }

    /// Poll the page until a Turnstile clearance terminal state is reached or
    /// `timeout` elapses.
    ///
    /// Handles both the **interactive** flow (challenge iframe mounts → we
    /// click it → token appears) and the **invisible** flow (no iframe;
    /// token is populated directly by Cloudflare's loader script). Resolution
    /// rules per tick are documented at module level.
    ///
    /// # Returns
    /// - `Ok(ClearanceOutcome::TokenAcquired(token))` — a configured token
    ///   input picked up a non-empty value, either after we clicked the
    ///   interactive iframe or because the page uses invisible Turnstile.
    /// - `Ok(ClearanceOutcome::ChallengeGone)` — the challenge stopped being
    ///   actionable without yielding a token (e.g. a clearance-cookie
    ///   shortcut): either an iframe we clicked is no longer a valid click
    ///   target, or challenge markers observed on an earlier tick are all
    ///   gone. Read [`ClearanceOutcome::ChallengeGone`] before treating this
    ///   as proof the gate was passed.
    /// - `Ok(ClearanceOutcome::TimedOut { saw_challenge })` — `timeout`
    ///   elapsed without a terminal clearance state. `saw_challenge` is
    ///   `true` if challenge markers were observed (markers present but
    ///   never resolved); `false` if none ever appeared (the caller likely
    ///   invoked the bypass on a page that has no Cloudflare gate). Not a
    ///   fault — a deadline here is a normal "retry or give up" signal.
    ///
    /// # Errors
    /// - [`CloudflareError::Call`] / [`CloudflareError::JsError`] — CDP or
    ///   in-page evaluator failure.
    ///
    /// ```no_run
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_cloudflare::CloudflareError> {
    /// use std::time::Duration;
    /// use zendriver_cloudflare::CloudflareBypass;
    ///
    /// let bypass = CloudflareBypass::new(tab)
    ///     .poll_interval(Duration::from_millis(250));
    /// let outcome = bypass.wait_for_clearance(Duration::from_secs(15)).await?;
    /// println!("{outcome:?}");
    /// # Ok(()) }
    /// ```
    pub async fn wait_for_clearance(
        self,
        timeout: Duration,
    ) -> Result<ClearanceOutcome, CloudflareError> {
        let deadline = Instant::now() + timeout;
        let mut clicks: u32 = 0;
        let mut ticks_since_click: u32 = 0;
        let mut ever_seen_markers = false;

        let mut stall_ticks: u32 = 0;
        let mut warned_stall = false;

        loop {
            let state = poll_state(self.session, &self.selectors).await?;
            // Snapshot the latch *before* folding in this tick: the
            // markers-vanished terminal below needs "seen on an earlier
            // tick", not "seen including right now".
            let seen_markers_before = ever_seen_markers;
            if state.has_markers {
                ever_seen_markers = true;
            }

            if let Some(token) = state.token {
                return Ok(ClearanceOutcome::TokenAcquired(token));
            }

            // Click cadence: the first clickable tick clicks straight away,
            // every later attempt waits `ClickPolicy::retry_ticks` ticks
            // after the click that landed, and a run spends at most
            // `ClickPolicy::max_attempts` clicks. So at the defaults the
            // second click lands on tick 5, not tick 4. `max_attempts: 0`
            // never clicks at all, which also keeps the scroll evaluator
            // from running.
            let may_click = clicks < self.click_policy.max_attempts
                && (clicks == 0 || ticks_since_click >= self.click_policy.retry_ticks);

            let mut clicked = false;
            match state.bbox {
                Some(_) if may_click => {
                    // Raw `Input.dispatchMouseEvent` coordinates are viewport
                    // relative, so a below-the-fold widget must be scrolled in
                    // and re-measured before the click can land on it.
                    if let Some(bbox) = scroll_into_view(self.session, &self.selectors).await? {
                        let target = ClickTarget {
                            bbox,
                            x: bbox.x + bbox.width * self.click_policy.x_fraction,
                            y: bbox.y + bbox.height * self.click_policy.y_fraction,
                        };
                        match &self.on_click {
                            Some(handler) => handler(self.session.clone(), target).await?,
                            None => click_at(self.session, target.x, target.y).await?,
                        }
                        clicks += 1;
                        ticks_since_click = 0;
                        clicked = true;
                    }
                }
                // The challenge stopped being actionable without a token:
                // either the iframe we clicked is no longer a valid click
                // target, or markers seen on an earlier tick are all gone (a
                // JS-only interstitial clearing itself). Both are usually the
                // clearance-cookie shortcut.
                //
                // OPEN QUESTION (decisions.md #6, deferred pending live
                // observation): `bbox` is `None` for a widget that is merely
                // hidden or zero-size, so after the first landed click any
                // unclickable tick hits this *success* terminal. Prescribed
                // fix: return `iframePresent` alongside `bbox` from the poll
                // evaluator and key the `clicks > 0` disjunct off presence,
                // not clickability. Still not applied — changing which runs
                // report clearance needs a human, and this PR only moved the
                // markers and the click into caller data.
                None if clicks > 0 || (seen_markers_before && !state.has_markers) => {
                    return Ok(ClearanceOutcome::ChallengeGone);
                }
                _ => {}
            }

            // Every tick that did not land a click is a tick with no progress,
            // the scroll-returned-`None` path included. That path used to skip
            // the counter, so a widget that stayed mounted but stopped being
            // measurable — the run this hint exists to explain — was the one
            // run that could never produce it.
            //
            // Except when the caller set `max_attempts: 0`. The hint reads
            // "clicks are going nowhere, check stealth", and a run configured
            // never to click has no clicks to go nowhere: `stall_ticks` only
            // resets on a landed click, so it would fire on every watch-only
            // run and blame stealth for behaviour the caller asked for.
            if clicked {
                stall_ticks = 0;
            } else if self.click_policy.max_attempts > 0 {
                stall_ticks += 1;
                if stall_ticks == 10 && !warned_stall {
                    tracing::warn!(
                        poll_interval_ms = self.poll_interval.as_millis() as u64,
                        "cloudflare clearance stalled — is BrowserBuilder::stealth enabled?"
                    );
                    // `stall_ticks` resets on a landed click, so `== 10` is
                    // reachable more than once per run; the latch is what
                    // keeps the hint to a single line.
                    warned_stall = true;
                }
            }

            // Only meaningful once something has been clicked — unguarded, it
            // counted ticks before the first click, which the `clicks == 0`
            // arm of `may_click` then had to ignore.
            if clicks > 0 {
                ticks_since_click = ticks_since_click.saturating_add(1);
            }

            if Instant::now() >= deadline {
                return Ok(ClearanceOutcome::TimedOut {
                    saw_challenge: ever_seen_markers,
                });
            }

            tokio::select! {
                () = tokio::time::sleep(self.poll_interval) => {}
                () = tokio::time::sleep_until(deadline) => {}
            }
        }
    }
}

/// Decoded payload from the unified poll evaluator. Combines token check,
/// iframe bbox, and a coarse "any challenge marker present" flag so each
/// tick is a single CDP round-trip.
#[derive(Debug, Deserialize)]
struct PollState {
    /// Non-empty value held by the first matching
    /// [`token_inputs`](TurnstileSelectors::token_inputs) selector, if any.
    #[serde(default)]
    token: Option<String>,
    /// Bounding box of the challenge iframe when it is a valid click target —
    /// mounted, non-zero size, and visible. Walks shadow roots so
    /// shadow-hosted widgets still surface. `None` for a hidden or 0×0
    /// iframe, which must never be clicked.
    #[serde(default)]
    bbox: Option<BoundingBox>,
    /// `true` when the page has any of the configured markers: a
    /// [`container`](TurnstileSelectors::container), a matching token input,
    /// or a live challenge iframe. Used to distinguish "wrong page" (no
    /// markers ever seen) from "real challenge that timed out".
    #[serde(default, rename = "hasMarkers")]
    has_markers: bool,
}

/// Run the poll evaluator against `session`'s main world and decode the
/// result.
async fn poll_state(
    session: &SessionHandle,
    selectors: &TurnstileSelectors,
) -> Result<PollState, CloudflareError> {
    let value = eval_main_world(session, &js::poll_expr(selectors)).await?;
    serde_json::from_value(value)
        .map_err(|e| CloudflareError::JsError(format!("invalid poll payload: {e}")))
}

/// Run the scroll evaluator against `session`'s main world: scroll the
/// challenge iframe into view if needed and return the rect to click, or
/// `None` when the widget is gone or not clickable.
async fn scroll_into_view(
    session: &SessionHandle,
    selectors: &TurnstileSelectors,
) -> Result<Option<BoundingBox>, CloudflareError> {
    let value = eval_main_world(session, &js::scroll_expr(selectors)).await?;
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|e| CloudflareError::JsError(format!("invalid scroll payload: {e}")))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};

    use super::*;
    use crate::testutil::{CMD_BUDGET, expect_cmd};
    use zendriver_transport::testing::{LogCapture, MockConnection};

    /// How long a serving loop waits for the driver's next command before
    /// deciding the run is over. Long enough to cover a poll interval plus
    /// scheduling jitter, short enough that a finished driver ends the loop
    /// promptly instead of hanging the test.
    const DRAIN_BUDGET: Duration = Duration::from_millis(300);

    /// Bounded `JoinHandle` await for a driver task.
    ///
    /// `wait_for_clearance` polls until a terminal or its own deadline, so a
    /// regression that makes a terminal unreachable leaves the handle
    /// pending forever. Unbounded, the test then hangs instead of failing —
    /// which is exactly what happened when the `ChallengeGone` gate was
    /// reverted to prove the marker-vanished test discriminates.
    async fn outcome_of(
        fut: tokio::task::JoinHandle<Result<ClearanceOutcome, CloudflareError>>,
    ) -> Result<ClearanceOutcome, CloudflareError> {
        match tokio::time::timeout(CMD_BUDGET, fut).await {
            Ok(joined) => joined.expect("driver task panicked"),
            Err(_) => panic!("driver never reached a terminal"),
        }
    }

    /// Wrap an evaluator result value in the CDP `Runtime.evaluate` envelope.
    fn eval_reply(value: Value) -> Value {
        json!({ "result": { "type": "object", "value": value } })
    }

    /// One poll-evaluator payload.
    fn poll_value(token: Option<&str>, bbox: Option<Value>, has_markers: bool) -> Value {
        json!({
            "token": token,
            "bbox": bbox,
            "hasMarkers": has_markers,
        })
    }

    /// A rect payload, as either evaluator returns it.
    fn rect(x: f64, y: f64, w: f64, h: f64) -> Value {
        json!({ "x": x, "y": y, "width": w, "height": h })
    }

    /// Answer the next `Runtime.evaluate`, dispatching on which evaluator it
    /// carries: the scroll helper gets `scroll`, the poll evaluator gets
    /// `poll`. Returns `true` when the answered call was the scroll helper.
    async fn answer_eval(mock: &mut MockConnection, poll: &Value, scroll: &Value) -> bool {
        let id = expect_cmd(mock, "Runtime.evaluate").await;
        let expr = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        let is_scroll = expr.contains("scrollIntoView");
        let value = if is_scroll {
            scroll.clone()
        } else {
            poll.clone()
        };
        mock.reply(id, eval_reply(value)).await;
        is_scroll
    }

    /// Answer the three `Input.dispatchMouseEvent` calls of one click and
    /// return the `(x, y)` they landed on.
    async fn answer_click(mock: &mut MockConnection) -> (f64, f64) {
        let mut coords = (f64::NAN, f64::NAN);
        for expected in ["mouseMoved", "mousePressed", "mouseReleased"] {
            let id = expect_cmd(mock, "Input.dispatchMouseEvent").await;
            let sent = mock.last_sent().clone();
            assert_eq!(sent["params"]["type"], expected);
            coords = (
                sent["params"]["x"].as_f64().unwrap(),
                sent["params"]["y"].as_f64().unwrap(),
            );
            mock.reply(id, json!({})).await;
        }
        coords
    }

    /// The stall hint must fire for the run it is most needed on: a widget
    /// that stays mounted (the poll evaluator keeps reporting a bbox) but
    /// never becomes measurable (the scroll evaluator keeps returning null),
    /// so no click ever lands. That tick used to skip the stall counter
    /// entirely, which meant the one diagnostic explaining a stuck run could
    /// never appear on the stuck run it explains.
    #[tokio::test]
    async fn stalled_unmeasurable_widget_emits_the_stealth_hint() {
        /// Stalled ticks to serve before letting the run finish. The hint
        /// fires on the tenth.
        const STALLED_TICKS: usize = 12;

        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let capture = LogCapture::new();

        let poll = poll_value(None, Some(rect(10.0, 20.0, 40.0, 40.0)), true);

        // Not spawned: `LogCapture` follows the future it wraps, not a task
        // spawned out of it, so the driver and the mock server share a task.
        // The generous budget is never reached — the server ends the run — so
        // the tick count is deterministic instead of clock-dependent.
        let driver = capture.capture(async {
            CloudflareBypass::new(&sess)
                .poll_interval(Duration::from_millis(1))
                .wait_for_clearance(Duration::from_secs(30))
                .await
        });

        let serving = async {
            let mut polls = 0usize;
            while let Some((method, id)) = mock.recv_cmd_timeout(DRAIN_BUDGET).await {
                assert_eq!(method, "Runtime.evaluate");
                let is_scroll = mock.last_sent()["params"]["expression"]
                    .as_str()
                    .unwrap()
                    .contains("scrollIntoView");
                let value = if is_scroll {
                    // Mounted on every poll, measurable on none: no click can
                    // land, so every tick is a tick without progress.
                    Value::Null
                } else {
                    polls += 1;
                    if polls > STALLED_TICKS {
                        poll_value(Some("LATE_TOKEN"), None, true)
                    } else {
                        poll.clone()
                    }
                };
                mock.reply(id, eval_reply(value)).await;
            }
        };

        let (outcome, ()) = tokio::join!(driver, serving);

        match outcome.unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, "LATE_TOKEN"),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        assert!(
            capture.contains("clearance stalled"),
            "a mounted-but-unmeasurable widget is a stalled run; captured {:?}",
            capture.events()
        );
        assert_eq!(
            capture.count("clearance stalled"),
            1,
            "the hint is latched to one line per run"
        );
        conn.shutdown();
    }

    /// The same hint must **not** fire for a caller who set
    /// `max_attempts: 0`.
    ///
    /// It reads "clearance stalled — is stealth enabled?", and stealth is
    /// not the problem: `stall_ticks` only resets on a landed click, so a
    /// run explicitly configured never to click accumulates them from tick
    /// one and trips the hint every time. That advice is wrong twice over —
    /// nothing is stalled, and the caller is told to go change an unrelated
    /// browser setting. `max_attempts: 0` is new in this PR, so the false
    /// positive would have been new with it.
    #[tokio::test]
    async fn a_watch_only_run_is_never_blamed_on_stealth() {
        /// Past the tenth tick, which is where the hint fires.
        const MOUNTED_TICKS: usize = 12;

        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let capture = LogCapture::new();

        // Not spawned: `LogCapture` follows the future it wraps, not a task
        // spawned out of it, so the driver and the mock server share a task.
        let driver = capture.capture(async {
            CloudflareBypass::new(&sess)
                .poll_interval(Duration::from_millis(1))
                .click_policy(ClickPolicy {
                    max_attempts: 0,
                    ..ClickPolicy::default()
                })
                .wait_for_clearance(Duration::from_secs(30))
                .await
        });

        let widget = widget_rect();
        let (outcome, calls) = tokio::join!(driver, serve(&mut mock, &widget, Some(MOUNTED_TICKS)));

        // The token only lands after MOUNTED_TICKS polls, so reaching it is
        // proof the run went well past the tick the hint fires on — this
        // cannot pass by the run having ended early.
        match outcome.unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, END_TOKEN),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        assert!(!saw(&calls, MOUSE), "the policy said do not click");
        assert!(
            !capture.contains("clearance stalled"),
            "a run told not to click is not a stalled run; captured {:?}",
            capture.events()
        );
        conn.shutdown();
    }

    /// Interactive happy path: poll #1 yields a bbox → the widget is scrolled
    /// into view → click_at fires the three mouse events at (15% × width,
    /// 50% × height) of the **post-scroll** rect → poll #2 observes the token
    /// → TokenAcquired.
    #[tokio::test]
    async fn wait_for_clearance_scrolls_into_view_then_clicks_at_bbox_offset() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        // Widget starts 2000px below the fold; after scrolling it sits at
        // (100, 300) with 60×40 → click at
        //   x = 100 + 60 * 0.15 = 109
        //   y = 300 + 40 * 0.50 = 320
        const SCROLLED_X: f64 = 100.0;
        const SCROLLED_Y: f64 = 300.0;
        const BBOX_W: f64 = 60.0;
        const BBOX_H: f64 = 40.0;
        const EXPECTED_CLICK_X: f64 = SCROLLED_X + BBOX_W * 0.15;
        const EXPECTED_CLICK_Y: f64 = SCROLLED_Y + BBOX_H * 0.50;

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_secs(5)).await
            }
        });

        // Poll #1: unified evaluator returns an off-screen bbox, no token.
        let id_poll1 = expect_cmd(&mut mock, "Runtime.evaluate").await;
        let expr = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            expr.contains("challenges.cloudflare.com"),
            "poll eval should walk for the challenges.cloudflare.com iframe"
        );
        assert!(
            expr.contains("cf-turnstile-response"),
            "poll eval should look at the cf-turnstile-response input"
        );
        mock.reply(
            id_poll1,
            eval_reply(poll_value(
                None,
                Some(rect(SCROLLED_X, 2000.0, BBOX_W, BBOX_H)),
                true,
            )),
        )
        .await;

        // Scroll helper runs before the click and re-measures the widget.
        let id_scroll = expect_cmd(&mut mock, "Runtime.evaluate").await;
        assert!(
            mock.last_sent()["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("scrollIntoView"),
            "a click must be preceded by a scroll-into-view + re-measure"
        );
        mock.reply(
            id_scroll,
            eval_reply(rect(SCROLLED_X, SCROLLED_Y, BBOX_W, BBOX_H)),
        )
        .await;

        // The click uses the post-scroll rect, not the stale off-screen one.
        let (x, y) = answer_click(&mut mock).await;
        assert_eq!(x, EXPECTED_CLICK_X);
        assert_eq!(y, EXPECTED_CLICK_Y);

        // Poll #2: token appears → TokenAcquired terminates the loop.
        let id_poll2 = expect_cmd(&mut mock, "Runtime.evaluate").await;
        mock.reply(
            id_poll2,
            eval_reply(poll_value(Some("TOKEN_XYZ"), None, true)),
        )
        .await;

        match outcome_of(fut).await.unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, "TOKEN_XYZ"),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        conn.shutdown();
    }

    /// A widget that mounts *late* and *below the fold* still gets clicked,
    /// at its post-scroll position.
    ///
    /// This is the run the click-once latch used to lose. Every other click
    /// test here hands the driver a bbox on poll #1, so all of them pass
    /// against a driver that only ever considers clicking on its first tick;
    /// this one does not. It is also the only test where the pre-scroll rect
    /// is far enough off-screen that clicking it would land nowhere, so it
    /// is what the scroll-and-re-measure step has to earn its round-trip
    /// against.
    #[tokio::test]
    async fn wait_for_clearance_clicks_a_widget_that_mounts_late_and_below_the_fold() {
        /// Polls that report markers but nothing clickable yet.
        const EMPTY_TICKS: usize = 3;
        // Mounts 3000px down, scrolls to (100, 200) at 80×40 → the click
        // belongs at (100 + 80*0.15, 200 + 40*0.50) = (112, 220). Clicking
        // the pre-scroll rect would give y = 3020 instead.
        const EXPECTED_CLICK: (f64, f64) = (112.0, 220.0);

        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_secs(5)).await
            }
        });

        // Nothing to click for the first few ticks — the widget has not
        // mounted yet.
        for _ in 0..EMPTY_TICKS {
            let id = expect_cmd(&mut mock, "Runtime.evaluate").await;
            mock.reply(id, eval_reply(poll_value(None, None, true)))
                .await;
        }

        // It mounts, far below the fold.
        let id = expect_cmd(&mut mock, "Runtime.evaluate").await;
        mock.reply(
            id,
            eval_reply(poll_value(
                None,
                Some(rect(100.0, 3000.0, 80.0, 40.0)),
                true,
            )),
        )
        .await;

        // The scroll evaluator brings it into view and re-measures it.
        let id = expect_cmd(&mut mock, "Runtime.evaluate").await;
        assert!(
            mock.last_sent()["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("scrollIntoView"),
            "a widget below the fold must be scrolled in before the click"
        );
        mock.reply(id, eval_reply(widget_rect())).await;

        assert_eq!(
            answer_click(&mut mock).await,
            EXPECTED_CLICK,
            "the click must use the post-scroll rect, not the off-screen one"
        );

        let id = expect_cmd(&mut mock, "Runtime.evaluate").await;
        mock.reply(id, eval_reply(poll_value(Some(END_TOKEN), None, true)))
            .await;

        match outcome_of(fut).await.unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, END_TOKEN),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        conn.shutdown();
    }

    /// Invisible-Turnstile path: no iframe ever mounts, the token is
    /// populated directly on the first poll → TokenAcquired without any
    /// click.
    #[tokio::test]
    async fn wait_for_clearance_returns_token_without_click_for_invisible_turnstile() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_secs(5)).await
            }
        });

        let id_poll = expect_cmd(&mut mock, "Runtime.evaluate").await;
        mock.reply(
            id_poll,
            eval_reply(poll_value(Some("INVISIBLE_TOKEN"), None, true)),
        )
        .await;

        match outcome_of(fut).await.unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, "INVISIBLE_TOKEN"),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        // No Input.dispatchMouseEvent should have been queued — the invisible
        // path never clicks. The mock connection asserts via its own drop
        // semantics that no pending sends remain.
        conn.shutdown();
    }

    /// Markers are present but no clickable widget is mounted yet (invisible
    /// Turnstile still computing): the loop must keep waiting for the token,
    /// NOT read "markers seen" as "challenge gone".
    #[tokio::test]
    async fn wait_for_clearance_keeps_waiting_while_markers_are_still_present() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_secs(5)).await
            }
        });

        // Two ticks with markers but no bbox and no token.
        for _ in 0..2 {
            let id = expect_cmd(&mut mock, "Runtime.evaluate").await;
            mock.reply(id, eval_reply(poll_value(None, None, true)))
                .await;
        }

        // Third tick: the token finally lands.
        let id = expect_cmd(&mut mock, "Runtime.evaluate").await;
        mock.reply(id, eval_reply(poll_value(Some("LATE_TOKEN"), None, true)))
            .await;

        match outcome_of(fut).await.unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, "LATE_TOKEN"),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        conn.shutdown();
    }

    /// JS-only interstitial that clears itself: challenge markers are seen on
    /// tick #1, no iframe is ever clickable, and on tick #2 every marker is
    /// gone → ChallengeGone. This terminal used to be unreachable without a
    /// click, so this flow returned TimedOut instead of success.
    #[tokio::test]
    async fn wait_for_clearance_returns_challenge_gone_when_markers_vanish_without_click() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_secs(5)).await
            }
        });

        // Tick #1: interstitial markers, no clickable widget.
        let id1 = expect_cmd(&mut mock, "Runtime.evaluate").await;
        mock.reply(id1, eval_reply(poll_value(None, None, true)))
            .await;

        // Tick #2: the interstitial cleared itself — every marker is gone.
        let id2 = expect_cmd(&mut mock, "Runtime.evaluate").await;
        mock.reply(id2, eval_reply(poll_value(None, None, false)))
            .await;

        let outcome = outcome_of(fut).await.unwrap();
        assert!(
            matches!(outcome, ClearanceOutcome::ChallengeGone),
            "expected ChallengeGone, got {outcome:?}"
        );
        conn.shutdown();
    }

    /// A page with no Cloudflare gate at all must never be reported as
    /// ChallengeGone: with no markers ever observed, the run ends in
    /// `TimedOut { saw_challenge: false }`.
    #[tokio::test]
    async fn wait_for_clearance_times_out_when_no_markers_ever_seen() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_millis(40)).await
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
            mock.reply(id, eval_reply(poll_value(None, None, false)))
                .await;
        }

        match outcome_of(fut).await.unwrap() {
            ClearanceOutcome::TimedOut { saw_challenge } => assert!(!saw_challenge),
            other => panic!("expected TimedOut, got {other:?}"),
        }
        conn.shutdown();
    }

    /// ChallengeGone path: poll #1 yields a bbox → scroll + click → poll #2
    /// reports no bbox and no token → ChallengeGone.
    #[tokio::test]
    async fn wait_for_clearance_returns_challenge_gone_when_iframe_disappears_after_click() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_secs(5)).await
            }
        });

        // Poll #1: bbox present. Then the scroll helper re-measures it.
        let widget = rect(10.0, 20.0, 40.0, 40.0);
        let is_scroll = answer_eval(
            &mut mock,
            &poll_value(None, Some(widget.clone()), true),
            &widget,
        )
        .await;
        assert!(!is_scroll, "first evaluate is the poll evaluator");
        let is_scroll = answer_eval(&mut mock, &poll_value(None, None, true), &widget).await;
        assert!(is_scroll, "second evaluate is the scroll helper");

        answer_click(&mut mock).await;

        // Poll #2: iframe gone, no token — markers linger in the DOM.
        let id_poll2 = expect_cmd(&mut mock, "Runtime.evaluate").await;
        mock.reply(id_poll2, eval_reply(poll_value(None, None, true)))
            .await;

        let outcome = outcome_of(fut).await.unwrap();
        assert!(matches!(outcome, ClearanceOutcome::ChallengeGone));
        conn.shutdown();
    }

    /// A widget that stays mounted and unanswered gets clicked again after
    /// [`ClickPolicy::retry_ticks`] ticks — but never more than
    /// [`ClickPolicy::max_attempts`] times in one run.
    ///
    /// The assertion is the *tick indices*, not the count. A count pins only
    /// the cap: it is satisfied whatever the spacing, so `retry_ticks` could
    /// be read as any constant — or dropped, clicking on every tick until
    /// the cap ran out — and three clicks would still be three clicks.
    ///
    /// Both numbers are literals rather than reads of `ClickPolicy::default`.
    /// An expectation computed from the value the driver reads is satisfied
    /// by any change to that value, including one that breaks retrying;
    /// `options::tests` is the one place that holds the defaults to 3 and 4.
    #[tokio::test]
    async fn wait_for_clearance_retries_click_at_the_default_cadence() {
        /// Ticks the widget stays mounted for. Comfortably past the third
        /// click, so a fourth would have room to show up and fail this.
        const MOUNTED_TICKS: usize = 14;

        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_secs(5)).await
            }
        });

        let widget = widget_rect();
        let calls = serve(&mut mock, &widget, Some(MOUNTED_TICKS)).await;

        assert_eq!(
            click_ticks(&calls),
            vec![0, 4, 8],
            "three clicks, four ticks apart, then nothing for the six ticks after"
        );
        match outcome_of(fut).await.unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, END_TOKEN),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        conn.shutdown();
    }

    /// The token the serving loop below ends a run with.
    const END_TOKEN: &str = "END";

    /// One CDP command the driver sent, tagged with the poll tick it belongs
    /// to. Mouse events carry their `type` so a test can count *clicks*
    /// rather than the three events each click is made of.
    type TickedCall = (usize, String);

    // Labels, not raw CDP method names: both evaluators arrive as
    // `Runtime.evaluate` and all three events of a click arrive as
    // `Input.dispatchMouseEvent`, so the method alone cannot tell a poll
    // from a scroll or a click from a pointer move. Kept mutually
    // non-prefixing so that [`saw`], which matches on a prefix, cannot
    // answer a question about one of them with a hit on another.
    const POLL_EVAL: &str = "Runtime.evaluate(poll)";
    const SCROLL_EVAL: &str = "Runtime.evaluate(scroll)";
    const MOUSE: &str = "Input.dispatchMouseEvent";
    const PRESS: &str = "Input.dispatchMouseEvent(mousePressed)";

    /// Serve a mounted, unanswered widget until the driver goes quiet,
    /// replying to the scroll evaluator with `widget` and to mouse events
    /// with `{}`.
    ///
    /// `bbox_ticks: Some(n)` switches the poll payload to a token on tick
    /// `n`, ending the run on a state the *test* chose. That matters more
    /// than it looks: the alternative is letting the driver's wall-clock
    /// deadline end it, which makes "exactly three clicks" an assertion
    /// about how many poll ticks fit in 200ms on the machine running CI.
    /// `None` keeps the widget mounted forever and lets the deadline decide,
    /// for the tests whose subject *is* the timeout.
    ///
    /// Returns every CDP command observed, in order, each tagged with the
    /// index of the poll tick it arrived on — so a test can assert not only
    /// how many clicks landed but *when*, and can assert on what was
    /// **absent**, which `expect_cmd` structurally cannot do since it
    /// discards non-matching frames.
    async fn serve(
        mock: &mut MockConnection,
        widget: &Value,
        bbox_ticks: Option<usize>,
    ) -> Vec<TickedCall> {
        let mut calls: Vec<TickedCall> = Vec::new();
        let mut polls = 0usize;
        let mut tick = 0usize;

        while let Some((method, id)) = mock.recv_cmd_timeout(DRAIN_BUDGET).await {
            let (label, reply) = match method.as_str() {
                "Runtime.evaluate" => {
                    let is_scroll = mock.last_sent()["params"]["expression"]
                        .as_str()
                        .unwrap()
                        .contains("scrollIntoView");
                    if is_scroll {
                        (SCROLL_EVAL.to_string(), eval_reply(widget.clone()))
                    } else {
                        // A poll evaluator opens a new tick; the scroll and
                        // the mouse events that follow belong to it.
                        tick = polls;
                        polls += 1;
                        let spent = bbox_ticks.is_some_and(|n| tick >= n);
                        let value = if spent {
                            poll_value(Some(END_TOKEN), None, true)
                        } else {
                            poll_value(None, Some(widget.clone()), true)
                        };
                        (POLL_EVAL.to_string(), eval_reply(value))
                    }
                }
                "Input.dispatchMouseEvent" => {
                    let kind = mock.last_sent()["params"]["type"].as_str().unwrap();
                    (format!("{method}({kind})"), json!({}))
                }
                other => (other.to_string(), json!({})),
            };
            calls.push((tick, label));
            mock.reply(id, reply).await;
        }
        calls
    }

    /// Index of the poll tick each click landed on, one entry per click.
    fn click_ticks(calls: &[TickedCall]) -> Vec<usize> {
        calls
            .iter()
            .filter(|(_, label)| label == PRESS)
            .map(|(tick, _)| *tick)
            .collect()
    }

    /// True if any command matching `prefix` was ever observed.
    fn saw(calls: &[TickedCall], prefix: &str) -> bool {
        calls.iter().any(|(_, label)| label.starts_with(prefix))
    }

    /// A widget rect the default policy and the custom policies below all
    /// map to *different* click points, so no assertion here can be
    /// satisfied by the wrong policy having been applied.
    fn widget_rect() -> Value {
        rect(100.0, 200.0, 80.0, 40.0)
    }

    /// `max_attempts: 0` is the caller saying "watch, do not touch" — the
    /// page's own script drives the widget and a synthetic click would only
    /// interfere. The driver must then never dispatch a mouse event, and
    /// never even run the scroll evaluator: scrolling a page the caller
    /// asked us not to touch is itself a change they did not ask for.
    ///
    /// Deadline-terminated rather than token-terminated, because this is
    /// also the crate's coverage of `TimedOut { saw_challenge: true }`. That
    /// outcome is what keeps the two absence assertions honest: it can only
    /// be reached by ticks that ran and saw markers, so they cannot pass by
    /// the run having done nothing at all.
    #[tokio::test]
    async fn click_policy_zero_attempts_never_dispatches_a_click() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                CloudflareBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .click_policy(ClickPolicy {
                        max_attempts: 0,
                        ..ClickPolicy::default()
                    })
                    .wait_for_clearance(Duration::from_millis(60))
                    .await
            }
        });

        let widget = widget_rect();
        let calls = serve(&mut mock, &widget, None).await;

        assert!(
            !saw(&calls, MOUSE),
            "max_attempts: 0 must not click; observed {calls:?}"
        );
        assert!(
            !saw(&calls, SCROLL_EVAL),
            "a run that will never click must not scroll the page either; observed {calls:?}"
        );
        match outcome_of(fut).await.unwrap() {
            ClearanceOutcome::TimedOut { saw_challenge } => assert!(saw_challenge),
            other => panic!("expected TimedOut, got {other:?}"),
        }
        conn.shutdown();
    }

    /// The attempt cap and the retry cadence are both the caller's to set,
    /// and both are moved here at once: five attempts where the built-in cap
    /// is three, one tick apart where the built-in cadence is four. So a
    /// driver still reading either constant produces different tick indices
    /// and fails, and this test plus the default-cadence one above pin
    /// `retry_ticks` at two separate values rather than at none.
    #[tokio::test]
    async fn click_policy_sets_the_attempt_cap_and_the_retry_cadence() {
        const ATTEMPTS: u32 = 5;
        const RETRY_TICKS: u32 = 1;
        /// Four ticks past the last expected click, so a sixth attempt has
        /// somewhere to appear.
        const MOUNTED_TICKS: usize = 9;

        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                CloudflareBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .click_policy(ClickPolicy {
                        max_attempts: ATTEMPTS,
                        retry_ticks: RETRY_TICKS,
                        ..ClickPolicy::default()
                    })
                    .wait_for_clearance(Duration::from_secs(5))
                    .await
            }
        });

        let widget = widget_rect();
        let calls = serve(&mut mock, &widget, Some(MOUNTED_TICKS)).await;

        assert_eq!(
            click_ticks(&calls),
            vec![0, 1, 2, 3, 4],
            "five clicks on consecutive ticks, then four quiet ticks"
        );
        match outcome_of(fut).await.unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, END_TOKEN),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        conn.shutdown();
    }

    /// Where inside the widget to click is a volatile detail of Cloudflare's
    /// own layout, so it is caller data. The fractions here land the click
    /// somewhere the built-in 15% / 50% never would.
    #[tokio::test]
    async fn click_policy_moves_the_click_point() {
        // 100 + 80 * 0.5 = 140, 200 + 40 * 0.25 = 210.
        // The built-in policy would give (112, 220) — neither coordinate
        // matches, so this cannot pass against the hardcoded offsets.
        const EXPECTED_X: f64 = 140.0;
        const EXPECTED_Y: f64 = 210.0;

        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                CloudflareBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .click_policy(ClickPolicy {
                        x_fraction: 0.5,
                        y_fraction: 0.25,
                        ..ClickPolicy::default()
                    })
                    .wait_for_clearance(Duration::from_secs(5))
                    .await
            }
        });

        let widget = widget_rect();
        let id_poll = expect_cmd(&mut mock, "Runtime.evaluate").await;
        mock.reply(
            id_poll,
            eval_reply(poll_value(None, Some(widget.clone()), true)),
        )
        .await;

        let id_scroll = expect_cmd(&mut mock, "Runtime.evaluate").await;
        mock.reply(id_scroll, eval_reply(widget)).await;

        let (x, y) = answer_click(&mut mock).await;
        assert_eq!((x, y), (EXPECTED_X, EXPECTED_Y));

        let id_poll2 = expect_cmd(&mut mock, "Runtime.evaluate").await;
        mock.reply(id_poll2, eval_reply(poll_value(Some("T"), None, true)))
            .await;

        match outcome_of(fut).await.unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, "T"),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        conn.shutdown();
    }

    /// The click is the volatile half of this flow — the half that has to
    /// change when Cloudflare changes. `on_click` hands it to the caller
    /// wholesale: the built-in raw mouse dispatch must not also run, so the
    /// decisive assertion is the *absence* of `Input.dispatchMouseEvent`.
    #[tokio::test]
    async fn on_click_replaces_the_built_in_mouse_dispatch() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let seen: Arc<Mutex<Vec<ClickTarget>>> = Arc::new(Mutex::new(Vec::new()));

        let fut = tokio::spawn({
            let s = sess.clone();
            let seen = Arc::clone(&seen);
            async move {
                CloudflareBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .on_click(move |_session, target| {
                        let seen = Arc::clone(&seen);
                        async move {
                            seen.lock().expect("poisoned").push(target);
                            Ok(())
                        }
                    })
                    .wait_for_clearance(Duration::from_secs(5))
                    .await
            }
        });

        let widget = widget_rect();
        let calls = serve(&mut mock, &widget, Some(6)).await;

        assert!(
            !saw(&calls, MOUSE),
            "on_click owns the click; the built-in dispatch must not fire too — saw {calls:?}"
        );
        let targets = seen.lock().expect("poisoned").clone();
        assert!(!targets.is_empty(), "on_click was never invoked");
        // The handler is handed the post-scroll rect and the point the
        // policy chose inside it, so a caller can click it their own way.
        assert_eq!(targets[0].bbox.width, 80.0);
        assert_eq!((targets[0].x, targets[0].y), (112.0, 220.0));

        match outcome_of(fut).await.unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, END_TOKEN),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        conn.shutdown();
    }

    /// A failing caller click is a failing run — swallowing it would leave
    /// the loop burning its budget on a click that never happens.
    #[tokio::test]
    async fn on_click_error_aborts_the_run() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                CloudflareBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .on_click(|_session, _target| async {
                        Err(CloudflareError::JsError("solver exploded".into()))
                    })
                    .wait_for_clearance(Duration::from_secs(5))
                    .await
            }
        });

        // The handler errors on the very first attempt, so the driver is
        // gone long before tick 6; `serve` ends on the mock going quiet.
        let widget = widget_rect();
        serve(&mut mock, &widget, Some(6)).await;

        let err = outcome_of(fut)
            .await
            .expect_err("handler error must propagate");
        assert!(err.to_string().contains("solver exploded"), "got {err}");
        conn.shutdown();
    }

    /// The markers are caller data all the way down: a custom marker must
    /// reach the evaluator the loop actually dispatches, not just the
    /// template that builds it.
    #[tokio::test]
    async fn configured_markers_reach_the_poll_evaluator() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                CloudflareBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .selectors(TurnstileSelectors {
                        iframe_src_contains: "gate.alien.invalid".into(),
                        ..TurnstileSelectors::default()
                    })
                    .wait_for_clearance(Duration::from_secs(5))
                    .await
            }
        });

        let id = expect_cmd(&mut mock, "Runtime.evaluate").await;
        let expr = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(expr.contains("gate.alien.invalid"), "custom marker ignored");
        assert!(
            !expr.contains("challenges.cloudflare.com"),
            "the default marker is still baked into the dispatched evaluator"
        );
        mock.reply(id, eval_reply(poll_value(Some("T"), None, true)))
            .await;

        match outcome_of(fut).await.unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, "T"),
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        conn.shutdown();
    }
}
