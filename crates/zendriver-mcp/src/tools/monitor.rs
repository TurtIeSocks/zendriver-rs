//! Network monitor tools — `browser_monitor_start` / `_read` / `_stop`.
//!
//! ## Drain-task model
//!
//! `browser_monitor_start` builds a lib-side `tab.monitor()` `Stream` and
//! spawns a *drain task* that owns that stream. The task pulls events, converts
//! each `zendriver::NetworkEvent` into the wire-level [`MonitorEvent`], and
//! pushes them into a per-handle bounded ring ([`MonitorBuffer`], capped at
//! [`MONITOR_BUFFER_CAP`]). The handle is returned synchronously; the task runs
//! until cancelled.
//!
//! `browser_monitor_read` drains the ring (up to `max`) and reports the running
//! `dropped` count (events evicted while the ring was full), then resets it.
//! `browser_monitor_stop` cancels the task's [`CancellationToken`] and aborts
//! its join handle, then drops the handle from the session registry.
//!
//! ## Why bodies are fetched in the drain task (not at read time)
//!
//! Chrome only retains a response body for a short window after the response
//! completes. So when the monitor was started with `capture_bodies`, the drain
//! task calls `exchange.body().await` *at observe-time* — the moment the
//! exchange is pulled from the stream — and inlines the result on the
//! [`MonitorEvent::Http`]. Deferring the fetch to `read` time would routinely
//! race Chrome's eviction and return empty bodies.
//!
//! ## Concurrency
//!
//! The ring lives behind its own `Arc<Mutex<MonitorBuffer>>`, shared by the
//! drain task and `read`. Each side locks it only briefly — the drain task per
//! pushed event, `read` per drain — and never holds it across an `.await` that
//! waits on the other side, so the pair cannot deadlock. Building the ring +
//! cancel token *before* spawning the task lets the real `JoinHandle` go
//! straight into [`MonitorState`] with no placeholder spawn.

#![cfg(feature = "monitor")]

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::StreamExt;
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use zendriver::{FrameDirection, NetworkEvent};

use crate::errors::map_error;
use crate::state::{MONITOR_BUFFER_CAP, MonitorBuffer, MonitorEvent, MonitorState, SessionState};
use crate::tools::common::current_tab;

// ---------- browser_monitor_start -----------------------------------------

/// Input for `browser_monitor_start`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartInput {
    /// Restrict buffered events to those whose URL contains this substring.
    /// Omit to buffer every observed event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_pattern: Option<String>,
    /// Fetch each HTTP response body at observe-time and inline it on the
    /// `http` event (`body` + `body_base64`). One extra CDP round-trip per
    /// exchange; off by default to keep the buffer light.
    #[serde(default)]
    pub capture_bodies: bool,
}

/// Output of `browser_monitor_start`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StartOutput {
    /// Opaque handle for the running monitor. Pass to `browser_monitor_read`
    /// and `browser_monitor_stop`.
    pub handle: String,
}

/// Start a network monitor over the current tab and begin buffering events.
///
/// Resolves the current tab, builds the lib-side monitor stream, spawns the
/// drain task, and registers the resulting [`MonitorState`] under a fresh
/// handle. Returns immediately — events accumulate in the background.
pub async fn start(
    state: Arc<Mutex<SessionState>>,
    input: StartInput,
) -> Result<StartOutput, ErrorData> {
    let tab = {
        let s = state.lock().await;
        current_tab(&s).await?
    };

    let mut mb = tab.monitor();
    if let Some(p) = &input.url_pattern {
        mb = mb.url_pattern(p.clone());
    }
    let mut stream = mb.start().await.map_err(map_error)?;

    // Build the shared ring + cancel token FIRST, then spawn the drain task
    // capturing clones, then construct `MonitorState` with the real join
    // handle — no placeholder spawn, no self-referential cycle.
    let buffer = Arc::new(Mutex::new(MonitorBuffer::default()));
    let cancel = CancellationToken::new();
    let capture = input.capture_bodies;

    let task = {
        let buffer = buffer.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    next = stream.next() => {
                        let Some(ev) = next else { break };
                        // Convert (and, when capturing, fetch the body) BEFORE
                        // taking the buffer lock so the lock is never held
                        // across the `body().await` CDP round-trip.
                        let wire = convert(ev, capture).await;
                        let mut buf = buffer.lock().await;
                        push_capped(&mut buf, wire);
                    }
                }
            }
        })
    };

    let handle = uuid::Uuid::new_v4().to_string();
    let mon = Arc::new(Mutex::new(MonitorState {
        buffer,
        cancel,
        task,
    }));
    state.lock().await.monitors.insert(handle.clone(), mon);
    Ok(StartOutput { handle })
}

