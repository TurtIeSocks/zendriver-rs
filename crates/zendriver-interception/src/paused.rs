//! [`PausedRequest`] — the per-event handle handed to stream consumers.
//!
//! Each `Fetch.requestPaused` event surfaces a `PausedRequest`. Code must
//! dispatch exactly one of [`continue_`], [`abort`], [`respond`],
//! [`modify_and_continue`], or [`continue_response`] to release the paused
//! request — Chrome holds the request open until one of these arrives.
//! [`body`] is a read-only side-call usable at the `Response` stage to inspect
//! the upstream body before deciding what to do; it does not release the
//! pause.
//!
//! [`continue_`]: PausedRequest::continue_
//! [`abort`]: PausedRequest::abort
//! [`respond`]: PausedRequest::respond
//! [`modify_and_continue`]: PausedRequest::modify_and_continue
//! [`continue_response`]: PausedRequest::continue_response
//! [`body`]: PausedRequest::body
//!
//! Internally `PausedRequest` carries a [`SessionHandle`] (not a full `Tab`)
//! so the type lives in this crate without a reverse dependency on
//! `zendriver`. The builder in T6/T7 constructs each instance from the actor
//! loop's session + the decoded event payload.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Map, Value, json};
use zendriver_transport::SessionHandle;

use crate::error::InterceptionError;
use crate::types::{AbortReason, RequestInfo, RequestOverrides, ResponseInfo};

/// A request paused by Chrome at the configured [`RequestStage`].
///
/// `PausedRequest` is consumed by exactly one of the action methods
/// ([`continue_`](Self::continue_), [`abort`](Self::abort),
/// [`respond`](Self::respond),
/// [`modify_and_continue`](Self::modify_and_continue),
/// [`continue_response`](Self::continue_response)) to release the pause.
/// [`body`](Self::body) is a read-only side-channel (`&self`) usable at the
/// `Response` stage to inspect the upstream body before deciding which
/// terminal action to take.
///
/// If a `PausedRequest` is dropped without one of the terminal actions
/// firing (e.g. the consuming task panicked or a `select!` arm cancelled
/// mid-handler), [`Drop`] dispatches a best-effort `Fetch.continueRequest`
/// in a detached task so Chrome doesn't hold the request open indefinitely.
/// See cdpdriver/zendriver#126 for the freeze pattern this prevents.
///
/// [`RequestStage`]: crate::types::RequestStage
#[derive(Debug)]
pub struct PausedRequest {
    /// Opaque CDP request id (`requestId` on `Fetch.requestPaused`). Must be
    /// echoed back on whichever terminal action releases the pause.
    pub request_id: String,
    /// Decoded request payload as Chrome surfaced it.
    pub request: RequestInfo,
    /// Decoded response payload — populated only when Chrome paused at the
    /// `Response` stage. `None` at the `Request` stage.
    pub response: Option<ResponseInfo>,
    session: SessionHandle,
    /// Flipped to `true` by every terminal action (`continue_`, `abort`,
    /// `respond`, `modify_and_continue`, `continue_response`) before the CDP
    /// call is dispatched.
    /// [`Drop`] inspects this to decide whether to fire a fallback
    /// `Fetch.continueRequest` — set means "already released, don't
    /// double-fire"; clear means "owner forgot us, release Chrome".
    released: bool,
}

