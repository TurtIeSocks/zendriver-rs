//! ConnectionActor: tokio task owning the WebSocket. Routes commands and
//! responses by id, fans events out on a broadcast bus.

use std::collections::HashMap;

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, trace, warn};

use crate::frame::{CdpCommand, CdpInbound, CdpRpcError, RawEvent};

/// Outbound command sent from a `Connection` handle to the actor.
pub(crate) struct OutboundCmd {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
    pub reply: oneshot::Sender<Result<Value, CdpRpcError>>,
}

/// Default broadcast bus capacity. Lagged subscribers drop frames.
pub(crate) const EVENT_BUS_CAPACITY: usize = 1024;

/// Runs the actor loop until `shutdown` is cancelled or the WS dies.
///
/// Generic over the WS sink + stream so tests can drive in-memory streams
/// instead of real WebSockets.
pub(crate) async fn run_actor<S>(
    mut ws: S,
    mut cmd_rx: mpsc::Receiver<OutboundCmd>,
    event_tx: broadcast::Sender<RawEvent>,
    shutdown: CancellationToken,
) where
    S: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let mut pending: HashMap<u64, oneshot::Sender<Result<Value, CdpRpcError>>> = HashMap::new();
    let mut next_id: u64 = 1;

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
                        break;
                    }
                    Some(Err(e)) => {
                        error!("ws read failed: {e}");
                        break;
                    }
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<CdpInbound>(&text) {
                            Ok(CdpInbound::Response { id, result, error, .. }) => {
                                if let Some(reply) = pending.remove(&id) {
                                    let res = match error {
                                        Some(e) => Err(e),
                                        None    => Ok(result.unwrap_or(Value::Null)),
                                    };
                                    let _ = reply.send(res);
                                } else {
                                    warn!(id, "response for unknown id (caller dropped?)");
                                }
                            }
                            Ok(CdpInbound::Event { method, params, session_id }) => {
                                let ev = RawEvent { method, params, session_id };
                                // Ignore SendError: zero subscribers is fine.
                                let _ = event_tx.send(ev);
                            }
                            Err(e) => warn!("frame parse failed: {e} (text: {text})"),
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        debug!("ws close frame; shutting down");
                        break;
                    }
                    Some(Ok(_)) => { /* ignore binary, ping, pong, frame */ }
                }
            }
        }
    }

    // Drain pending into shutdown errors so callers don't hang.
    for (_id, reply) in pending.drain() {
        let _ = reply.send(Err(CdpRpcError {
            code: -32001,
            message: "connection shut down".into(),
            data: None,
        }));
    }
    debug!("actor exit");
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::connection::test_only::DriverStream;
    use serde_json::json;
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
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

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
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

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
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

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
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

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
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

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
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

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
}
