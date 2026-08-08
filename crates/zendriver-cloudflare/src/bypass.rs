//! Cloudflare Turnstile bypass driver.
//!
//! Public entry is [`CloudflareBypass`] — constructed via
//! `Tab::cloudflare()` (zendriver crate, feature-gated). The driver runs a
//! single CDP poll loop that, per tick:
//!
//! 1. Re-evaluates a unified shadow-DOM walker in the page's main world
//!    (private `POLL_JS`) returning, in one round-trip:
//!    - the `cf-turnstile-response` (or legacy `cf_challenge_response`) token
//!      if present,
//!    - the Turnstile challenge iframe's bounding box if mounted,
//!    - whether *any* challenge marker exists on the page
//!      (container, hidden input, or live iframe).
//! 2. Resolves to [`ClearanceOutcome::TokenAcquired`] the first tick a
//!    non-empty token is observed — including the **invisible Turnstile**
//!    path where the iframe never mounts and the token is populated
//!    directly.
//! 3. If a *clickable* challenge iframe is mounted (non-zero size and
//!    visible), scrolls it into view, re-reads its box, and dispatches a raw
//!    left-click at the canonical
//!    `bbox.x + bbox.width * 0.15, bbox.y + bbox.height * 0.50` offset
//!    (15% from left, 50% from top — Python's `cloudflare.py` convention).
//!    Up to `MAX_CLICK_ATTEMPTS` clicks are spent, spaced
//!    `CLICK_RETRY_TICKS` ticks apart, so a swallowed first click is not
//!    the end of the run.
//! 4. Resolves to [`ClearanceOutcome::ChallengeGone`] when the challenge
//!    disappears without yielding a token — either the iframe we clicked is
//!    gone, or every challenge marker observed on an earlier tick has
//!    vanished (the JS-only interstitial that clears itself).
//! 5. Resolves to [`ClearanceOutcome::TimedOut`] on deadline, carrying
//!    `saw_challenge` — `true` when challenge markers were seen but never
//!    resolved, `false` when the entire timeout window elapsed without
//!    observing any challenge markers (the caller likely invoked the bypass
//!    on a page that has no Cloudflare gate at all).

use std::time::Duration;

use serde::Deserialize;
use tokio::time::Instant;
use zendriver_transport::SessionHandle;

use crate::click::click_at;
use crate::detection::{BoundingBox, eval_main_world};
use crate::error::CloudflareError;

/// Result of a clearance attempt.
#[derive(Debug, Clone)]
pub enum ClearanceOutcome {
    /// Turnstile produced a token (value of `cf-turnstile-response`).
    TokenAcquired(String),
    /// The challenge disappeared without yielding a token — the iframe we
    /// clicked was torn down, or all challenge markers seen earlier in the
    /// run have vanished (a JS-only interstitial that cleared itself).
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

/// How many clicks a single `wait_for_clearance` run may spend on the
/// interactive widget. Cloudflare drops clicks that land while the widget is
/// still booting, so one latched attempt strands the run until the deadline.
const MAX_CLICK_ATTEMPTS: u32 = 3;

/// Poll ticks to wait after a click before spending another attempt. At the
/// default 500ms interval this leaves the widget ~2s to answer a click.
const CLICK_RETRY_TICKS: u32 = 4;

/// Drives a Cloudflare Turnstile clearance flow against a single tab's session.
///
/// Constructed via `Tab::cloudflare()`.
#[derive(Debug)]
pub struct CloudflareBypass<'a> {
    pub(crate) session: &'a SessionHandle,
    pub(crate) poll_interval: Duration,
}

