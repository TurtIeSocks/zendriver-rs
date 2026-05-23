//! `SessionHandle`: a `Connection` bound to a particular CDP `sessionId`.
//! All commands sent through the handle are routed to that target.

use std::sync::Arc;

use futures::{Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::connection::Connection;
use crate::error::CallError;
use crate::frame::RawEvent;

#[derive(Clone)]
pub struct SessionHandle {
    inner: Arc<Inner>,
}

struct Inner {
    conn: Connection,
    session_id: String,
}

impl SessionHandle {
    pub fn new(conn: Connection, session_id: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                conn,
                session_id: session_id.into(),
            }),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.inner.session_id
    }

    pub fn connection(&self) -> &Connection {
        &self.inner.conn
    }

    /// Send a CDP command routed to this session.
    pub async fn call(&self, method: impl Into<String>, params: Value) -> Result<Value, CallError> {
        self.inner
            .conn
            .call_raw(method, params, Some(self.inner.session_id.clone()))
            .await
    }

    /// Subscribe to events for this session only (others are filtered out).
    pub fn subscribe<T>(&self, method: &'static str) -> impl Stream<Item = T> + Send + Unpin
    where
        T: DeserializeOwned + Send + 'static,
    {
        let sid = self.inner.session_id.clone();
        let raw = self.inner.conn.subscribe_raw();
        Box::pin(raw.filter_map(move |ev: RawEvent| {
            let sid = sid.clone();
            async move {
                if ev.session_id.as_deref() == Some(sid.as_str()) && ev.method == method {
                    serde_json::from_value(ev.params).ok()
                } else {
                    None
                }
            }
        }))
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::connection::spawn_actor;
    use crate::connection::test_only::DriverStream;
    use serde_json::json;
    use tokio::sync::mpsc;
    use tokio_tungstenite::tungstenite::Message;

    fn duplex_pair() -> (
        DriverStream,
        mpsc::Sender<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        mpsc::Receiver<Message>,
    ) {
        let (tx_out, rx_out) = mpsc::channel(32);
        let (tx_in, rx_in) = mpsc::channel(32);
        (
            DriverStream {
                tx: tx_out,
                rx: rx_in,
            },
            tx_in,
            rx_out,
        )
    }

    #[tokio::test]
    async fn session_call_includes_session_id() {
        let (ws, _test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        let sess = SessionHandle::new(conn.clone(), "S1");

        let call = tokio::spawn({
            let s = sess.clone();
            async move {
                s.call("Page.navigate", json!({ "url": "https://x.test" }))
                    .await
            }
        });

        let sent = test_rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(match &sent {
            Message::Text(t) => t,
            _ => panic!("expected text frame"),
        })
        .unwrap();
        assert_eq!(v["sessionId"], "S1");

        // Cancel via dropping
        drop(call);
        conn.shutdown();
    }
}
