//! Transport-layer errors.

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    #[error("websocket closed unexpectedly")]
    Disconnected,

    #[error("websocket: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("framing: {0}")]
    Frame(#[from] serde_json::Error),

    #[error("connection shut down")]
    Shutdown,

    #[error("response channel dropped before reply (id={id})")]
    ResponseDropped { id: u64 },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_disconnected_is_stable() {
        assert_eq!(TransportError::Disconnected.to_string(), "websocket closed unexpectedly");
    }

    #[test]
    fn display_shutdown_is_stable() {
        assert_eq!(TransportError::Shutdown.to_string(), "connection shut down");
    }

    #[test]
    fn display_response_dropped_includes_id() {
        let e = TransportError::ResponseDropped { id: 42 };
        assert_eq!(e.to_string(), "response channel dropped before reply (id=42)");
    }

    #[test]
    fn source_preserved_through_ws_wrap() {
        // Construct a tungstenite error and wrap it; check source chain works.
        let tung = tokio_tungstenite::tungstenite::Error::ConnectionClosed;
        let wrapped = TransportError::Ws(tung);
        // Display starts with "websocket: "
        assert!(wrapped.to_string().starts_with("websocket: "));
        // source() returns the inner
        assert!(std::error::Error::source(&wrapped).is_some());
    }
}
