//! `Connection` — the public handle to the transport actor.

use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

use crate::actor::{EVENT_BUS_CAPACITY, OutboundCmd, run_actor};
use crate::error::{CallError, TransportError};
use crate::frame::RawEvent;
use crate::observer::TargetObserver;

/// Default ceiling on a single observer's `on_target_attached` future, applied
/// before the actor releases the debugger via
/// `Runtime.runIfWaitingForDebugger`. Slow observers don't block the actor
/// indefinitely; a misbehaving one trips the timeout and the debugger releases.
pub(crate) const DEFAULT_OBSERVER_TIMEOUT: Duration = Duration::from_secs(5);

/// Internal JSON-RPC code stamped onto drained pendings when the transport
/// actor shuts down. Mapped back to [`TransportError::Shutdown`] by
/// [`Connection::call_raw`]. Picked from the reserved internal range
/// (-32000 to -32099 per JSON-RPC) and chosen far enough from
/// [`CdpRpcError`] codes Chrome actually emits that an unambiguous code
/// check suffices — no message-string match required.
pub(crate) const SHUTDOWN_DRAIN_CODE: i32 = -32001;

/// Cheap-to-clone handle to the connection actor. All `Tab`s and `Element`s
/// hold one of these (via `Arc<...>`); the actor itself runs in a separate
/// tokio task.
#[derive(Clone, Debug)]
pub struct Connection {
    pub(crate) inner: Arc<ConnectionInner>,
}

#[derive(Debug)]
pub(crate) struct ConnectionInner {
    pub(crate) cmd_tx: mpsc::Sender<OutboundCmd>,
    pub(crate) event_tx: broadcast::Sender<RawEvent>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) observer_timeout: Duration,
}

impl Connection {
    /// Send a CDP command and await its response.
    ///
    /// `method` is the dotted CDP method name (e.g. `"Page.navigate"`).
    /// `params` is the JSON value for the command's parameters.
    /// `session_id` routes the command to a particular target's session.
    ///
    /// Returns [`CallError::Rpc`] when Chrome answered with a JSON-RPC error
    /// (preserving `code`, `message`, and `data`), and [`CallError::Transport`]
    /// for connection-level failures.
    pub async fn call_raw(
        &self,
        method: impl Into<String>,
        params: Value,
        session_id: Option<String>,
    ) -> Result<Value, CallError> {
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
            Ok(Err(rpc_err)) => {
                // Preserve the transport-shutdown sentinel for shutdown-drained
                // pendings; everything else surfaces as a typed RPC error.
                // The sentinel code is reserved internally — Chrome never
                // emits it — so a code-only check is unambiguous.
                if rpc_err.code == SHUTDOWN_DRAIN_CODE {
                    Err(CallError::Transport(TransportError::Shutdown))
                } else {
                    Err(CallError::Rpc(rpc_err.code, rpc_err.message, rpc_err.data))
                }
            }
            Err(_) => Err(CallError::Transport(TransportError::Shutdown)),
        }
    }

    /// Subscribe to all events on this connection (no filtering).
    pub fn subscribe_raw(&self) -> impl Stream<Item = RawEvent> + Send + Unpin + use<> {
        Box::pin(
            BroadcastStream::new(self.inner.event_tx.subscribe()).filter_map(|res| async move {
                // Lagged frames are dropped.
                res.ok()
            }),
        )
    }

    /// Subscribe to events of a specific CDP method, deserialized into `T`.
    pub fn subscribe<T>(
        &self,
        method: &'static str,
    ) -> impl Stream<Item = T> + Send + Unpin + use<T>
    where
        T: DeserializeOwned + Send + 'static,
    {
        Box::pin(
            BroadcastStream::new(self.inner.event_tx.subscribe()).filter_map(
                move |res| async move {
                    let ev = res.ok()?;
                    if ev.method == method {
                        serde_json::from_value(ev.params).ok()
                    } else {
                        None
                    }
                },
            ),
        )
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

    /// Per-connection observer timeout. Exposed for the actor's handler
    /// (and for tests that override the default).
    pub(crate) fn observer_timeout(&self) -> Duration {
        self.inner.observer_timeout
    }
}

/// Ceiling on a single WebSocket message/frame, in bytes (256 MiB). tungstenite
/// defaults to a 64 MiB message / 16 MiB frame cap, and exceeding it silently
/// drops the socket — large CDP payloads (full-page screenshots, big response
/// bodies, DOM dumps) routinely blow past that. Both upstream Python drivers
/// raise the cap to 256 MiB (`2**28`); we match them.
const WS_MAX_BYTES: usize = 256 << 20;

