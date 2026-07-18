//! [`ResponseExpectation`] + [`MatchedResponse`] + [`crate::Tab::expect_response`]
//! (gated `expect`).
//!
//! Mirrors [`crate::expect::request`] but watches `Network.responseReceived`
//! and exposes [`MatchedResponse::body`] for fetching the response body via
//! `Network.getResponseBody`. The subscriber task self-cancels after sending
//! the first match so each `expect_response` call is observably one-shot.
//!
//! `Network.enable` is already on for every Tab via the per-Tab in-flight
//! network tracker, so this module does not re-enable the domain.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::time::Sleep;
use zendriver_transport::SessionHandle;

use crate::error::{Result, ZendriverError};
use crate::expect::UrlMatcher;

/// Default outer timeout for a [`ResponseExpectation`] — matches the rest of
/// the high-level surface (`wait_for_load`, etc).
const DEFAULT_EXPECT_TIMEOUT: Duration = Duration::from_secs(30);

/// A network response observed via `Network.responseReceived` that matched
/// an [`UrlMatcher`] passed to [`crate::Tab::expect_response`].
///
/// Decoded from the CDP event payload. The original session handle is
/// retained so [`Self::body`] can issue `Network.getResponseBody` against the
/// same target the response arrived on.
///
/// # Lifetime contract
///
/// `MatchedResponse` does not extend the lifetime of the owning
/// [`crate::Browser`] / [`crate::Tab`]. Once those are dropped, the
/// underlying session is torn down and [`Self::body`] returns a
/// [`ZendriverError`] sourced from the transport layer. In practice the
/// pattern is: await the expectation, immediately call `.body()` (or copy
/// the field you need), then it is safe to drop the originating handles.
///
/// `Debug` is manually implemented since [`SessionHandle`] does not derive
/// it; the session is rendered as a placeholder.
#[derive(Clone)]
pub struct MatchedResponse {
    /// The response URL.
    pub url: String,
    /// HTTP status code (e.g. `200`, `404`).
    pub status: u16,
    /// HTTP status text (e.g. `"OK"`, `"Not Found"`).
    pub status_text: String,
    /// Response headers as reported by Chrome.
    pub headers: HashMap<String, String>,
    /// CDP `requestId` — used by [`Self::body`] when fetching the payload.
    pub request_id: String,
    /// Session this response arrived on. Retained so [`Self::body`] dispatches
    /// `Network.getResponseBody` against the correct target. Crate-private
    /// because the lifetime contract above requires the field to flow
    /// through `Self::body` (which surfaces transport errors when the
    /// session is gone), not direct user dispatch.
    pub(crate) session: SessionHandle,
}

impl std::fmt::Debug for MatchedResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchedResponse")
            .field("url", &self.url)
            .field("status", &self.status)
            .field("status_text", &self.status_text)
            .field("headers", &self.headers)
            .field("request_id", &self.request_id)
            .field("session", &"<SessionHandle>")
            .finish()
    }
}

impl MatchedResponse {
    /// Fetch the response body for this matched response.
    ///
    /// Dispatches `Network.getResponseBody { requestId }`. Per CDP the result
    /// carries a `body` string plus a `base64Encoded: bool` flag: when true,
    /// we base64-decode; when false, we return the UTF-8 bytes verbatim.
    ///
    /// Note: response bodies are only retained by Chrome for a short window
    /// after the response completes — call promptly after the expectation
    /// resolves.
    ///
    /// # Lifetime
    ///
    /// `MatchedResponse` does not keep the owning [`crate::Browser`] /
    /// [`crate::Tab`] alive — if they have been dropped before this call,
    /// the underlying session is torn down and the error surfaces from the
    /// transport layer as [`ZendriverError`]. Call `body()` before dropping
    /// the Browser / Tab.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Cdp`] if Chrome returned no body or
    /// invalid base64. Returns a transport error if the originating session
    /// has been torn down (see "Lifetime" above).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let exp = tab.expect_response("/api/users");
    /// // ... trigger the request ...
    /// let resp = exp.await?;
    /// let body = resp.body().await?;
    /// # let _ = body;
    /// # Ok(()) }
    /// ```
    pub async fn body(&self) -> Result<Vec<u8>> {
        let res = self
            .session
            .call(
                "Network.getResponseBody",
                json!({ "requestId": self.request_id }),
            )
            .await?;
        let body = res
            .get("body")
            .and_then(Value::as_str)
            .ok_or_else(|| ZendriverError::Cdp {
                code: 0,
                message: "Network.getResponseBody returned no body field".into(),
                data: None,
            })?;
        let base64_encoded = res
            .get("base64Encoded")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if base64_encoded {
            BASE64.decode(body).map_err(|e| ZendriverError::Cdp {
                code: 0,
                message: format!("Network.getResponseBody returned invalid base64: {e}"),
                data: None,
            })
        } else {
            Ok(body.as_bytes().to_vec())
        }
    }
}

