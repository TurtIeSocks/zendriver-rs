//! Background interception actor.
//!
//! The crate-private `run_actor` is the rule-driven loop spawned by
//! [`InterceptBuilder::start`](crate::builder::InterceptBuilder::start). It
//! owns a single tab's `Fetch.*` interception lifecycle:
//!
//! 1. Subscribes to `Fetch.requestPaused` on the supplied [`SessionHandle`]
//!    **before** firing `Fetch.enable`. Mirrors the subscriber pattern used
//!    by the zendriver core's frame-lifecycle and network-idle trackers —
//!    events Chrome fires between the enable round-trip and our subscription
//!    would otherwise be dropped, and the `MockConnection` test harness in
//!    `zendriver-transport` (gated `feature = "testing"`) never replies to
//!    fire-and-forget enables anyway.
//! 2. Sends `Fetch.enable { patterns, handleAuthRequests }` with the
//!    explicit pattern list supplied by the builder; `handleAuthRequests`
//!    flips to `true` when the builder also called
//!    [`InterceptBuilder::handle_auth`](crate::builder::InterceptBuilder::handle_auth)
//!    so Chrome surfaces `Fetch.authRequired` events the actor answers with
//!    `Fetch.continueWithAuth`.
//! 3. Per `Fetch.requestPaused` event: walks `rules` in registration order,
//!    first match wins, dispatches the matching action's CDP call. No
//!    match → plain `Fetch.continueRequest` (let through).
//! 4. On cancellation: fires `Fetch.disable` and exits. The handle returned
//!    by the builder owns a [`CancellationToken`] that fires on `Drop`, so
//!    interception always tears down deterministically when the handle
//!    leaves scope.
//!
//! [`InterceptHandle`] is the user-facing RAII guard. Its [`stop`] method is
//! the explicit-shutdown path — it cancels the token *and* awaits a oneshot
//! the actor signals on exit, so the caller observes `Fetch.disable` has
//! reached the wire before `stop()` returns.
//!
//! [`stop`]: InterceptHandle::stop

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};
use zendriver_transport::SessionHandle;

use crate::builder::RequestPattern;
use crate::error::InterceptionError;
use crate::rule::Rule;
use crate::types::{RequestInfo, RequestOverrides, ResourceType, ResponseInfo, ResponseOverrides};

/// RAII guard returned by `InterceptBuilder::start` (Task 7).
///
/// The guard cancels the actor on [`Drop`] so interception always tears down
/// when the handle leaves scope. Call [`stop`](Self::stop) instead when the
/// caller needs to observe `Fetch.disable` reaching the wire before
/// proceeding — `Drop` is fire-and-forget by construction.
#[derive(Debug)]
#[must_use = "interception stops when the handle is dropped — bind it to a variable to keep it alive"]
pub struct InterceptHandle {
    cancel: CancellationToken,
    // `Option` so `stop(self)` can `.take()` the receiver without `Drop`
    // racing on a half-moved field. `None` after `stop()` consumed it.
    done: Option<oneshot::Receiver<()>>,
}

impl InterceptHandle {
    /// Construct a handle from the cancel token + actor-exit receiver. The
    /// constructor is `pub(crate)` so the only public path is via
    /// [`InterceptBuilder::start`](crate::builder::InterceptBuilder::start).
    pub(crate) fn new(cancel: CancellationToken, done: oneshot::Receiver<()>) -> Self {
        Self {
            cancel,
            done: Some(done),
        }
    }

