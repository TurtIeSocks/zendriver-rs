//! CDP frame envelope types. Wraps `chromiumoxide_cdp` typed parameters with
//! the id / method / session_id envelope CDP requires on the wire.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Outbound command frame, wire-encoded JSON sent to Chrome.
#[derive(Debug, Serialize)]
pub struct CdpCommand<'a> {
    /// Monotonic per-connection identifier; Chrome echoes this in the response.
    pub id: u64,
    /// Dotted CDP method name, e.g. `"Page.navigate"`.
    pub method: &'a str,
    /// JSON-encoded parameters for the command.
    pub params: Value,
    /// Optional CDP session identifier; omitted from the wire when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sessionId")]
    pub session_id: Option<&'a str>,
}

/// Inbound frame from Chrome — either a command response or a domain event.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CdpInbound {
    /// Reply to a command identified by `id`.
    Response {
        /// Echoed identifier matching the originating [`CdpCommand::id`].
        id: u64,
        /// Successful result payload, when `error` is absent.
        #[serde(default)]
        result: Option<Value>,
        /// Failure payload, when present.
        #[serde(default)]
        error: Option<CdpRpcError>,
        /// Session the response is associated with, if any.
        #[serde(default, rename = "sessionId")]
        session_id: Option<String>,
    },
    /// Domain event delivered out-of-band.
    Event {
        /// Dotted CDP event name, e.g. `"Page.frameStoppedLoading"`.
        method: String,
        /// JSON-encoded event parameters.
        #[serde(default)]
        params: Value,
        /// Session the event is associated with, if any.
        #[serde(default, rename = "sessionId")]
        session_id: Option<String>,
    },
}

/// CDP-style RPC error returned by Chrome for a failing command.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CdpRpcError {
    /// JSON-RPC error code (e.g. `-32601` for method-not-found).
    pub code: i32,
    /// Human-readable error message from Chrome.
    pub message: String,
    /// Optional structured payload providing additional context.
    #[serde(default)]
    pub data: Option<Value>,
}

/// Untyped event as it leaves the actor's broadcast bus.
///
/// Subscribers downcast to a `chromiumoxide_cdp` typed event by matching on
/// `method` and deserializing `params`.
#[derive(Debug, Clone)]
pub struct RawEvent {
    /// Dotted CDP event name, e.g. `"Page.frameStoppedLoading"`.
    pub method: String,
    /// JSON-encoded event parameters.
    pub params: Value,
    /// Session the event is associated with, if any.
    pub session_id: Option<String>,
}

