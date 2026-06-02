//! `Connection` — the public handle to the transport actor.

use std::sync::{Arc, Mutex};
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

/// Internal JSON-RPC code stamped onto drained pendings when the WebSocket
/// dies *unexpectedly* — a Chrome-sent Close frame, a read error, or the
/// stream ending — as opposed to a caller-requested
/// [`Connection::shutdown`]. Mapped back to [`TransportError::Disconnected`]
/// by [`Connection::call_raw`], which the `zendriver` crate in turn surfaces
/// as `ZendriverError::Disconnected`. Distinct from [`SHUTDOWN_DRAIN_CODE`]
/// so a long-running caller can tell "Chrome died / socket dropped" apart
/// from "I closed it." Picked from the reserved internal JSON-RPC range
/// (-32000 to -32099); Chrome never emits it, so a code-only check is
/// unambiguous.
pub(crate) const DISCONNECTED_CODE: i32 = -32002;

/// Cheap-to-clone handle to the connection actor. All `Tab`s and `Element`s
/// hold one of these (via `Arc<...>`); the actor itself runs in a separate
/// tokio task.
#[derive(Clone, Debug)]
pub struct Connection {
    pub(crate) inner: Arc<ConnectionInner>,
}

pub(crate) struct ConnectionInner {
    /// Command channel into the *current* actor. Wrapped in a `Mutex` so
    /// [`Connection::reconnect`] can atomically swap it for the channel of a
    /// freshly-spawned actor without invalidating the `Arc<ConnectionInner>`
    /// every `Tab`/`SessionHandle` holds. Held only long enough to clone the
    /// sender; the `.send().await` happens after the guard is dropped.
    pub(crate) cmd_tx: Mutex<mpsc::Sender<OutboundCmd>>,
    /// Broadcast event bus. **Never** swapped on reconnect — existing
    /// subscribers stay attached across a reconnect because the same `Sender`
    /// keeps feeding them; the new actor is spawned with a clone of this very
    /// sender.
    pub(crate) event_tx: broadcast::Sender<RawEvent>,
    /// Cancellation token of the *current* actor. Swapped on reconnect: the
    /// old actor is cancelled (its already-dead `pending` drains) and the new
    /// actor gets a fresh token. Each spawned actor owns its own clone, so
    /// swapping here only ever cancels the latest actor.
    pub(crate) shutdown: Mutex<CancellationToken>,
    pub(crate) observer_timeout: Duration,
    /// Observer chain, retained so [`Connection::reconnect`] can re-spawn the
    /// actor with the same observers (so stealth re-injection etc. re-fire on
    /// the new targets). Stored as the `Vec` directly — an empty `Vec` means no
    /// observers — so reconnect needs no extra plumbing from the caller.
    pub(crate) observers: Vec<Arc<dyn TargetObserver>>,
}

