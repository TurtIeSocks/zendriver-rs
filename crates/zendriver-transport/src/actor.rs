//! ConnectionActor: tokio task owning the WebSocket. Routes commands and
//! responses by id, fans events out on a broadcast bus, and dispatches
//! `TargetObserver`s on `Target.attachedToTarget`.

use std::any::Any;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Weak};
use std::time::Duration;

use futures::{FutureExt, SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, trace, warn};

use crate::connection::{Connection, ConnectionInner};
use crate::frame::{AccountedRawEvent, CdpCommand, CdpInbound, CdpRpcError, RawEvent};
use crate::observer::{ObserverFailurePolicy, PausedSession, TargetInfo, TargetObserver};

/// Outbound command sent from a `Connection` handle to the actor.
#[derive(Debug)]
pub(crate) struct OutboundCmd {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
    pub reply: oneshot::Sender<Result<Value, CdpRpcError>>,
}

/// Default broadcast bus capacity. Lagged subscribers drop frames.
pub(crate) const EVENT_BUS_CAPACITY: usize = 1024;

/// Accounted-bus plumbing for one actor run: the second, opt-in broadcast
/// sender plus this run's generation number. Bundled into a struct (rather
/// than two more [`run_actor`] parameters) to keep the function signature
/// under clippy's `too_many_arguments` threshold and to make the bus/
/// generation pairing explicit at each call site — mirrors the
/// [`crate::connection`]-side rationale for bundling wide handshake inputs
/// into a struct.
pub(crate) struct AccountedBus {
    /// Second broadcast bus behind
    /// [`Connection::subscribe_raw_accounted`](crate::connection::Connection::subscribe_raw_accounted).
    pub(crate) tx: broadcast::Sender<AccountedRawEvent>,
    /// This actor run's generation number, stamped onto every
    /// [`AccountedRawEvent`] it produces.
    pub(crate) generation: u64,
}

/// `Target.attachedToTarget` event payload (we only deserialize what we need).
#[derive(serde::Deserialize)]
struct TargetAttached {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "targetInfo")]
    target_info: TargetInfo,
}

/// `Target.detachedFromTarget` event payload.
#[derive(serde::Deserialize)]
struct TargetDetached {
    #[serde(rename = "sessionId")]
    session_id: String,
}

