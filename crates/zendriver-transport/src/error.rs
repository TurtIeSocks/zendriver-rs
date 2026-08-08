//! Transport-layer errors.

/// Connection-level failure modes — anything that happens "below" a CDP
/// response getting routed back to its caller.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// The WebSocket closed without Chrome having sent a Close frame.
    #[error("websocket closed unexpectedly")]
    Disconnected,

    /// Tungstenite raised an error on the underlying WebSocket.
    #[error("websocket: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),

    /// JSON serialization or framing failed.
    #[error("framing: {0}")]
    Frame(#[from] serde_json::Error),

    /// The actor task has been told to shut down — pending calls drain with
    /// this variant so callers don't hang forever.
    #[error("connection shut down")]
    Shutdown,

    /// The actor sent a reply but the oneshot receiver had already been
    /// dropped. Carries the originating command id for diagnostics.
    #[error("response channel dropped before reply (id={id})")]
    ResponseDropped {
        /// Command id whose reply landed without a receiver.
        id: u64,
    },

    /// An I/O error occurred (typically inside tungstenite).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Result of a CDP call: either a transport-level failure, or a structured
/// JSON-RPC error returned by Chrome. Higher layers (the `zendriver` crate)
/// map `Rpc` into the typed `ZendriverError::Cdp` variant.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CallError {
    /// Connection-level failure (see [`TransportError`]).
    #[error("transport: {0}")]
    Transport(#[from] TransportError),
    /// Chrome answered the command with a structured JSON-RPC error. Carries
    /// the JSON-RPC `code`, `message`, and optional `data` payload.
    #[error("CDP RPC error [{0}] {1}")]
    Rpc(i32, String, Option<serde_json::Value>),

    /// The call did not complete within its budget. The budget covers the
    /// **whole** call, so this has two routes:
    ///
    /// - the command was written to the socket and Chrome never answered it;
    ///   or
    /// - the command never reached the socket at all, because the actor's
    ///   command channel stayed full for the entire budget.
    ///
    /// The variant does not distinguish them — a caller cannot tell from the
    /// error whether Chrome ever saw the command.
    ///
    /// Deliberately distinct from both siblings, because the three mean
    /// different things and warrant different responses:
    ///
    /// - [`CallError::Rpc`] — Chrome heard the command and **said no**. The
    ///   browser is healthy; the command was wrong. Retrying is pointless.
    /// - [`CallError::Transport`] — the **connection broke**. Chrome may be
    ///   gone; the handle is unusable.
    /// - `Timeout` — the connection has **not reported a failure**. Chrome is
    ///   wedged, the operation is slower than the budget allows, or the actor
    ///   is too backed up to take the command. Retrying (or raising the
    ///   budget) can be reasonable.
    ///
    /// Carries the method name because that is the diagnostic that makes a
    /// stuck call actionable: "the call did not complete" is not a bug report,
    /// "`Page.navigate` did not complete within 180s" is.
    #[error("CDP call `{method}` did not complete within {budget:?}")]
    Timeout {
        /// The CDP method that did not complete (e.g. `"Page.navigate"`).
        method: String,
        /// The budget that elapsed. It bounds the whole call, so it may have
        /// run out while the command was still queued, before Chrome saw it.
        budget: std::time::Duration,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_disconnected_is_stable() {
        assert_eq!(
            TransportError::Disconnected.to_string(),
            "websocket closed unexpectedly"
        );
    }

    #[test]
    fn display_shutdown_is_stable() {
        assert_eq!(TransportError::Shutdown.to_string(), "connection shut down");
    }

    #[test]
    fn display_response_dropped_includes_id() {
        let e = TransportError::ResponseDropped { id: 42 };
        assert_eq!(
            e.to_string(),
            "response channel dropped before reply (id=42)"
        );
    }

    /// The wording is load-bearing, not cosmetic: the budget wraps the enqueue
    /// as well as the reply, so a `Timeout` can be returned for a command
    /// Chrome never saw. "went unanswered" claimed the command reached the
    /// socket, which is only true on one of the two routes into this variant.
    #[test]
    fn display_call_timeout_names_the_method_and_budget_without_claiming_chrome_saw_it() {
        let e = CallError::Timeout {
            method: "Page.navigate".into(),
            budget: std::time::Duration::from_secs(180),
        };
        assert_eq!(
            e.to_string(),
            "CDP call `Page.navigate` did not complete within 180s"
        );
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