/// WebSocket transport config applied at connect time. Factored out of
/// [`connect_with_observers`] so the cap can be unit-tested without a live
/// socket. Only the message/frame size limits are overridden; everything else
/// keeps tungstenite's defaults.
fn ws_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(WS_MAX_BYTES),
        max_frame_size: Some(WS_MAX_BYTES),
        ..Default::default()
    }
}

/// Connect to a Chrome DevTools WebSocket URL and spawn the actor with no
/// observers. Convenience wrapper for [`connect_with_observers`].
pub async fn connect(ws_url: &str) -> Result<Connection, TransportError> {
    connect_with_observers(ws_url, Vec::new()).await
}

/// Connect to a Chrome DevTools WebSocket URL and spawn the actor with the
/// provided `TargetObserver` chain. Observers fire on `Target.attachedToTarget`
/// (serially, in registration order) before the actor releases the debugger.
pub async fn connect_with_observers(
    ws_url: &str,
    observers: Vec<Arc<dyn TargetObserver>>,
) -> Result<Connection, TransportError> {
    use tokio_tungstenite::connect_async_with_config;
    let (ws, _resp) = connect_async_with_config(ws_url, Some(ws_config()), false).await?;
    Ok(spawn_actor_with_observers(ws, observers))
}

/// Spawn the actor on the given pre-connected WebSocket with no observers.
/// Mainly for tests and for `connect`; production code uses `connect`.
pub fn spawn_actor<S>(ws: S) -> Connection
where
    S: futures::Sink<
            tokio_tungstenite::tungstenite::Message,
            Error = tokio_tungstenite::tungstenite::Error,
        > + futures::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Send
        + Unpin
        + 'static,
{
    spawn_actor_with_observers(ws, Vec::new())
}

/// Spawn the actor on the given pre-connected WebSocket with `observers`.
pub fn spawn_actor_with_observers<S>(ws: S, observers: Vec<Arc<dyn TargetObserver>>) -> Connection
where
    S: futures::Sink<
            tokio_tungstenite::tungstenite::Message,
            Error = tokio_tungstenite::tungstenite::Error,
        > + futures::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Send
        + Unpin
        + 'static,
{
    spawn_actor_with_observers_and_timeout(ws, observers, DEFAULT_OBSERVER_TIMEOUT)
}

/// Spawn the actor with a custom `observer_timeout`. Exposed primarily for
/// tests that need to assert timeout behavior without waiting on the 5 s
/// default; production callers should prefer [`spawn_actor_with_observers`].
pub fn spawn_actor_with_observers_and_timeout<S>(
    ws: S,
    observers: Vec<Arc<dyn TargetObserver>>,
    observer_timeout: Duration,
) -> Connection
where
    S: futures::Sink<
            tokio_tungstenite::tungstenite::Message,
            Error = tokio_tungstenite::tungstenite::Error,
        > + futures::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Send
        + Unpin
        + 'static,
{
    let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(64);
    let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
    let shutdown = CancellationToken::new();
    let inner = Arc::new(ConnectionInner {
        cmd_tx,
        event_tx: event_tx.clone(),
        shutdown: shutdown.clone(),
        observer_timeout,
    });
    // Actor task uses a weak ref to ConnectionInner so it can reconstruct a
    // Connection for observer-handler tasks without forming a strong cycle
    // (the actor's lifetime would otherwise transitively own itself).
    let weak_inner = Arc::downgrade(&inner);
    tokio::spawn(run_actor(
        ws, cmd_rx, event_tx, shutdown, observers, weak_inner,
    ));
    Connection { inner }
}

/// Re-export the test `DriverStream` type at a shared visibility level so
/// both `actor::tests` and `connection::tests` can construct it. Also used
/// by the `testing` module when the `testing` feature is enabled, so
/// downstream crates can build a `MockConnection` against the same plumbing.
#[cfg(any(test, feature = "testing"))]
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

        fn start_send(self: std::pin::Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
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
        let (tx_in, rx_in) = tokio::sync::mpsc::channel::<
            Result<Message, tokio_tungstenite::tungstenite::Error>,
        >(32);
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

    #[test]
    fn ws_config_uses_256mib_cap() {
        assert_eq!(WS_MAX_BYTES, 256 << 20);
        let cfg = ws_config();
        assert_eq!(cfg.max_message_size, Some(WS_MAX_BYTES));
        assert_eq!(cfg.max_frame_size, Some(WS_MAX_BYTES));
    }
}