    /// Test-support constructor: build a no-op handle backed by an unused
    /// cancel token + a `oneshot::channel`'s receiver whose sender is
    /// immediately dropped. Intended for downstream unit tests that need
    /// to populate a registry of `InterceptHandle`s without going through
    /// the actor pipeline. Dropping the handle still calls `.cancel()`
    /// on the token (no observable side effect — nothing is listening).
    ///
    /// Gated behind the `test-support` feature so production builds don't
    /// expose it.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn for_tests() -> Self {
        let (_done_tx, done_rx) = oneshot::channel();
        Self {
            cancel: CancellationToken::new(),
            done: Some(done_rx),
        }
    }

    /// Stop the actor and wait for it to acknowledge exit.
    ///
    /// Cancels the actor's token, then awaits the oneshot the actor sends
    /// after `Fetch.disable` reaches the wire. Returns
    /// [`InterceptionError::SubscriptionClosed`] if the actor was already
    /// gone (channel closed without a signal — e.g. transport torn down
    /// mid-flight); callers can usually treat that as success since the
    /// effect (interception is off) is identical.
    pub async fn stop(mut self) -> Result<(), InterceptionError> {
        self.cancel.cancel();
        match self.done.take() {
            Some(rx) => rx.await.map_err(|_| InterceptionError::SubscriptionClosed),
            None => Ok(()),
        }
    }
}

impl Drop for InterceptHandle {
    fn drop(&mut self) {
        // Fire-and-forget on drop: cancel the actor's token. The actor's
        // own `Fetch.disable` call will race the transport teardown, but
        // since `Fetch.disable` is harmless when the session is already
        // closing we don't try to await anything here.
        self.cancel.cancel();
    }
}