// Hand-rolled `Debug` because `Vec<Arc<dyn TargetObserver>>` doesn't derive
// (trait objects are intentionally not `Debug`-bounded). Renders the observers
// field as `<N observers>` so the rest of the struct stays inspectable.
impl std::fmt::Debug for ConnectionInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionInner")
            .field("cmd_tx", &self.cmd_tx)
            .field("event_tx", &self.event_tx)
            .field("shutdown", &self.shutdown)
            .field("observer_timeout", &self.observer_timeout)
            .field(
                "observers",
                &format_args!("<{} observers>", self.observers.len()),
            )
            .finish()
    }
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
        // Clone the current sender out from under the lock, then release the
        // guard before the `.await` — `std::sync::Mutex` must not be held
        // across an await point, and a concurrent `reconnect()` swap is fine
        // because each `call_raw` captures a snapshot sender for its own send.
        let cmd_tx = {
            let guard = self
                .inner
                .cmd_tx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.clone()
        };
        cmd_tx
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
                // Preserve the transport drain sentinels for drained pendings;
                // everything else surfaces as a typed RPC error. Both sentinel
                // codes are reserved internally — Chrome never emits them — so
                // a code-only check is unambiguous.
                match rpc_err.code {
                    SHUTDOWN_DRAIN_CODE => Err(CallError::Transport(TransportError::Shutdown)),
                    DISCONNECTED_CODE => {
                        Err(CallError::Transport(TransportError::Disconnected))
                    }
                    _ => Err(CallError::Rpc(rpc_err.code, rpc_err.message, rpc_err.data)),
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
        self.inner
            .shutdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancel();
    }

    /// Public accessor for advanced users who need to drive the underlying
    /// shutdown token directly.
    ///
    /// Note: after a [`Connection::reconnect`] the returned token reflects the
    /// actor live *at the time of this call* — a token captured before a
    /// reconnect cancels the old (already-dead) actor, not the new one.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.inner
            .shutdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Re-establish the transport on a freshly-dialed WebSocket, **reusing this
    /// same `Connection` handle** (and therefore the same broadcast event bus,
    /// so raw event subscribers re-attach automatically).
    ///
    /// The old actor is cancelled and a new one is spawned on `ws` with the
    /// original observer chain (so `on_target_attached` re-fires — stealth
    /// re-injects on every new target) and a clone of the existing event bus.
    /// The command channel and shutdown token are swapped in place under their
    /// mutexes.
    ///
    /// This is the transport half of [`crate::Connection`]-level reconnection;
    /// the `zendriver` crate's `Browser::reconnect` wraps it with ws re-dial,
    /// `Target.setAutoAttach`, and tab-registry refresh.
    ///
    /// # Caveat
    ///
    /// CDP sessions do **not** survive across the underlying socket: any
    /// pre-existing `SessionHandle` / `Tab` keeps its old `sessionId`, which is
    /// now stale. Callers must re-acquire handles after a reconnect.
    pub fn reconnect<S>(&self, ws: S)
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
        // Cancel the previous actor — its `pending` (all stale by now) drains
        // and its loop exits.
        {
            let guard = self
                .inner
                .shutdown
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.cancel();
        }

        // Fresh command channel + shutdown token for the new actor; reuse the
        // SAME event bus so existing subscribers keep receiving.
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(64);
        let new_shutdown = CancellationToken::new();
        let weak_inner = Arc::downgrade(&self.inner);
        tokio::spawn(run_actor(
            ws,
            cmd_rx,
            self.inner.event_tx.clone(),
            new_shutdown.clone(),
            self.inner.observers.clone(),
            weak_inner,
        ));

        // Swap the routing channel + shutdown token in place. Order doesn't
        // matter: in-flight `call_raw`s already captured a snapshot sender, and
        // a `shutdown()` racing this swap either cancels the old token (no-op,
        // already cancelled) or the new one (intended).
        *self
            .inner
            .cmd_tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = cmd_tx;
        *self
            .inner
            .shutdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = new_shutdown;
    }

    /// Dial `ws_url` afresh (re-applying the P-A A4 [`WebSocketConfig`] max-size
    /// cap) and reconnect this handle onto the new socket via
    /// [`Connection::reconnect`].
    ///
    /// Convenience wrapper for the production reconnect path so callers don't
    /// need to reach into `tokio_tungstenite` or re-derive the size config.
    /// Tests that drive an in-memory duplex stream call [`Connection::reconnect`]
    /// directly instead.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Ws`] when the dial fails (Chrome gone, refused
    /// connection, bad URL). The existing actor is left **untouched** on dial
    /// failure — only a successful dial swaps the socket.
    ///
    /// [`WebSocketConfig`]: tokio_tungstenite::tungstenite::protocol::WebSocketConfig
    pub async fn redial(&self, ws_url: &str) -> Result<(), TransportError> {
        use tokio_tungstenite::connect_async_with_config;
        // Dial FIRST; only swap the live actor if the new socket is up, so a
        // failed reconnect doesn't tear down a still-usable connection.
        let (ws, _resp) = connect_async_with_config(ws_url, Some(ws_config()), false).await?;
        self.reconnect(ws);
        Ok(())
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
        cmd_tx: Mutex::new(cmd_tx),
        event_tx: event_tx.clone(),
        shutdown: Mutex::new(shutdown.clone()),
        observer_timeout,
        // Retain the observer chain so `reconnect` can re-spawn the actor with
        // the same observers without the caller re-supplying them.
        observers: observers.clone(),
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

    #[tokio::test]
    async fn call_raw_maps_unexpected_disconnect_to_disconnected_error() {
        use crate::error::{CallError, TransportError};
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);

        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw("Page.navigate", json!({ "url": "https://x.test" }), None)
                    .await
            }
        });

        // Wait for the command to land so the pending entry exists, then sever
        // the socket without a caller-requested shutdown.
        let _ = test_rx.recv().await.unwrap();
        drop(test_tx);

        let res = call.await.unwrap();
        assert!(
            matches!(res, Err(CallError::Transport(TransportError::Disconnected))),
            "unexpected disconnect must map to TransportError::Disconnected, got {res:?}"
        );

        conn.shutdown();
    }

    #[tokio::test]
    async fn call_raw_maps_clean_shutdown_to_shutdown_error() {
        use crate::error::{CallError, TransportError};
        let (ws, _test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);

        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw("Page.navigate", json!({ "url": "https://x.test" }), None)
                    .await
            }
        });

        let _ = test_rx.recv().await.unwrap();
        // Caller-requested shutdown — must stay `Shutdown`, never `Disconnected`.
        conn.shutdown();

        let res = call.await.unwrap();
        assert!(
            matches!(res, Err(CallError::Transport(TransportError::Shutdown))),
            "clean shutdown must map to TransportError::Shutdown, got {res:?}"
        );
    }

    #[tokio::test]
    async fn reconnect_routes_calls_through_new_socket() {
        let (ws_a, _tx_a, _rx_a) = duplex_pair();
        let conn = spawn_actor(ws_a);

        // Swap onto a fresh socket.
        let (ws_b, tx_b, mut rx_b) = duplex_pair();
        conn.reconnect(ws_b);

        // A new call must travel over socket B and resolve from B's reply.
        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw("Page.navigate", json!({ "url": "https://x.test" }), None)
                    .await
            }
        });
        let sent = rx_b.recv().await.expect("call routed to new socket");
        let id = serde_json::from_str::<Value>(match &sent {
            Message::Text(t) => t,
            _ => panic!("expected text frame"),
        })
        .unwrap()["id"]
            .as_u64()
            .unwrap();
        tx_b
            .send(Ok(Message::text(
                json!({ "id": id, "result": { "frameId": "F-new" } }).to_string(),
            )))
            .await
            .unwrap();
        let res = call.await.unwrap().unwrap();
        assert_eq!(res["frameId"], "F-new");

        conn.shutdown();
    }

    #[tokio::test]
    async fn reconnect_then_set_auto_attach_routes_over_new_socket() {
        // Mirrors what `Browser::reconnect` does after re-dialing: re-arm
        // `Target.setAutoAttach` over the swapped socket. Proves the re-arm
        // call lands on the NEW actor.
        let (ws_a, _tx_a, _rx_a) = duplex_pair();
        let conn = spawn_actor(ws_a);

        let (ws_b, tx_b, mut rx_b) = duplex_pair();
        conn.reconnect(ws_b);

        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw(
                    "Target.setAutoAttach",
                    json!({ "autoAttach": true, "flatten": true }),
                    None,
                )
                .await
            }
        });
        let sent = rx_b.recv().await.expect("setAutoAttach routed to new socket");
        let v: Value = serde_json::from_str(match &sent {
            Message::Text(t) => t,
            _ => panic!("expected text frame"),
        })
        .unwrap();
        assert_eq!(v["method"], "Target.setAutoAttach");
        assert_eq!(v["params"]["flatten"], true);
        let id = v["id"].as_u64().unwrap();
        tx_b
            .send(Ok(Message::text(
                json!({ "id": id, "result": {} }).to_string(),
            )))
            .await
            .unwrap();
        call.await.unwrap().unwrap();

        conn.shutdown();
    }

    #[tokio::test]
    async fn reconnect_preserves_event_subscribers() {
        let (ws_a, _tx_a, _rx_a) = duplex_pair();
        let conn = spawn_actor(ws_a);

        // Subscribe BEFORE the reconnect — the same broadcast bus must keep
        // feeding this subscriber after the socket swap.
        let mut sub = conn.subscribe_raw();

        let (ws_b, tx_b, _rx_b) = duplex_pair();
        conn.reconnect(ws_b);

        // Emit an event on the NEW socket; the pre-existing subscriber sees it.
        tx_b
            .send(Ok(Message::text(
                json!({ "method": "Page.loadEventFired", "params": {} }).to_string(),
            )))
            .await
            .unwrap();

        let ev = tokio::time::timeout(std::time::Duration::from_secs(1), sub.next())
            .await
            .expect("subscriber received an event after reconnect")
            .expect("stream not closed");
        assert_eq!(ev.method, "Page.loadEventFired");

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
