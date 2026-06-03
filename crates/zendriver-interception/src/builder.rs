//! [`InterceptBuilder`] — fluent rule + pattern registration.
//!
//! Two-phase API:
//! - **Configure**: chain [`block`], [`block_hosts`], [`redirect`], [`respond`],
//!   [`modify_request`], [`modify_response`] for declarative rules, plus
//!   [`pattern`] / [`at_request`] / [`at_response`] / [`resource`] to control
//!   which CDP `Fetch.RequestPattern` entries are sent on `Fetch.enable`.
//! - **Activate**: [`start`](InterceptBuilder::start) spawns the actor task
//!   (T6) with the registered rules + patterns, returning an
//!   [`InterceptHandle`] for RAII teardown. Alternatively,
//!   [`subscribe`](InterceptBuilder::subscribe) returns a
//!   `Stream<Item = PausedRequest>` for the manual escape-hatch path —
//!   callers drive Chrome's interception loop themselves.
//!
//! The `tab` field is a borrow of [`SessionHandle`] (not the full `Tab` from
//! `zendriver` core) — this crate must not depend on `zendriver` (cycle).
//! `Tab::intercept()` in `zendriver` constructs the builder via
//! `InterceptBuilder::new(self.session())`.
//!
//! [`block`]: InterceptBuilder::block
//! [`block_hosts`]: InterceptBuilder::block_hosts
//! [`redirect`]: InterceptBuilder::redirect
//! [`respond`]: InterceptBuilder::respond
//! [`modify_request`]: InterceptBuilder::modify_request
//! [`modify_response`]: InterceptBuilder::modify_response
//! [`pattern`]: InterceptBuilder::pattern
//! [`at_request`]: InterceptBuilder::at_request
//! [`at_response`]: InterceptBuilder::at_response
//! [`resource`]: InterceptBuilder::resource

use std::sync::Arc;

use futures::stream::{Stream, StreamExt};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use zendriver_transport::SessionHandle;

use crate::actor::{
    InterceptHandle, RequestPausedEvent, build_request_info, build_response_info, run_actor,
    serialize_pattern,
};
use crate::error::InterceptionError;
use crate::host_matcher::HostMatcher;
use crate::paused::PausedRequest;
use crate::rule::Rule;
use crate::types::{
    RequestInfo, RequestOverrides, RequestStage, ResourceType, ResponseInfo, ResponseOverrides,
};
use crate::url_pattern::UrlPattern;

/// A pending `Fetch.RequestPattern` entry to send on `Fetch.enable`.
///
/// CDP's [`Fetch.RequestPattern`] takes an optional `urlPattern`,
/// `resourceType`, and `requestStage`. We mirror it 1:1 here. The builder
/// accumulates these via [`InterceptBuilder::pattern`] / `at_request` /
/// `at_response` / `resource`, mutating the last-pushed entry per chain — so
/// `builder.pattern("*").at_response().resource(Image)` produces a single
/// `RequestPattern` with all three fields set.
///
/// [`Fetch.RequestPattern`]: https://chromedevtools.github.io/devtools-protocol/tot/Fetch/#type-RequestPattern
#[derive(Debug, Clone, Default)]
pub struct RequestPattern {
    /// URL pattern in CDP wildcard syntax. `None` means "match any URL"
    /// (CDP default).
    pub url_pattern: Option<String>,
    /// Resource type filter (e.g. `Image`, `XHR`). `None` means "all types".
    pub resource_type: Option<ResourceType>,
    /// Lifecycle stage at which to pause. `None` means CDP's default
    /// (`Request`).
    pub request_stage: Option<RequestStage>,
}

/// Fluent builder for rule-based interception against a single tab session.
///
/// Construct via `Tab::intercept()` (gated `feature = "interception"`, wired
/// in Task 7). Chain configuration methods to register rules and declare CDP
/// `Fetch.enable` patterns, then call [`start`](Self::start) (Task 7) to
/// activate the background actor or [`subscribe`](Self::subscribe) (Task 7)
/// for the stream-driven escape hatch.
///
/// `'tab` ties the builder's lifetime to the tab's session — the borrow lasts
/// only until `start()` / `subscribe()` consumes the builder.
//
// `Debug` works because `Rule` has a hand-written `Debug` impl that renders
// the closure variant's body as `<closure>`. Inner `Vec<Rule>` derives via
// that.
#[derive(Debug)]
pub struct InterceptBuilder<'tab> {
    tab: &'tab SessionHandle,
    patterns: Vec<RequestPattern>,
    rules: Vec<Rule>,
    /// Optional proxy/server credentials. When set, `Fetch.enable` is sent
    /// with `handleAuthRequests: true` and the actor responds to each
    /// `Fetch.authRequired` event with `Fetch.continueWithAuth` carrying
    /// these credentials. See cdpdriver/zendriver#208.
    auth: Option<(String, String)>,
}