/// Awaitable handle returned by [`crate::Tab::expect_response`]. Resolves
/// with the first matched [`MatchedResponse`] or [`ZendriverError::Timeout`]
/// if no match arrives within the configured timeout.
///
/// Implements [`Future`] directly — `.await` works without calling
/// `.matched()`. The `.matched()` accessor exists for parity with the
/// Playwright-style fluent API.
#[derive(Debug)]
pub struct ResponseExpectation {
    rx: oneshot::Receiver<Result<MatchedResponse>>,
    timeout: Duration,
    sleep: Option<Pin<Box<Sleep>>>,
}

impl ResponseExpectation {
    /// Override the default 30s timeout.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let resp = tab.expect_response("/api/data").timeout(Duration::from_secs(5)).await?;
    /// # let _ = resp;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        // Reset any already-armed sleep — the next poll will rebuild it
        // with the new deadline.
        self.sleep = None;
        self
    }

    /// Playwright-style alias for `.await`.
    ///
    /// Functionally identical to awaiting the expectation directly.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let resp = tab.expect_response("/api/data").matched().await?;
    /// # let _ = resp;
    /// # Ok(()) }
    /// ```
    pub async fn matched(self) -> Result<MatchedResponse> {
        self.await
    }
}

impl Future for ResponseExpectation {
    type Output = Result<MatchedResponse>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Poll the oneshot first — if the subscriber already sent, return
        // without ever arming the sleep timer.
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(Ok(resp))) => return Poll::Ready(Ok(resp)),
            // Subscriber observed a delivery-loss boundary (disconnect,
            // reconnect, lag, or a same-method decode failure) before a
            // matching response arrived — see `crate::expect::watch`. We
            // can no longer prove the response didn't arrive just before
            // the boundary, so reporting a plain timeout would be a lie.
            Poll::Ready(Ok(Err(e))) => return Poll::Ready(Err(e)),
            Poll::Ready(Err(_)) => {
                // Sender dropped without sending anything — the accounted
                // stream itself ended (e.g. a clean `Connection::shutdown`,
                // which never emits a `Disconnected` boundary) with no
                // boundary and no match observed. Surface as timeout: same
                // observable shape as a genuine no-show.
                return Poll::Ready(Err(ZendriverError::Timeout(self.timeout)));
            }
            Poll::Pending => {}
        }

        // Lazily arm the timer on first poll so `timeout(...)` overrides
        // take effect.
        let timeout = self.timeout;
        let sleep = self
            .sleep
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(timeout)));
        match sleep.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(ZendriverError::Timeout(timeout))),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// CDP `Network.responseReceived` payload subset we care about. Field names
/// follow the protocol (camelCase) via serde rename.
#[derive(Debug, Deserialize)]
struct ResponseReceivedEvent {
    #[serde(rename = "requestId")]
    request_id: String,
    response: ResponsePayload,
}

#[derive(Debug, Deserialize)]
struct ResponsePayload {
    url: String,
    status: u16,
    #[serde(rename = "statusText", default)]
    status_text: String,
    #[serde(default)]
    headers: HashMap<String, String>,
}