/// Push `event` into the bounded ring, evicting the oldest (and bumping
/// `dropped`) when the ring is already at [`MONITOR_BUFFER_CAP`].
fn push_capped(buf: &mut MonitorBuffer, event: MonitorEvent) {
    if buf.events.len() >= MONITOR_BUFFER_CAP {
        buf.events.pop_front();
        buf.dropped += 1;
    }
    buf.events.push_back(event);
}

/// Convert a lib-side [`NetworkEvent`] into the wire mirror, fetching the HTTP
/// response body at observe-time when `capture` is set (before Chrome evicts
/// it). A body-fetch failure degrades to a body-less event rather than an
/// error — the exchange's metadata is still worth surfacing.
async fn convert(ev: NetworkEvent, capture: bool) -> MonitorEvent {
    match ev {
        NetworkEvent::Http(ex) => {
            let (body, body_base64) = if capture {
                match ex.body().await {
                    Ok(bytes) => (
                        Some(String::from_utf8_lossy(&bytes).into_owned()),
                        Some(BASE64.encode(&bytes)),
                    ),
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            };
            MonitorEvent::Http {
                url: ex.request.url.clone(),
                method: ex.request.method.clone(),
                status: ex.status(),
                error: ex.error.clone(),
                body,
                body_base64,
            }
        }
        NetworkEvent::WebSocketOpen { request_id, url } => {
            MonitorEvent::WebSocketOpen { request_id, url }
        }
        NetworkEvent::WebSocketFrame {
            request_id,
            direction,
            opcode,
            payload,
        } => MonitorEvent::WebSocketFrame {
            request_id,
            direction: match direction {
                FrameDirection::Sent => "sent",
                FrameDirection::Received => "received",
            }
            .to_string(),
            opcode,
            payload,
        },
        NetworkEvent::WebSocketClose { request_id } => MonitorEvent::WebSocketClose { request_id },
        NetworkEvent::EventSourceMessage {
            request_id,
            event_name,
            event_id,
            data,
        } => MonitorEvent::EventSourceMessage {
            request_id,
            event_name,
            event_id,
            data,
        },
    }
}

// ---------- browser_monitor_read ------------------------------------------

/// Input for `browser_monitor_read`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadInput {
    /// Handle returned by an earlier `browser_monitor_start`.
    pub handle: String,
    /// Maximum number of events to drain this call. Omit to drain all buffered
    /// events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
}

/// Output of `browser_monitor_read`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ReadOutput {
    /// Drained events, oldest first.
    pub events: Vec<MonitorEvent>,
    /// Events evicted (ring full) since the previous `read`. Non-zero means the
    /// buffer overflowed — read more often or stop the monitor.
    pub dropped: usize,
}

/// Drain up to `max` buffered events from a running monitor.
///
/// Looks the handle up in the session registry, then drains its ring. The
/// running `dropped` count is returned and reset to 0.
pub async fn read(
    state: Arc<Mutex<SessionState>>,
    input: ReadInput,
) -> Result<ReadOutput, ErrorData> {
    let mon = {
        let s = state.lock().await;
        s.monitors.get(&input.handle).cloned()
    }
    .ok_or_else(|| unknown_handle(&input.handle))?;

    // Lock the MonitorState briefly to reach its shared ring, then the ring
    // itself — neither lock is held across an `.await` that the drain task
    // waits on.
    let buffer = {
        let m = mon.lock().await;
        m.buffer.clone()
    };
    let mut buf = buffer.lock().await;
    let (events, dropped) = drain(&mut buf, input.max.unwrap_or(usize::MAX));
    Ok(ReadOutput { events, dropped })
}

/// Drain up to `max` events from `buf`, returning them (oldest first) plus the
/// `dropped` count, which is reset to 0.
///
/// Extracted as a free fn so the drain + dropped-reset semantics are
/// unit-testable without a browser, a `SessionState`, or the drain task.
fn drain(buf: &mut MonitorBuffer, max: usize) -> (Vec<MonitorEvent>, usize) {
    let take = max.min(buf.events.len());
    let events: Vec<MonitorEvent> = buf.events.drain(..take).collect();
    let dropped = std::mem::take(&mut buf.dropped);
    (events, dropped)
}

// ---------- browser_monitor_stop ------------------------------------------

/// Input for `browser_monitor_stop`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopInput {
    /// Handle returned by an earlier `browser_monitor_start`.
    pub handle: String,
}

/// Output of `browser_monitor_stop`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StopOutput {
    /// `true` if a live monitor was found and stopped; `false` if the handle
    /// was unknown (already stopped, or never started).
    pub stopped: bool,
}