/// Runs the actor loop until `shutdown` is cancelled or the WS dies.
///
/// Generic over the WS sink + stream so tests can drive in-memory streams
/// instead of real WebSockets.
///
/// `observers` are invoked serially (in registration order) on each new
/// `Target.attachedToTarget`. The handler runs in a spawned task so the actor
/// loop stays responsive; `weak_inner` lets the handler reconstruct a
/// `Connection` (without forming a strong cycle) to talk back through this
/// same actor.
///
/// `accounted` bundles the second, opt-in broadcast bus behind
/// [`Connection::subscribe_raw_accounted`](crate::connection::Connection::subscribe_raw_accounted)
/// with this actor run's generation number, stamped onto every
/// [`AccountedRawEvent`] it produces. Mirroring onto `accounted.tx` — the
/// per-event clone of [`RawEvent`] and the send itself — is gated behind
/// `accounted.tx.receiver_count() > 0` so a connection with no accounted
/// subscribers pays nothing beyond the existing `event_tx` broadcast.
pub(crate) async fn run_actor<S>(
    mut ws: S,
    mut cmd_rx: mpsc::Receiver<OutboundCmd>,
    event_tx: broadcast::Sender<RawEvent>,
    accounted: AccountedBus,
    shutdown: CancellationToken,
    observers: Vec<Arc<dyn TargetObserver>>,
    weak_inner: Weak<ConnectionInner>,
) where
    S: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let mut pending: HashMap<u64, oneshot::Sender<Result<Value, CdpRpcError>>> = HashMap::new();
    let mut next_id: u64 = 1;
    // Per-generation monotonic counter for `AccountedRawEvent::Event::sequence`.
    // Reset to 1 on every actor run (a fresh call to `run_actor`, i.e. every
    // reconnect); only advances when `accounted_tx` actually has a
    // subscriber, since it counts positions *on the accounted bus*.
    let mut next_accounted_sequence: u64 = 1;

    // Sentinel stamped onto drained pendings when the loop exits. Defaults to
    // the clean-shutdown code; the unexpected-ws-death branches below flip it
    // to `DISCONNECTED_CODE` so callers awaiting a CDP call during a dropped
    // connection can distinguish "Chrome died / socket dropped" from a
    // caller-requested `shutdown()`.
    let mut drain_code = crate::connection::SHUTDOWN_DRAIN_CODE;

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                debug!("actor shutdown received; draining {} pending", pending.len());
                break;
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else {
                    debug!("cmd channel closed; shutting down");
                    break;
                };
                let id = next_id;
                next_id = next_id.wrapping_add(1);
                let frame = CdpCommand {
                    id,
                    method: &cmd.method,
                    params: cmd.params,
                    session_id: cmd.session_id.as_deref(),
                };
                match serde_json::to_string(&frame) {
                    Ok(s) => {
                        trace!(id, method = %cmd.method, "send");
                        if let Err(e) = ws.send(Message::text(s)).await {
                            error!("ws send failed: {e}");
                            let _ = cmd.reply.send(Err(CdpRpcError {
                                code: -32000,
                                message: format!("ws send failed: {e}"),
                                data: None,
                            }));
                            // A failed write means the socket died — treat the
                            // remaining pendings as an unexpected disconnect,
                            // consistent with the read-side death branches.
                            drain_code = crate::connection::DISCONNECTED_CODE;
                            break;
                        }
                        pending.insert(id, cmd.reply);
                    }
                    Err(e) => {
                        let _ = cmd.reply.send(Err(CdpRpcError {
                            code: -32700,
                            message: format!("serialize: {e}"),
                            data: None,
                        }));
                    }
                }
            }
            frame = ws.next() => {
                match frame {
                    None => {
                        debug!("ws stream ended");
                        // Stream vanished without a caller-requested shutdown:
                        // an unexpected disconnect, not a clean close.
                        drain_code = crate::connection::DISCONNECTED_CODE;
                        break;
                    }
                    Some(Err(e)) => {
                        error!("ws read failed: {e}");
                        drain_code = crate::connection::DISCONNECTED_CODE;
                        break;
                    }
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<CdpInbound>(&text) {
                            Ok(CdpInbound::Response { id, result, error, .. }) => {
                                match pending.remove(&id) { Some(reply) => {
                                    let res = match error {
                                        Some(e) => Err(e),
                                        None    => Ok(result.unwrap_or(Value::Null)),
                                    };
                                    let _ = reply.send(res);
                                } _ => {
                                    warn!(id, "response for unknown id (caller dropped?)");
                                }}
                            }
                            Ok(CdpInbound::Event { method, params, session_id }) => {
                                // Branch on target-lifecycle events before broadcasting.
                                if method == "Target.attachedToTarget" && !observers.is_empty() {
                                    match serde_json::from_value::<TargetAttached>(params.clone()) {
                                        Ok(ev) => {
                                            match weak_inner.upgrade() { Some(strong) => {
                                                let conn = Connection { inner: strong };
                                                let timeout_dur = conn.observer_timeout();
                                                let observers_clone = observers.clone();
                                                tokio::spawn(async move {
                                                    handle_target_attached(
                                                        conn,
                                                        ev,
                                                        observers_clone,
                                                        timeout_dur,
                                                    )
                                                    .await;
                                                });
                                            } _ => {
                                                warn!(
                                                    "Target.attachedToTarget arrived but \
                                                     Connection has dropped; skipping observers"
                                                );
                                            }}
                                        }
                                        Err(e) => error!(
                                            "bad Target.attachedToTarget payload: {e}"
                                        ),
                                    }
                                } else if method == "Target.detachedFromTarget"
                                    && !observers.is_empty()
                                {
                                    if let Ok(ev) =
                                        serde_json::from_value::<TargetDetached>(params.clone())
                                    {
                                        // Mirror the `attachedToTarget` path:
                                        // observers can panic in user code, so
                                        // wrap each call in AssertUnwindSafe +
                                        // catch_unwind and a soft timeout. We
                                        // can't tear down the session on
                                        // failure (it's already detaching),
                                        // but logging gives users a fighting
                                        // chance to find a regression instead
                                        // of dropping the panic silently.
                                        let timeout_dur = match weak_inner.upgrade()
                                        { Some(strong) => {
                                            Connection { inner: strong }.observer_timeout()
                                        } _ => {
                                            // Connection has dropped — pick a
                                            // short default so the task can't
                                            // hang forever.
                                            Duration::from_secs(5)
                                        }};
                                        for obs in &observers {
                                            let obs2 = obs.clone();
                                            let sid = ev.session_id.clone();
                                            let name = obs2.name();
                                            tokio::spawn(async move {
                                                let fut = obs2.on_target_detached(&sid);
                                                match tokio::time::timeout(
                                                    timeout_dur,
                                                    AssertUnwindSafe(fut).catch_unwind(),
                                                )
                                                .await
                                                {
                                                    Ok(Ok(())) => {}
                                                    Ok(Err(panic)) => {
                                                        let msg = panic_payload(&panic);
                                                        error!(
                                                            observer = name,
                                                            session_id = %sid,
                                                            panic = %msg,
                                                            "detached-target observer panicked",
                                                        );
                                                    }
                                                    Err(_) => warn!(
                                                        observer = name,
                                                        session_id = %sid,
                                                        "detached-target observer timed out",
                                                    ),
                                                }
                                            });
                                        }
                                    } else {
                                        warn!("bad Target.detachedFromTarget payload");
                                    }
                                }
                                let ev = RawEvent { method, params, session_id };
                                // Gate the per-event clone + accounted-bus send
                                // behind an actual subscriber: cloning `ev`
                                // (method String + params Value — potentially
                                // large Network.* JSON) is wasted work when
                                // nobody is listening on the accounted bus.
                                if accounted.tx.receiver_count() > 0 {
                                    let sequence = next_accounted_sequence;
                                    next_accounted_sequence =
                                        next_accounted_sequence.wrapping_add(1);
                                    let _ = accounted.tx.send(AccountedRawEvent::Event {
                                        generation: accounted.generation,
                                        sequence,
                                        event: ev.clone(),
                                    });
                                }
                                // Ignore SendError: zero subscribers is fine.
                                let _ = event_tx.send(ev);
                            }
                            Err(e) => warn!("frame parse failed: {e} (text: {text})"),
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        debug!("ws close frame; disconnecting");
                        // Chrome closed the socket on us — treat as an
                        // unexpected disconnect so in-flight callers see
                        // `Disconnected` rather than the clean-shutdown code.
                        drain_code = crate::connection::DISCONNECTED_CODE;
                        break;
                    }
                    Some(Ok(_)) => { /* ignore binary, ping, pong, frame */ }
                }
            }
        }
    }

    // Drain pending into transport errors so callers don't hang. `drain_code`
    // is a reserved sentinel matched by `Connection::call_raw`: it surfaces
    // drained pendings as `TransportError::Shutdown` (clean shutdown) or
    // `TransportError::Disconnected` (unexpected ws death) rather than a
    // generic `Rpc` error.
    let message = if drain_code == crate::connection::DISCONNECTED_CODE {
        "connection lost"
    } else {
        "connection shut down"
    };
    // A genuine ws death (not a caller-requested `shutdown()` and not a
    // `reconnect()` — both cancel via `shutdown.cancelled()`, which never
    // sets `DISCONNECTED_CODE`) gets exactly one `Disconnected` on the
    // accounted bus, gated the same as the per-event mirroring above.
    if drain_code == crate::connection::DISCONNECTED_CODE && accounted.tx.receiver_count() > 0 {
        let _ = accounted.tx.send(AccountedRawEvent::Disconnected {
            generation: accounted.generation,
        });
    }
    for (_id, reply) in pending.drain() {
        let _ = reply.send(Err(CdpRpcError {
            code: drain_code,
            message: message.into(),
            data: None,
        }));
    }
    debug!(drain_code, "actor exit");
}

