//! `Connection` — the public handle to the transport actor.

use std::sync::Arc;

use futures::{Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

use crate::actor::{run_actor, OutboundCmd, EVENT_BUS_CAPACITY};
use crate::error::TransportError;
use crate::frame::{CdpRpcError, RawEvent};

/// Cheap-to-clone handle to the connection actor. All `Tab`s and `Element`s
/// hold one of these (via `Arc<...>`); the actor itself runs in a separate
/// tokio task.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<ConnectionInner>,
}

pub(crate) struct ConnectionInner {
    pub(crate) cmd_tx: mpsc::Sender<OutboundCmd>,
    pub(crate) event_tx: broadcast::Sender<RawEvent>,
    pub(crate) shutdown: CancellationToken,
}

impl Connection {
    /// Send a CDP command and await its response.
    ///
    /// `method` is the dotted CDP method name (e.g. `"Page.navigate"`).
    /// `params` is the JSON value for the command's parameters.
    /// `session_id` routes the command to a particular target's session.
    pub async fn call_raw(
        &self,
        method: impl Into<String>,
        params: Value,
        session_id: Option<String>,
    ) -> Result<Value, TransportError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .send(OutboundCmd {
                method: method.into(),
                params,
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| TransportError::Shutdown)?;
        match reply_rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(rpc_err)) => Err(rpc_err_to_transport(rpc_err)),
            Err(_) => Err(TransportError::Shutdown),
        }
    }

    /// Subscribe to all events on this connection (no filtering).
    pub fn subscribe_raw(&self) -> impl Stream<Item = RawEvent> + Send + Unpin {
        Box::pin(
            BroadcastStream::new(self.inner.event_tx.subscribe()).filter_map(|res| async move {
                // Lagged frames are dropped.
                res.ok()
            }),
        )
    }

    /// Subscribe to events of a specific CDP method, deserialized into `T`.
    pub fn subscribe<T>(&self, method: &'static str) -> impl Stream<Item = T> + Send + Unpin
    where
        T: DeserializeOwned + Send + 'static,
    {
        Box::pin(BroadcastStream::new(self.inner.event_tx.subscribe()).filter_map(
            move |res| async move {
                let ev = res.ok()?;
                if ev.method == method {
                    serde_json::from_value(ev.params).ok()
                } else {
                    None
                }
            },
        ))
    }

    /// Trigger graceful shutdown of the underlying actor.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// Public accessor for advanced users who need to drive the underlying
    /// shutdown token directly.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.inner.shutdown.clone()
    }
}

fn rpc_err_to_transport(e: CdpRpcError) -> TransportError {
    // Mapping is mostly for the transport-level cases that round-trip; higher
    // layers map richer CDP errors. Here we just preserve the message.
    if e.code == -32001 && e.message.contains("shut down") {
        TransportError::Shutdown
    } else {
        // Embed the JSON-RPC error as an io error so it survives the trait
        // bounds; richer mapping is the job of the zendriver crate.
        TransportError::Io(std::io::Error::other(format!(
            "[{code}] {msg}",
            code = e.code,
            msg = e.message
        )))
    }
}

/// Connect to a Chrome DevTools WebSocket URL and spawn the actor.
pub async fn connect(ws_url: &str) -> Result<Connection, TransportError> {
    use tokio_tungstenite::connect_async;
    let (ws, _resp) = connect_async(ws_url).await?;
    Ok(spawn_actor(ws))
}

/// Spawn the actor on the given pre-connected WebSocket. Mainly for tests
/// and for `connect`; production code uses `connect`.
pub fn spawn_actor<S>(ws: S) -> Connection
where
    S: futures::Sink<tokio_tungstenite::tungstenite::Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures::Stream<
            Item = Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>,
        > + Send
        + Unpin
        + 'static,
{
    let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(64);
    let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
    let shutdown = CancellationToken::new();
    tokio::spawn(run_actor(ws, cmd_rx, event_tx.clone(), shutdown.clone()));
    Connection {
        inner: Arc::new(ConnectionInner {
            cmd_tx,
            event_tx,
            shutdown,
        }),
    }
}

/// Re-export the test `DriverStream` type at a shared visibility level so
/// both `actor::tests` and `connection::tests` can construct it.
#[cfg(test)]
pub(crate) mod test_only {
    use tokio::sync::mpsc;
    use tokio_tungstenite::tungstenite::Message;

    pub struct DriverStream {
        pub tx: mpsc::Sender<Message>,
        pub rx: mpsc::Receiver<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    }

    impl futures::Sink<Message> for DriverStream {
        type Error = tokio_tungstenite::tungstenite::Error;

        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn start_send(
            self: std::pin::Pin<&mut Self>,
            item: Message,
        ) -> Result<(), Self::Error> {
            self.tx
                .try_send(item)
                .map_err(|_| tokio_tungstenite::tungstenite::Error::ConnectionClosed)
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    impl futures::Stream for DriverStream {
        type Item = Result<Message, tokio_tungstenite::tungstenite::Error>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            self.rx.poll_recv(cx)
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::connection::test_only::DriverStream;
    use serde_json::json;
    use tokio_tungstenite::tungstenite::Message;

    fn duplex_pair() -> (
        DriverStream,
        tokio::sync::mpsc::Sender<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        tokio::sync::mpsc::Receiver<Message>,
    ) {
        let (tx_out, rx_out) = tokio::sync::mpsc::channel::<Message>(32);
        let (tx_in, rx_in) =
            tokio::sync::mpsc::channel::<Result<Message, tokio_tungstenite::tungstenite::Error>>(32);
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
    async fn call_raw_round_trips_through_actor() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);

        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw("Page.navigate", json!({ "url": "https://x.test" }), None)
                    .await
            }
        });

        let sent = test_rx.recv().await.unwrap();
        let id = serde_json::from_str::<Value>(match &sent {
            Message::Text(t) => t,
            _ => panic!("expected text frame"),
        })
        .unwrap()["id"]
            .as_u64()
            .unwrap();

        test_tx
            .send(Ok(Message::text(
                json!({ "id": id, "result": { "frameId": "F1" } }).to_string(),
            )))
            .await
            .unwrap();

        let res = call.await.unwrap().unwrap();
        assert_eq!(res["frameId"], "F1");

        conn.shutdown();
    }
}