impl<'tab> InterceptBuilder<'tab> {
    /// Construct a fresh builder bound to `tab`'s session.
    ///
    /// `pub` so adapter crates (e.g. `zendriver` core's `Tab::intercept()`
    /// shim) can construct it from a `&SessionHandle` without going through
    /// a trait. End users go through `Tab::intercept()` rather than calling
    /// this directly.
    ///
    /// ```no_run
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_interception::InterceptionError> {
    /// use zendriver_interception::InterceptBuilder;
    ///
    /// let _handle = InterceptBuilder::new(tab)
    ///     .block("*/tracker.js")?
    ///     .start();
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn new(tab: &'tab SessionHandle) -> Self {
        Self {
            tab,
            patterns: Vec::new(),
            rules: Vec::new(),
            auth: None,
        }
    }

    /// Auto-respond to `Fetch.authRequired` challenges with the given
    /// credentials.
    ///
    /// This is the proxy-auth (and HTTP basic-auth) path: `Fetch.enable` is
    /// sent with `handleAuthRequests: true` and every `Fetch.authRequired`
    /// event is answered with `Fetch.continueWithAuth { authChallengeResponse:
    /// { response: "ProvideCredentials", username, password } }`.
    ///
    /// Compose with rules: an `InterceptBuilder` configured with `handle_auth`
    /// and `block` / `redirect` / `respond` rules handles both paths from the
    /// same actor. Combine with [`BrowserBuilder::proxy_auth`] in the
    /// `zendriver` crate if you want the wiring installed automatically on
    /// every tab.
    ///
    /// See cdpdriver/zendriver#208.
    #[must_use]
    pub fn handle_auth(mut self, user: impl Into<String>, pass: impl Into<String>) -> Self {
        self.auth = Some((user.into(), pass.into()));
        self
    }

    /// Push a new pattern entry with the given URL pattern string.
    ///
    /// Subsequent [`at_request`](Self::at_request) /
    /// [`at_response`](Self::at_response) / [`resource`](Self::resource) calls
    /// mutate this newest entry, so a chain like
    /// `.pattern("*").at_response().resource(ResourceType::XHR)` produces one
    /// `RequestPattern` with all three fields populated.
    #[must_use]
    pub fn pattern(mut self, pattern: impl Into<String>) -> Self {
        self.patterns.push(RequestPattern {
            url_pattern: Some(pattern.into()),
            ..RequestPattern::default()
        });
        self
    }

    /// Pause matching requests at the `Request` stage on the most-recently
    /// pushed pattern.
    ///
    /// If no pattern has been pushed yet, this creates an empty one (matches
    /// every URL by CDP default) and sets the stage on it.
    #[must_use]
    pub fn at_request(mut self) -> Self {
        self.ensure_pattern().request_stage = Some(RequestStage::Request);
        self
    }

    /// Pause matching requests at the `Response` stage on the most-recently
    /// pushed pattern.
    #[must_use]
    pub fn at_response(mut self) -> Self {
        self.ensure_pattern().request_stage = Some(RequestStage::Response);
        self
    }

    /// Restrict the most-recently pushed pattern to a single resource type.
    #[must_use]
    pub fn resource(mut self, kind: ResourceType) -> Self {
        self.ensure_pattern().resource_type = Some(kind);
        self
    }

    /// Register a [`Rule::Block`] for `pattern`.
    ///
    /// Compiles `pattern` eagerly; an invalid pattern fails the builder chain
    /// with [`InterceptionError::InvalidPattern`] returned as `Err(Self)` via
    /// the `Result` wrapper.
    pub fn block(mut self, pattern: impl Into<String>) -> Result<Self, InterceptionError> {
        self.rules.push(Rule::Block {
            pattern: UrlPattern::new(pattern)?,
        });
        Ok(self)
    }

    /// Register a [`Rule::BlockHosts`] backed by `matcher`.
    ///
    /// Every request whose host is in `matcher` (exact, or a parent domain on
    /// a dot boundary) is failed with `BlockedByClient`. Composes with other
    /// rules in registration order. `zendriver` core's tracker-blocklist
    /// wiring uses this; most callers reach it via `BrowserBuilder::block_trackers`.
    #[must_use]
    pub fn block_hosts(mut self, matcher: Arc<HostMatcher>) -> Self {
        self.rules.push(Rule::BlockHosts { matcher });
        self
    }

    /// Register a [`Rule::Redirect`] that rewrites `from` → `to`.
    pub fn redirect(
        mut self,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> Result<Self, InterceptionError> {
        self.rules.push(Rule::Redirect {
            from: UrlPattern::new(from)?,
            to: to.into(),
        });
        Ok(self)
    }

    /// Register a [`Rule::Respond`] serving a synthesized response.
    pub fn respond(
        mut self,
        pattern: impl Into<String>,
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<Self, InterceptionError> {
        self.rules.push(Rule::Respond {
            pattern: UrlPattern::new(pattern)?,
            status,
            headers,
            body,
        });
        Ok(self)
    }

    /// Register a [`Rule::Modify`] driven by a user closure.
    ///
    /// The closure runs on the actor task per matching request — it must be
    /// `Send + Sync` and `'static`. Wrap shared state in `Arc` if needed.
    pub fn modify_request<F>(
        mut self,
        pattern: impl Into<String>,
        modify: F,
    ) -> Result<Self, InterceptionError>
    where
        F: Fn(&RequestInfo) -> RequestOverrides + Send + Sync + 'static,
    {
        self.rules.push(Rule::Modify {
            pattern: UrlPattern::new(pattern)?,
            modify: Arc::new(modify),
        });
        Ok(self)
    }

    /// Register a [`Rule::ModifyResponse`] driven by a user closure.
    ///
    /// The closure rewrites an upstream response's status/headers (keeping
    /// Chrome's body) and only fires at the `Response` stage — pair this with
    /// [`at_response`](Self::at_response) so Chrome actually pauses there.
    /// Header overrides are *replacement*, not merge (CDP semantics): return
    /// every header you want forwarded.
    ///
    /// Like [`modify_request`](Self::modify_request), the closure runs on the
    /// actor task per matching response, so it must be `Send + Sync` and
    /// `'static`. Wrap shared state in `Arc` if needed.
    pub fn modify_response<F>(
        mut self,
        pattern: impl Into<String>,
        modify: F,
    ) -> Result<Self, InterceptionError>
    where
        F: Fn(&ResponseInfo) -> ResponseOverrides + Send + Sync + 'static,
    {
        self.rules.push(Rule::ModifyResponse {
            pattern: UrlPattern::new(pattern)?,
            modify: Arc::new(modify),
        });
        Ok(self)
    }

    /// Activate the rule-based interception loop.
    ///
    /// Spawns the background actor task with the registered rules and CDP
    /// `RequestPattern` list, and returns an [`InterceptHandle`] whose
    /// [`Drop`] (or explicit [`stop`](InterceptHandle::stop)) tears the
    /// actor down.
    ///
    /// If no [`pattern`](Self::pattern) entries were added, a single
    /// match-all (`"*"`) pattern is sent so Chrome actually pauses requests
    /// — without it, `Fetch.enable` would attach to nothing and the rule
    /// list would never fire.
    #[must_use = "interception stops when the handle is dropped — bind the returned InterceptHandle to keep it alive"]
    pub fn start(mut self) -> InterceptHandle {
        if self.patterns.is_empty() {
            // Default to a single match-all pattern. Without it Chrome's
            // `Fetch.enable` receives an empty `patterns` array and pauses
            // nothing — silently making every rule a no-op. The actor still
            // sends `handleAuthRequests: false` either way.
            self.patterns.push(RequestPattern {
                url_pattern: Some("*".into()),
                ..RequestPattern::default()
            });
        }
        let cancel = CancellationToken::new();
        let (done_tx, done_rx) = oneshot::channel();
        let actor_session = self.tab.clone();
        let actor_cancel = cancel.clone();
        let actor_rules = self.rules;
        let actor_patterns = self.patterns;
        let actor_auth = self.auth;
        tokio::spawn(async move {
            run_actor(
                actor_session,
                actor_rules,
                actor_patterns,
                actor_auth,
                actor_cancel,
                done_tx,
            )
            .await;
        });
        InterceptHandle::new(cancel, done_rx)
    }

    /// Manual escape-hatch: subscribe to raw [`PausedRequest`] events.
    ///
    /// Enables `Fetch` interception with the declared patterns (defaulting
    /// to a single match-all `"*"` pattern when none were added) and returns
    /// a [`Stream`] that yields one [`PausedRequest`] per `Fetch.requestPaused`
    /// CDP event. Callers must dispatch one of `PausedRequest`'s terminal
    /// methods (`continue_` / `abort` / `respond` / `modify_and_continue`)
    /// to release each pause — Chrome holds the request open otherwise.
    ///
    /// Rules registered via `block` / `redirect` / `respond` / `modify_request`
    /// are ignored on this path: stream consumers drive every paused request
    /// themselves. Use [`start`](Self::start) when you want the actor to
    /// apply rules automatically.
    ///
    /// The returned stream owns the underlying CDP subscription. Dropping
    /// the stream tears the subscription down — Chrome's interception stays
    /// active until the session is closed, but no further pauses surface to
    /// the caller.
    #[must_use = "the returned stream is the only handle on the subscription"]
    pub fn subscribe(mut self) -> impl Stream<Item = PausedRequest> + Send + use<> {
        if self.patterns.is_empty() {
            self.patterns.push(RequestPattern {
                url_pattern: Some("*".into()),
                ..RequestPattern::default()
            });
        }
        // Same ordering as the actor: subscribe BEFORE the (fire-and-forget)
        // enable so we don't drop events Chrome emits between the enable
        // round-trip and the subscription registration.
        let raw = self.tab.subscribe::<Value>("Fetch.requestPaused");
        let session = self.tab.clone();
        let enable_session = session.clone();
        let enable_patterns: Vec<Value> = self.patterns.iter().map(serialize_pattern).collect();
        tokio::spawn(async move {
            if let Err(e) = enable_session
                .call(
                    "Fetch.enable",
                    json!({
                        "patterns": enable_patterns,
                        "handleAuthRequests": false,
                    }),
                )
                .await
            {
                warn!(error = %e, "interception: Fetch.enable failed; subscribe() stream will be empty");
            }
        });
        raw.filter_map(move |ev_value| {
            let session = session.clone();
            async move {
                let ev: RequestPausedEvent = match serde_json::from_value(ev_value) {
                    Ok(ev) => ev,
                    Err(e) => {
                        warn!(error = %e, "interception: skipping malformed Fetch.requestPaused event");
                        return None;
                    }
                };
                let info = build_request_info(&ev);
                let response = build_response_info(&ev);
                Some(PausedRequest::new(ev.request_id, info, response, session))
            }
        })
    }

    /// Lazily push an empty pattern if none exists, so the stage/resource
    /// setters always have a target. Mirrors CDP's "missing fields default to
    /// match-all" semantics.
    fn ensure_pattern(&mut self) -> &mut RequestPattern {
        if self.patterns.is_empty() {
            self.patterns.push(RequestPattern::default());
        }
        self.patterns
            .last_mut()
            .expect("ensure_pattern pushed if empty")
    }

    /// Test-only accessor: number of registered rules. Used by the Task 5
    /// builder test (and future actor tests) without exposing the rule list
    /// as public API.
    #[cfg(test)]
    pub(crate) fn rules_count(&self) -> usize {
        self.rules.len()
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::time::Duration;
    use zendriver_transport::testing::MockConnection;

    /// Register three rules (block + redirect + respond) on a fresh builder
    /// and assert the rule list grew to length 3. Verifies the chain wiring
    /// without touching the actor (Task 6) or CDP dispatch (Task 7).
    #[tokio::test]
    async fn three_rules_register_and_count() {
        let (_mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let builder = InterceptBuilder::new(&sess)
            .block("*/ads/*")
            .unwrap()
            .redirect("*/old/*", "https://example.com/new/")
            .unwrap()
            .respond(
                "*/api/health",
                200,
                vec![("content-type".into(), "application/json".into())],
                br#"{"ok":true}"#.to_vec(),
            )
            .unwrap();

        assert_eq!(builder.rules_count(), 3);
        conn.shutdown();
    }

    /// End-to-end on the rule-driven `start()` path: register a Block rule,
    /// spawn the actor via `start()`, observe `Fetch.enable`, emit a matching
    /// `Fetch.requestPaused`, and assert `Fetch.failRequest` is dispatched.
    ///
    /// This is the actor test from T6 reframed through the `start()` entry
    /// point — proves the builder properly forwards rules + patterns to
    /// `run_actor` (and that `start()` actually spawns the task).
    #[tokio::test]
    async fn start_spawns_actor_with_rules() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let handle = InterceptBuilder::new(&sess)
            .block("*/blocked/*")
            .unwrap()
            .pattern("*")
            .start();

        // The actor's `Fetch.enable` side-task fires fire-and-forget; wait
        // for it to land so the `Fetch.requestPaused` subscription is in
        // place before we emit an event.
        let enable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.enable"))
                .await
                .expect("actor did not send Fetch.enable within 2s");
        let enable_params = mock.last_sent()["params"].clone();
        assert_eq!(enable_params["handleAuthRequests"], false);
        assert_eq!(enable_params["patterns"][0]["urlPattern"], "*");
        mock.reply(enable_id, json!({})).await;

        // Emit a paused-event whose URL matches the Block rule.
        mock.emit_event_for_session(
            "Fetch.requestPaused",
            json!({
                "requestId": "REQ-1",
                "request": {
                    "url": "https://example.test/blocked/banner.png",
                    "method": "GET",
                    "headers": {},
                },
                "resourceType": "Image",
            }),
            "S1",
        )
        .await;

        // Actor should dispatch Fetch.failRequest with BlockedByClient.
        let fail_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.failRequest"))
                .await
                .expect("actor did not send Fetch.failRequest within 2s");
        let fail_params = mock.last_sent()["params"].clone();
        assert_eq!(fail_params["requestId"], "REQ-1");
        assert_eq!(fail_params["errorReason"], "BlockedByClient");
        mock.reply(fail_id, json!({})).await;

        // Teardown via the handle: stop() cancels + awaits the oneshot the
        // actor signals after Fetch.disable lands.
        let stop_fut = tokio::spawn(handle.stop());
        let disable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.disable"))
                .await
                .expect("actor did not send Fetch.disable on stop()");
        mock.reply(disable_id, json!({})).await;
        stop_fut
            .await
            .expect("stop() task panicked")
            .expect("stop() returned Err");
        conn.shutdown();
    }

    /// `start()` injects a match-all `"*"` pattern when the caller did not
    /// add any via [`pattern`](InterceptBuilder::pattern) — otherwise
    /// `Fetch.enable` would arrive with an empty patterns array and Chrome
    /// would silently pause nothing.
    #[tokio::test]
    async fn start_defaults_to_match_all_pattern_when_none_registered() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let handle = InterceptBuilder::new(&sess)
            .block("*/blocked/*")
            .unwrap()
            .start();

        let enable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.enable"))
                .await
                .expect("actor did not send Fetch.enable within 2s");
        let patterns = mock.last_sent()["params"]["patterns"].clone();
        let arr = patterns.as_array().expect("patterns must be a JSON array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["urlPattern"], "*");
        mock.reply(enable_id, json!({})).await;

        // Drop the handle to tear down; we don't need to observe the disable.
        drop(handle);
        conn.shutdown();
    }

    /// On the `subscribe()` path: each `Fetch.requestPaused` event becomes
    /// a `PausedRequest` yielded from the stream, with the request payload
    /// decoded into [`RequestInfo`].
    #[tokio::test]
    async fn subscribe_yields_paused_request_per_event() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let mut stream = Box::pin(InterceptBuilder::new(&sess).subscribe());

        // Wait for the side-task's Fetch.enable to land so the subscription
        // is in place before we emit.
        let enable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.enable"))
                .await
                .expect("subscribe() did not send Fetch.enable within 2s");
        mock.reply(enable_id, json!({})).await;

        mock.emit_event_for_session(
            "Fetch.requestPaused",
            json!({
                "requestId": "REQ-1",
                "request": {
                    "url": "https://example.test/widget.json",
                    "method": "GET",
                    "headers": {"accept": "application/json"},
                },
                "resourceType": "XHR",
            }),
            "S1",
        )
        .await;

        let paused = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("subscribe() stream did not yield within 2s")
            .expect("subscribe() stream closed before yielding");
        assert_eq!(paused.request_id, "REQ-1");
        assert_eq!(paused.request.url, "https://example.test/widget.json");
        assert_eq!(paused.request.method, "GET");
        assert_eq!(
            paused
                .request
                .headers
                .iter()
                .find(|(k, _)| k == "accept")
                .map(|(_, v)| v.as_str()),
            Some("application/json"),
        );
        assert!(
            paused.response.is_none(),
            "request-stage event has no response"
        );

        drop(stream);
        conn.shutdown();
    }
}
