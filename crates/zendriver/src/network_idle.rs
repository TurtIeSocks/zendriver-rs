//! Per-Tab in-flight network request tracker.
//!
//! Powers [`crate::tab::Tab::wait_for_idle`] / `wait_for_idle_with` —
//! Playwright `networkidle` semantics: resolve when the set of in-flight
//! requests sustains zero for a configurable quiet window (default 500ms).
//!
//! ## Wiring
//!
//! Each [`crate::Tab`] owns one tracker. At construction time, the tab
//! spawns a background task that:
//! 1. Sends `Network.enable` once on the tab's session.
//! 2. Subscribes to the four lifecycle events Chrome emits per request:
//!    - `Network.requestWillBeSent` — insert `requestId` into the set.
//!    - `Network.responseReceived` / `loadingFailed` / `loadingFinished`
//!      — remove `requestId` from the set.
//! 3. Notifies the [`tokio::sync::Notify`] on every membership change so
//!    waiters can wake instantly rather than poll on a fixed interval.
//!
//! The task runs until the supplied [`CancellationToken`] fires — typically
//! when the owning Tab is dropped (P4 wires `network_cancel` into
//! `TabInner` so `Drop` triggers cancellation).
//!
//! ## Why a separate task (not poll-on-demand)
//!
//! `wait_for_idle` callers need to see the in-flight set's history, not just
//! its instantaneous value. A request that started 200ms ago and is still
//! pending when `wait_for_idle` is called must keep the count >0 until it
//! finishes. Tracking continuously lets us answer "has the count been 0 for
//! N ms?" without losing events that fired before the call.

use std::collections::HashSet;
use std::sync::Arc;

use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};
use zendriver_transport::SessionHandle;

/// Tracks the set of in-flight `requestId`s for a single Tab's session.
///
/// Constructed by [`InFlightTracker::new`], driven by a background task
/// spawned via [`InFlightTracker::run`]. Readers consult `in_flight` for
/// the current count and wait on `notifier` for change events.
#[derive(Debug)]
pub(crate) struct InFlightTracker {
    /// Currently-pending request IDs. Inserted on `Network.requestWillBeSent`,
    /// removed on terminal events (`responseReceived` / `loadingFailed` /
    /// `loadingFinished`).
    pub(crate) in_flight: Mutex<HashSet<String>>,
    /// Notified on every membership change. `wait_for_idle` selects on this
    /// alongside a 50ms tick to wake immediately when the count moves.
    pub(crate) notifier: Notify,
}

/// Lightweight projection of the four CDP event payloads we care about.
/// All four carry `requestId: string` at the top level; nothing else is
/// needed for membership tracking.
#[derive(Debug, Deserialize)]
struct RequestEvent {
    #[serde(rename = "requestId")]
    request_id: String,
}