/// Run all `observers` serially against the just-attached session. Returns
/// after either: (a) every observer succeeds — in which case we release the
/// debugger via `Runtime.runIfWaitingForDebugger`; (b) one observer errors or
/// panics — we detach via `Target.detachFromTarget` and return without
/// releasing; or (c) an observer exceeds `observer_timeout` — behavior then
/// depends on [`TargetObserver::failure_policy`]:
/// [`ObserverFailurePolicy::Required`] (the default) detaches the same as
/// (b), fail closed, so a hung observer can never leave the page running
/// without its setup; [`ObserverFailurePolicy::BestEffort`] logs and falls
/// through to release the debugger anyway, matching the pre-existing
/// fail-open behavior for observers that opt into it.
///
/// ## Cancellation
///
/// `AssertUnwindSafe + catch_unwind` covers panics inside the observer
/// future, not cancellation of the outer task. In practice this function
/// runs inside a `tokio::spawn`'d task that has no `JoinHandle` retained
/// by the actor loop — the spawned task therefore cannot be cancelled by
/// the actor dropping. The task owns the [`Connection`] via the moved
/// `conn` field below, so the underlying transport stays alive at least
/// until the task completes naturally. The only way for in-flight CDP
/// calls to orphan is a runtime-wide shutdown, in which case the entire
/// process is tearing down and the calls are moot.
async fn handle_target_attached(
    conn: Connection,
    ev: TargetAttached,
    observers: Vec<Arc<dyn TargetObserver>>,
    observer_timeout: Duration,
) {
    let session_id = ev.session_id.clone();
    for obs in &observers {
        let paused = PausedSession {
            session_id: &session_id,
            target_info: &ev.target_info,
            conn: &conn,
        };
        let name = obs.name();
        let fut = obs.on_target_attached(paused);
        match tokio::time::timeout(observer_timeout, AssertUnwindSafe(fut).catch_unwind()).await {
            Ok(Ok(Ok(()))) => continue,
            Ok(Ok(Err(e))) => {
                error!(observer = name, %session_id, error = %e, "observer failed; detaching");
                detach_target(&conn, &session_id).await;
                return;
            }
            Ok(Err(panic)) => {
                let msg = panic_payload(&panic);
                error!(observer = name, %session_id, panic = %msg, "observer panicked; detaching");
                detach_target(&conn, &session_id).await;
                return;
            }
            Err(_) => match obs.failure_policy() {
                ObserverFailurePolicy::Required => {
                    warn!(
                        observer = name,
                        %session_id,
                        "observer timed out; detaching (fail-closed)"
                    );
                    detach_target(&conn, &session_id).await;
                    return;
                }
                ObserverFailurePolicy::BestEffort => {
                    warn!(
                        observer = name,
                        %session_id,
                        "observer timed out; releasing (best-effort)"
                    );
                    break;
                }
            },
        }
    }
    let _ = conn
        .call_raw(
            "Runtime.runIfWaitingForDebugger",
            json!({}),
            Some(session_id),
        )
        .await;
}

