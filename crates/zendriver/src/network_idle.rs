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
//! 2. Subscribes to three lifecycle events Chrome emits per request:
//!    - `Network.requestWillBeSent` — insert `requestId` into the set.
//!    - `Network.loadingFinished` / `Network.loadingFailed` — remove
//!      `requestId` from the set.
//!
//!    `Network.responseReceived` is deliberately **not** subscribed as a
//!    terminal event: it fires when response *headers* arrive, not when the
//!    body finishes streaming. Treating it as terminal let `wait_for_idle`
//!    report idle while a response body was still in flight — see this
//!    module's `InFlightTracker::run` for the fuller rationale.
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

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};
use zendriver_transport::SessionHandle;

/// Controls how [`crate::Tab::wait_for_idle_opts`] reacts when it notices —
/// during its own wait window — that the underlying CDP event stream lost
/// delivery continuity: a lagging broadcast subscriber, a
/// [`zendriver_transport::Connection::reconnect`], or the WebSocket dying.
///
/// This is independent of the in-flight tracker's own subscription (which
/// always uses [`zendriver_transport::Connection::subscribe_raw`] and
/// silently drops anything a lag causes it to miss — that's what keeps a
/// tracker cheap enough to run for the whole lifetime of every
/// [`crate::Tab`]). `IdleLossPolicy` only governs what one
/// [`crate::Tab::wait_for_idle_opts`] *call* does when the opt-in
/// [`zendriver_transport::Connection::subscribe_raw_accounted`] stream
/// reports such a gap while that call is in progress.
///
/// # Examples
///
/// ```no_run
/// # use zendriver::{IdleLossPolicy, IdleOptions};
/// # async fn ex() -> zendriver::Result<()> {
/// # let browser = zendriver::Browser::builder().launch().await?;
/// # let tab = browser.main_tab();
/// // Fail loudly instead of silently reporting a best-effort idle when a
/// // delivery gap happens mid-wait.
/// tab.wait_for_idle_opts(IdleOptions {
///     loss_policy: IdleLossPolicy::Strict,
///     ..Default::default()
/// })
/// .await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IdleLossPolicy {
    /// Best-effort — today's historical behavior, and the default. A
    /// delivery gap observed during the wait is tolerated; the call still
    /// resolves once the in-flight set it *did* observe looks idle.
    #[default]
    Lenient,
    /// Abort the wait with [`crate::ZendriverError::EventStreamIncomplete`]
    /// the instant a `Lagged`, `Reconnected`, or `Disconnected` boundary is
    /// observed on the accounted stream. Once that happens the wait can no
    /// longer prove nothing relevant to idleness was missed, so it refuses
    /// to report a possibly-wrong idle rather than guessing.
    Strict,
}

/// Tracks the set of in-flight `requestId`s for a single Tab's session.
///
/// Constructed by [`InFlightTracker::new`], driven by a background task
/// spawned via [`InFlightTracker::run`]. Readers consult `in_flight` for
/// the current count and wait on `notifier` for change events.
#[derive(Debug)]
pub(crate) struct InFlightTracker {
    /// Currently-pending requests, mapping `requestId` to the instant it was
    /// inserted (on `Network.requestWillBeSent`). Entries are removed on
    /// terminal events (`loadingFinished` / `loadingFailed` — a response's
    /// body has finished or failed). `responseReceived` (headers only) does
    /// **not** remove an entry; see [`InFlightTracker::run`]. The insertion
    /// instant lets `wait_for_idle_opts` ignore requests that have been in
    /// flight longer than [`crate::IdleOptions::max_inflight_age`] (stuck /
    /// background requests).
    pub(crate) in_flight: Mutex<HashMap<String, tokio::time::Instant>>,
    /// Notified on every membership change. `wait_for_idle` selects on this
    /// alongside a 50ms tick to wake immediately when the count moves.
    pub(crate) notifier: Notify,
}

/// Lightweight projection of the three CDP event payloads we care about
/// (`requestWillBeSent`, `loadingFinished`, `loadingFailed`). All three
/// carry `requestId: string` at the top level; nothing else is needed for
/// membership tracking.
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
            in_flight: Mutex::new(HashMap::new()),
            notifier: Notify::new(),
        })
    }

    /// Drive the tracker until `cancel` fires.
    ///
    /// Calls `Network.enable` once on `session`, then loops over the merged
    /// stream of `Network.requestWillBeSent` (insert) and the two terminal
    /// events (`loadingFailed`, `loadingFinished` — both remove). Notifies
    /// the [`Notify`] on every membership change so `wait_for_idle` waiters
    /// wake immediately.
    ///
    /// # Why `responseReceived` does not clear an entry
    ///
    /// `Network.responseReceived` fires once Chrome has the response
    /// **headers** — the body may still be streaming for an arbitrarily
    /// long time afterward (a large download, a slow origin, chunked
    /// transfer encoding). Treating it as terminal (a defect in earlier
    /// versions of this tracker) let `wait_for_idle` report idle while a
    /// response body was still actively arriving. Only `loadingFinished`
    /// (body complete) and `loadingFailed` (request aborted/errored) are
    /// genuinely terminal, so those are the only two events that clear an
    /// in-flight entry. A request that never emits either — a hung
    /// beacon, an abandoned long-poll — is covered by
    /// [`crate::IdleOptions::max_inflight_age`], not by relaxing what counts
    /// as "done" here.
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
        // than three typed subscriptions in `tokio::select!`. The select!
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
                            set.insert(parsed.request_id, tokio::time::Instant::now());
                            true
                        }
                        // `Network.responseReceived` is intentionally absent
                        // here — it fires on response *headers*, not on body
                        // completion. See `InFlightTracker::run`'s doc
                        // comment for the full rationale.
                        "Network.loadingFailed" | "Network.loadingFinished" => {
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
    /// the in_flight set transitions to size 1. Then emit
    /// `Network.responseReceived` for the same id and assert the entry is
    /// STILL present — headers arriving is not completion, so this must NOT
    /// clear it (the regression this task fixes). Finally emit
    /// `Network.loadingFinished` and assert it now drains to size 0.
    /// Notifications fire on the insert and on the genuinely-terminal
    /// removal, but not on `responseReceived`.
    #[tokio::test]
    async fn response_received_does_not_clear_in_flight_only_loading_finished_does() {
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
            assert!(set.contains_key("R1"));
        }

        // Headers arrive — must NOT remove the entry.
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({ "requestId": "R1" }),
            "S1",
        )
        .await;

        // Give the tracker a beat to (not) process it as a removal, then
        // assert R1 is still tracked as in-flight.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        {
            let set = tracker.in_flight.lock().await;
            assert_eq!(
                set.len(),
                1,
                "responseReceived must not clear an in-flight entry — \
                 headers arriving is not completion",
            );
            assert!(set.contains_key("R1"));
        }

        // Body finishes — THIS clears the entry.
        mock.emit_event_for_session(
            "Network.loadingFinished",
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
            assert!(
                set.is_empty(),
                "expected in-flight set to drain to empty after loadingFinished"
            );
        }

        cancel.cancel();
        task.await.unwrap();
        conn.shutdown();
    }
}