/// Loss-accounted event as delivered by
/// [`Connection::subscribe_raw_accounted`](crate::connection::Connection::subscribe_raw_accounted).
///
/// [`Connection::subscribe_raw`](crate::connection::Connection::subscribe_raw)
/// silently drops any [`RawEvent`] a lagging subscriber missed and has no way
/// to signal a reconnect or a dead socket. That is fine for the lenient
/// default, but a capture/replay/monitor consumer built on top of it is
/// silently misled about what it actually saw. `AccountedRawEvent` is the
/// opt-in, honest alternative: every gap, reconnect, and disconnect is
/// reported explicitly instead of vanishing.
///
/// Every variant carries a `generation` — the
/// [`Connection::connection_generation`](crate::connection::Connection::connection_generation)
/// active when the variant was produced. `generation` starts at `1` and
/// bumps by one on every
/// [`Connection::reconnect`](crate::connection::Connection::reconnect); each
/// generation gets its own `sequence` counter, restarted at `1`.
#[derive(Debug, Clone)]
pub enum AccountedRawEvent {
    /// A CDP event delivered in order.
    Event {
        /// Generation this event belongs to.
        generation: u64,
        /// Monotonic, per-generation position of this event, starting at 1.
        /// Within a single generation, a gap between two observed `sequence`
        /// values equals the `missed` count of an intervening
        /// [`AccountedRawEvent::Lagged`] — `sequence` resumes after a loss
        /// rather than resetting. Do NOT compare `sequence` across a
        /// [`AccountedRawEvent::Reconnected`]: the counter restarts at 1 for
        /// the new generation, and a `Lagged`'s `missed` may also count the
        /// non-sequenced `Reconnected`/`Disconnected` markers, so only
        /// same-`generation` values are comparable.
        sequence: u64,
        /// The underlying raw CDP event.
        event: RawEvent,
    },
    /// This subscriber fell behind the broadcast bus and missed `missed`
    /// events that were subsequently overwritten. Unlike
    /// [`Connection::subscribe_raw`](crate::connection::Connection::subscribe_raw),
    /// which drops lagged frames without a trace, this variant surfaces the
    /// loss explicitly so a consumer can decide how to react (resync, alert,
    /// abort) instead of silently missing data.
    Lagged {
        /// Generation active when the loss was detected.
        generation: u64,
        /// Number of events this subscriber missed.
        missed: u64,
    },
    /// The connection re-established a fresh WebSocket via
    /// [`Connection::reconnect`](crate::connection::Connection::reconnect).
    /// `sequence` resets to 1 for the new `generation`.
    Reconnected {
        /// Generation of the actor that was replaced.
        previous: u64,
        /// Generation of the newly spawned actor.
        generation: u64,
    },
    /// The underlying WebSocket died unexpectedly — a Chrome-sent Close
    /// frame, a read/write error, or the stream ending — as opposed to a
    /// caller-requested
    /// [`Connection::shutdown`](crate::connection::Connection::shutdown) or
    /// a [`Connection::reconnect`](crate::connection::Connection::reconnect).
    /// Emitted exactly once per generation's genuine death; a shutdown or a
    /// reconnect never produces this variant.
    Disconnected {
        /// Generation whose WebSocket died.
        generation: u64,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn command_serialize_omits_session_id_when_none() {
        let cmd = CdpCommand {
            id: 1,
            method: "Page.navigate",
            params: json!({ "url": "https://example.com" }),
            session_id: None,
        };
        let s = serde_json::to_string(&cmd).expect("ser");
        assert!(
            !s.contains("sessionId"),
            "sessionId should be omitted when None, got: {s}"
        );
        assert!(s.contains(r#""id":1"#));
        assert!(s.contains(r#""method":"Page.navigate""#));
    }

    #[test]
    fn command_serialize_includes_session_id_when_some() {
        let cmd = CdpCommand {
            id: 7,
            method: "Page.navigate",
            params: json!({ "url": "https://example.com" }),
            session_id: Some("S1"),
        };
        let s = serde_json::to_string(&cmd).expect("ser");
        assert!(s.contains(r#""sessionId":"S1""#), "got: {s}");
    }

    #[test]
    fn inbound_deserialize_response_with_result() {
        let raw = r#"{"id":3,"result":{"frameId":"F1"}}"#;
        let parsed: CdpInbound = serde_json::from_str(raw).expect("de");
        match parsed {
            CdpInbound::Response {
                id,
                result,
                error,
                session_id,
            } => {
                assert_eq!(id, 3);
                assert_eq!(result.unwrap()["frameId"], "F1");
                assert!(error.is_none());
                assert!(session_id.is_none());
            }
            CdpInbound::Event { .. } => panic!("expected Response, got Event"),
        }
    }

    #[test]
    fn inbound_deserialize_response_with_error() {
        let raw = r#"{"id":3,"error":{"code":-32602,"message":"Invalid params"}}"#;
        let parsed: CdpInbound = serde_json::from_str(raw).expect("de");
        match parsed {
            CdpInbound::Response { error: Some(e), .. } => {
                assert_eq!(e.code, -32602);
                assert_eq!(e.message, "Invalid params");
            }
            _ => panic!("expected Response with error"),
        }
    }

    #[test]
    fn inbound_deserialize_event() {
        let raw =
            r#"{"method":"Page.frameStoppedLoading","params":{"frameId":"F1"},"sessionId":"S1"}"#;
        let parsed: CdpInbound = serde_json::from_str(raw).expect("de");
        match parsed {
            CdpInbound::Event {
                method,
                params,
                session_id,
            } => {
                assert_eq!(method, "Page.frameStoppedLoading");
                assert_eq!(params["frameId"], "F1");
                assert_eq!(session_id.as_deref(), Some("S1"));
            }
            _ => panic!("expected Event"),
        }
    }
}