/// Stop a running monitor: cancel its drain task and drop its registry entry.
///
/// Removing the handle from the registry drops the `MonitorState` (and its
/// shared ring) once this function's lock guard releases; cancelling + aborting
/// the task first guarantees it stops touching the stream promptly. An unknown
/// handle returns `{ stopped: false }` (idempotent stop) rather than an error.
pub async fn stop(
    state: Arc<Mutex<SessionState>>,
    input: StopInput,
) -> Result<StopOutput, ErrorData> {
    let Some(mon) = state.lock().await.monitors.remove(&input.handle) else {
        return Ok(StopOutput { stopped: false });
    };
    let m = mon.lock().await;
    m.cancel.cancel();
    m.task.abort();
    Ok(StopOutput { stopped: true })
}

/// Wire error for a `read` / `stop` against a handle that isn't registered.
fn unknown_handle(handle: &str) -> ErrorData {
    ErrorData::invalid_params(
        format!(
            "unknown monitor handle `{handle}`. Start one with `browser_monitor_start`; the handle may have already been stopped."
        ),
        None,
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    //! No-browser unit coverage of the buffer drain + cap-eviction logic.
    //!
    //! The drain-task path (which constructs a real `tab.monitor()` stream and
    //! spawns the correlator) needs a live Chrome and is exercised in the gated
    //! integration tests. Here we drive `drain` / `push_capped` directly against
    //! a synthetic `MonitorBuffer`.
    use std::collections::VecDeque;

    use super::*;

    /// A cheap synthetic event for buffer tests.
    fn ev(n: usize) -> MonitorEvent {
        MonitorEvent::Http {
            url: format!("https://example.com/{n}"),
            method: "GET".into(),
            status: Some(200),
            error: None,
            body: None,
            body_base64: None,
        }
    }

    #[test]
    fn drain_returns_up_to_max_and_clears_them() {
        let mut buf = MonitorBuffer {
            events: (0..5).map(ev).collect(),
            dropped: 0,
        };
        let (events, dropped) = drain(&mut buf, 3);
        assert_eq!(events.len(), 3, "drains exactly `max`");
        assert_eq!(events[0], ev(0), "oldest first");
        assert_eq!(events[2], ev(2));
        assert_eq!(dropped, 0);
        // Remaining events stay, in order, for the next read.
        assert_eq!(buf.events.len(), 2);
        assert_eq!(buf.events.front().unwrap(), &ev(3));
    }

    #[test]
    fn drain_caps_at_buffer_len_when_max_exceeds_it() {
        let mut buf = MonitorBuffer {
            events: (0..2).map(ev).collect(),
            dropped: 0,
        };
        let (events, _) = drain(&mut buf, usize::MAX);
        assert_eq!(events.len(), 2, "never over-drains past what's buffered");
        assert!(buf.events.is_empty());
    }

    #[test]
    fn drain_returns_and_resets_dropped() {
        let mut buf = MonitorBuffer {
            events: VecDeque::new(),
            dropped: 3,
        };
        let (events, dropped) = drain(&mut buf, usize::MAX);
        assert!(events.is_empty());
        assert_eq!(dropped, 3, "reports the running dropped count");
        assert_eq!(buf.dropped, 0, "and resets it to zero");
    }

    #[test]
    fn push_capped_evicts_oldest_and_counts_dropped_over_cap() {
        let mut buf = MonitorBuffer::default();
        // Fill exactly to capacity — no drops yet.
        for n in 0..MONITOR_BUFFER_CAP {
            push_capped(&mut buf, ev(n));
        }
        assert_eq!(buf.events.len(), MONITOR_BUFFER_CAP);
        assert_eq!(buf.dropped, 0);

        // One more over the cap evicts the oldest (event 0) and counts it.
        push_capped(&mut buf, ev(MONITOR_BUFFER_CAP));
        assert_eq!(buf.events.len(), MONITOR_BUFFER_CAP, "stays at the cap");
        assert_eq!(buf.dropped, 1, "the evicted event is counted");
        assert_eq!(
            buf.events.front().unwrap(),
            &ev(1),
            "event 0 was evicted; event 1 is now oldest"
        );
        assert_eq!(
            buf.events.back().unwrap(),
            &ev(MONITOR_BUFFER_CAP),
            "the newest push is retained"
        );
    }

    #[tokio::test]
    async fn read_unknown_handle_is_invalid_params() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = read(
            state,
            ReadInput {
                handle: "nope".into(),
                max: None,
            },
        )
        .await
        .expect_err("expected unknown-handle error");
        assert!(err.message.contains("unknown monitor handle"));
    }

    #[tokio::test]
    async fn stop_unknown_handle_reports_not_stopped() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let out = stop(
            state,
            StopInput {
                handle: "nope".into(),
            },
        )
        .await
        .expect("stop is idempotent for unknown handles");
        assert!(!out.stopped, "unknown handle reports stopped=false");
    }
}