impl<'a> CloudflareBypass<'a> {
    /// Create a new bypass driver bound to `session` with a default 500ms
    /// poll interval.
    pub fn new(session: &'a SessionHandle) -> Self {
        Self {
            session,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    /// Override the default 500ms poll interval used by
    /// [`wait_for_clearance`](Self::wait_for_clearance).
    #[must_use]
    pub fn poll_interval(mut self, dur: Duration) -> Self {
        self.poll_interval = dur;
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
    /// - `Ok(ClearanceOutcome::TokenAcquired(token))` — the
    ///   `cf-turnstile-response` input picked up a non-empty value, either
    ///   after we clicked the interactive iframe or because the page uses
    ///   invisible Turnstile.
    /// - `Ok(ClearanceOutcome::ChallengeGone)` — the challenge went away
    ///   without yielding a token (e.g. a clearance-cookie shortcut): either
    ///   an iframe we clicked was torn down, or challenge markers observed on
    ///   an earlier tick are all gone.
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
            let state = poll_state(self.session).await?;
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
            // every later attempt waits `CLICK_RETRY_TICKS` ticks after the
            // click that landed, and a run spends at most
            // `MAX_CLICK_ATTEMPTS` clicks. So at the default interval the
            // second click lands on tick 5, not tick 4.
            let may_click = clicks < MAX_CLICK_ATTEMPTS
                && (clicks == 0 || ticks_since_click >= CLICK_RETRY_TICKS);

            let mut clicked = false;
            match state.bbox {
                Some(_) if may_click => {
                    // Raw `Input.dispatchMouseEvent` coordinates are viewport
                    // relative, so a below-the-fold widget must be scrolled in
                    // and re-measured before the click can land on it.
                    if let Some(target) = scroll_into_view(self.session).await? {
                        let click_x = target.x + target.width * 0.15;
                        let click_y = target.y + target.height * 0.50;
                        click_at(self.session, click_x, click_y).await?;
                        clicks += 1;
                        ticks_since_click = 0;
                        clicked = true;
                    }
                }
                // The challenge went away without a token: either the iframe
                // we clicked was torn down, or markers seen on an earlier tick
                // are all gone (a JS-only interstitial clearing itself). Both
                // are the clearance-cookie shortcut.
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
            if clicked {
                stall_ticks = 0;
            } else {
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
    /// Non-empty `cf-turnstile-response` / `cf_challenge_response` value, if
    /// any.
    #[serde(default)]
    token: Option<String>,
    /// Bounding box of the Turnstile challenge iframe when it is a valid
    /// click target — mounted, non-zero size, and visible. Walks shadow roots
    /// so shadow-hosted widgets still surface. `None` for a hidden or 0×0
    /// iframe, which must never be clicked.
    #[serde(default)]
    bbox: Option<BoundingBox>,
    /// `true` when the page has any of: a `.cf-turnstile` / `.turnstile`
    /// container, a `cf-turnstile-response` hidden input, or a live
    /// challenge iframe. Used to distinguish "wrong page" (no markers ever
    /// seen) from "real challenge that timed out".
    #[serde(default, rename = "hasMarkers")]
    has_markers: bool,
}

/// Shared in-page prelude for the two evaluators below. Declares:
///
/// - `findChallengeIframe(root)` — the first `challenges.cloudflare.com`
///   iframe reachable from `root`, descending into open shadow roots
///   (Cloudflare sometimes hosts the widget inside a shadow root), else null.
/// - `clickableRect(el)` — `el`'s viewport rect *only if it is a real click
///   target*: non-zero size and not hidden by `visibility` / `display` /
///   `opacity`. A zero-size or hidden iframe is not a target — invisible
///   Turnstile mounts a 0×0 iframe whose token is populated with no click,
///   and clicking it would dispatch mouse events at a meaningless point.
///
/// Never evaluated on its own — [`main_world_expr`] wraps it, so nothing here
/// reaches the page's global object. See that function for why.
///
/// # JavaScript style
/// Injected scripts in this crate declare with `var` and iterate with
/// indexed loops, never `for...of` — `detect.js` included. `for...of` over a
/// `NodeList` goes through `NodeList.prototype[Symbol.iterator]`, which the
/// page can redefine to watch someone walk its DOM; an indexed loop reads
/// `length` and integer keys, which a page cannot instrument without
/// breaking its own scripts. In a crate whose job is to be unobservable that
/// is worth the plainer syntax. This is a rule about declarations and
/// iteration only — modern methods with no such hook, `String.includes`
/// among them, are used freely.
const WALKER_JS: &str = r#"
function findChallengeIframe(root) {
    var iframes = root.querySelectorAll ? root.querySelectorAll("iframe") : [];
    for (var i = 0; i < iframes.length; i++) {
        var f = iframes[i];
        if (f.src && f.src.includes("challenges.cloudflare.com")) return f;
    }
    var all = root.querySelectorAll ? root.querySelectorAll("*") : [];
    for (var j = 0; j < all.length; j++) {
        if (all[j].shadowRoot) {
            var sub = findChallengeIframe(all[j].shadowRoot);
            if (sub) return sub;
        }
    }
    return null;
}
function clickableRect(el) {
    if (!el) return null;
    var r = el.getBoundingClientRect();
    if (!(r.width > 0 && r.height > 0)) return null;
    var view = el.ownerDocument && el.ownerDocument.defaultView;
    var style = view && view.getComputedStyle ? view.getComputedStyle(el) : null;
    if (style && (style.visibility === "hidden" || style.display === "none" || style.opacity === "0")) {
        return null;
    }
    return { x: r.left, y: r.top, width: r.width, height: r.height };
}
"#;

/// Body of the unified poll evaluator — statements only, run inside
/// [`main_world_expr`]'s wrapper alongside [`WALKER_JS`]. Returns:
/// - `token` — non-empty `cf-turnstile-response` (or legacy
///   `cf_challenge_response`) input value, else null.
/// - `bbox` — the challenge iframe's rect when it is a valid click target,
///   else null.
/// - `hasMarkers` — true when *any* Cloudflare challenge marker is present
///   (container, hidden input, or challenge iframe — visible or not).
///
/// The token input is in light DOM by design (page JS reads it to submit
/// forms), so `document.querySelector` is sufficient there.
const POLL_JS: &str = r#"
    var iframe = findChallengeIframe(document);
    var bbox = clickableRect(iframe);
    var input =
        document.querySelector('[name="cf-turnstile-response"]') ||
        document.querySelector('[name="cf_challenge_response"]');
    var token = (input && input.value) ? input.value : null;
    var hasContainer = !!document.querySelector('.cf-turnstile, .turnstile, [data-sitekey]');
    var hasMarkers = hasContainer || !!input || !!iframe;
    return { token: token, bbox: bbox, hasMarkers: hasMarkers };
"#;

/// Body of the scroll-into-view evaluator — statements only, run inside
/// [`main_world_expr`]'s wrapper alongside [`WALKER_JS`]. Brings the
/// challenge iframe fully into the viewport when it isn't already, then
/// returns its *post-scroll* rect — or null if the widget vanished or is not
/// a valid click target.
///
/// `behavior: "instant"` is load-bearing: the default `"auto"` resolves to
/// the element's computed `scroll-behavior`, so on a page setting
/// `html { scroll-behavior: smooth }` the scroll animates and the
/// `clickableRect` call on the next synchronous line reads the *pre*-scroll
/// rect — the one thing this round-trip exists to avoid.
const SCROLL_JS: &str = r#"
    var iframe = findChallengeIframe(document);
    if (!iframe) return null;
    var r = iframe.getBoundingClientRect();
    var vw = window.innerWidth || document.documentElement.clientWidth;
    var vh = window.innerHeight || document.documentElement.clientHeight;
    var fullyVisible = r.top >= 0 && r.left >= 0 && r.bottom <= vh && r.right <= vw;
    if (!fullyVisible && iframe.scrollIntoView) {
        iframe.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
    }
    return clickableRect(iframe);
"#;

/// Compose the in-page source for `body`: [`WALKER_JS`] and `body` together
/// inside a single IIFE, whose completion value is whatever `body` returns.
///
/// The wrapper is a stealth requirement, not formatting. `Runtime.evaluate`
/// with no `contextId` runs a *classic script* in the page's main world,
/// where a top-level `function foo() {}` becomes a property of the global
/// object. Evaluating `WALKER_JS` unwrapped therefore published
/// `window.findChallengeIframe` and `window.clickableRect` to the page on
/// every poll tick — names belonging to an automation library, sitting in
/// the same realm as the challenge script, for anything enumerating `window`
/// to find. Inside the IIFE they are ordinary locals and the page's globals
/// are untouched.
fn main_world_expr(body: &str) -> String {
    format!("(function(){{{WALKER_JS}{body}}})()")
}

/// Run [`POLL_JS`] against `session`'s main world and decode the result.
async fn poll_state(session: &SessionHandle) -> Result<PollState, CloudflareError> {
    let value = eval_main_world(session, &main_world_expr(POLL_JS)).await?;
    serde_json::from_value(value)
        .map_err(|e| CloudflareError::JsError(format!("invalid poll payload: {e}")))
}

/// Run [`SCROLL_JS`] against `session`'s main world: scroll the challenge
/// iframe into view if needed and return the rect to click, or `None` when
/// the widget is gone or not clickable.
async fn scroll_into_view(session: &SessionHandle) -> Result<Option<BoundingBox>, CloudflareError> {
    let value = eval_main_world(session, &main_world_expr(SCROLL_JS)).await?;
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
    use serde_json::{Value, json};

    use super::*;
    use zendriver_transport::testing::{LogCapture, MockConnection};

    /// How long a drain loop waits for the driver's next command before
    /// deciding the run is over. Long enough to cover a poll interval plus
    /// scheduling jitter, short enough that a finished driver ends the loop
    /// promptly instead of hanging the test.
    const DRAIN_BUDGET: Duration = Duration::from_millis(300);

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
        let id = mock.expect_cmd("Runtime.evaluate").await;
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
            let id = mock.expect_cmd("Input.dispatchMouseEvent").await;
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

    /// Names of every *named* `function` declaration in `src` that sits at
    /// `{}` depth 0. An anonymous `function (` — the IIFE wrapper itself — is
    /// a function expression and binds nothing, so it is not counted.
    ///
    /// Brace counting is only sound because no injected source puts a brace
    /// inside a string literal. Keep it that way; the alternative is a JS
    /// parser for a cookie-name-sized job.
    fn top_level_function_declarations(src: &str) -> Vec<String> {
        const KEYWORD: &str = "function";
        let mut found = Vec::new();
        let mut depth = 0i32;
        for (i, ch) in src.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            if !src[i..].starts_with(KEYWORD) {
                continue;
            }
            let rest = src[i + KEYWORD.len()..].trim_start();
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            // A name means a declaration; `function (` is an expression.
            if depth == 0 && !name.is_empty() {
                found.push(name);
            }
        }
        found
    }

    /// The leak guard has to be able to fail. Fed the pre-fix shape — the
    /// walker evaluated beside the IIFE rather than inside it — it must name
    /// the declaration; fed the shipped shape it must stay quiet.
    #[test]
    fn top_level_function_scan_catches_a_leaked_declaration() {
        assert_eq!(
            top_level_function_declarations(
                "function findChallengeIframe(root) { return null; }\n(function(){ return 1; })()"
            ),
            vec!["findChallengeIframe".to_string()],
        );
        assert!(
            top_level_function_declarations("(function(){ function nested(){} return 1; })()")
                .is_empty(),
            "a declaration inside the wrapper is a local, not a global"
        );
    }

    /// `Runtime.evaluate` with no `contextId` runs a classic script in the
    /// page's main world, where a top-level `function foo() {}` becomes
    /// `window.foo`. Publishing `findChallengeIframe` / `clickableRect` there
    /// hands the challenge script — same realm, actively looking — a pair of
    /// names belonging to an automation library, on every poll tick. Every
    /// source this crate injects keeps its helpers inside a function scope.
    #[test]
    fn injected_sources_declare_no_page_globals() {
        for (name, src) in [
            ("poll evaluator", main_world_expr(POLL_JS)),
            ("scroll evaluator", main_world_expr(SCROLL_JS)),
            ("detect.js", include_str!("detect.js").to_string()),
        ] {
            let leaked = top_level_function_declarations(&src);
            assert!(
                leaked.is_empty(),
                "{name} publishes {leaked:?} onto the page's global object"
            );
        }

        // The two composed sources must still *contain* the helpers, so this
        // cannot pass by the walker having quietly gone missing.
        for body in [POLL_JS, SCROLL_JS] {
            let expr = main_world_expr(body);
            assert!(expr.contains("function findChallengeIframe("));
            assert!(expr.contains("function clickableRect("));
            assert!(
                expr.starts_with("(function(){") && expr.ends_with("})()"),
                "the composed source must be exactly one IIFE"
            );
        }
    }

    /// `scrollIntoView`'s default `behavior: "auto"` resolves to the
    /// element's computed `scroll-behavior`, so on a page setting
    /// `html { scroll-behavior: smooth }` the scroll animates and the
    /// `clickableRect` call on the next synchronous line reads the
    /// *pre*-scroll rect — the one thing this extra round-trip exists to
    /// avoid, leaving the click aimed outside the viewport.
    #[test]
    fn scroll_evaluator_pins_instant_scroll_behavior() {
        const CALL: &str = "scrollIntoView(";
        let src = main_world_expr(SCROLL_JS);
        let mut calls = 0;
        for (idx, _) in src.match_indices(CALL) {
            let after = &src[idx + CALL.len()..];
            let end = after.find(')').expect("unterminated scrollIntoView call");
            let args = &after[..end];
            assert!(
                args.contains(r#"behavior: "instant""#),
                "scrollIntoView({args}) leaves behavior to the page's computed scroll-behavior"
            );
            calls += 1;
        }
        assert_eq!(calls, 1, "expected one scrollIntoView call to guard");
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
        let id_poll1 = mock.expect_cmd("Runtime.evaluate").await;
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
        let id_scroll = mock.expect_cmd("Runtime.evaluate").await;
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
        let id_poll2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_poll2,
            eval_reply(poll_value(Some("TOKEN_XYZ"), None, true)),
        )
        .await;

        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::TokenAcquired(t) => assert_eq!(t, "TOKEN_XYZ"),
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

        let id_poll = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_poll,
            eval_reply(poll_value(Some("INVISIBLE_TOKEN"), None, true)),
        )
        .await;

        match fut.await.unwrap().unwrap() {
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
            let id = mock.expect_cmd("Runtime.evaluate").await;
            mock.reply(id, eval_reply(poll_value(None, None, true)))
                .await;
        }

        // Third tick: the token finally lands.
        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id, eval_reply(poll_value(Some("LATE_TOKEN"), None, true)))
            .await;

        match fut.await.unwrap().unwrap() {
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
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id1, eval_reply(poll_value(None, None, true)))
            .await;

        // Tick #2: the interstitial cleared itself — every marker is gone.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id2, eval_reply(poll_value(None, None, false)))
            .await;

        let outcome = fut.await.unwrap().unwrap();
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

        match fut.await.unwrap().unwrap() {
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
        let id_poll2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id_poll2, eval_reply(poll_value(None, None, true)))
            .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(matches!(outcome, ClearanceOutcome::ChallengeGone));
        conn.shutdown();
    }

    /// A widget that stays mounted and unanswered gets clicked again after
    /// [`CLICK_RETRY_TICKS`] ticks — but never more than
    /// [`MAX_CLICK_ATTEMPTS`] times in one run.
    #[tokio::test]
    async fn wait_for_clearance_retries_click_up_to_max_attempts() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                let b = CloudflareBypass::new(&s).poll_interval(Duration::from_millis(1));
                b.wait_for_clearance(Duration::from_millis(200)).await
            }
        });

        let widget = rect(10.0, 20.0, 40.0, 40.0);
        let poll = poll_value(None, Some(widget.clone()), true);

        let mut clicks: u32 = 0;
        while let Some((method, id)) = mock.recv_cmd_timeout(DRAIN_BUDGET).await {
            match method.as_str() {
                "Runtime.evaluate" => {
                    let is_scroll = mock.last_sent()["params"]["expression"]
                        .as_str()
                        .unwrap()
                        .contains("scrollIntoView");
                    let value = if is_scroll {
                        widget.clone()
                    } else {
                        poll.clone()
                    };
                    mock.reply(id, eval_reply(value)).await;
                }
                "Input.dispatchMouseEvent" => {
                    if mock.last_sent()["params"]["type"] == "mousePressed" {
                        clicks += 1;
                    }
                    mock.reply(id, json!({})).await;
                }
                other => panic!("unexpected CDP method {other}"),
            }
        }

        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::TimedOut { saw_challenge } => assert!(saw_challenge),
            other => panic!("expected TimedOut, got {other:?}"),
        }
        assert_eq!(
            clicks, MAX_CLICK_ATTEMPTS,
            "a mounted, unanswered widget should be retried up to the cap"
        );
        conn.shutdown();
    }
}
