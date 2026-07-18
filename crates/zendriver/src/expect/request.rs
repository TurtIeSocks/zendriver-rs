//! [`RequestExpectation`] + [`MatchedRequest`] + [`crate::Tab::expect_request`]
//! (gated `expect`).
//!
//! Registers a one-shot subscription against `Network.requestWillBeSent` on
//! a tab's session, filters by [`UrlMatcher`], and resolves with the first
//! matching event via a `oneshot` channel. The subscriber task self-cancels
//! after sending so each `expect_request` call is observably one-shot.
//!
//! `Network.enable` is already on for every Tab via the per-Tab in-flight
//! network tracker, so this module does not re-enable the domain.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::oneshot;
use tokio::time::Sleep;
use zendriver_transport::SessionHandle;

use crate::error::{Result, ZendriverError};
use crate::expect::UrlMatcher;

/// Default outer timeout for a [`RequestExpectation`] — matches the rest of
/// the high-level surface (`wait_for_load`, etc).
const DEFAULT_EXPECT_TIMEOUT: Duration = Duration::from_secs(30);

/// A network request observed via `Network.requestWillBeSent` that matched
/// an [`UrlMatcher`] passed to [`crate::Tab::expect_request`].
///
/// Decoded from the CDP event payload. `post_data` is the raw request body
/// bytes if Chrome reported any (POST/PUT/etc).
#[derive(Debug, Clone)]
pub struct MatchedRequest {
    /// The request URL.
    pub url: String,
    /// HTTP method (GET, POST, ...).
    pub method: String,
    /// Request headers as reported by Chrome.
    pub headers: HashMap<String, String>,
    /// Request body if present. `None` for methods without a body.
    pub post_data: Option<Vec<u8>>,
    /// CDP `requestId` — same id reported by `Network.responseReceived`
    /// for the matching response.
    pub request_id: String,
}

/// Awaitable handle returned by [`crate::Tab::expect_request`]. Resolves
/// with the first matched [`MatchedRequest`] or [`ZendriverError::Timeout`]
/// if no match arrives within the configured timeout.
///
/// Implements [`Future`] directly — `.await` works without calling
/// `.matched()`. The `.matched()` accessor exists for parity with the
/// Playwright-style fluent API.
#[derive(Debug)]
pub struct RequestExpectation {
    rx: oneshot::Receiver<Result<MatchedRequest>>,
    timeout: Duration,
    sleep: Option<Pin<Box<Sleep>>>,
}

impl RequestExpectation {
    /// Override the default 30s timeout.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let exp = tab.expect_request("/api/users").timeout(Duration::from_secs(5));
    /// let req = exp.await?;
    /// # let _ = req;
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
    /// let req = tab.expect_request("/api/users").matched().await?;
    /// # let _ = req;
    /// # Ok(()) }
    /// ```
    pub async fn matched(self) -> Result<MatchedRequest> {
        self.await
    }
}

impl Future for RequestExpectation {
    type Output = Result<MatchedRequest>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Poll the oneshot first — if the subscriber already sent, return
        // without ever arming the sleep timer.
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(Ok(req))) => return Poll::Ready(Ok(req)),
            // Subscriber observed a delivery-loss boundary (disconnect,
            // reconnect, lag, or a same-method decode failure) before a
            // matching request arrived — see `crate::expect::watch`. We can
            // no longer prove the request didn't fire just before the
            // boundary, so reporting a plain timeout would be a lie.
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

/// CDP `Network.requestWillBeSent` payload subset we care about. Field
/// names follow the protocol (camelCase) via serde rename.
#[derive(Debug, Deserialize)]
struct RequestWillBeSentEvent {
    #[serde(rename = "requestId")]
    request_id: String,
    request: RequestPayload,
}

#[derive(Debug, Deserialize)]
struct RequestPayload {
    url: String,
    method: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(rename = "postData", default)]
    post_data: Option<String>,
}