impl PausedRequest {
    /// Construct a `PausedRequest` from the actor/builder. `pub(crate)` so
    /// the public API stays "consume one of the action methods" — callers
    /// never assemble a `PausedRequest` by hand.
    pub(crate) fn new(
        request_id: impl Into<String>,
        request: RequestInfo,
        response: Option<ResponseInfo>,
        session: SessionHandle,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            request,
            response,
            session,
            released: false,
        }
    }

    /// Release the pause and let Chrome send the request as-is.
    ///
    /// Dispatches `Fetch.continueRequest { requestId }` with no overrides.
    /// Use [`modify_and_continue`](Self::modify_and_continue) instead if any
    /// field needs to be rewritten.
    ///
    /// ```no_run
    /// # use futures::StreamExt;
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_interception::InterceptionError> {
    /// use zendriver_interception::InterceptBuilder;
    ///
    /// let mut stream = Box::pin(InterceptBuilder::new(tab).subscribe());
    /// while let Some(req) = stream.next().await {
    ///     req.continue_().await?;
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn continue_(mut self) -> Result<(), InterceptionError> {
        self.released = true;
        self.session
            .call(
                "Fetch.continueRequest",
                json!({ "requestId": self.request_id }),
            )
            .await?;
        Ok(())
    }

    /// Abort the request with the given Chrome [`Network.ErrorReason`].
    ///
    /// Dispatches `Fetch.failRequest { requestId, errorReason }`. The exact
    /// reason surfaces to JS as the rejected `fetch()` / `XHR` error.
    ///
    /// ```no_run
    /// # use futures::StreamExt;
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_interception::InterceptionError> {
    /// use zendriver_interception::{AbortReason, InterceptBuilder};
    ///
    /// let mut stream = Box::pin(InterceptBuilder::new(tab).subscribe());
    /// if let Some(req) = stream.next().await {
    ///     req.abort(AbortReason::BlockedByClient).await?;
    /// }
    /// # Ok(()) }
    /// ```
    ///
    /// [`Network.ErrorReason`]: https://chromedevtools.github.io/devtools-protocol/tot/Network/#type-ErrorReason
    pub async fn abort(mut self, reason: AbortReason) -> Result<(), InterceptionError> {
        self.released = true;
        self.session
            .call(
                "Fetch.failRequest",
                json!({
                    "requestId": self.request_id,
                    "errorReason": reason.as_cdp_str(),
                }),
            )
            .await?;
        Ok(())
    }

    /// Synthesize a response and serve it in place of the upstream one.
    ///
    /// Dispatches `Fetch.fulfillRequest { requestId, responseCode,
    /// responseHeaders: [{name, value}, ...], body: base64(body) }`. Headers
    /// are CDP's name/value array form; `body` is base64-encoded on the wire
    /// per CDP spec (callers pass raw bytes).
    pub async fn respond(
        mut self,
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<(), InterceptionError> {
        self.released = true;
        let response_headers = crate::actor::headers_to_cdp(&headers);
        self.session
            .call(
                "Fetch.fulfillRequest",
                json!({
                    "requestId": self.request_id,
                    "responseCode": status,
                    "responseHeaders": response_headers,
                    "body": BASE64.encode(&body),
                }),
            )
            .await?;
        Ok(())
    }

    /// Release the pause with per-field overrides applied.
    ///
    /// Dispatches `Fetch.continueRequest { requestId, url?, method?, headers?,
    /// postData? }`. Only fields set on [`RequestOverrides`] are forwarded —
    /// `None` leaves Chrome's original value untouched. Per CDP, `postData`
    /// crosses the wire base64-encoded and `headers` is the CDP name/value
    /// array form (replacement, not merge).
    pub async fn modify_and_continue(
        mut self,
        overrides: RequestOverrides,
    ) -> Result<(), InterceptionError> {
        self.released = true;
        let mut params = Map::new();
        params.insert("requestId".into(), Value::String(self.request_id.clone()));
        if let Some(url) = overrides.url {
            params.insert("url".into(), Value::String(url));
        }
        if let Some(method) = overrides.method {
            params.insert("method".into(), Value::String(method));
        }
        if let Some(headers) = overrides.headers {
            params.insert(
                "headers".into(),
                Value::Array(crate::actor::headers_to_cdp(&headers)),
            );
        }
        if let Some(post_data) = overrides.post_data {
            params.insert("postData".into(), Value::String(BASE64.encode(&post_data)));
        }
        self.session
            .call("Fetch.continueRequest", Value::Object(params))
            .await?;
        Ok(())
    }

    /// Forward the upstream response, optionally rewriting its status and/or
    /// headers, while keeping Chrome's original body.
    ///
    /// Only valid at the `Response` stage — Chrome only accepts
    /// `Fetch.continueResponse` once the upstream response headers have
    /// arrived. Called on a `Request`-stage `PausedRequest` (where
    /// [`response`](Self::response) is `None`), it returns
    /// [`InterceptionError::WrongStage`] without dispatching anything; use
    /// [`respond`](Self::respond) to serve a synthetic response at the
    /// `Request` stage instead.
    ///
    /// Dispatches `Fetch.continueResponse { requestId, responseCode?,
    /// responsePhrase?, responseHeaders? }`, omitting any argument left `None`
    /// so Chrome keeps its original value. Per CDP, `responseHeaders` is the
    /// name/value array form and is *replacement*, not merge — pass every
    /// header you want forwarded.
    ///
    /// ```no_run
    /// # use futures::StreamExt;
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_interception::InterceptionError> {
    /// use zendriver_interception::InterceptBuilder;
    ///
    /// let mut stream = Box::pin(
    ///     InterceptBuilder::new(tab).pattern("*").at_response().subscribe(),
    /// );
    /// if let Some(req) = stream.next().await {
    ///     // Rewrite to 204 No Content, keep the upstream body.
    ///     req.continue_response(Some(204), None, None).await?;
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn continue_response(
        mut self,
        status: Option<u16>,
        phrase: Option<String>,
        headers: Option<Vec<(String, String)>>,
    ) -> Result<(), InterceptionError> {
        // Mark released before the stage check: Chrome rejects
        // continueResponse at the Request stage, but the pause is still ours
        // to release. Suppressing the Drop fallback here would strand it, so
        // we leave `released` clear on the WrongStage path and let Drop fire
        // a best-effort continueRequest — matching the "always release Chrome"
        // contract the other terminals uphold via the happy path.
        if self.response.is_none() {
            return Err(InterceptionError::WrongStage);
        }
        self.released = true;
        let mut params = Map::new();
        params.insert("requestId".into(), Value::String(self.request_id.clone()));
        if let Some(status) = status {
            params.insert("responseCode".into(), Value::from(status));
        }
        if let Some(phrase) = phrase {
            params.insert("responsePhrase".into(), Value::String(phrase));
        }
        if let Some(headers) = headers {
            params.insert(
                "responseHeaders".into(),
                Value::Array(crate::actor::headers_to_cdp(&headers)),
            );
        }
        self.session
            .call("Fetch.continueResponse", Value::Object(params))
            .await?;
        Ok(())
    }

    /// Fetch the upstream response body. Only useful at the `Response`
    /// stage — at the `Request` stage Chrome has no body to return.
    ///
    /// Dispatches `Fetch.getResponseBody { requestId }`. Per CDP the result
    /// carries a `body` string plus a `base64Encoded: bool` flag: when true,
    /// we base64-decode; when false, we return the UTF-8 bytes verbatim. Keeps
    /// `&self` (not `self`) so callers can inspect-then-decide.
    pub async fn body(&self) -> Result<Vec<u8>, InterceptionError> {
        let res = self
            .session
            .call(
                "Fetch.getResponseBody",
                json!({ "requestId": self.request_id }),
            )
            .await?;
        let body = res.get("body").and_then(Value::as_str).ok_or_else(|| {
            InterceptionError::InvalidResponse(
                "Fetch.getResponseBody returned no body field".into(),
            )
        })?;
        let base64_encoded = res
            .get("base64Encoded")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if base64_encoded {
            BASE64
                .decode(body)
                .map_err(|e| InterceptionError::InvalidResponse(format!("invalid base64: {e}")))
        } else {
            Ok(body.as_bytes().to_vec())
        }
    }
}

/// Safety net for cdpdriver/zendriver#126: a paused request that gets dropped
/// without a terminal action (panicked handler, cancelled `select!` arm,
/// stream consumer exited early) would otherwise leave Chrome waiting on
/// `requestId` forever and freeze the whole client. Spawning a detached
/// `Fetch.continueRequest` releases the pause best-effort; failures are
/// logged at `debug!` because the session may already be torn down (e.g. tab
/// closed mid-pause), which is the same condition that produced the original
/// freeze and not an actionable error.
impl Drop for PausedRequest {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let session = self.session.clone();
        let request_id = std::mem::take(&mut self.request_id);
        tokio::spawn(async move {
            if let Err(e) = session
                .call("Fetch.continueRequest", json!({ "requestId": request_id }))
                .await
            {
                tracing::debug!(
                    error = %e,
                    request_id = %request_id,
                    "PausedRequest::drop: best-effort Fetch.continueRequest failed (session likely closed)"
                );
            }
        });
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::types::ResourceType;
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

    fn make_request_info() -> RequestInfo {
        RequestInfo {
            url: "https://example.test/widget".into(),
            method: "GET".into(),
            headers: Vec::new(),
            post_data: None,
            resource_type: ResourceType::XHR,
        }
    }

    fn make_response_info() -> ResponseInfo {
        ResponseInfo {
            status: 200,
            status_text: "OK".into(),
            headers: Vec::new(),
        }
    }

    #[tokio::test]
    async fn continue_dispatches_fetch_continue_request() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let req = PausedRequest::new("REQ-1", make_request_info(), None, sess);

        let fut = tokio::spawn(async move { req.continue_().await });

        let id = mock.expect_cmd("Fetch.continueRequest").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["requestId"], "REQ-1");
        // No overrides on plain continue_: requestId is the only param.
        let params_obj = sent["params"].as_object().unwrap();
        assert_eq!(params_obj.len(), 1);
        mock.reply(id, serde_json::json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn drop_without_terminal_action_fires_fallback_continue() {
        // cdpdriver/zendriver#126: a PausedRequest that goes out of scope
        // without continue_/abort/respond/modify_and_continue must release
        // Chrome — otherwise the InterceptionId stays armed forever and
        // freezes the next render.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        {
            let req = PausedRequest::new("REQ-DROP", make_request_info(), None, sess);
            drop(req);
        }
        // The Drop spawns the call on the current runtime; expect_cmd waits
        // for it to land.
        let id = mock.expect_cmd("Fetch.continueRequest").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["requestId"], "REQ-DROP");
        mock.reply(id, serde_json::json!({})).await;
        conn.shutdown();
    }

    #[tokio::test]
    async fn continue_does_not_double_fire_on_drop() {
        // After continue_() consumes self and returns, Drop runs on the moved
        // value. `released = true` set by continue_() must suppress the Drop
        // fallback — otherwise every successful path would send two CDP
        // round-trips.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let req = PausedRequest::new("REQ-ONCE", make_request_info(), None, sess);

        let fut = tokio::spawn(async move { req.continue_().await });

        let id = mock.expect_cmd("Fetch.continueRequest").await;
        assert_eq!(mock.last_sent()["params"]["requestId"], "REQ-ONCE");
        mock.reply(id, serde_json::json!({})).await;
        fut.await.unwrap().unwrap();

        // Give the runtime a tick — if Drop had spawned a second call it
        // would be queued by now. `try_recv_cmd` returns None if the channel
        // is empty.
        tokio::task::yield_now().await;
        assert!(
            mock.try_recv_cmd().is_none(),
            "Drop fired a second Fetch.continueRequest after continue_ already released"
        );
        conn.shutdown();
    }

    #[tokio::test]
    async fn respond_dispatches_fetch_fulfill_with_base64_body() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let req = PausedRequest::new("REQ-2", make_request_info(), None, sess);

        let body = b"hello world".to_vec();
        let expected_b64 = BASE64.encode(&body);
        let fut = tokio::spawn(async move {
            req.respond(
                200,
                vec![("content-type".into(), "text/plain".into())],
                body,
            )
            .await
        });

        let id = mock.expect_cmd("Fetch.fulfillRequest").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["requestId"], "REQ-2");
        assert_eq!(sent["params"]["responseCode"], 200);
        assert_eq!(sent["params"]["body"], expected_b64);
        let headers = sent["params"]["responseHeaders"].as_array().unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0]["name"], "content-type");
        assert_eq!(headers[0]["value"], "text/plain");
        mock.reply(id, serde_json::json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn continue_response_dispatches_fetch_continue_response() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let req = PausedRequest::new(
            "REQ-CR",
            make_request_info(),
            Some(make_response_info()),
            sess,
        );

        let fut = tokio::spawn(async move {
            req.continue_response(Some(204), None, Some(vec![("x".into(), "y".into())]))
                .await
        });

        let id = mock.expect_cmd("Fetch.continueResponse").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["requestId"], "REQ-CR");
        assert_eq!(sent["params"]["responseCode"], 204);
        // `phrase: None` must be omitted entirely, not sent as null.
        assert!(
            sent["params"]
                .as_object()
                .unwrap()
                .get("responsePhrase")
                .is_none()
        );
        let headers = sent["params"]["responseHeaders"].as_array().unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0]["name"], "x");
        assert_eq!(headers[0]["value"], "y");
        mock.reply(id, serde_json::json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn continue_response_wrong_stage_errs() {
        // Called on a Request-stage PausedRequest (response: None), Chrome
        // would reject continueResponse — we short-circuit with WrongStage
        // and dispatch nothing.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let req = PausedRequest::new("REQ-WS", make_request_info(), None, sess);

        let err = req
            .continue_response(Some(200), None, None)
            .await
            .expect_err("continue_response at Request stage must error");
        assert!(matches!(err, InterceptionError::WrongStage));

        // No Fetch.continueResponse was sent. (A Drop fallback
        // continueRequest *may* land — that's the freeze-safety net, not a
        // continueResponse dispatch.) Give the runtime a tick for any spawned
        // Drop task to enqueue, then drain whatever queued.
        tokio::task::yield_now().await;
        while let Some((method, _id)) = mock.try_recv_cmd() {
            assert_ne!(
                method, "Fetch.continueResponse",
                "WrongStage path must not dispatch Fetch.continueResponse"
            );
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn continue_response_no_double_fire_on_drop() {
        // After continue_response() consumes self and returns, Drop runs on
        // the moved value. `released = true` set by continue_response() must
        // suppress the Drop fallback continueRequest — otherwise the happy
        // path would send two CDP round-trips.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let req = PausedRequest::new(
            "REQ-CR-ONCE",
            make_request_info(),
            Some(make_response_info()),
            sess,
        );

        let fut = tokio::spawn(async move { req.continue_response(Some(200), None, None).await });

        let id = mock.expect_cmd("Fetch.continueResponse").await;
        assert_eq!(mock.last_sent()["params"]["requestId"], "REQ-CR-ONCE");
        mock.reply(id, serde_json::json!({})).await;
        fut.await.unwrap().unwrap();

        tokio::task::yield_now().await;
        assert!(
            mock.try_recv_cmd().is_none(),
            "Drop fired a fallback continueRequest after continue_response already released"
        );
        conn.shutdown();
    }
}
