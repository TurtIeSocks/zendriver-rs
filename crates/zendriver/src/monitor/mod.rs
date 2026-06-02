//! Persistent network monitor: a `Stream<NetworkEvent>` over HTTP exchanges,
//! WebSocket frames, and EventSource messages. Passive (Network domain) —
//! read-only; use the `interception` feature (Fetch domain) to modify requests.

mod events;

use std::collections::HashMap;

/// One observed network event emitted by a running `NetworkMonitor`.
///
/// Produced by the correlator task that subscribes to CDP `Network.*` events
/// and assembles them into completed exchanges or per-frame notifications.
/// TODO: `NetworkMonitor` / `MonitorBuilder` added in T4.
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    /// A completed HTTP request/response pair (or a failed request).
    Http(NetworkExchange),
    /// A new WebSocket connection was opened.
    WebSocketOpen {
        /// The CDP request ID for this WebSocket connection.
        request_id: String,
        /// The WebSocket URL.
        url: String,
    },
    /// A WebSocket frame was sent or received.
    WebSocketFrame {
        /// The CDP request ID for the owning WebSocket connection.
        request_id: String,
        /// Whether the frame was sent by the page or received from the server.
        direction: FrameDirection,
        /// WebSocket opcode (1 = text, 2 = binary, 8 = close, …).
        opcode: u8,
        /// Frame payload (text frames as UTF-8; binary frames as base64).
        payload: String,
    },
    /// A WebSocket connection was closed.
    WebSocketClose {
        /// The CDP request ID for the closed WebSocket connection.
        request_id: String,
    },
    /// An SSE `EventSource` message was received.
    EventSourceMessage {
        /// The CDP request ID for the `EventSource` stream.
        request_id: String,
        /// The SSE `event:` field (empty string if omitted).
        event_name: String,
        /// The SSE `id:` field (empty string if omitted).
        event_id: String,
        /// The SSE `data:` payload.
        data: String,
    },
}

/// Direction of a WebSocket frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDirection {
    /// Frame sent by the page to the server.
    Sent,
    /// Frame received by the page from the server.
    Received,
}

/// The request half of a completed HTTP exchange.
#[derive(Debug, Clone)]
pub struct MonitoredRequest {
    /// The full request URL.
    pub url: String,
    /// HTTP method (e.g. `"GET"`, `"POST"`).
    pub method: String,
    /// Request headers as sent.
    pub headers: HashMap<String, String>,
    /// Request body for POST/PUT requests, if present.
    pub post_data: Option<String>,
}

/// The response half of a completed HTTP exchange.
#[derive(Debug, Clone)]
pub struct MonitoredResponse {
    /// HTTP status code.
    pub status: u16,
    /// HTTP status text (e.g. `"OK"`, `"Not Found"`).
    pub status_text: String,
    /// Response headers.
    pub headers: HashMap<String, String>,
    /// MIME type reported by Chrome (e.g. `"application/json"`).
    pub mime_type: String,
}

/// A completed HTTP request/response pair observed by the network monitor.
///
/// The `session` field is `pub(crate)` and excluded from the `Debug` impl
/// because `SessionHandle` does not implement `Debug`. Body bytes are fetched
/// on demand via `body()` / `text()` (added in T5).
#[derive(Clone)]
pub struct NetworkExchange {
    /// The observed request.
    pub request: MonitoredRequest,
    /// The response, if one was received before the request finished.
    pub response: Option<MonitoredResponse>,
    /// Network-level error text, if the request failed (`loadingFailed`).
    pub error: Option<String>,
    /// CDP `requestId` — used by `body()` / `text()` to call `getResponseBody`.
    #[allow(dead_code)] // consumed in T5 (NetworkExchange::body/text)
    pub(crate) request_id: String,
    /// Session handle used to issue `getResponseBody` CDP calls.
    #[allow(dead_code)] // consumed in T5 (NetworkExchange::body/text)
    pub(crate) session: zendriver_transport::SessionHandle,
}

impl std::fmt::Debug for NetworkExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkExchange")
            .field("request", &self.request)
            .field("response", &self.response)
            .field("error", &self.error)
            .finish()
    }
}

impl NetworkExchange {
    /// Returns the HTTP status code of the response, if one was received.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        self.response.as_ref().map(|r| r.status)
    }

    /// Returns `true` if the response has a 2xx status code.
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self.status(), Some(s) if (200..300).contains(&s))
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    // `MockConnection::pair()` spawns the connection actor, which requires a
    // tokio runtime — so all tests that construct a `NetworkExchange` are async.
    async fn make_exchange(status: Option<u16>, error: Option<&str>) -> NetworkExchange {
        use zendriver_transport::testing::MockConnection;
        let (_mock, conn) = MockConnection::pair();
        let session = zendriver_transport::SessionHandle::new(conn, "test-session");
        let req = MonitoredRequest {
            url: "https://example.com/api".into(),
            method: "GET".into(),
            headers: HashMap::new(),
            post_data: None,
        };
        let resp = status.map(|s| MonitoredResponse {
            status: s,
            status_text: "OK".into(),
            headers: HashMap::new(),
            mime_type: "application/json".into(),
        });
        NetworkExchange {
            request: req,
            response: resp,
            error: error.map(ToOwned::to_owned),
            request_id: "r1".into(),
            session,
        }
    }

    #[tokio::test]
    async fn status_returns_none_when_no_response() {
        let ex = make_exchange(None, None).await;
        assert!(ex.status().is_none());
        assert!(!ex.is_success());
    }

    #[tokio::test]
    async fn status_returns_some_for_200() {
        let ex = make_exchange(Some(200), None).await;
        assert_eq!(ex.status(), Some(200));
        assert!(ex.is_success());
    }

    #[tokio::test]
    async fn status_304_is_not_success() {
        let ex = make_exchange(Some(304), None).await;
        assert!(!ex.is_success());
    }

    #[tokio::test]
    async fn status_404_is_not_success() {
        let ex = make_exchange(Some(404), None).await;
        assert!(!ex.is_success());
    }

    #[tokio::test]
    async fn debug_does_not_include_session_field() {
        let ex = make_exchange(Some(200), None).await;
        let s = format!("{ex:?}");
        assert!(s.contains("NetworkExchange"));
        assert!(s.contains("request"));
        assert!(s.contains("response"));
        assert!(!s.contains("session"));
    }

    #[tokio::test]
    async fn error_field_is_set_on_failed_exchange() {
        let ex = make_exchange(None, Some("net::ERR_ABORTED")).await;
        assert_eq!(ex.error.as_deref(), Some("net::ERR_ABORTED"));
    }

    #[test]
    fn frame_direction_copy_and_eq() {
        let d = FrameDirection::Sent;
        let d2 = d;
        assert_eq!(d, d2);
        assert_ne!(FrameDirection::Sent, FrameDirection::Received);
    }

    #[test]
    fn network_event_debug_roundtrip() {
        let ev = NetworkEvent::WebSocketOpen {
            request_id: "r1".into(),
            url: "wss://echo.example.com".into(),
        };
        let s = format!("{ev:?}");
        assert!(s.contains("WebSocketOpen"));
        assert!(s.contains("wss://echo.example.com"));
    }
}
