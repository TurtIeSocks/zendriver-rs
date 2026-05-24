//! Test-only helpers — gated behind `cfg(any(test, feature = "testing"))`.
//!
//! Provides [`MockConnection`], a paired pseudo-Chrome that lets downstream
//! tests drive a real [`Connection`] without spawning a WebSocket. The mock
//! and the connection share an in-memory duplex pipe built on the same
//! `DriverStream` plumbing used by this crate's internal actor tests.

#![allow(clippy::expect_used, clippy::panic, clippy::missing_panics_doc)]

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::connection::{spawn_actor, spawn_actor_with_observers, Connection};
use crate::observer::TargetObserver;

/// A paired pseudo-Chrome: tests push frames the driver would read, and read
/// frames the driver sent. Driving an end-to-end interaction looks like:
///
/// ```ignore
/// use serde_json::json;
/// use zendriver_transport::testing::MockConnection;
///
/// # tokio_test::block_on(async {
/// let (mut mock, conn) = MockConnection::pair();
/// let call = tokio::spawn({
///     let c = conn.clone();
///     async move { c.call_raw("Page.navigate", json!({}), None).await }
/// });
/// let id = mock.expect_cmd("Page.navigate").await;
/// mock.reply(id, json!({ "frameId": "F1" })).await;
/// let res = call.await.unwrap().unwrap();
/// # });
/// ```
#[derive(Debug)]
pub struct MockConnection {
    server_in: mpsc::Sender<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    server_out: mpsc::Receiver<Message>,
    last_sent: Option<Value>,
}

impl MockConnection {
    /// Pair a `MockConnection` with a driver-side [`Connection`]. The
    /// connection's actor is spawned onto the current tokio runtime; drop the
    /// connection (and call [`Connection::shutdown`]) to stop the actor.
    #[must_use]
    pub fn pair() -> (Self, Connection) {
        let (tx_to_driver, rx_driver) =
            mpsc::channel::<Result<Message, tokio_tungstenite::tungstenite::Error>>(64);
        let (tx_from_driver, rx_test) = mpsc::channel::<Message>(64);
        let driver = crate::connection::test_only::DriverStream {
            tx: tx_from_driver,
            rx: rx_driver,
        };
        let conn = spawn_actor(driver);
        let mock = MockConnection {
            server_in: tx_to_driver,
            server_out: rx_test,
            last_sent: None,
        };
        (mock, conn)
    }

    /// Variant of [`Self::pair`] that spawns the actor with the given
    /// `observers` chain. Used by downstream crates (notably
    /// `zendriver-stealth`) to assert their observer drives the correct
    /// sequence of CDP calls on `Target.attachedToTarget`.
    #[must_use]
    pub fn pair_with_observers(observers: Vec<Arc<dyn TargetObserver>>) -> (Self, Connection) {
        let (tx_to_driver, rx_driver) =
            mpsc::channel::<Result<Message, tokio_tungstenite::tungstenite::Error>>(64);
        let (tx_from_driver, rx_test) = mpsc::channel::<Message>(64);
        let driver = crate::connection::test_only::DriverStream {
            tx: tx_from_driver,
            rx: rx_driver,
        };
        let conn = spawn_actor_with_observers(driver, observers);
        let mock = MockConnection {
            server_in: tx_to_driver,
            server_out: rx_test,
            last_sent: None,
        };
        (mock, conn)
    }

    /// Block until the driver sends a command whose `method` field matches.
    /// Returns the command id. Wrap in [`tokio::time::timeout`] at test sites
    /// — there is no built-in timeout.
    ///
    /// # Panics
    /// Panics if the driver channel closes, the frame is not a text frame, or
    /// the frame cannot be parsed as JSON with `id` and `method` fields.
    pub async fn expect_cmd(&mut self, method: &str) -> u64 {
        loop {
            let msg = self.server_out.recv().await.expect("driver did not send");
            let text = match msg {
                Message::Text(t) => t,
                other => panic!("expected text frame, got {other:?}"),
            };
            let v: Value = serde_json::from_str(&text).expect("invalid frame");
            self.last_sent = Some(v.clone());
            if v["method"] == method {
                return v["id"].as_u64().expect("frame missing id");
            }
            // Otherwise, keep waiting for the right method.
        }
    }

    /// Returns the most recently observed outbound frame as a JSON value.
    ///
    /// # Panics
    /// Panics if called before any command has been observed via
    /// [`Self::expect_cmd`].
    #[must_use]
    pub fn last_sent(&self) -> &Value {
        self.last_sent.as_ref().expect("no command observed yet")
    }

    /// Reply to command `id` with a success `result`.
    pub async fn reply(&self, id: u64, result: Value) {
        let frame = serde_json::json!({ "id": id, "result": result }).to_string();
        self.server_in
            .send(Ok(Message::text(frame)))
            .await
            .expect("driver closed");
    }

    /// Reply to command `id` with an error payload (`code`, `message`).
    pub async fn reply_err(&self, id: u64, code: i32, message: &str) {
        let frame = serde_json::json!({
            "id": id,
            "error": { "code": code, "message": message }
        })
        .to_string();
        self.server_in
            .send(Ok(Message::text(frame)))
            .await
            .expect("driver closed");
    }

    /// Emit a CDP event with no session id.
    pub async fn emit_event(&self, method: &str, params: Value) {
        let frame = serde_json::json!({ "method": method, "params": params }).to_string();
        self.server_in
            .send(Ok(Message::text(frame)))
            .await
            .expect("driver closed");
    }

    /// Emit a CDP event scoped to a specific session.
    pub async fn emit_event_for_session(&self, method: &str, params: Value, session_id: &str) {
        let frame = serde_json::json!({
            "method": method,
            "params": params,
            "sessionId": session_id,
        })
        .to_string();
        self.server_in
            .send(Ok(Message::text(frame)))
            .await
            .expect("driver closed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn mock_round_trips_a_call() {
        let (mut mock, conn) = MockConnection::pair();
        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw("Page.navigate", json!({ "url": "https://x.test" }), None)
                    .await
            }
        });
        let id = mock.expect_cmd("Page.navigate").await;
        assert_eq!(mock.last_sent()["params"]["url"], "https://x.test");
        mock.reply(id, json!({ "frameId": "F1" })).await;
        let res = call.await.unwrap().unwrap();
        assert_eq!(res["frameId"], "F1");
        conn.shutdown();
    }
}