/// Decoded `Fetch.requestPaused` event payload.
///
/// Projects only the fields the actor consumes. Extra fields Chrome sends
/// (e.g. `frameId`, `networkId`) are deliberately ignored — the rule API
/// surfaces what callers asked for via [`RequestInfo`] / [`ResponseInfo`].
///
/// `pub(crate)` so [`crate::builder::InterceptBuilder::subscribe`] can reuse
/// the same projection on the stream path.
#[derive(Debug, Deserialize)]
pub(crate) struct RequestPausedEvent {
    #[serde(rename = "requestId")]
    pub(crate) request_id: String,
    pub(crate) request: RequestPayload,
    #[serde(rename = "resourceType", default)]
    pub(crate) resource_type: Option<String>,
    // Only populated at the `Response` stage.
    #[serde(rename = "responseStatusCode", default)]
    pub(crate) response_status_code: Option<u16>,
    #[serde(rename = "responseStatusText", default)]
    pub(crate) response_status_text: Option<String>,
    #[serde(rename = "responseHeaders", default)]
    pub(crate) response_headers: Option<Vec<HeaderPair>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RequestPayload {
    pub(crate) url: String,
    pub(crate) method: String,
    #[serde(default)]
    pub(crate) headers: HashMap<String, String>,
    /// Chrome's text representation of the request body. For multipart /
    /// binary uploads this can be lossy — Chrome rebuilds via UTF-8 best
    /// effort. Prefer [`Self::post_data_entries`] when present.
    #[serde(rename = "postData", default)]
    pub(crate) post_data: Option<String>,
    #[serde(rename = "hasPostData", default)]
    _has_post_data: Option<bool>,
    /// Per-chunk base64-encoded bytes. Chrome emits this for binary /
    /// multipart bodies where the text representation would be lossy.
    /// When present, it is the canonical source of truth for the body.
    #[serde(rename = "postDataEntries", default)]
    pub(crate) post_data_entries: Option<Vec<PostDataEntry>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PostDataEntry {
    /// Base64-encoded body bytes. Per CDP `Network.PostDataEntry`.
    #[serde(default)]
    pub(crate) bytes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HeaderPair {
    pub(crate) name: String,
    pub(crate) value: String,
}

/// Run the interception actor until `cancel` fires.
///
/// See the module-level docs for the lifecycle contract. The function exits
/// after `Fetch.disable` is dispatched on cancellation, or immediately if
/// the event stream closes (e.g. transport torn down).
///
/// `done` is the oneshot the actor signals on exit so the matching
/// [`InterceptHandle::stop`] call can synchronize on actor teardown.
pub(crate) async fn run_actor(
    session: SessionHandle,
    rules: Vec<Rule>,
    patterns: Vec<RequestPattern>,
    auth: Option<(String, String)>,
    cancel: CancellationToken,
    done: oneshot::Sender<()>,
) {
    // Step 1: subscribe BEFORE enable (P4 pattern). Events Chrome emits
    // between our enable round-trip and the subscription registration would
    // otherwise be lost. Also: the mock test harness never replies to the
    // synthetic `Fetch.enable` call, so awaiting it first would deadlock the
    // actor before any subscription existed.
    let mut paused = session.subscribe::<Value>("Fetch.requestPaused");
    // When `handleAuthRequests: true`, Chrome additionally emits
    // `Fetch.authRequired` events for proxy / HTTP basic-auth challenges.
    // Subscribe up-front for the same race-free reason as `requestPaused`.
    let mut auth_required = session.subscribe::<Value>("Fetch.authRequired");

    // Step 2: fire-and-forget `Fetch.enable`. Mirrors `InFlightTracker::run`
    // / `frame::lifecycle::run`: a failed enable surfaces as a `warn!` but
    // the actor keeps running (no events arrive — interception silently
    // no-ops — which is the same observable behavior the user gets from
    // any other torn-down session).
    let enable_session = session.clone();
    let enable_patterns: Vec<Value> = patterns.iter().map(serialize_pattern).collect();
    let handle_auth_requests = auth.is_some();
    tokio::spawn(async move {
        if let Err(e) = enable_session
            .call(
                "Fetch.enable",
                json!({
                    "patterns": enable_patterns,
                    "handleAuthRequests": handle_auth_requests,
                }),
            )
            .await
        {
            warn!(error = %e, "interception: Fetch.enable failed; interception inactive");
        }
    });

    // Step 3: event loop.
    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                trace!("interception: cancellation received, disabling Fetch and exiting");
                break;
            }
            Some(ev_value) = paused.next() => {
                // Decode to our projection. Chrome may add fields in future
                // protocol versions — we skip ones we don't understand.
                let ev: RequestPausedEvent = match serde_json::from_value(ev_value) {
                    Ok(ev) => ev,
                    Err(e) => {
                        warn!(error = %e, "interception: skipping malformed Fetch.requestPaused event");
                        continue;
                    }
                };
                if let Err(e) = handle_paused(&session, &rules, ev).await {
                    warn!(error = %e, "interception: handler dispatch failed");
                }
            }
            Some(ev_value) = auth_required.next() => {
                // `Fetch.authRequired` carries a `requestId` we must echo
                // back via `Fetch.continueWithAuth`. If `auth` is None the
                // user didn't ask for auth handling — fall back to
                // `Default` so Chrome surfaces a normal auth dialog instead
                // of hanging the pause forever.
                let Some(request_id) = ev_value
                    .get("requestId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                else {
                    warn!("interception: Fetch.authRequired without requestId");
                    continue;
                };
                let response = match &auth {
                    Some((user, pass)) => json!({
                        "response": "ProvideCredentials",
                        "username": user,
                        "password": pass,
                    }),
                    None => json!({ "response": "Default" }),
                };
                if let Err(e) = session
                    .call(
                        "Fetch.continueWithAuth",
                        json!({
                            "requestId": request_id,
                            "authChallengeResponse": response,
                        }),
                    )
                    .await
                {
                    warn!(error = %e, "interception: Fetch.continueWithAuth failed");
                }
            }
            else => {
                // Stream closed (transport gone). Nothing left to observe.
                trace!("interception: event stream closed, exiting without Fetch.disable");
                // Skip the disable below — the transport is gone, the call
                // would fail anyway.
                let _ = done.send(());
                return;
            }
        }
    }

    // Step 4: best-effort `Fetch.disable` on shutdown. If it fails (session
    // already torn down) we log and exit — the handle's caller still gets
    // the oneshot signal so `stop()` doesn't hang.
    if let Err(e) = session.call("Fetch.disable", json!({})).await {
        warn!(error = %e, "interception: Fetch.disable failed during shutdown");
    }
    // Signal exit. The receiver may already be gone (handle dropped without
    // `stop()`), which is fine — the `Drop` path didn't await it.
    let _ = done.send(());
}