/// Spawn a one-shot subscriber that watches `Network.requestWillBeSent` on
/// `session`, sends the first event whose URL matches `matcher` through
/// the `tx`, and exits. Subscription registers synchronously before the
/// returned [`RequestExpectation`] is constructed so events fired
/// immediately after a trigger action cannot slip past us.
pub(crate) fn register(session: &SessionHandle, matcher: UrlMatcher) -> RequestExpectation {
    let (tx, rx) = oneshot::channel();
    let mut stream =
        crate::expect::watch::<RequestWillBeSentEvent>(session, "Network.requestWillBeSent");
    tokio::spawn(async move {
        while let Some(res) = stream.next().await {
            match res {
                Ok(ev) => {
                    if matcher.matches(&ev.request.url) {
                        let matched = MatchedRequest {
                            url: ev.request.url,
                            method: ev.request.method,
                            headers: ev.request.headers,
                            post_data: ev.request.post_data.map(String::into_bytes),
                            request_id: ev.request_id,
                        };
                        // Send is fallible only if the receiver was
                        // dropped; in that case the caller no longer
                        // cares and we just exit.
                        let _ = tx.send(Ok(matched));
                        return;
                    }
                    // Non-matching request — keep waiting.
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            }
        }
    });
    RequestExpectation {
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
    /// `Network.requestWillBeSent`, and assert the expectation resolves
    /// with the decoded [`MatchedRequest`].
    #[tokio::test]
    async fn expect_request_resolves_on_matching_event() {
        let (mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let expectation = register(&session, UrlMatcher::from("/api/"));

        // Emit a non-matching event first to verify the matcher actually
        // filters; the subscriber should ignore it.
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "R0",
                "request": {
                    "url": "https://example.com/static/app.js",
                    "method": "GET",
                    "headers": {},
                },
            }),
            "S1",
        )
        .await;

        // Now emit the matching event.
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "R1",
                "request": {
                    "url": "https://example.com/api/users",
                    "method": "POST",
                    "headers": {
                        "content-type": "application/json",
                    },
                    "postData": "{\"name\":\"x\"}",
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
        assert_eq!(matched.method, "POST");
        assert_eq!(
            matched.headers.get("content-type").map(String::as_str),
            Some("application/json"),
        );
        assert_eq!(
            matched.post_data.as_deref(),
            Some(b"{\"name\":\"x\"}".as_slice())
        );

        conn.shutdown();
    }

    /// Register an expectation with a 50ms timeout, emit no events, and
    /// assert it returns [`ZendriverError::Timeout`] carrying that
    /// duration.
    #[tokio::test]
    async fn expect_request_times_out() {
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
    /// before any matching request arrives. The wait must resolve with
    /// `EventStreamIncomplete`, not `Timeout` — we lost the ability to
    /// observe the event, we didn't observe its genuine absence.
    #[tokio::test]
    async fn expect_request_returns_event_stream_incomplete_on_disconnect() {
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

    /// Sibling of the disconnect test above, forcing a `Lagged` boundary
    /// instead: `MockConnection::pair_with_accounted_capacity(2)` gives the
    /// accounted bus a 2-slot capacity, so pushing 5 unrelated events
    /// overflows it and the next accounted poll reports `Lagged`. Per the
    /// documented policy on [`crate::expect::watch`], `Lagged` can't prove
    /// the awaited request wasn't among the dropped frames, so it also
    /// surfaces as `EventStreamIncomplete` rather than `Timeout`.
    #[tokio::test]
    async fn expect_request_returns_event_stream_incomplete_on_lagged_boundary() {
        let (mock, conn) = MockConnection::pair_with_accounted_capacity(2);
        let session = SessionHandle::new(conn.clone(), "S1");

        let expectation = register(&session, UrlMatcher::from("/api/"));

        // Overflow the 2-slot accounted bus with unrelated events before the
        // expectation's subscriber ever gets to poll it.
        for i in 0..5u32 {
            mock.emit_event("Test.dummy", json!({ "i": i })).await;
        }

        let res = tokio::time::timeout(Duration::from_secs(2), expectation)
            .await
            .expect("expectation did not resolve within 2s after the lag");
        assert!(
            matches!(res, Err(ZendriverError::EventStreamIncomplete)),
            "expected EventStreamIncomplete after a Lagged boundary, got {res:?}",
        );

        conn.shutdown();
    }
}