impl InFlightTracker {
    /// Construct an empty tracker. Returns an [`Arc`] so the same handle can
    /// be held by both the owning Tab and the spawned [`InFlightTracker::run`]
    /// task without an extra wrapper layer.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            in_flight: Mutex::new(HashSet::new()),
            notifier: Notify::new(),
        })
    }

    /// Drive the tracker until `cancel` fires.
    ///
    /// Calls `Network.enable` once on `session`, then loops over the merged
    /// stream of `Network.requestWillBeSent` (insert) and the three terminal
    /// events (`responseReceived`, `loadingFailed`, `loadingFinished` — all
    /// remove). Notifies the [`Notify`] on every membership change so
    /// `wait_for_idle` waiters wake immediately.
    ///
    /// `Network.enable` failure is logged + ignored — the tracker still runs
    /// (events from a previously-enabled domain may still arrive). Calls into
    /// `wait_for_idle` would resolve trivially on an empty set; this matches
    /// the "best-effort observability" contract: never block the caller on
    /// our subscription bookkeeping.
    pub(crate) async fn run(self: Arc<Self>, session: SessionHandle, cancel: CancellationToken) {
        // Subscribe to the connection's raw event stream BEFORE awaiting
        // `Network.enable` for two reasons:
        // 1. Production correctness — Chrome can fire events between
        //    `Network.enable`'s reply and our subscription registration.
        //    Subscribing first plugs that race.
        // 2. Test ergonomics — the `MockConnection` flow never replies to
        //    the synthetic `Network.enable` call (no test wants to babysit
        //    background subscription bookkeeping). If we awaited the call
        //    first the task would block before any subscription existed,
        //    and events emitted by the test would be dropped on the floor.
        //
        // We use a single raw subscription and dispatch on `method` rather
        // than four typed subscriptions in `tokio::select!`. The select!
        // form picks a ready arm at random, which can deliver
        // `loadingFinished` to the handler before the matching
        // `requestWillBeSent` — the REMOVE then runs against an empty
        // set, the later INSERT installs the id, and the request leaks
        // forever. One stream means CDP's wire order is preserved.
        let session_id = session.session_id().to_string();
        let mut events = session.connection().subscribe_raw();

        // Fire-and-forget `Network.enable`. We don't await the response
        // because the mock test harness never replies; the production
        // connection actor does reply, but our subscriber stream above is
        // ready either way. If the call errors (e.g. session torn down)
        // we log and continue — subscriptions will simply receive nothing.
        let enable_session = session.clone();
        tokio::spawn(async move {
            if let Err(e) = enable_session
                .call("Network.enable", serde_json::json!({}))
                .await
            {
                warn!(error = %e, "InFlightTracker: Network.enable failed; events may be inactive");
            }
        });

        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    trace!("InFlightTracker: cancellation received, exiting");
                    return;
                }
                next = events.next() => {
                    let Some(ev) = next else {
                        // Event stream closed (transport torn down). Nothing
                        // left to observe — exit so the task doesn't sit
                        // forever on a closed stream.
                        trace!("InFlightTracker: event stream closed, exiting");
                        return;
                    };
                    if ev.session_id.as_deref() != Some(session_id.as_str()) {
                        continue;
                    }
                    let changed = match ev.method.as_str() {
                        "Network.requestWillBeSent" => {
                            let Ok(parsed) = serde_json::from_value::<RequestEvent>(ev.params) else { continue };
                            let mut set = self.in_flight.lock().await;
                            set.insert(parsed.request_id);
                            true
                        }
                        "Network.responseReceived"
                        | "Network.loadingFailed"
                        | "Network.loadingFinished" => {
                            let Ok(parsed) = serde_json::from_value::<RequestEvent>(ev.params) else { continue };
                            let mut set = self.in_flight.lock().await;
                            set.remove(&parsed.request_id);
                            true
                        }
                        _ => false,
                    };
                    if changed {
                        self.notifier.notify_waiters();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    /// End-to-end: emit `Network.requestWillBeSent` for one request, assert
    /// the in_flight set transitions to size 1, then emit `responseReceived`
    /// for the same id and assert it returns to size 0. Notifications fire
    /// on both transitions.
    #[tokio::test]
    async fn round_trip_request_will_be_sent_and_response_received() {
        let (mut mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");
        let tracker = InFlightTracker::new();
        let cancel = CancellationToken::new();

        let task = tokio::spawn({
            let t = tracker.clone();
            let s = session.clone();
            let c = cancel.clone();
            async move {
                t.run(s, c).await;
            }
        });

        // Tracker calls Network.enable first.
        let id = mock.expect_cmd("Network.enable").await;
        mock.reply(id, json!({})).await;

        // Insert via requestWillBeSent.
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({ "requestId": "R1" }),
            "S1",
        )
        .await;

        // Poll the set until it shows {R1}; the subscriber task is async
        // and may not have processed the event yet at this point.
        for _ in 0..50 {
            if tracker.in_flight.lock().await.len() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        {
            let set = tracker.in_flight.lock().await;
            assert_eq!(set.len(), 1, "expected exactly one in-flight request");
            assert!(set.contains("R1"));
        }

        // Remove via responseReceived.
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({ "requestId": "R1" }),
            "S1",
        )
        .await;

        for _ in 0..50 {
            if tracker.in_flight.lock().await.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        {
            let set = tracker.in_flight.lock().await;
            assert!(set.is_empty(), "expected in-flight set to drain to empty");
        }

        cancel.cancel();
        task.await.unwrap();
        conn.shutdown();
    }
}
