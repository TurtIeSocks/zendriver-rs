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
