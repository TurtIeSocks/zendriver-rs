//! `Connection` — the public handle to the transport actor.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_util::sync::CancellationToken;

use crate::actor::{AccountedBus, EVENT_BUS_CAPACITY, OutboundCmd, run_actor};
use crate::error::{CallError, TransportError};
use crate::frame::{AccountedRawEvent, RawEvent};
use crate::observer::TargetObserver;

/// Default ceiling on a single observer's `on_target_attached` future, applied
/// before the actor releases the debugger via
/// `Runtime.runIfWaitingForDebugger`. Slow observers don't block the actor
/// indefinitely; a misbehaving one trips the timeout and the debugger releases.
pub(crate) const DEFAULT_OBSERVER_TIMEOUT: Duration = Duration::from_secs(5);

/// Default ceiling on how long [`Connection::call_raw`] waits for Chrome to
/// answer a single CDP command.
///
/// # Why this exists
///
/// Without it, `call_raw` awaited its reply oneshot bare, and the **only**
/// thing that could resolve a pending call early was the websocket dying. A
/// Chrome that accepted a command and never answered hung the caller forever
/// — no error, no retry, nothing in the log naming the stuck method. That is
/// not a hypothetical: it is what stalled `goto` on the windows-latest CI
/// runner for 83 minutes, and it is the same disease as the unbounded launch
/// handshake fixed in b5705aa9 — that fix just happened to bound only the
/// handshake.
///
/// # Why 180s
///
/// This is a **backstop against a wedged browser**, not a latency policy.
/// Sizing is bounded from both directions:
///
/// - **Floor — must not break legitimately slow work.** A single CDP
///   round-trip against a local socket is sub-millisecond warm. The slow
///   cases are `Page.navigate` to a tarpit origin and a heavy
///   `Runtime.evaluate` with `awaitPromise`; both are far under 3 minutes in
///   practice, and Chrome's own network stack surfaces `net::ERR_*` for a
///   dead origin well inside that — so a legitimately slow navigation still
///   gets Chrome's real error rather than ours. A tighter blanket default
///   would convert working code into flaky failures, which is strictly worse
///   than the hang: a false timeout is a silent behavior change in every
///   consumer, while the hang at least only bites a wedged browser.
/// - **Ceiling — must stay well under `.config/nextest.toml`'s
///   `terminate-after = 6` (6 x 60s).** A default at or above that is
///   unreachable in CI: nextest hard-kills the test first, and we learn
///   nothing. At 180s the call errors with ~3 minutes left for the test to
///   unwind and report — the whole point being that a stuck call produces a
///   *diagnosable* error rather than a stall.
/// - **Must not preempt tighter, more specific guards.** `HANDSHAKE_TIMEOUT`
///   (30s) in the `zendriver` crate wraps the launch handshake's CDP calls,
///   and `BROWSER_CLOSE_TIMEOUT` (3s) wraps the quit. Both are far tighter,
///   so both still fire first and report their own typed errors; this default
///   only catches calls nobody else bounded. Locked by
///   `default_call_timeout_does_not_preempt_the_launch_handshake_guard`.
///
/// Callers needing something else use
/// [`Connection::call_raw_with_timeout`] (per-call) or
/// [`Connection::set_call_timeout`] (per-connection).
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(180);

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
    /// Second, opt-in broadcast bus behind [`Connection::subscribe_raw_accounted`].
    /// Like `event_tx`, **never** swapped on reconnect — the same `Sender`
    /// keeps feeding existing accounted subscribers across a reconnect, with
    /// [`AccountedRawEvent::Reconnected`] marking the transition.
    pub(crate) accounted_tx: broadcast::Sender<AccountedRawEvent>,
    /// Current connection generation, read by [`Connection::connection_generation`].
    /// Starts at 1; bumped by [`Connection::reconnect`] before the new actor
    /// is spawned. `Relaxed` suffices: advisory value with no ordering
    /// relationship to anything else, same rationale as `call_timeout_ms`.
    pub(crate) generation: AtomicU64,
    /// Cancellation token of the *current* actor. Swapped on reconnect: the
    /// old actor is cancelled (its already-dead `pending` drains) and the new
    /// actor gets a fresh token. Each spawned actor owns its own clone, so
    /// swapping here only ever cancels the latest actor.
    pub(crate) shutdown: Mutex<CancellationToken>,
    pub(crate) observer_timeout: Duration,
    /// Default per-call reply budget, in milliseconds. See
    /// [`DEFAULT_CALL_TIMEOUT`].
    ///
    /// Atomic rather than plain so it can be retuned on a live `Connection`
    /// (which is shared behind an `Arc` by every `Tab`/`SessionHandle`)
    /// without another `spawn_actor_*` constructor overload or a re-spawn.
    /// `Relaxed` is right: this is an advisory backstop read once per call,
    /// with no ordering relationship to anything else.
    pub(crate) call_timeout_ms: AtomicU64,
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
            .field("accounted_tx", &self.accounted_tx)
            .field("generation", &self.generation)
            .field("shutdown", &self.shutdown)
            .field("observer_timeout", &self.observer_timeout)
            .field("call_timeout_ms", &self.call_timeout_ms)
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
    /// Bounded by this connection's [`call_timeout`](Connection::call_timeout)
    /// ([`DEFAULT_CALL_TIMEOUT`] unless changed): a Chrome that accepts the
    /// command and never answers yields [`CallError::Timeout`] rather than
    /// hanging the caller forever. Use [`Connection::call_raw_with_timeout`]
    /// to override the budget for one call.
    ///
    /// Returns [`CallError::Rpc`] when Chrome answered with a JSON-RPC error
    /// (preserving `code`, `message`, and `data`), [`CallError::Transport`]
    /// for connection-level failures, and [`CallError::Timeout`] when Chrome
    /// never answered at all.
    pub async fn call_raw(
        &self,
        method: impl Into<String>,
        params: Value,
        session_id: Option<String>,
    ) -> Result<Value, CallError> {
        self.call_raw_with_timeout(method, params, session_id, Some(self.call_timeout()))
            .await
    }

    /// This connection's default per-call reply budget.
    pub fn call_timeout(&self) -> Duration {
        Duration::from_millis(self.inner.call_timeout_ms.load(Ordering::Relaxed))
    }

    /// Retune the default per-call reply budget for **every** future call on
    /// this connection (and on every `Tab`/`SessionHandle` sharing it).
    ///
    /// Prefer [`Connection::call_raw_with_timeout`] when only one call is
    /// unusual; reach for this when a whole workload is (a deliberately
    /// throttled proxy, say). Raising it is safe; lowering it below a real
    /// operation's latency turns success into [`CallError::Timeout`].
    pub fn set_call_timeout(&self, budget: Duration) {
        // Saturate rather than wrap: a `Duration` larger than u64 ms is
        // ~584M years and unambiguously means "effectively never".
        let ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX);
        self.inner.call_timeout_ms.store(ms, Ordering::Relaxed);
    }

    /// [`Connection::call_raw`] with an explicit reply budget.
    ///
    /// `budget` of `Some(d)` bounds the wait at `d`; `None` opts out entirely
    /// and restores the old unbounded behavior for a call the caller knows is
    /// genuinely long-lived. `None` is a loaded gun — an unbounded call
    /// resolves only if the socket dies — so it exists for deliberate,
    /// documented use, not convenience.
    ///
    /// Note the budget covers **only** the wait for Chrome's reply. Queueing
    /// the command onto the actor is bounded independently by the actor's
    /// channel capacity and fails fast with [`TransportError::Shutdown`].
    pub async fn call_raw_with_timeout(
        &self,
        method: impl Into<String>,
        params: Value,
        session_id: Option<String>,
        budget: Option<Duration>,
    ) -> Result<Value, CallError> {
        let method = method.into();
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
                // Cloned so the error path can name the stuck method — one
                // small alloc against a websocket round-trip.
                method: method.clone(),
                params,
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| TransportError::Shutdown)?;

        // The bare `reply_rx.await` this replaces was THE hang: only a dying
        // websocket could resolve a pending call, so a wedged-but-connected
        // Chrome blocked the caller forever.
        let reply = match budget {
            Some(budget) => match tokio::time::timeout(budget, reply_rx).await {
                Ok(reply) => reply,
                Err(_elapsed) => return Err(CallError::Timeout { method, budget }),
            },
            None => reply_rx.await,
        };

        match reply {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(rpc_err)) => {
                // Preserve the transport drain sentinels for drained pendings;
                // everything else surfaces as a typed RPC error. Both sentinel
                // codes are reserved internally — Chrome never emits them — so
                // a code-only check is unambiguous.
                match rpc_err.code {
                    SHUTDOWN_DRAIN_CODE => Err(CallError::Transport(TransportError::Shutdown)),
                    DISCONNECTED_CODE => Err(CallError::Transport(TransportError::Disconnected)),
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

    /// Subscribe to all events on this connection, annotated with
    /// generation, sequence, and explicit loss/reconnect/disconnect signals.
    ///
    /// Unlike [`Connection::subscribe_raw`], which silently drops frames a
    /// lagging subscriber missed, this stream reports every gap explicitly
    /// via [`AccountedRawEvent::Lagged`], every [`Connection::reconnect`] via
    /// [`AccountedRawEvent::Reconnected`], and the underlying WebSocket dying
    /// via [`AccountedRawEvent::Disconnected`] — so a capture/replay/monitor
    /// consumer is never silently misled about what it saw.
    ///
    /// Opt-in and additive: `subscribe_raw` is completely unaffected by
    /// whether this method is ever called. The actor gates the per-event
    /// clone and the send onto this bus behind this stream having at least
    /// one live subscriber, so a connection with no accounted subscribers
    /// pays nothing beyond what `subscribe_raw` already costs.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use futures::StreamExt;
    /// use zendriver_transport::{AccountedRawEvent, Connection};
    ///
    /// # async fn example(conn: Connection) {
    /// let mut events = conn.subscribe_raw_accounted();
    /// while let Some(ev) = events.next().await {
    ///     match ev {
    ///         AccountedRawEvent::Event { generation, sequence, event } => {
    ///             println!("gen {generation} seq {sequence}: {}", event.method);
    ///         }
    ///         AccountedRawEvent::Lagged { generation, missed } => {
    ///             eprintln!("gen {generation}: missed {missed} events");
    ///         }
    ///         AccountedRawEvent::Reconnected { previous, generation } => {
    ///             eprintln!("reconnected: gen {previous} -> {generation}");
    ///         }
    ///         AccountedRawEvent::Disconnected { generation } => {
    ///             eprintln!("gen {generation} disconnected");
    ///             break;
    ///         }
    ///     }
    /// }
    /// # }
    /// ```
    pub fn subscribe_raw_accounted(
        &self,
    ) -> impl Stream<Item = AccountedRawEvent> + Send + Unpin + use<> {
        let inner = Arc::clone(&self.inner);
        Box::pin(
            BroadcastStream::new(self.inner.accounted_tx.subscribe()).filter_map(move |res| {
                let inner = Arc::clone(&inner);
                async move {
                    match res {
                        Ok(ev) => Some(ev),
                        Err(BroadcastStreamRecvError::Lagged(missed)) => {
                            Some(AccountedRawEvent::Lagged {
                                // Read fresh rather than a value captured at
                                // subscribe time, so a `Lagged` detected after
                                // a `reconnect()` reports the generation
                                // active *now*, not the one active when this
                                // subscriber first subscribed.
                                generation: inner.generation.load(Ordering::Relaxed),
                                missed,
                            })
                        }
                    }
                }
            }),
        )
    }

    /// The current connection generation.
    ///
    /// Starts at `1` for a freshly-spawned actor and increments by one on
    /// every [`Connection::reconnect`]. Paired with the `generation` field on
    /// every [`AccountedRawEvent`] so a
    /// [`Connection::subscribe_raw_accounted`] consumer can tell which live
    /// actor an event, lag, or disconnect came from.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn example(conn: zendriver_transport::Connection) {
    /// let generation = conn.connection_generation();
    /// assert!(generation >= 1);
    /// # }
    /// ```
    pub fn connection_generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Relaxed)
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
        // and its loop exits. This goes through the `shutdown.cancelled()`
        // branch of the actor loop (same as `Connection::shutdown`), so it
        // never sets `DISCONNECTED_CODE` and therefore never emits
        // `AccountedRawEvent::Disconnected` — a reconnect is announced solely
        // via `Reconnected` below, not a spurious disconnect.
        {
            let guard = self
                .inner
                .shutdown
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.cancel();
        }

        // Bump the generation and announce the transition on the accounted
        // bus BEFORE spawning the new actor, so a subscriber never observes
        // an event tagged with the new generation before `Reconnected`.
        // `fetch_add` returns the *previous* value, exactly what
        // `AccountedRawEvent::Reconnected::previous` needs.
        let previous_generation = self.inner.generation.fetch_add(1, Ordering::Relaxed);
        let new_generation = previous_generation + 1;
        if self.inner.accounted_tx.receiver_count() > 0 {
            let _ = self
                .inner
                .accounted_tx
                .send(AccountedRawEvent::Reconnected {
                    previous: previous_generation,
                    generation: new_generation,
                });
        }

        // Fresh command channel + shutdown token for the new actor; reuse the
        // SAME event buses so existing subscribers keep receiving.
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(64);
        let new_shutdown = CancellationToken::new();
        let weak_inner = Arc::downgrade(&self.inner);
        tokio::spawn(run_actor(
            ws,
            cmd_rx,
            self.inner.event_tx.clone(),
            AccountedBus {
                tx: self.inner.accounted_tx.clone(),
                generation: new_generation,
            },
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
    spawn_actor_with_observers_timeout_and_capacity(
        ws,
        observers,
        observer_timeout,
        EVENT_BUS_CAPACITY,
    )
}

/// Spawn the actor with a custom `observer_timeout` **and** a
/// caller-controlled accounted-bus capacity. `pub(crate)` — reached through
/// [`spawn_actor_with_observers_and_timeout`] (which fixes the capacity at
/// [`EVENT_BUS_CAPACITY`]) for production/general test code, and through
/// `testing::MockConnection::pair_with_accounted_capacity` for downstream
/// crates that need to force a deterministic
/// [`AccountedRawEvent::Lagged`] by overflowing a small accounted bus
/// without pushing thousands of frames through the real 1024-deep one.
pub(crate) fn spawn_actor_with_observers_timeout_and_capacity<S>(
    ws: S,
    observers: Vec<Arc<dyn TargetObserver>>,
    observer_timeout: Duration,
    accounted_capacity: usize,
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
    let (accounted_tx, _accounted_rx) = broadcast::channel::<AccountedRawEvent>(accounted_capacity);
    let shutdown = CancellationToken::new();
    let inner = Arc::new(ConnectionInner {
        cmd_tx: Mutex::new(cmd_tx),
        event_tx: event_tx.clone(),
        accounted_tx: accounted_tx.clone(),
        generation: AtomicU64::new(1),
        shutdown: Mutex::new(shutdown.clone()),
        observer_timeout,
        call_timeout_ms: AtomicU64::new(DEFAULT_CALL_TIMEOUT.as_millis() as u64),
        // Retain the observer chain so `reconnect` can re-spawn the actor with
        // the same observers without the caller re-supplying them.
        observers: observers.clone(),
    });
    // Actor task uses a weak ref to ConnectionInner so it can reconstruct a
    // Connection for observer-handler tasks without forming a strong cycle
    // (the actor's lifetime would otherwise transitively own itself).
    let weak_inner = Arc::downgrade(&inner);
    tokio::spawn(run_actor(
        ws,
        cmd_rx,
        event_tx,
        AccountedBus {
            tx: accounted_tx,
            generation: 1,
        },
        shutdown,
        observers,
        weak_inner,
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
        tx_b.send(Ok(Message::text(
            json!({ "id": id, "result": { "frameId": "F-new" } }).to_string(),
        )))
        .await
        .unwrap();
        let res = call.await.unwrap().unwrap();
        assert_eq!(res["frameId"], "F-new");

        conn.shutdown();
    }

    /// The core regression lock. Before this, `call_raw` awaited the reply
    /// oneshot bare: a Chrome that accepted a command and never answered hung
    /// the caller **forever**, resolving only if the socket happened to die.
    /// That is what stalled `goto` on the windows-latest runner.
    #[tokio::test]
    async fn call_raw_times_out_when_the_reply_never_arrives() {
        let (ws, _test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        conn.set_call_timeout(Duration::from_millis(100));

        let started = tokio::time::Instant::now();
        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw("Page.navigate", json!({ "url": "https://x.test" }), None)
                    .await
            }
        });

        // The command lands; we deliberately never reply and never drop the
        // socket, so nothing but the timeout can resolve this call.
        let _ = test_rx.recv().await.unwrap();

        let res = call.await.unwrap();
        match res {
            Err(CallError::Timeout { method, budget }) => {
                assert_eq!(method, "Page.navigate", "must name the stuck method");
                assert_eq!(budget, Duration::from_millis(100));
            }
            other => panic!("expected CallError::Timeout, got {other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout must fire on budget, not hang; took {:?}",
            started.elapsed()
        );

        conn.shutdown();
    }

    /// A caller that knows its call is legitimately slow (or legitimately
    /// must not be bounded) can say so per-call without retuning the whole
    /// connection.
    #[tokio::test]
    async fn call_raw_with_timeout_honours_the_per_call_override() {
        let (ws, _test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        // Connection default stays generous; the per-call override is what
        // must win, proving the override is actually consulted.
        assert_eq!(conn.call_timeout(), DEFAULT_CALL_TIMEOUT);

        let started = tokio::time::Instant::now();
        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw_with_timeout(
                    "Runtime.evaluate",
                    json!({ "expression": "1" }),
                    None,
                    Some(Duration::from_millis(50)),
                )
                .await
            }
        });
        let _ = test_rx.recv().await.unwrap();

        let res = call.await.unwrap();
        assert!(
            matches!(res, Err(CallError::Timeout { ref method, budget })
                if method == "Runtime.evaluate" && budget == Duration::from_millis(50)),
            "per-call override must win over the connection default, got {res:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "override must fire on its own budget, not the 180s default"
        );

        conn.shutdown();
    }

    /// `None` is the explicit opt-out for a genuinely unbounded call. Proven
    /// by the fact that the call is still pending long after a 50ms-style
    /// budget would have fired, and then still completes normally.
    #[tokio::test]
    async fn call_raw_with_timeout_none_is_unbounded() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        conn.set_call_timeout(Duration::from_millis(50));

        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw_with_timeout("Page.navigate", json!({}), None, None)
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

        // Far past the connection default; an opted-out call must still be
        // alive rather than already timed out.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!call.is_finished(), "None budget must not time out");

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

    /// A reply that arrives inside the budget must be completely unaffected —
    /// the timeout is a backstop, not a latency policy.
    #[tokio::test]
    async fn call_raw_reply_within_budget_is_unaffected() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        conn.set_call_timeout(Duration::from_secs(30));

        let call = tokio::spawn({
            let c = conn.clone();
            async move { c.call_raw("Page.navigate", json!({}), None).await }
        });
        let sent = test_rx.recv().await.unwrap();
        let id = serde_json::from_str::<Value>(match &sent {
            Message::Text(t) => t,
            _ => panic!("expected text frame"),
        })
        .unwrap()["id"]
            .as_u64()
            .unwrap();

        // Slow, but well inside budget.
        tokio::time::sleep(Duration::from_millis(150)).await;
        test_tx
            .send(Ok(Message::text(
                json!({ "id": id, "result": { "frameId": "OK" } }).to_string(),
            )))
            .await
            .unwrap();

        let res = call.await.unwrap().expect("in-budget reply must succeed");
        assert_eq!(res["frameId"], "OK");

        conn.shutdown();
    }

    /// A timeout must be distinguishable from "Chrome said no" (`Rpc`) and
    /// "the connection broke" (`Transport`). Callers key retry/diagnosis off
    /// exactly this distinction.
    #[tokio::test]
    async fn call_raw_timeout_is_distinct_from_rpc_and_transport() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        conn.set_call_timeout(Duration::from_millis(100));

        // A real RPC refusal must NOT be reported as a timeout.
        let call = tokio::spawn({
            let c = conn.clone();
            async move { c.call_raw("Bogus.method", json!({}), None).await }
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
                json!({ "id": id, "error": { "code": -32601, "message": "not found" } })
                    .to_string(),
            )))
            .await
            .unwrap();
        let res = call.await.unwrap();
        assert!(
            matches!(res, Err(CallError::Rpc(-32601, _, _))),
            "an answered-but-refused call must stay Rpc, got {res:?}"
        );

        conn.shutdown();
    }

    /// The default must stay comfortably under `.config/nextest.toml`'s
    /// `terminate-after = 6` (6 x 60s). A default above that ceiling would be
    /// unreachable in CI: nextest would kill the test before the call could
    /// report a diagnosable error, which is the entire point of having one.
    #[test]
    fn default_call_timeout_is_under_the_nextest_terminate_after_ceiling() {
        const NEXTEST_TERMINATE_AFTER: Duration = Duration::from_secs(6 * 60);
        assert!(
            DEFAULT_CALL_TIMEOUT < NEXTEST_TERMINATE_AFTER,
            "DEFAULT_CALL_TIMEOUT ({DEFAULT_CALL_TIMEOUT:?}) must be under nextest's \
             {NEXTEST_TERMINATE_AFTER:?} hard kill or it can never surface as an error"
        );
        // And it must leave real room to unwind and report, not squeak under.
        assert!(
            DEFAULT_CALL_TIMEOUT <= NEXTEST_TERMINATE_AFTER / 2,
            "DEFAULT_CALL_TIMEOUT must leave at least half the nextest budget \
             for the test to report the failure"
        );
    }

    /// `HANDSHAKE_TIMEOUT` (30s, in the `zendriver` crate) wraps the launch
    /// handshake's CDP calls. The transport default must sit strictly *under*
    /// it in generosity terms — i.e. be larger — so the tighter, more specific
    /// launch guard is always the one that reports. If this inverted, a slow
    /// handshake would surface as a generic per-call timeout instead of
    /// `BrowserError::HandshakeTimeout`, and the launch child-kill branch
    /// keyed to it would not run.
    #[test]
    fn default_call_timeout_does_not_preempt_the_launch_handshake_guard() {
        const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
        assert!(
            DEFAULT_CALL_TIMEOUT > HANDSHAKE_TIMEOUT,
            "the launch handshake's own 30s guard must fire first; a transport \
             default below it would preempt HandshakeTimeout"
        );
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
        let sent = rx_b
            .recv()
            .await
            .expect("setAutoAttach routed to new socket");
        let v: Value = serde_json::from_str(match &sent {
            Message::Text(t) => t,
            _ => panic!("expected text frame"),
        })
        .unwrap();
        assert_eq!(v["method"], "Target.setAutoAttach");
        assert_eq!(v["params"]["flatten"], true);
        let id = v["id"].as_u64().unwrap();
        tx_b.send(Ok(Message::text(
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
        tx_b.send(Ok(Message::text(
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

    // ---------- `subscribe_raw_accounted` / `AccountedRawEvent` (Task 1) ----------

    /// Like [`spawn_actor`] but with a caller-controlled accounted-bus
    /// capacity, so lag tests can force a broadcast overflow deterministically
    /// without pushing thousands of frames through the real
    /// [`EVENT_BUS_CAPACITY`] (1024). Thin wrapper over
    /// [`spawn_actor_with_observers_timeout_and_capacity`], the same
    /// entry point `testing::MockConnection::pair_with_accounted_capacity`
    /// uses for downstream crates.
    fn spawn_actor_with_accounted_capacity<S>(ws: S, accounted_capacity: usize) -> Connection
    where
        S: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
            + futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Send
            + Unpin
            + 'static,
    {
        spawn_actor_with_observers_timeout_and_capacity(
            ws,
            Vec::new(),
            DEFAULT_OBSERVER_TIMEOUT,
            accounted_capacity,
        )
    }

    #[tokio::test]
    async fn subscribe_raw_accounted_assigns_monotonic_sequence_starting_at_one() {
        let (ws, test_tx, _test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        let mut sub = conn.subscribe_raw_accounted();

        for i in 0..3u64 {
            test_tx
                .send(Ok(Message::text(
                    json!({ "method": "Test.evt", "params": { "i": i } }).to_string(),
                )))
                .await
                .unwrap();
        }

        for expected_seq in 1..=3u64 {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(1), sub.next())
                .await
                .expect("accounted event arrived")
                .expect("stream not closed");
            match ev {
                AccountedRawEvent::Event {
                    generation,
                    sequence,
                    event,
                } => {
                    assert_eq!(generation, 1);
                    assert_eq!(sequence, expected_seq);
                    assert_eq!(event.method, "Test.evt");
                }
                other => panic!("expected Event, got {other:?}"),
            }
        }

        conn.shutdown();
    }

    #[tokio::test]
    async fn subscribe_raw_accounted_reports_lagged_with_correct_missed_count_and_resumes_monotonically()
     {
        // Capacity 2: pushing 5 events with the subscriber never polling
        // forces exactly 3 missed (5 sent - 2 retained).
        let (ws, test_tx, _test_rx) = duplex_pair();
        let conn = spawn_actor_with_accounted_capacity(ws, 2);
        let mut sub = conn.subscribe_raw_accounted();

        for i in 0..5u64 {
            test_tx
                .send(Ok(Message::text(
                    json!({ "method": "Test.evt", "params": { "i": i } }).to_string(),
                )))
                .await
                .unwrap();
        }
        // Give the actor time to drain all 5 frames onto the accounted bus
        // before this subscriber ever polls it.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let first = tokio::time::timeout(std::time::Duration::from_secs(1), sub.next())
            .await
            .expect("lagged notification arrived")
            .expect("stream not closed");
        match first {
            AccountedRawEvent::Lagged { generation, missed } => {
                assert_eq!(generation, 1);
                assert_eq!(missed, 3, "5 sent - capacity 2 retained = 3 missed");
            }
            other => panic!("expected Lagged, got {other:?}"),
        }

        // Sequence must resume monotonically from where the loss left off
        // (4, 5) rather than resetting — the actor's counter keeps
        // incrementing for every event mirrored, independent of any one
        // subscriber's lag.
        for expected_seq in [4u64, 5u64] {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(1), sub.next())
                .await
                .expect("event arrived")
                .expect("stream not closed");
            match ev {
                AccountedRawEvent::Event {
                    generation,
                    sequence,
                    ..
                } => {
                    assert_eq!(generation, 1);
                    assert_eq!(sequence, expected_seq);
                }
                other => panic!("expected Event, got {other:?}"),
            }
        }

        conn.shutdown();
    }

    #[tokio::test]
    async fn reconnect_emits_reconnected_with_bumped_generation_and_resets_sequence() {
        let (ws_a, _tx_a, _rx_a) = duplex_pair();
        let conn = spawn_actor(ws_a);
        assert_eq!(conn.connection_generation(), 1);

        // Subscribe BEFORE the reconnect, same as `reconnect_preserves_event_subscribers`.
        let mut sub = conn.subscribe_raw_accounted();

        let (ws_b, tx_b, _rx_b) = duplex_pair();
        conn.reconnect(ws_b);

        let first = tokio::time::timeout(std::time::Duration::from_secs(1), sub.next())
            .await
            .expect("reconnected notification arrived")
            .expect("stream not closed");
        match first {
            AccountedRawEvent::Reconnected {
                previous,
                generation,
            } => {
                assert_eq!(previous, 1);
                assert_eq!(generation, 2);
            }
            other => panic!("expected Reconnected, got {other:?}"),
        }
        assert_eq!(conn.connection_generation(), 2);

        // An event on the NEW socket must carry the bumped generation and a
        // sequence freshly reset to 1 — a new actor run, fresh counter.
        tx_b.send(Ok(Message::text(
            json!({ "method": "Page.loadEventFired", "params": {} }).to_string(),
        )))
        .await
        .unwrap();

        let second = tokio::time::timeout(std::time::Duration::from_secs(1), sub.next())
            .await
            .expect("event arrived")
            .expect("stream not closed");
        match second {
            AccountedRawEvent::Event {
                generation,
                sequence,
                event,
            } => {
                assert_eq!(generation, 2);
                assert_eq!(sequence, 1);
                assert_eq!(event.method, "Page.loadEventFired");
            }
            other => panic!("expected Event, got {other:?}"),
        }

        conn.shutdown();
    }

    #[tokio::test]
    async fn ws_death_emits_exactly_one_disconnected() {
        let (ws, test_tx, _test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        let mut sub = conn.subscribe_raw_accounted();

        // Drop the server side so the stream returns `None` (socket vanished) —
        // the same trigger `ws_stream_end_drains_pending_with_disconnected_code`
        // uses in actor.rs.
        drop(test_tx);

        let first = tokio::time::timeout(std::time::Duration::from_secs(1), sub.next())
            .await
            .expect("disconnected notification arrived")
            .expect("stream not closed");
        match first {
            AccountedRawEvent::Disconnected { generation } => assert_eq!(generation, 1),
            other => panic!("expected Disconnected, got {other:?}"),
        }

        // Exactly one: nothing else should follow within a short window.
        let extra = tokio::time::timeout(std::time::Duration::from_millis(200), sub.next()).await;
        assert!(
            extra.is_err(),
            "expected no further accounted events after the single Disconnected, got {extra:?}"
        );

        conn.shutdown();
    }

    #[tokio::test]
    async fn raw_subscriber_unaffected_when_no_accounted_subscriber_exists() {
        // No `subscribe_raw_accounted()` call anywhere in this test — the
        // accounted bus has zero subscribers for its whole lifetime, which
        // must gate off the per-event clone + accounted-bus send without
        // disturbing `subscribe_raw` in any way.
        let (ws, test_tx, _test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        let mut raw_sub = conn.subscribe_raw();

        test_tx
            .send(Ok(Message::text(
                json!({ "method": "Page.loadEventFired", "params": {} }).to_string(),
            )))
            .await
            .unwrap();

        let ev = tokio::time::timeout(std::time::Duration::from_secs(1), raw_sub.next())
            .await
            .expect("raw subscriber received the event")
            .expect("stream not closed");
        assert_eq!(ev.method, "Page.loadEventFired");

        conn.shutdown();
    }
}