/// Walk the rule list against `ev.request.url` and dispatch the first match.
/// No match → plain `Fetch.continueRequest` so Chrome proceeds as if no
/// interception were registered.
async fn handle_paused(
    session: &SessionHandle,
    rules: &[Rule],
    ev: RequestPausedEvent,
) -> Result<(), InterceptionError> {
    let url = ev.request.url.clone();

    // Find the first rule whose pattern matches. Walk the slice rather than
    // building an iterator — the rule list is small (typically < 10) and
    // this keeps the borrow checker quiet without `find` + closure lifetimes.
    let matched = rules.iter().find(|r| r.matches(&url));

    match matched {
        Some(Rule::Block { .. }) | Some(Rule::BlockHosts { .. }) => {
            fail_request(session, &ev.request_id, "BlockedByClient").await
        }
        Some(Rule::Redirect { to, .. }) => continue_with_url(session, &ev.request_id, to).await,
        Some(Rule::Respond {
            status,
            headers,
            body,
            ..
        }) => fulfill_request(session, &ev.request_id, *status, headers, body).await,
        Some(Rule::Modify { modify, .. }) => {
            let info = build_request_info(&ev);
            let overrides = modify(&info);
            continue_with_overrides(session, &ev.request_id, overrides).await
        }
        Some(Rule::ModifyResponse { modify, .. }) => match build_response_info(&ev) {
            Some(info) => {
                let overrides = modify(&info);
                continue_response_with_overrides(session, &ev.request_id, overrides).await
            }
            None => {
                // Matched at the Request stage: Chrome has no response yet, so
                // `Fetch.continueResponse` would be rejected. Let the request
                // through unchanged so the pause is still released — the
                // closure runs later if a Response-stage pattern re-pauses it.
                tracing::debug!(
                    request_id = %ev.request_id,
                    url = %url,
                    "interception: ModifyResponse matched at Request stage; no response yet, passing through"
                );
                continue_passthrough(session, &ev.request_id).await
            }
        },
        None => continue_passthrough(session, &ev.request_id).await,
    }
}

/// Serialize a [`RequestPattern`] into the JSON shape CDP expects on
/// `Fetch.enable.patterns[]`. All three fields are optional per CDP.
pub(crate) fn serialize_pattern(p: &RequestPattern) -> Value {
    let mut obj = Map::new();
    if let Some(url) = &p.url_pattern {
        obj.insert("urlPattern".into(), Value::String(url.clone()));
    }
    if let Some(rt) = p.resource_type {
        obj.insert("resourceType".into(), Value::String(rt.as_cdp_str().into()));
    }
    if let Some(stage) = p.request_stage {
        obj.insert(
            "requestStage".into(),
            Value::String(stage.as_cdp_str().into()),
        );
    }
    Value::Object(obj)
}

/// Build a [`RequestInfo`] from the decoded event for `Modify` closures.
///
/// Body precedence: `postDataEntries` (canonical, base64-decoded + concatenated)
/// when present, else `postData` interpreted as UTF-8 bytes. The string
/// fallback is necessarily lossy for binary bodies — Chrome only emits
/// `postDataEntries` when it knows the text form would mangle the bytes.
///
/// Headers come from `Network.Request.headers` (CDP object) so we materialize
/// them as a `Vec<(name, value)>` on the boundary; the upstream HashMap may
/// have collapsed duplicates already, but for the request side CDP also
/// pre-merges so this is faithful.
pub(crate) fn build_request_info(ev: &RequestPausedEvent) -> RequestInfo {
    RequestInfo {
        url: ev.request.url.clone(),
        method: ev.request.method.clone(),
        headers: ev
            .request
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        post_data: decode_post_data(&ev.request),
        resource_type: parse_resource_type(ev.resource_type.as_deref()),
    }
}