/// Spawn a one-shot subscriber that watches `Network.responseReceived` on
/// `session`, sends the first event whose URL matches `matcher` through the
/// `tx`, and exits. Subscription registers synchronously before the returned
/// [`ResponseExpectation`] is constructed so events fired immediately after
/// a trigger action cannot slip past us.
pub(crate) fn register(session: &SessionHandle, matcher: UrlMatcher) -> ResponseExpectation {
    let (tx, rx) = oneshot::channel();
    let mut stream =
        crate::expect::watch::<ResponseReceivedEvent>(session, "Network.responseReceived");
    let session_for_match = session.clone();
    tokio::spawn(async move {
        while let Some(res) = stream.next().await {
            match res {
                Ok(ev) => {
                    if matcher.matches(&ev.response.url) {
                        let matched = MatchedResponse {
                            url: ev.response.url,
                            status: ev.response.status,
                            status_text: ev.response.status_text,
                            headers: ev.response.headers,
                            request_id: ev.request_id,
                            session: session_for_match,
                        };
                        // Send is fallible only if the receiver was
                        // dropped; in that case the caller no longer
                        // cares and we just exit.
                        let _ = tx.send(Ok(matched));
                        return;
                    }
                    // Non-matching response — keep waiting.
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            }
        }
    });
    ResponseExpectation {
        rx,
        timeout: DEFAULT_EXPECT_TIMEOUT,
        sleep: None,
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    /// Register an expectation for `*/api/*`, emit a matching
    /// `Network.responseReceived`, and assert the expectation resolves
    /// with the decoded [`MatchedResponse`].
    #[tokio::test]
    async fn expect_response_resolves_on_matching_event() {
        let (mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let expectation = register(&session, UrlMatcher::from("/api/"));

        // Emit a non-matching event first to verify the matcher actually
        // filters; the subscriber should ignore it.
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({
                "requestId": "R0",
                "response": {
                    "url": "https://example.com/static/app.js",
                    "status": 200,
                    "statusText": "OK",
                    "headers": {},
                },
            }),
            "S1",
        )
        .await;

        // Now emit the matching event.
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({
                "requestId": "R1",
                "response": {
                    "url": "https://example.com/api/users",
                    "status": 201,
                    "statusText": "Created",
                    "headers": {
                        "content-type": "application/json",
                    },
                },
            }),
            "S1",
        )
        .await;

        let matched = tokio::time::timeout(Duration::from_secs(2), expectation)
            .await
            .expect("expectation did not resolve within 2s")
            .expect("expectation returned Err");

        assert_eq!(matched.request_id, "R1");
        assert_eq!(matched.url, "https://example.com/api/users");
        assert_eq!(matched.status, 201);
        assert_eq!(matched.status_text, "Created");
        assert_eq!(
            matched.headers.get("content-type").map(String::as_str),
            Some("application/json"),
        );

        conn.shutdown();
    }

    /// Register an expectation with a 50ms timeout, emit no events, and
    /// assert a genuine no-show still returns `ZendriverError::Timeout` —
    /// the teardown fix must not change this path.
    #[tokio::test]
    async fn expect_response_times_out() {
        let (_mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let expectation =
            register(&session, UrlMatcher::from("/api/")).timeout(Duration::from_millis(50));

        let res = expectation.await;
        match res {
            Err(ZendriverError::Timeout(d)) => {
                assert_eq!(d, Duration::from_millis(50));
            }
            other => panic!("expected Timeout(50ms), got {other:?}"),
        }

        conn.shutdown();
    }

    /// Register an expectation, then simulate a transport teardown
    /// (`AccountedRawEvent::Disconnected`, via `MockConnection::disconnect`)
    /// before any matching response arrives. The wait must resolve with
    /// `EventStreamIncomplete`, not `Timeout` — we lost the ability to
    /// observe the event, we didn't observe its genuine absence.
    #[tokio::test]
    async fn expect_response_returns_event_stream_incomplete_on_disconnect() {
        let (mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let expectation = register(&session, UrlMatcher::from("/api/"));

        mock.disconnect();

        let res = tokio::time::timeout(Duration::from_secs(2), expectation)
            .await
            .expect("expectation did not resolve within 2s after disconnect");
        assert!(
            matches!(res, Err(ZendriverError::EventStreamIncomplete)),
            "expected EventStreamIncomplete after transport teardown, got {res:?}",
        );

        conn.shutdown();
    }

    /// Register an expectation, then simulate a transport *reconnect*
    /// (`AccountedRawEvent::Reconnected`, via `MockConnection::reconnect`)
    /// before any matching response arrives. Like the disconnect case, the
    /// wait must resolve with `EventStreamIncomplete`, not `Timeout` — a
    /// reconnect resets the event stream, so any awaited event that would have
    /// arrived on the old socket can no longer be proven absent. This gives
    /// the `Reconnected` boundary its own end-to-end coverage at the `expect`
    /// layer, independent of the shared `Lagged`/`Disconnected` match arm.
    #[tokio::test]
    async fn expect_response_returns_event_stream_incomplete_on_reconnect() {
        let (mut mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let expectation = register(&session, UrlMatcher::from("/api/"));

        mock.reconnect(&conn);

        let res = tokio::time::timeout(Duration::from_secs(2), expectation)
            .await
            .expect("expectation did not resolve within 2s after reconnect");
        assert!(
            matches!(res, Err(ZendriverError::EventStreamIncomplete)),
            "expected EventStreamIncomplete after transport reconnect, got {res:?}",
        );

        conn.shutdown();
    }

    /// Call `MatchedResponse::body()`, assert the outgoing CDP request is
    /// `Network.getResponseBody { requestId }`, reply with a base64-encoded
    /// payload, and assert the helper returns the decoded bytes.
    #[tokio::test]
    async fn body_dispatches_get_response_body_and_decodes_base64() {
        let (mut mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let raw = b"\x89PNG\r\n\x1a\nfake".to_vec();
        let encoded = BASE64.encode(&raw);

        let matched = MatchedResponse {
            url: "https://example.com/img.png".into(),
            status: 200,
            status_text: "OK".into(),
            headers: HashMap::new(),
            request_id: "REQ-42".into(),
            session: session.clone(),
        };

        let fut = tokio::spawn(async move { matched.body().await });

        let id = mock.expect_cmd("Network.getResponseBody").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["requestId"], "REQ-42");
        mock.reply(
            id,
            json!({
                "body": encoded,
                "base64Encoded": true,
            }),
        )
        .await;

        let bytes = fut
            .await
            .expect("body task panicked")
            .expect("body returned Err");
        assert_eq!(bytes, raw);

        conn.shutdown();
    }
}