/// Shared detach path for every observer failure mode (`Err`, panic, or a
/// `Required`-policy timeout): force-detach the session via
/// `Target.detachFromTarget` so the target is never handed back to Chrome
/// with `Runtime.runIfWaitingForDebugger`.
async fn detach_target(conn: &Connection, session_id: &str) {
    let _ = conn
        .call_raw(
            "Target.detachFromTarget",
            json!({ "sessionId": session_id }),
            None,
        )
        .await;
}

/// Best-effort extraction of a textual panic message from a `catch_unwind`
/// payload. The standard library only guarantees a `Box<dyn Any + Send>`; we
/// downcast to `&str` and `String` (the two cases the macros produce) and
/// fall back to a placeholder for everything else.
fn panic_payload(payload: &Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<unknown panic payload>".to_string()
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::connection::{
        spawn_actor_with_observers, spawn_actor_with_observers_and_timeout, test_only::DriverStream,
    };
    use crate::observer::{ObserverError, ObserverFailurePolicy, PausedSession, TargetObserver};
    use serde_json::json;
    use std::sync::Mutex;
    use tokio_tungstenite::tungstenite::Message;

    /// Build a paired (driver-side, test-side) Sink/Stream of tungstenite
    /// `Message`s using mpsc channels. Driver writes go to `test_rx`; test
    /// writes go to `driver_rx`.
    fn duplex_pair() -> (
        DriverStream,
        mpsc::Sender<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        mpsc::Receiver<Message>,
    ) {
        let (driver_tx_out, test_rx) = mpsc::channel::<Message>(32);
        let (test_tx_in, driver_rx_in) =
            mpsc::channel::<Result<Message, tokio_tungstenite::tungstenite::Error>>(32);

        // Driver-side: sink writes to driver_tx_out; stream reads from driver_rx_in.
        let driver = DriverStream {
            tx: driver_tx_out,
            rx: driver_rx_in,
        };
        (driver, test_tx_in, test_rx)
    }

    #[tokio::test]
    async fn cmd_id_assigned_starting_at_one_and_serialized_correctly() {
        let (ws, _test_tx, mut test_rx) = duplex_pair();
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(
            ws,
            cmd_rx,
            event_tx,
            AccountedBus {
                tx: broadcast::channel::<AccountedRawEvent>(EVENT_BUS_CAPACITY).0,
                generation: 1,
            },
            shutdown.clone(),
            Vec::new(),
            Weak::new(),
        ));

        let (reply_tx, _reply_rx) = oneshot::channel();
        cmd_tx
            .send(OutboundCmd {
                method: "Page.navigate".into(),
                params: json!({ "url": "https://example.com" }),
                session_id: None,
                reply: reply_tx,
            })
            .await
            .unwrap();

        let sent = test_rx.recv().await.expect("driver sent something");
        let text = match sent {
            Message::Text(t) => t,
            other => panic!("unexpected frame: {other:?}"),
        };
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "Page.navigate");
        assert_eq!(v["params"]["url"], "https://example.com");
        assert!(v.get("sessionId").is_none());

        shutdown.cancel();
        actor_handle.await.unwrap();
    }

    #[tokio::test]
    async fn response_routes_to_correct_oneshot() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(
            ws,
            cmd_rx,
            event_tx,
            AccountedBus {
                tx: broadcast::channel::<AccountedRawEvent>(EVENT_BUS_CAPACITY).0,
                generation: 1,
            },
            shutdown.clone(),
            Vec::new(),
            Weak::new(),
        ));

        let (reply_tx, reply_rx) = oneshot::channel();
        cmd_tx
            .send(OutboundCmd {
                method: "Page.navigate".into(),
                params: json!({ "url": "https://x.test" }),
                session_id: None,
                reply: reply_tx,
            })
            .await
            .unwrap();

        let sent = test_rx.recv().await.unwrap();
        let id = serde_json::from_str::<Value>(match &sent {
            Message::Text(t) => t,
            _ => panic!("expected text frame"),
        })
        .unwrap()["id"]
            .as_u64()
            .unwrap();

        // Simulate Chrome reply.
        test_tx
            .send(Ok(Message::text(
                json!({ "id": id, "result": { "frameId": "F1" } }).to_string(),
            )))
            .await
            .unwrap();

        let res = reply_rx.await.unwrap().unwrap();
        assert_eq!(res["frameId"], "F1");

        shutdown.cancel();
        actor_handle.await.unwrap();
    }

    #[tokio::test]
    async fn response_error_propagates_to_caller() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(
            ws,
            cmd_rx,
            event_tx,
            AccountedBus {
                tx: broadcast::channel::<AccountedRawEvent>(EVENT_BUS_CAPACITY).0,
                generation: 1,
            },
            shutdown.clone(),
            Vec::new(),
            Weak::new(),
        ));

        let (reply_tx, reply_rx) = oneshot::channel();
        cmd_tx
            .send(OutboundCmd {
                method: "Foo.bar".into(),
                params: json!({}),
                session_id: None,
                reply: reply_tx,
            })
            .await
            .unwrap();

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
                json!({ "id": id, "error": { "code": -32601, "message": "Method not found" } })
                    .to_string(),
            )))
            .await
            .unwrap();

        let res = reply_rx.await.unwrap();
        let err = res.unwrap_err();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");

        shutdown.cancel();
        actor_handle.await.unwrap();
    }

    #[tokio::test]
    async fn event_fanned_out_to_multiple_subscribers() {
        let (ws, test_tx, _test_rx) = duplex_pair();
        let (_cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let mut sub_a = event_tx.subscribe();
        let mut sub_b = event_tx.subscribe();
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(
            ws,
            cmd_rx,
            event_tx,
            AccountedBus {
                tx: broadcast::channel::<AccountedRawEvent>(EVENT_BUS_CAPACITY).0,
                generation: 1,
            },
            shutdown.clone(),
            Vec::new(),
            Weak::new(),
        ));

        test_tx
            .send(Ok(Message::text(
                json!({ "method": "Page.frameStoppedLoading", "params": { "frameId": "F1" } })
                    .to_string(),
            )))
            .await
            .unwrap();

        let a = sub_a.recv().await.unwrap();
        let b = sub_b.recv().await.unwrap();
        assert_eq!(a.method, "Page.frameStoppedLoading");
        assert_eq!(b.method, "Page.frameStoppedLoading");

        shutdown.cancel();
        actor_handle.await.unwrap();
    }

    #[tokio::test]
    async fn lagged_subscriber_recovers_with_lagged_error() {
        // Small bus to force the subscriber to lag.
        let (ws, test_tx, _test_rx) = duplex_pair();
        let (_cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(2);
        let mut sub = event_tx.subscribe();
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(
            ws,
            cmd_rx,
            event_tx,
            AccountedBus {
                tx: broadcast::channel::<AccountedRawEvent>(EVENT_BUS_CAPACITY).0,
                generation: 1,
            },
            shutdown.clone(),
            Vec::new(),
            Weak::new(),
        ));

        // Push 5 events while sub doesn't consume.
        for i in 0..5 {
            test_tx
                .send(Ok(Message::text(
                    json!({ "method": "Test.evt", "params": { "i": i } }).to_string(),
                )))
                .await
                .unwrap();
        }

        // Give the actor a tick to drain.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // First recv should be Lagged.
        let first = sub.recv().await;
        assert!(matches!(
            first,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
        ));

        shutdown.cancel();
        actor_handle.await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_drains_pending_with_shutdown_error() {
        let (ws, _test_tx, _test_rx) = duplex_pair();
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(
            ws,
            cmd_rx,
            event_tx,
            AccountedBus {
                tx: broadcast::channel::<AccountedRawEvent>(EVENT_BUS_CAPACITY).0,
                generation: 1,
            },
            shutdown.clone(),
            Vec::new(),
            Weak::new(),
        ));

        let (reply_tx, reply_rx) = oneshot::channel();
        cmd_tx
            .send(OutboundCmd {
                method: "Page.navigate".into(),
                params: json!({ "url": "https://x.test" }),
                session_id: None,
                reply: reply_tx,
            })
            .await
            .unwrap();

        // Give actor time to register the pending entry before cancelling.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        shutdown.cancel();

        let res = reply_rx.await.unwrap();
        let err = res.unwrap_err();
        assert_eq!(err.code, -32001);
        assert!(err.message.contains("shut down"));

        actor_handle.await.unwrap();
    }

    #[tokio::test]
    async fn unexpected_ws_close_frame_drains_pending_with_disconnected_code() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(
            ws,
            cmd_rx,
            event_tx,
            AccountedBus {
                tx: broadcast::channel::<AccountedRawEvent>(EVENT_BUS_CAPACITY).0,
                generation: 1,
            },
            shutdown.clone(),
            Vec::new(),
            Weak::new(),
        ));

        let (reply_tx, reply_rx) = oneshot::channel();
        cmd_tx
            .send(OutboundCmd {
                method: "Page.navigate".into(),
                params: json!({ "url": "https://x.test" }),
                session_id: None,
                reply: reply_tx,
            })
            .await
            .unwrap();
        // Drain the outbound so the pending entry is registered.
        let _ = test_rx.recv().await.unwrap();

        // Chrome sends an unsolicited Close frame — the connection dropped, this
        // is NOT a caller-requested shutdown.
        test_tx.send(Ok(Message::Close(None))).await.unwrap();

        let res = reply_rx.await.unwrap();
        let err = res.unwrap_err();
        assert_eq!(
            err.code,
            crate::connection::DISCONNECTED_CODE,
            "an unexpected close must use the disconnected sentinel, not the shutdown one"
        );
        assert_ne!(err.code, crate::connection::SHUTDOWN_DRAIN_CODE);

        shutdown.cancel();
        actor_handle.await.unwrap();
    }

    #[tokio::test]
    async fn ws_stream_end_drains_pending_with_disconnected_code() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(
            ws,
            cmd_rx,
            event_tx,
            AccountedBus {
                tx: broadcast::channel::<AccountedRawEvent>(EVENT_BUS_CAPACITY).0,
                generation: 1,
            },
            shutdown.clone(),
            Vec::new(),
            Weak::new(),
        ));

        let (reply_tx, reply_rx) = oneshot::channel();
        cmd_tx
            .send(OutboundCmd {
                method: "Page.navigate".into(),
                params: json!({ "url": "https://x.test" }),
                session_id: None,
                reply: reply_tx,
            })
            .await
            .unwrap();
        let _ = test_rx.recv().await.unwrap();

        // Drop the server side so the stream returns `None` (socket vanished).
        drop(test_tx);

        let res = reply_rx.await.unwrap();
        let err = res.unwrap_err();
        assert_eq!(err.code, crate::connection::DISCONNECTED_CODE);

        shutdown.cancel();
        actor_handle.await.unwrap();
    }

    // ---------- Observer-dispatch tests (Task 11) ----------

    /// Observer behavior matrix used by the dispatch tests.
    enum ObserverBehavior {
        Ok,
        Err,
        Panic,
        Sleep(Duration),
    }

    /// Test observer that records the session id of every `on_target_attached`
    /// invocation (under its `name`) and then executes the configured behavior.
    struct RecordingObserver {
        name: &'static str,
        calls: Arc<Mutex<Vec<(&'static str, String)>>>,
        behavior: ObserverBehavior,
        policy: ObserverFailurePolicy,
    }

    #[async_trait::async_trait]
    impl TargetObserver for RecordingObserver {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn on_target_attached(
            &self,
            session: PausedSession<'_>,
        ) -> Result<(), ObserverError> {
            self.calls
                .lock()
                .unwrap()
                .push((self.name, session.session_id.to_string()));
            match &self.behavior {
                ObserverBehavior::Ok => Ok(()),
                ObserverBehavior::Err => Err(ObserverError::Other("boom".into())),
                ObserverBehavior::Panic => panic!("observer panic"),
                ObserverBehavior::Sleep(d) => {
                    tokio::time::sleep(*d).await;
                    Ok(())
                }
            }
        }

        fn failure_policy(&self) -> ObserverFailurePolicy {
            self.policy
        }
    }

    /// Read the next text frame from the driver-side and parse it as JSON.
    async fn next_frame(rx: &mut mpsc::Receiver<Message>) -> Value {
        let msg = rx.recv().await.expect("driver closed");
        let text = match msg {
            Message::Text(t) => t,
            other => panic!("expected text frame, got {other:?}"),
        };
        serde_json::from_str(&text).expect("invalid frame json")
    }

    /// Emit a `Target.attachedToTarget` event with the given session id.
    async fn emit_attached(
        test_tx: &mpsc::Sender<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        session_id: &str,
    ) {
        let frame = json!({
            "method": "Target.attachedToTarget",
            "params": {
                "sessionId": session_id,
                "targetInfo": {
                    "targetId": "T1",
                    "type": "page",
                    "url": "about:blank",
                    "attached": true,
                },
                "waitingForDebugger": true,
            },
        });
        test_tx
            .send(Ok(Message::text(frame.to_string())))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn observer_fires_with_correct_session_id() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let obs: Arc<dyn TargetObserver> = Arc::new(RecordingObserver {
            name: "rec",
            calls: calls.clone(),
            behavior: ObserverBehavior::Ok,
            policy: ObserverFailurePolicy::Required,
        });
        let conn = spawn_actor_with_observers(ws, vec![obs]);

        emit_attached(&test_tx, "S-42").await;

        // The handler should release the debugger after the observer succeeds.
        let frame = next_frame(&mut test_rx).await;
        assert_eq!(frame["method"], "Runtime.runIfWaitingForDebugger");
        assert_eq!(frame["sessionId"], "S-42");

        let recorded = calls.lock().unwrap().clone();
        assert_eq!(recorded, vec![("rec", "S-42".to_string())]);

        conn.shutdown();
    }

    #[tokio::test]
    async fn observer_err_triggers_detach_from_target() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let obs: Arc<dyn TargetObserver> = Arc::new(RecordingObserver {
            name: "bad",
            calls: calls.clone(),
            behavior: ObserverBehavior::Err,
            policy: ObserverFailurePolicy::Required,
        });
        let conn = spawn_actor_with_observers(ws, vec![obs]);

        emit_attached(&test_tx, "S-err").await;

        let frame = next_frame(&mut test_rx).await;
        assert_eq!(frame["method"], "Target.detachFromTarget");
        assert_eq!(frame["params"]["sessionId"], "S-err");
        // Detach is sent without a session-scoped envelope (browser-level call).
        assert!(frame.get("sessionId").is_none());

        conn.shutdown();
    }

    #[tokio::test]
    async fn observer_panic_triggers_detach_and_actor_keeps_running() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let obs: Arc<dyn TargetObserver> = Arc::new(RecordingObserver {
            name: "kaboom",
            calls: calls.clone(),
            behavior: ObserverBehavior::Panic,
            policy: ObserverFailurePolicy::Required,
        });
        let conn = spawn_actor_with_observers(ws, vec![obs]);

        emit_attached(&test_tx, "S-panic").await;

        // Observer panic -> detach is sent.
        let detach = next_frame(&mut test_rx).await;
        assert_eq!(detach["method"], "Target.detachFromTarget");
        assert_eq!(detach["params"]["sessionId"], "S-panic");

        // Now prove the actor is still alive by routing a regular call.
        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw("Page.navigate", json!({ "url": "https://x.test" }), None)
                    .await
            }
        });

        let nav = next_frame(&mut test_rx).await;
        assert_eq!(nav["method"], "Page.navigate");
        let id = nav["id"].as_u64().unwrap();
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

    /// Default (`Required`) policy: a timeout must fail closed, exactly like
    /// an `Err` or a panic — detach, and never hand the target out via
    /// `Runtime.runIfWaitingForDebugger`. This replaces the old fail-open
    /// `observer_timeout_releases_debugger_anyway` test — that behavior is
    /// now `BestEffort`-only, covered by
    /// `best_effort_observer_timeout_releases_debugger_anyway` below.
    #[tokio::test]
    async fn required_observer_timeout_detaches_target() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let obs: Arc<dyn TargetObserver> = Arc::new(RecordingObserver {
            name: "slow-required",
            calls: calls.clone(),
            behavior: ObserverBehavior::Sleep(Duration::from_secs(10)),
            policy: ObserverFailurePolicy::Required,
        });
        let conn =
            spawn_actor_with_observers_and_timeout(ws, vec![obs], Duration::from_millis(100));

        emit_attached(&test_tx, "S-slow-required").await;

        // Within ~200ms the timeout must fire and, for a Required observer,
        // detach the target instead of releasing it.
        let frame = tokio::time::timeout(Duration::from_millis(500), next_frame(&mut test_rx))
            .await
            .expect("timeout waiting for Target.detachFromTarget");
        assert_eq!(frame["method"], "Target.detachFromTarget");
        assert_eq!(frame["params"]["sessionId"], "S-slow-required");
        assert!(frame.get("sessionId").is_none());

        // The target must never be handed out: no runIfWaitingForDebugger
        // frame should follow the detach.
        let no_release =
            tokio::time::timeout(Duration::from_millis(300), next_frame(&mut test_rx)).await;
        assert!(
            no_release.is_err(),
            "Required observer timeout must not release the debugger after detaching"
        );

        conn.shutdown();
    }

    /// `BestEffort` policy: keeps today's original fail-open timeout
    /// behavior — log and release the debugger anyway so Chrome doesn't
    /// stay paused indefinitely on a non-critical observer.
    #[tokio::test]
    async fn best_effort_observer_timeout_releases_debugger_anyway() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let obs: Arc<dyn TargetObserver> = Arc::new(RecordingObserver {
            name: "slow-best-effort",
            calls: calls.clone(),
            behavior: ObserverBehavior::Sleep(Duration::from_secs(10)),
            policy: ObserverFailurePolicy::BestEffort,
        });
        let conn =
            spawn_actor_with_observers_and_timeout(ws, vec![obs], Duration::from_millis(100));

        emit_attached(&test_tx, "S-slow-best-effort").await;

        // Within ~200ms the timeout must fire and the debugger must be released
        // anyway so Chrome doesn't stay paused indefinitely.
        let frame = tokio::time::timeout(Duration::from_millis(500), next_frame(&mut test_rx))
            .await
            .expect("timeout waiting for runIfWaitingForDebugger");
        assert_eq!(frame["method"], "Runtime.runIfWaitingForDebugger");
        assert_eq!(frame["sessionId"], "S-slow-best-effort");

        conn.shutdown();
    }

    #[tokio::test]
    async fn multiple_observers_fire_in_registration_order() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let make = |name: &'static str| -> Arc<dyn TargetObserver> {
            Arc::new(RecordingObserver {
                name,
                calls: calls.clone(),
                behavior: ObserverBehavior::Ok,
                policy: ObserverFailurePolicy::Required,
            })
        };
        let conn =
            spawn_actor_with_observers(ws, vec![make("first"), make("second"), make("third")]);

        emit_attached(&test_tx, "S-multi").await;

        // Wait for the debugger-release to land so we know all observers
        // completed before we read `calls`.
        let frame = next_frame(&mut test_rx).await;
        assert_eq!(frame["method"], "Runtime.runIfWaitingForDebugger");

        let recorded = calls.lock().unwrap().clone();
        assert_eq!(
            recorded,
            vec![
                ("first", "S-multi".to_string()),
                ("second", "S-multi".to_string()),
                ("third", "S-multi".to_string()),
            ]
        );

        conn.shutdown();
    }
}