fn decode_post_data(req: &RequestPayload) -> Option<Vec<u8>> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;

    if let Some(entries) = req.post_data_entries.as_ref() {
        let mut buf = Vec::new();
        for entry in entries {
            let Some(b64) = entry.bytes.as_deref() else {
                continue;
            };
            match BASE64.decode(b64) {
                Ok(bytes) => buf.extend_from_slice(&bytes),
                Err(e) => {
                    tracing::warn!(error = %e, "interception: bad base64 in postDataEntries; skipping entry");
                }
            }
        }
        return Some(buf);
    }
    req.post_data.as_deref().map(|s| s.as_bytes().to_vec())
}

/// Build a [`ResponseInfo`] from the decoded event when Chrome paused at the
/// `Response` stage. Returns `None` at the `Request` stage (the event
/// payload's `responseStatusCode` is absent).
///
/// Used on both the rule-driven actor path and the
/// [`crate::builder::InterceptBuilder::subscribe`] stream path.
pub(crate) fn build_response_info(ev: &RequestPausedEvent) -> Option<ResponseInfo> {
    let status = ev.response_status_code?;
    let status_text = ev.response_status_text.clone().unwrap_or_default();
    let headers: Vec<(String, String)> = ev
        .response_headers
        .as_ref()
        .map(|hs| {
            hs.iter()
                .map(|h| (h.name.clone(), h.value.clone()))
                .collect()
        })
        .unwrap_or_default();
    Some(ResponseInfo {
        status,
        status_text,
        headers,
    })
}

/// Serialize a `[(name, value)]` slice into CDP's `[{name, value}]` JSON
/// array shape used by `Fetch.continueRequest.headers` and
/// `Fetch.fulfillRequest.responseHeaders`.
pub(crate) fn headers_to_cdp(headers: &[(String, String)]) -> Vec<Value> {
    headers
        .iter()
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect()
}

/// Best-effort parse of a CDP `Network.ResourceType` string into our enum.
/// Defaults to [`ResourceType::Other`] for unknown strings rather than
/// failing the whole event — Chrome occasionally adds new types we don't
/// know about yet, and dropping a real intercepted request for that would
/// be a worse failure mode than reporting `Other`.
fn parse_resource_type(s: Option<&str>) -> ResourceType {
    match s.unwrap_or("Other") {
        "Document" => ResourceType::Document,
        "Stylesheet" => ResourceType::Stylesheet,
        "Image" => ResourceType::Image,
        "Media" => ResourceType::Media,
        "Font" => ResourceType::Font,
        "Script" => ResourceType::Script,
        "TextTrack" => ResourceType::TextTrack,
        "XHR" => ResourceType::XHR,
        "Fetch" => ResourceType::Fetch,
        "EventSource" => ResourceType::EventSource,
        "WebSocket" => ResourceType::WebSocket,
        "Manifest" => ResourceType::Manifest,
        "SignedExchange" => ResourceType::SignedExchange,
        "Ping" => ResourceType::Ping,
        "CSPViolationReport" => ResourceType::CSPViolationReport,
        "Preflight" => ResourceType::Preflight,
        _ => ResourceType::Other,
    }
}

// --- CDP dispatch helpers --------------------------------------------------

async fn fail_request(
    session: &SessionHandle,
    request_id: &str,
    error_reason: &str,
) -> Result<(), InterceptionError> {
    session
        .call(
            "Fetch.failRequest",
            json!({
                "requestId": request_id,
                "errorReason": error_reason,
            }),
        )
        .await?;
    Ok(())
}

async fn continue_passthrough(
    session: &SessionHandle,
    request_id: &str,
) -> Result<(), InterceptionError> {
    session
        .call("Fetch.continueRequest", json!({ "requestId": request_id }))
        .await?;
    Ok(())
}

async fn continue_with_url(
    session: &SessionHandle,
    request_id: &str,
    url: &str,
) -> Result<(), InterceptionError> {
    session
        .call(
            "Fetch.continueRequest",
            json!({
                "requestId": request_id,
                "url": url,
            }),
        )
        .await?;
    Ok(())
}

