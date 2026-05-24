//! [`PausedRequest`] — the per-event handle handed to stream consumers.
//!
//! Each `Fetch.requestPaused` event surfaces a `PausedRequest`. Code must
//! dispatch exactly one of [`continue_`], [`abort`], [`respond`], or
//! [`modify_and_continue`] to release the paused request — Chrome holds the
//! request open until one of these arrives. [`body`] is a read-only side-call
//! usable at the `Response` stage to inspect the upstream body before deciding
//! what to do; it does not release the pause.
//!
//! [`continue_`]: PausedRequest::continue_
//! [`abort`]: PausedRequest::abort
//! [`respond`]: PausedRequest::respond
//! [`modify_and_continue`]: PausedRequest::modify_and_continue
//! [`body`]: PausedRequest::body
//!
//! Internally `PausedRequest` carries a [`SessionHandle`] (not a full `Tab`)
//! so the type lives in this crate without a reverse dependency on
//! `zendriver`. The builder in T6/T7 constructs each instance from the actor
//! loop's session + the decoded event payload.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Map, Value};
use zendriver_transport::SessionHandle;

use crate::error::InterceptionError;
use crate::types::{AbortReason, RequestInfo, RequestOverrides, ResponseInfo};

/// A request paused by Chrome at the configured [`RequestStage`].
///
/// `PausedRequest` is consumed by exactly one of the action methods
/// ([`continue_`](Self::continue_), [`abort`](Self::abort),
/// [`respond`](Self::respond), [`modify_and_continue`](Self::modify_and_continue))
/// to release the pause. [`body`](Self::body) is a read-only side-channel
/// (`&self`) usable at the `Response` stage to inspect the upstream body
/// before deciding which terminal action to take.
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
    pub async fn continue_(self) -> Result<(), InterceptionError> {
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
    pub async fn abort(self, reason: AbortReason) -> Result<(), InterceptionError> {
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
        self,
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<(), InterceptionError> {
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
        self,
        overrides: RequestOverrides,
    ) -> Result<(), InterceptionError> {
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

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::types::ResourceType;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    fn make_request_info() -> RequestInfo {
        RequestInfo {
            url: "https://example.test/widget".into(),
            method: "GET".into(),
            headers: Vec::new(),
            post_data: None,
            resource_type: ResourceType::XHR,
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
}