async fn continue_with_overrides(
    session: &SessionHandle,
    request_id: &str,
    overrides: RequestOverrides,
) -> Result<(), InterceptionError> {
    let mut params = Map::new();
    params.insert("requestId".into(), Value::String(request_id.into()));
    if let Some(url) = overrides.url {
        params.insert("url".into(), Value::String(url));
    }
    if let Some(method) = overrides.method {
        params.insert("method".into(), Value::String(method));
    }
    if let Some(headers) = overrides.headers {
        params.insert("headers".into(), Value::Array(headers_to_cdp(&headers)));
    }
    if let Some(post_data) = overrides.post_data {
        params.insert("postData".into(), Value::String(BASE64.encode(&post_data)));
    }
    session
        .call("Fetch.continueRequest", Value::Object(params))
        .await?;
    Ok(())
}

async fn fulfill_request(
    session: &SessionHandle,
    request_id: &str,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(), InterceptionError> {
    let response_headers = headers_to_cdp(headers);
    session
        .call(
            "Fetch.fulfillRequest",
            json!({
                "requestId": request_id,
                "responseCode": status,
                "responseHeaders": response_headers,
                "body": BASE64.encode(body),
            }),
        )
        .await?;
    Ok(())
}

/// Dispatch `Fetch.continueResponse` with the closure-produced overrides for a
/// [`Rule::ModifyResponse`](crate::rule::Rule::ModifyResponse) match. Mirrors
/// [`PausedRequest::continue_response`](crate::PausedRequest::continue_response):
/// `None` fields are omitted so Chrome keeps its originals, and
/// `responseHeaders` is *replacement* per CDP. The caller guarantees the
/// `Response` stage (we only reach here after `build_response_info` succeeded).
async fn continue_response_with_overrides(
    session: &SessionHandle,
    request_id: &str,
    overrides: ResponseOverrides,
) -> Result<(), InterceptionError> {
    let mut params = Map::new();
    params.insert("requestId".into(), Value::String(request_id.into()));
    if let Some(status) = overrides.status {
        params.insert("responseCode".into(), Value::from(status));
    }
    if let Some(phrase) = overrides.phrase {
        params.insert("responsePhrase".into(), Value::String(phrase));
    }
    if let Some(headers) = overrides.headers {
        params.insert(
            "responseHeaders".into(),
            Value::Array(headers_to_cdp(&headers)),
        );
    }
    session
        .call("Fetch.continueResponse", Value::Object(params))
        .await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::url_pattern::UrlPattern;
    use std::time::Duration;
    use zendriver_transport::testing::MockConnection;

    /// End-to-end mock drive of the rule-based actor:
    ///   1. Spawn `run_actor` with a single Block rule for `*/blocked/*`.
    ///   2. Expect the fire-and-forget `Fetch.enable` and reply.
    ///   3. Emit a matching `Fetch.requestPaused` event.
    ///   4. Assert the actor dispatches `Fetch.failRequest` with
    ///      `errorReason = BlockedByClient`.
    ///   5. Cancel + expect `Fetch.disable` (RAII teardown contract).
    #[tokio::test]
    async fn block_rule_dispatches_fail_request_with_blocked_by_client() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let rules = vec![Rule::Block {
            pattern: UrlPattern::new("*/blocked/*").unwrap(),
        }];
        let patterns = vec![RequestPattern {
            url_pattern: Some("*".into()),
            ..RequestPattern::default()
        }];
        let cancel = CancellationToken::new();
        let (done_tx, done_rx) = oneshot::channel();
        let actor_cancel = cancel.clone();
        let actor = tokio::spawn(async move {
            run_actor(sess, rules, patterns, None, actor_cancel, done_tx).await;
        });

        // Step 1: the actor fires `Fetch.enable` in a side-task. The mock
        // never replies to the call (per the P4 pattern — InFlightTracker /
        // frame::lifecycle do the same); we just observe it landed so the
        // subsequent `emit_event_for_session` runs after the subscription
        // is in place.
        let enable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.enable"))
                .await
                .expect("actor did not send Fetch.enable within 2s");
        let enable_params = mock.last_sent()["params"].clone();
        assert_eq!(enable_params["handleAuthRequests"], false);
        assert_eq!(enable_params["patterns"][0]["urlPattern"], "*");
        // Reply so the side-task completes cleanly (not strictly required —
        // the mock harness usually doesn't — but it keeps the warn! quiet).
        mock.reply(enable_id, json!({})).await;

        // Step 2: emit a `Fetch.requestPaused` event whose URL matches the
        // Block rule. The actor should dispatch `Fetch.failRequest`.
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

        // Step 3: expect the fail_request dispatch.
        let fail_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.failRequest"))
                .await
                .expect("actor did not send Fetch.failRequest within 2s");
        let fail_params = mock.last_sent()["params"].clone();
        assert_eq!(fail_params["requestId"], "REQ-1");
        assert_eq!(fail_params["errorReason"], "BlockedByClient");
        mock.reply(fail_id, json!({})).await;

        // Step 4: cancel the actor + verify it dispatches `Fetch.disable`
        // on shutdown and signals exit through the oneshot.
        cancel.cancel();
        let disable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.disable"))
                .await
                .expect("actor did not send Fetch.disable on cancel");
        mock.reply(disable_id, json!({})).await;

        tokio::time::timeout(Duration::from_secs(2), done_rx)
            .await
            .expect("actor did not signal exit within 2s")
            .expect("oneshot sender dropped without sending");
        actor.await.unwrap();
        conn.shutdown();
    }

    /// Same end-to-end drive as the `Block` test, but the rule is a
    /// `BlockHosts` matcher and the request matches by host (subdomain walk).
    #[tokio::test]
    async fn block_hosts_rule_dispatches_fail_request() {
        use crate::host_matcher::HostMatcher;
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let rules = vec![Rule::BlockHosts {
            matcher: std::sync::Arc::new(HostMatcher::new(["evil.com".to_string()])),
        }];
        let patterns = vec![RequestPattern {
            url_pattern: Some("*".into()),
            ..RequestPattern::default()
        }];
        let cancel = CancellationToken::new();
        let (done_tx, done_rx) = oneshot::channel();
        let actor_cancel = cancel.clone();
        let actor = tokio::spawn(async move {
            run_actor(sess, rules, patterns, None, actor_cancel, done_tx).await;
        });

        let enable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.enable"))
                .await
                .expect("actor did not send Fetch.enable within 2s");
        mock.reply(enable_id, json!({})).await;

        // Subdomain of a listed host -> must be failed.
        mock.emit_event_for_session(
            "Fetch.requestPaused",
            json!({
                "requestId": "REQ-1",
                "request": {
                    "url": "https://cdn.evil.com/fp.js",
                    "method": "GET",
                    "headers": {},
                },
                "resourceType": "Script",
            }),
            "S1",
        )
        .await;

        let fail_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.failRequest"))
                .await
                .expect("actor did not send Fetch.failRequest within 2s");
        let fail_params = mock.last_sent()["params"].clone();
        assert_eq!(fail_params["requestId"], "REQ-1");
        assert_eq!(fail_params["errorReason"], "BlockedByClient");
        mock.reply(fail_id, json!({})).await;

        cancel.cancel();
        let disable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.disable"))
                .await
                .expect("actor did not send Fetch.disable on cancel");
        mock.reply(disable_id, json!({})).await;

        tokio::time::timeout(Duration::from_secs(2), done_rx)
            .await
            .expect("actor did not signal exit within 2s")
            .expect("oneshot sender dropped without sending");
        actor.await.unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn actor_handles_auth_required_with_credentials() {
        // cdpdriver/zendriver#208: proxy / HTTP basic-auth support. When the
        // builder is configured with `handle_auth(user, pass)`, the actor
        // must (a) send `Fetch.enable { handleAuthRequests: true }` and
        // (b) respond to each `Fetch.authRequired` event with
        // `Fetch.continueWithAuth { authChallengeResponse:
        // ProvideCredentials + user/pass }`.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let cancel = CancellationToken::new();
        let (done_tx, done_rx) = oneshot::channel();
        let actor_cancel = cancel.clone();
        let auth = Some(("user1".to_string(), "pass1".to_string()));
        let actor = tokio::spawn(async move {
            run_actor(
                sess,
                Vec::new(),
                vec![RequestPattern {
                    url_pattern: Some("*".into()),
                    ..RequestPattern::default()
                }],
                auth,
                actor_cancel,
                done_tx,
            )
            .await;
        });

        let enable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.enable"))
                .await
                .expect("actor did not send Fetch.enable within 2s");
        assert_eq!(
            mock.last_sent()["params"]["handleAuthRequests"],
            true,
            "auth-enabled actor must flip handleAuthRequests"
        );
        mock.reply(enable_id, json!({})).await;

        mock.emit_event_for_session(
            "Fetch.authRequired",
            json!({
                "requestId": "AUTH-REQ-1",
                "request": { "url": "https://example.test/", "method": "GET" },
                "frameId": "F1",
                "resourceType": "Document",
                "authChallenge": {
                    "source": "Proxy",
                    "origin": "http://proxy.test",
                    "scheme": "basic",
                    "realm": "",
                },
            }),
            "S1",
        )
        .await;

        let auth_id = tokio::time::timeout(
            Duration::from_secs(2),
            mock.expect_cmd("Fetch.continueWithAuth"),
        )
        .await
        .expect("actor did not send Fetch.continueWithAuth within 2s");
        let params = mock.last_sent()["params"].clone();
        assert_eq!(params["requestId"], "AUTH-REQ-1");
        assert_eq!(
            params["authChallengeResponse"]["response"],
            "ProvideCredentials"
        );
        assert_eq!(params["authChallengeResponse"]["username"], "user1");
        assert_eq!(params["authChallengeResponse"]["password"], "pass1");
        mock.reply(auth_id, json!({})).await;

        cancel.cancel();
        let disable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.disable"))
                .await
                .expect("actor did not send Fetch.disable on cancel");
        mock.reply(disable_id, json!({})).await;
        tokio::time::timeout(Duration::from_secs(2), done_rx)
            .await
            .expect("actor did not signal exit")
            .expect("oneshot sender dropped");
        actor.await.unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn actor_without_auth_responds_default_to_auth_required() {
        // Defensive: even when the builder did NOT configure auth, an
        // `authRequired` event must be released (Default response) so Chrome
        // doesn't hang. handleAuthRequests stays false so this path only
        // triggers if the server pushed a stray event we didn't ask for —
        // exercising it confirms the actor degrades gracefully.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S2");
        let cancel = CancellationToken::new();
        let (done_tx, done_rx) = oneshot::channel();
        let actor_cancel = cancel.clone();
        let actor = tokio::spawn(async move {
            run_actor(
                sess,
                Vec::new(),
                vec![RequestPattern {
                    url_pattern: Some("*".into()),
                    ..RequestPattern::default()
                }],
                None,
                actor_cancel,
                done_tx,
            )
            .await;
        });

        let enable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.enable"))
                .await
                .expect("actor did not send Fetch.enable");
        assert_eq!(mock.last_sent()["params"]["handleAuthRequests"], false);
        mock.reply(enable_id, json!({})).await;

        mock.emit_event_for_session(
            "Fetch.authRequired",
            json!({ "requestId": "AUTH-REQ-2" }),
            "S2",
        )
        .await;

        let auth_id = tokio::time::timeout(
            Duration::from_secs(2),
            mock.expect_cmd("Fetch.continueWithAuth"),
        )
        .await
        .expect("actor did not respond to stray authRequired");
        assert_eq!(
            mock.last_sent()["params"]["authChallengeResponse"]["response"],
            "Default"
        );
        mock.reply(auth_id, json!({})).await;

        cancel.cancel();
        let disable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.disable"))
                .await
                .expect("actor did not send Fetch.disable");
        mock.reply(disable_id, json!({})).await;
        tokio::time::timeout(Duration::from_secs(2), done_rx)
            .await
            .expect("actor did not exit")
            .expect("oneshot dropped");
        actor.await.unwrap();
        conn.shutdown();
    }
}
